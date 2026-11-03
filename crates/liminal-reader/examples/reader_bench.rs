//! Headless interaction benchmark for the reader's document pipeline.
//!
//! `cargo run --release -p liminaldoc-reader --example reader_bench -- FILE [--json OUT]`
//!
//! Drives the production engine, layout, conductor and caches exactly as the
//! window does, without a window: it publishes the same viewport intents the
//! application builds for a cold open, sequential PageDown reading, random
//! seeks, a zoom change and a search, and reports how long each step took to
//! reach *paintable* pixels (first fallback and exact final tiles), plus the
//! background index sweep time and peak memory.
//!
//! Timings start when the intent is published and end when the painter's own
//! immutable snapshot contains the surfaces it would draw. Text-first
//! placeholders count as "first pixels"; "exact" requires every visible tile
//! at the Final tier in the requested bucket.
//!
//! `--generate DIR` writes the synthetic PDF corpus the recorded benchmarks
//! use (classic multipage text, scanned raster pages) and exits.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use liminaldoc_reader::document::markdown::engine::MarkdownEngine;
#[cfg(feature = "pdf-engine")]
use liminaldoc_reader::document::pdf_engine::PdfEngine;
use liminaldoc_reader::document::{
    ConductorHandle, DocumentEngine, MemoryArbiter, NavigationMode, PageIndex, PageLayoutIndex,
    SessionUpdate, TileCache, TileKey, TileTier, UpdateQueue, ViewportIntent, ViewportPlanner,
    WakeSink,
};
use liminaldoc_reader::geometry::{SizeF, Vec2d};
use liminaldoc_reader::text::SearchIndex;
use liminaldoc_reader::theme::Theme;

#[derive(Debug)]
struct ChannelWake(mpsc::SyncSender<()>);

impl WakeSink for ChannelWake {
    fn wake(&self) {
        let _ = self.0.try_send(());
    }
}

const VIEWPORT: SizeF = SizeF {
    width: 1100.0,
    height: 800.0,
};
const STEP_DEADLINE: Duration = Duration::from_secs(20);

struct Session {
    engine: Arc<dyn DocumentEngine>,
    layout: Arc<PageLayoutIndex>,
    updates: Arc<UpdateQueue>,
    wake_rx: mpsc::Receiver<()>,
    memory: MemoryArbiter,
    tiles: Arc<TileCache>,
    conductor: ConductorHandle,
    planner: ViewportPlanner,
    generation: u64,
    search: SearchIndex,
    indexed: HashSet<PageIndex>,
    index_complete_at: Option<Duration>,
    started: Instant,
    scroll: Vec2d,
    zoom: f64,
    updates_drained: u64,
}

#[derive(Debug, Default, Clone)]
struct StepTiming {
    first_pixels_ms: Option<f64>,
    exact_ms: Option<f64>,
}

impl Session {
    fn open(engine: Arc<dyn DocumentEngine>) -> Self {
        let started = Instant::now();
        let layout = Arc::new(PageLayoutIndex::build(
            &engine.descriptor().page_geometries,
            &Theme::default().metrics,
        ));
        let (wake_tx, wake_rx) = mpsc::sync_channel(1);
        let updates = UpdateQueue::new(4096, Arc::new(ChannelWake(wake_tx)));
        let memory = MemoryArbiter::new(1024 * 1024 * 1024);
        let tiles = Arc::new(TileCache::new(engine.descriptor().id, memory.clone()));
        let conductor = ConductorHandle::spawn(
            engine.clone(),
            layout.clone(),
            updates.clone(),
            memory.clone(),
            tiles.clone(),
        )
        .expect("conductor");
        let zoom = {
            let first = layout.placement(PageIndex(0)).expect("page 0");
            // The window's automatic zoom: one step above fit-page, capped by
            // fit-width. Fit-width is the common case for a book page.
            let fit_width = (VIEWPORT.width - 24.0) / first.bounds.width.max(1.0);
            let fit_page = ((VIEWPORT.height - 24.0) / first.bounds.height.max(1.0)).min(fit_width);
            (fit_page * std::f64::consts::SQRT_2).min(fit_width)
        };
        Self {
            search: SearchIndex::with_memory(memory.clone()),
            engine,
            layout,
            updates,
            wake_rx,
            memory,
            tiles,
            conductor,
            planner: ViewportPlanner::default(),
            generation: 0,
            indexed: HashSet::new(),
            index_complete_at: None,
            started,
            scroll: Vec2d::ZERO,
            zoom,
            updates_drained: 0,
        }
    }

