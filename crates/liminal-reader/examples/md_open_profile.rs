//! Time the stages of opening a Markdown document: read, parse, layout.
//!
//! `cargo run --release -p liminaldoc-reader --example md_open_profile -- notes.md [repeats]`

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Instant;

use liminaldoc_reader::document::DocumentEngine;
use liminaldoc_reader::document::markdown::engine::MarkdownEngine;
use liminaldoc_reader::document::markdown::layout::{MdFonts, layout_document};
use liminaldoc_reader::document::markdown::model::parse_markdown;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().expect("usage: md_open_profile FILE [repeats]");
    let repeats: usize = args.get(1).map_or(3, |n| n.parse().expect("repeats"));
    let bytes = std::fs::read(path).expect("read");
    let text = String::from_utf8_lossy(&bytes).into_owned();
    println!("{path}: {} bytes", bytes.len());
    for round in 0..repeats {
        let started = Instant::now();
        let blocks = parse_markdown(&text);
        let parsed = started.elapsed();
        let fonts_started = Instant::now();
        let mut fonts = MdFonts::new();
        let fonts_ready = fonts_started.elapsed();
        let layout_started = Instant::now();
        let pages = layout_document(&mut fonts, &blocks);
        let laid = layout_started.elapsed();
        let engine_started = Instant::now();
        let engine = MarkdownEngine::open(std::path::Path::new(path)).expect("open");
        let opened = engine_started.elapsed();
        println!(
            "round {round}: parse {:.2} ms ({} blocks), fonts {:.2} ms, layout {:.2} ms ({} pages), full open {:.2} ms ({} pages)",
            parsed.as_secs_f64() * 1e3,
            blocks.len(),
            fonts_ready.as_secs_f64() * 1e3,
            laid.as_secs_f64() * 1e3,
            pages.len(),
            opened.as_secs_f64() * 1e3,
            engine.descriptor().page_count
        );
    }
}
