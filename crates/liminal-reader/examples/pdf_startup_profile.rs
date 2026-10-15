//! Measure production PDF scheduling from open to paintable pixels.
//!
//! `cargo run --release -p liminaldoc-reader --example pdf_startup_profile -- file.pdf [page]`
//!
//! Each invocation opens one document in a fresh process, with an empty cache.
//! Times include PDF open, layout and worker startup, but exclude window creation
//! and presentation. Final tiles and canonical previews contain actual rendered
//! PDF pixels; text-first placeholders do not count as page visibility.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use liminaldoc_reader::document::pdf_engine::PdfEngine;
use liminaldoc_reader::document::{
    ConductorHandle, DocumentEngine, MemoryArbiter, PageIndex, PageLayoutIndex, SessionUpdate,
    TileCache, TileKey, TileSurface, TileTier, UpdateQueue, ViewportPlanner, WakeSink,
};
use liminaldoc_reader::geometry::{SizeF, Vec2d};
use liminaldoc_reader::theme::Theme;

#[derive(Debug)]
struct ChannelWake(mpsc::SyncSender<()>);

impl WakeSink for ChannelWake {
    fn wake(&self) {
        // One outstanding notification suffices; UpdateQueue closes the drain race.
        let _ = self.0.try_send(());
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(
        args.next()
            .ok_or("usage: pdf_startup_profile <file.pdf> [zero-based-page]")?,
    );
    let page = PageIndex(match args.next() {
        Some(value) => value.to_str().ok_or("page must be UTF-8")?.parse()?,
        None => 0,
    });
    if args.next().is_some() {
        return Err("unexpected extra argument".into());
    }

    let started = Instant::now();
    let engine = Arc::new(PdfEngine::open(&path, None)?);
    let opened = started.elapsed();
    let layout = Arc::new(PageLayoutIndex::build_with_options(
        &engine.descriptor().page_geometries,
        &[],
        false,
        liminaldoc_reader::document::ColorMode::Original,
        1,
        &Theme::default().metrics,
    ));
    let placement = layout.placement(page).ok_or("page is out of range")?;
    let viewport = SizeF {
        width: 1100.0,
        height: 800.0,
    };
    let zoom = (viewport.width - 24.0) / placement.bounds.width.max(1.0);
    let scroll = Vec2d {
        x: ((placement.bounds.x + placement.bounds.width * 0.5) * zoom - viewport.width * 0.5)
            .max(0.0),
        y: (placement.bounds.y * zoom - 12.0).max(0.0),
    };
    let intent =
        ViewportPlanner::default().build(1, &layout, scroll, Vec2d::ZERO, viewport, zoom, None);
    let visible_keys: HashSet<TileKey> = intent
        .visible_tiles
        .iter()
        .map(|demand| demand.key(engine.descriptor().id, intent.bucket, TileTier::Final))
        .collect();
    if visible_keys.is_empty() {
        return Err("viewport has no visible tiles".into());
    }
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    let updates = UpdateQueue::new(4096, Arc::new(ChannelWake(wake_tx)));
    let memory = MemoryArbiter::new(1024 * 1024 * 1024);
    let tiles = Arc::new(TileCache::new(engine.descriptor().id, memory.clone()));
    let conductor = ConductorHandle::spawn(
        engine.clone(),
        layout.clone(),
        updates.clone(),
        memory,
        tiles.clone(),
    )?;
    let previews = conductor.previews();
    conductor.publish_intent(intent.clone());
    let submitted = started.elapsed();

    let deadline = started + Duration::from_secs(30);
    let mut compiled = None;
    let mut operation_count = 0;
    let mut first_final = None;
    let mut preview_ready = None;
    let mut all_final = None;
    let mut ready = HashSet::new();
    let mut final_surfaces = Vec::new();
    let mut preview_surface = None;
    while all_final.is_none() || preview_ready.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "startup timed out: {}/{} final tiles, preview={}",
                ready.len(),
                visible_keys.len(),
                preview_ready.is_some()
            )
            .into());
        }
        wake_rx.recv_timeout(remaining)?;
        let elapsed = started.elapsed();
        for update in updates.drain() {
            match update {
                SessionUpdate::PageCompiled(update) if update.page == page => {
                    compiled.get_or_insert(elapsed);
                    operation_count = update.operation_count;
                }
                SessionUpdate::PageError { page, message, .. }
                    if visible_keys.iter().any(|key| key.page == page) =>
                {
                    return Err(format!("page {} failed: {message}", page.0).into());
                }
                _ => {}
            }
        }
        // Inspect the same immutable cache snapshots used by the painter. A
        // notification alone does not prove a paintable current-variant surface.
        let snapshot = tiles.frame_snapshot();
        for demand in intent.visible_tiles.iter() {
            let key = demand.key(engine.descriptor().id, intent.bucket, TileTier::Final);
            if ready.contains(&key) {
                continue;
            }
            for surface in snapshot.best_covering(*demand, intent.bucket) {
                if surface.key == key && !surface.pixels.pixels.is_empty() {
                    first_final.get_or_insert(elapsed);
                    ready.insert(key);
                    final_surfaces.push(surface);
                }
            }
        }
        if ready.len() == visible_keys.len() {
            all_final.get_or_insert(elapsed);
        }
        if preview_ready.is_none()
            && let Some(surface) = previews.get_variant(page, layout.render_variant)
            && !surface.pixels.pixels.is_empty()
        {
            preview_ready = Some(elapsed);
            preview_surface = Some(surface);
        }
    }

    // Pixel scans happen after all timings, so checksum work cannot delay them.
    final_surfaces.sort_by_key(|surface| surface.key);
    let (final_checksum, nonwhite, degraded) = pixel_evidence(&final_surfaces);
    let (preview_checksum, preview_nonwhite, _) =
        pixel_evidence(&preview_surface.into_iter().collect::<Vec<_>>());
    println!("file: {}", path.display());
    println!(
        "page: {}, viewport: 1100x800, zoom: {:.4}, bucket: {}, visible tiles: {}",
        page.0,
        zoom,
        intent.bucket.0,
        visible_keys.len()
    );
    println!("open_ms: {:.3}", millis(opened));
    println!("setup_ms: {:.3}", millis(submitted - opened));
    print_time("primary_page_compiled_total_ms", compiled);
    print_time("first_visible_final_total_ms", first_final);
    print_time("canonical_preview_total_ms", preview_ready);
    print_time("all_visible_final_total_ms", all_final);
    println!("primary_page_operations: {operation_count}");
    println!(
        "final_checksum: {final_checksum:016x}, nonwhite_pixels: {nonwhite}, degraded_tiles: {degraded}"
    );
    println!("preview_checksum: {preview_checksum:016x}, nonwhite_pixels: {preview_nonwhite}");
    println!("routing: {:?}", engine.image_renderer_telemetry());
    Ok(())
}

fn pixel_evidence(surfaces: &[Arc<TileSurface>]) -> (u64, usize, usize) {
    let mut checksum = 0xcbf2_9ce4_8422_2325u64;
    let mut nonwhite = 0;
    let mut degraded = 0;
    for surface in surfaces {
        degraded += usize::from(surface.degraded);
        for &pixel in surface.pixels.pixels.iter() {
            checksum ^= u64::from(pixel);
            checksum = checksum.wrapping_mul(0x0000_0100_0000_01b3);
            nonwhite += usize::from(pixel & 0x00ff_ffff != 0x00ff_ffff);
        }
    }
    (checksum, nonwhite, degraded)
}

fn print_time(label: &str, duration: Option<Duration>) {
    match duration {
        Some(duration) => println!("{label}: {:.3}", millis(duration)),
        None => println!("{label}: unavailable"),
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