    fn page_top(&self, page: PageIndex) -> f64 {
        let placement = self.layout.placement(page).expect("page in range");
        ((placement.bounds.y - 12.0) * self.zoom).max(0.0)
    }

    fn publish(&mut self, mode: NavigationMode) -> ViewportIntent {
        self.generation += 1;
        let intent = self.planner.build_with_navigation(
            self.generation,
            &self.layout,
            self.scroll,
            Vec2d::ZERO,
            VIEWPORT,
            self.zoom,
            None,
            mode,
        );
        self.conductor.publish_intent(intent.clone());
        intent
    }

    fn drain(&mut self) {
        for update in self.updates.drain() {
            self.updates_drained += 1;
            match update {
                SessionUpdate::PageCompiled(update) => {
                    self.search.insert(update.page, update.text);
                    self.indexed.insert(update.page);
                }
                SessionUpdate::PageIndexed { page, text, .. } => {
                    self.search.insert(page, text);
                    self.indexed.insert(page);
                }
                SessionUpdate::PageError { page, message, .. } => {
                    panic!("page {} failed: {message}", page.0);
                }
                _ => {}
            }
        }
        if self.index_complete_at.is_none()
            && self.indexed.len() as u32 >= self.engine.descriptor().page_count
        {
            self.index_complete_at = Some(self.started.elapsed());
        }
    }

    /// Wait until the visible tiles of `intent` are paintable. Returns the
    /// time to the first paintable pixels and to the exact final tiles.
    fn settle(&mut self, intent: &ViewportIntent, published: Instant) -> StepTiming {
        let document = self.engine.descriptor().id;
        let final_keys: Vec<TileKey> = intent
            .visible_tiles
            .iter()
            .map(|demand| demand.key(document, intent.bucket, TileTier::Final))
            .collect();
        let mut timing = StepTiming::default();
        let deadline = published + STEP_DEADLINE;
        loop {
            let snapshot = self
                .tiles
                .frame_snapshot_for_pages(intent.visible_tiles.iter().map(|demand| demand.page));
            if timing.first_pixels_ms.is_none() {
                let any = intent.visible_tiles.iter().any(|demand| {
                    snapshot
                        .best_covering(*demand, intent.bucket)
                        .iter()
                        .any(|surface| !surface.pixels.pixels.is_empty())
                });
                if any {
                    timing.first_pixels_ms = Some(millis(published.elapsed()));
                }
            }
            if final_keys.iter().all(|key| self.tiles.contains(*key)) {
                timing.exact_ms = Some(millis(published.elapsed()));
                timing
                    .first_pixels_ms
                    .get_or_insert(timing.exact_ms.unwrap());
                return timing;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                eprintln!(
                    "warning: step timed out after {STEP_DEADLINE:?} ({}/{} final tiles)",
                    final_keys
                        .iter()
                        .filter(|key| self.tiles.contains(**key))
                        .count(),
                    final_keys.len()
                );
                return timing;
            }
            let _ = self
                .wake_rx
                .recv_timeout(remaining.min(Duration::from_millis(5)));
            self.drain();
        }
    }

    fn step(&mut self, mode: NavigationMode) -> StepTiming {
        let published = Instant::now();
        let intent = self.publish(mode);
        self.settle(&intent, published)
    }

    fn wait_for_index(&mut self, limit: Duration) -> Option<Duration> {
        // Idle at the current position: the conductor sweeps the document.
        let intent = self.publish(NavigationMode::Idle);
        let _ = intent;
        let deadline = Instant::now() + limit;
        while self.index_complete_at.is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let _ = self
                .wake_rx
                .recv_timeout(remaining.min(Duration::from_millis(20)));
            self.drain();
        }
        self.index_complete_at
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((sorted.len() as f64 - 1.0) * fraction).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn summary(label: &str, values: &[f64], out: &mut Vec<(String, f64)>) {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if sorted.is_empty() {
        println!("{label}: no samples");
        return;
    }
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let p50 = percentile(&sorted, 0.5);
    let p95 = percentile(&sorted, 0.95);
    let p99 = percentile(&sorted, 0.99);
    let max = sorted[sorted.len() - 1];
    println!(
        "{label}: n={} mean={mean:.2} p50={p50:.2} p95={p95:.2} p99={p99:.2} max={max:.2} ms",
        sorted.len()
    );
    for (suffix, value) in [
        ("mean", mean),
        ("p50", p50),
        ("p95", p95),
        ("p99", p99),
        ("max", max),
    ] {
        out.push((format!("{label}_{suffix}_ms"), value));
    }
}

fn rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(resident * 4096)
}

