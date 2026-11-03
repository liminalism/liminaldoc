//! Keystroke latency on a long manuscript, through the same `Editor` path
//! the window drives (edit, incremental layout, full frame paint).
//!
//! `cargo run --release -p docwrite-app --example typing_profile -- BOOK.legebook [KEYS] [--json OUT]`
//!
//! Types `KEYS` characters (default 120) at the start, middle and end of the
//! manuscript, then deletes them, undoes, pages, seeks and searches, and
//! reports p50/p95/p99/max for each interaction, plus open and save times.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use docwrite_app::{Editor, PromptKind};
use docwrite_model::{Position, Selection};

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

fn summary(label: &str, samples: &[Duration], out: &mut Vec<(String, f64)>) {
    let mut sorted: Vec<f64> = samples.iter().map(|d| millis(*d)).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if sorted.is_empty() {
        return;
    }
    let pick = |fraction: f64| sorted[(((sorted.len() - 1) as f64) * fraction).round() as usize];
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let (p50, p95, p99, max) = (pick(0.5), pick(0.95), pick(0.99), sorted[sorted.len() - 1]);
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

fn rss_mib() -> f64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|statm| statm.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0.0, |pages| pages as f64 * 4096.0 / (1024.0 * 1024.0))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut path = None;
    let mut keys = 120usize;
    let mut json = None;
    let mut only_typing = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--json" => json = Some(PathBuf::from(iter.next().expect("--json OUT"))),
            "--only-typing" => only_typing = true,
            other if path.is_none() => path = Some(PathBuf::from(other)),
            other => keys = other.parse().expect("KEYS"),
        }
    }
    let path = path.expect("usage: typing_profile BOOK.legebook [KEYS] [--json OUT]");
    let mut results: Vec<(String, f64)> = Vec::new();

    let started = Instant::now();
    let mut editor = Editor::open_existing(&path).expect("open bundle");
    let open = started.elapsed();
    let mut frame = liminalkit_raster::WindowBuffer::new(1100, 800);
    let started = Instant::now();
    editor.paint(&mut frame);
    let first_paint = started.elapsed();
    let pages = editor.pager().pages();
    println!(
        "{}: {pages} pages; open {:.2} ms, first layout+paint {:.2} ms, rss {:.1} MiB",
        path.display(),
        millis(open),
        millis(first_paint),
        rss_mib()
    );
    results.push(("pages".into(), f64::from(pages)));
    results.push(("open_ms".into(), millis(open)));
    results.push(("first_paint_ms".into(), millis(first_paint)));

    // Typing at three positions, then deleting the same characters back.
    let blocks = editor.book().block_ids();
    let mut typing = Vec::new();
    let mut deleting = Vec::new();
    let mut undoing = Vec::new();
    let text = "the quick brown fox jumps over the lazy dog ";
    let mut phase_totals = [Duration::ZERO; 7];
    for fraction in [0.0, 0.5, 0.999] {
        let block = blocks[((blocks.len() - 1) as f64 * fraction) as usize];
        editor
            .book_mut()
            .set_selection(Selection::collapsed(Position::new(block, 5)))
            .expect("caret");
        editor.type_text(" ");
        editor.paint(&mut frame);
        for ch in text.chars().cycle().take(keys) {
            let mut buffer = [0u8; 4];
            let started = Instant::now();
            editor.type_text(ch.encode_utf8(&mut buffer));
            editor.paint(&mut frame);
            typing.push(started.elapsed());
            let phases = editor.last_frame_phases();
            phase_totals[0] += phases.update.inputs;
            phase_totals[1] += phases.update.apply;
            phase_totals[2] += phases.update.settle;
            phase_totals[3] += phases.layout;
            phase_totals[4] += phases.pages;
            phase_totals[5] += phases.chrome;
            phase_totals[6] += phases.total;
        }
        for _ in 0..keys / 2 {
            let started = Instant::now();
            editor.backspace();
            editor.paint(&mut frame);
            deleting.push(started.elapsed());
        }
        for _ in 0..keys / 4 {
            let started = Instant::now();
            editor.undo();
            editor.paint(&mut frame);
            undoing.push(started.elapsed());
        }
    }
    summary("type", &typing, &mut results);
    let count = typing.len().max(1) as f64;
    for (name, total) in [
        "inputs", "apply", "settle", "layout", "pages", "chrome", "frame",
    ]
    .iter()
    .zip(phase_totals)
    {
        println!("  type phase {name}: mean {:.3} ms", millis(total) / count);
        results.push((format!("type_phase_{name}_ms"), millis(total) / count));
    }
    summary("backspace", &deleting, &mut results);
    summary("undo", &undoing, &mut results);
    if only_typing {
        return;
    }

    // Paging and seeking.
    let mut paging = Vec::new();
    editor.jump_to_page(0);
    editor.paint(&mut frame);
    for _ in 0..40 {
        let started = Instant::now();
        editor.page(true, false);
        editor.paint(&mut frame);
        paging.push(started.elapsed());
    }
    for _ in 0..40 {
        let started = Instant::now();
        editor.page(false, false);
        editor.paint(&mut frame);
        paging.push(started.elapsed());
    }
    summary("page", &paging, &mut results);
    let mut seeking = Vec::new();
    for page in [1, pages / 2, pages, 2, pages - 1, pages / 3, pages / 4 * 3] {
        editor.open_prompt(PromptKind::GoToPage);
        editor.prompt_type(&page.to_string());
        let started = Instant::now();
        editor.commit_prompt();
        editor.paint(&mut frame);
        seeking.push(started.elapsed());
    }
    summary("seek", &seeking, &mut results);

    // Find.
    let mut finding = Vec::new();
    for query in ["archive", "clerk", "zzzz", "Chapter 50"] {
        editor.open_prompt(PromptKind::Find);
        editor.prompt_type(query);
        let started = Instant::now();
        editor.commit_prompt();
        editor.paint(&mut frame);
        finding.push(started.elapsed());
    }
    summary("find", &finding, &mut results);

    // Save and reopen.
    let scratch = tempfile::tempdir().expect("tempdir");
    let copy = scratch.path().join("copy.legebook");
    let started = Instant::now();
    editor.save_as_bundle(&copy).expect("save as");
    let save = started.elapsed();
    let started = Instant::now();
    let mut reopened = Editor::open_existing(&copy).expect("reopen");
    reopened.paint(&mut frame);
    let reopen = started.elapsed();
    println!(
        "save {:.2} ms, reopen+paint {:.2} ms, rss {:.1} MiB",
        millis(save),
        millis(reopen),
        rss_mib()
    );
    results.push(("save_ms".into(), millis(save)));
    results.push(("reopen_paint_ms".into(), millis(reopen)));
    results.push(("rss_mib".into(), rss_mib()));

    if let Some(out) = json {
        let mut text = String::from("{\n");
        for (index, (key, value)) in results.iter().enumerate() {
            text.push_str(&format!(
                "  \"{key}\": {value:.3}{}\n",
                if index + 1 < results.len() { "," } else { "" }
            ));
        }
        text.push_str("}\n");
        std::fs::write(&out, text).expect("write json");
        println!("wrote {}", out.display());
    }
}