fn open_engine(path: &Path) -> Arc<dyn DocumentEngine> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "pdf" => {
            #[cfg(feature = "pdf-engine")]
            {
                Arc::new(PdfEngine::open(path, None).expect("open pdf"))
            }
            #[cfg(not(feature = "pdf-engine"))]
            {
                panic!("built without the pdf-engine feature")
            }
        }
        _ => Arc::new(MarkdownEngine::open(path).expect("open markdown")),
    }
}

/// Deterministic page sequence for random seeks: a linear congruential walk
/// so every run visits the same pages.
fn seek_pages(page_count: u32, count: usize) -> Vec<PageIndex> {
    let mut state = 0x9e37_79b9_u64;
    (0..count)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            PageIndex(((state >> 33) % u64::from(page_count.max(1))) as u32)
        })
        .collect()
}

#[cfg(feature = "pdf-engine")]
fn generate_corpus(dir: &Path) {
    std::fs::create_dir_all(dir).expect("corpus dir");
    let classic = pdf_test_support::builder::multipage_classic(1000);
    std::fs::write(dir.join("classic-1000.pdf"), classic).expect("write");
    let scan = pdf_test_support::builder::raster_scan(9, true);
    std::fs::write(dir.join("scan-9.pdf"), scan).expect("write");
    let mixed = pdf_test_support::builder::vector_with_raster_outlier(200, 100);
    std::fs::write(dir.join("vector-200.pdf"), mixed).expect("write");
    println!(
        "wrote classic-1000.pdf, scan-9.pdf, vector-200.pdf to {}",
        dir.display()
    );
}

#[cfg(not(feature = "pdf-engine"))]
fn generate_corpus(_dir: &Path) {
    panic!("corpus generation needs the pdf-engine feature");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut file = None;
    let mut json_out = None;
    let mut turns = 30usize;
    let mut seeks = 20usize;
    let mut index_limit = Duration::from_secs(120);
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--generate" => {
                generate_corpus(Path::new(iter.next().expect("--generate DIR")));
                return;
            }
            "--json" => json_out = Some(PathBuf::from(iter.next().expect("--json OUT"))),
            "--turns" => turns = iter.next().expect("--turns N").parse().expect("turns"),
            "--seeks" => seeks = iter.next().expect("--seeks N").parse().expect("seeks"),
            "--index-limit-s" => {
                index_limit =
                    Duration::from_secs(iter.next().expect("seconds").parse().expect("secs"));
            }
            other => file = Some(PathBuf::from(other)),
        }
    }
    let path = file.expect("usage: reader_bench FILE [--json OUT] [--turns N] [--seeks N]");

    let mut results: Vec<(String, f64)> = Vec::new();
    let process_start = Instant::now();
    let engine = open_engine(&path);
    let open_ms = millis(process_start.elapsed());
    let page_count = engine.descriptor().page_count;
    println!("file: {} ({page_count} pages)", path.display());
    println!("open_ms: {open_ms:.2}");
    results.push(("open_ms".into(), open_ms));
    results.push(("pages".into(), f64::from(page_count)));

    let mut session = Session::open(engine);
    let setup_ms = millis(session.started.elapsed());
    results.push(("setup_ms".into(), setup_ms));

    // Cold first page.
    let first = session.step(NavigationMode::Idle);
    println!(
        "first_page: first_pixels={:.2} exact={:.2} ms (from open: {:.2} ms)",
        first.first_pixels_ms.unwrap_or(f64::NAN),
        first.exact_ms.unwrap_or(f64::NAN),
        millis(process_start.elapsed())
    );
    results.push((
        "first_page_first_pixels_ms".into(),
        first.first_pixels_ms.unwrap_or(f64::NAN),
    ));
    results.push((
        "first_page_exact_ms".into(),
        first.exact_ms.unwrap_or(f64::NAN),
    ));
    results.push((
        "first_page_exact_from_open_ms".into(),
        millis(process_start.elapsed()),
    ));

    // Sequential reading: PageDown through the first `turns` pages, giving
    // the conductor the same 140 ms settle the window gives it.
    let mut turn_first = Vec::new();
    let mut turn_exact = Vec::new();
    for page in 1..=turns.min(page_count.saturating_sub(1) as usize) {
        session.scroll.y = session.page_top(PageIndex(page as u32));
        let timing = session.step(NavigationMode::SequentialForward);
        turn_first.push(timing.first_pixels_ms.unwrap_or(f64::NAN));
        turn_exact.push(timing.exact_ms.unwrap_or(f64::NAN));
        std::thread::sleep(Duration::from_millis(140));
        let idle = session.publish(NavigationMode::Idle);
        let _ = idle;
    }
    summary("page_turn_first_pixels", &turn_first, &mut results);
    summary("page_turn_exact", &turn_exact, &mut results);

    // Random seeks: the scrollbar / go-to-page path. JumpLikely then settle.
    let mut seek_first = Vec::new();
    let mut seek_exact = Vec::new();
    for page in seek_pages(page_count, seeks) {
        session.scroll.y = session.page_top(page);
        let timing = session.step(NavigationMode::JumpLikely);
        seek_first.push(timing.first_pixels_ms.unwrap_or(f64::NAN));
        seek_exact.push(timing.exact_ms.unwrap_or(f64::NAN));
        std::thread::sleep(Duration::from_millis(140));
        let _ = session.publish(NavigationMode::Idle);
    }
    summary("seek_first_pixels", &seek_first, &mut results);
    summary("seek_exact", &seek_exact, &mut results);

    // Zoom in two buckets and back at the current page.
    let base_zoom = session.zoom;
    let mut zoom_exact = Vec::new();
    for factor in [std::f64::consts::SQRT_2, 2.0, 1.0] {
        let page = session
            .layout
            .page_at_y(session.scroll.y / session.zoom)
            .map(|placement| placement.page)
            .unwrap_or(PageIndex(0));
        session.zoom = base_zoom * factor;
        session.scroll.y = session.page_top(page);
        let timing = session.step(NavigationMode::Idle);
        zoom_exact.push(timing.exact_ms.unwrap_or(f64::NAN));
    }
    summary("zoom_exact", &zoom_exact, &mut results);

    // Background index sweep, then search.
    session.scroll.y = 0.0;
    let index_wait_started = Instant::now();
    let indexed_at = session.wait_for_index(index_limit);
    match indexed_at {
        Some(at) => {
            println!(
                "index_complete: {:.2} ms after session start ({:.2} ms waited here)",
                millis(at),
                millis(index_wait_started.elapsed())
            );
            results.push(("index_complete_ms".into(), millis(at)));
        }
        None => {
            let missing: Vec<u32> = (0..page_count)
                .filter(|page| !session.indexed.contains(&PageIndex(*page)))
                .take(10)
                .collect();
            println!(
                "index_complete: not within {index_limit:?} ({} of {page_count} pages; missing {missing:?})",
                session.indexed.len()
            );
        }
    }
    let queries = ["the", "page", "zzzz-not-present", "Chapter"];
    let mut search_ms = Vec::new();
    for query in queries {
        let started = Instant::now();
        let hits = session.search.search_case_insensitive(query, 10_000);
        let elapsed = millis(started.elapsed());
        println!("search {query:?}: {} hits in {elapsed:.2} ms", hits.len());
        search_ms.push(elapsed);
    }
    summary("search", &search_ms, &mut results);

    // Document switch: reopen the same file in a fresh session, as the
    // window does when another document is opened.
    let switch_started = Instant::now();
    let engine2 = open_engine(&path);
    let mut session2 = Session::open(engine2);
    let timing = session2.step(NavigationMode::Idle);
    let switch_ms = millis(switch_started.elapsed());
    println!(
        "document_switch: exact first page {:.2} ms after reopen",
        timing.exact_ms.unwrap_or(f64::NAN)
    );
    results.push(("document_switch_exact_ms".into(), switch_ms));
    drop(session2);

    let cache_bytes = session.memory.total_bytes();
    let rss = rss_bytes().unwrap_or(0);
    println!(
        "memory: arbiter={:.1} MiB rss={:.1} MiB tiles={} updates_drained={}",
        cache_bytes as f64 / (1024.0 * 1024.0),
        rss as f64 / (1024.0 * 1024.0),
        session.tiles.len(),
        session.updates_drained
    );
    results.push(("arbiter_mib".into(), cache_bytes as f64 / (1024.0 * 1024.0)));
    results.push(("rss_mib".into(), rss as f64 / (1024.0 * 1024.0)));
    results.push(("updates_drained".into(), session.updates_drained as f64));

    if let Some(out) = json_out {
        let mut json = String::from("{\n");
        json.push_str(&format!("  \"file\": {:?},\n", path.display().to_string()));
        for (index, (key, value)) in results.iter().enumerate() {
            let value = if value.is_finite() {
                format!("{value:.3}")
            } else {
                "null".to_owned()
            };
            json.push_str(&format!(
                "  \"{key}\": {value}{}\n",
                if index + 1 < results.len() { "," } else { "" }
            ));
        }
        json.push_str("}\n");
        std::fs::write(&out, json).expect("write json");
        println!("wrote {}", out.display());
    }
}
