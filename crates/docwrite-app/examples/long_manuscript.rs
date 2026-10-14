//! Prepare clipboard payloads or inspect a real, window-created long manuscript.
//! `cargo run --release -p docwrite-app --example long_manuscript -- prepare OUT_DIR`
//! `cargo run --release -p docwrite-app --example long_manuscript -- inspect BOOK.legebook`

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use docwrite_app::Editor;
use std::path::Path;
use std::time::Instant;

const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.";

fn payload(chapter: usize, paragraphs: usize) -> String {
    (1..=paragraphs)
        .map(|p| format!("C{chapter:02} P{p:04}. {LOREM}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn pages(text: &str) -> u32 {
    let mut editor = Editor::new_document("Calibration");
    editor.paste_text(text);
    editor.refresh_layout();
    editor.pager().pages()
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let path = Path::new(
        args.get(2)
            .expect("prepare OUT_DIR | inspect BOOK.legebook"),
    );
    if args[1] == "prepare" {
        std::fs::create_dir_all(path).unwrap();
        for chapter in 1..=7 {
            let target = if chapter == 7 { 20 } else { 30 };
            let (mut lo, mut hi) = (1, 500);
            while lo < hi {
                let mid = (lo + hi + 1) / 2;
                if pages(&payload(chapter, mid)) <= target {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            let text = payload(chapter, lo);
            assert_eq!(pages(&text), target);
            std::fs::write(path.join(format!("chapter-{chapter}.txt")), &text).unwrap();
            println!(
                "chapter {chapter}: {lo} paragraphs, {} bytes, {target} pages",
                text.len()
            );
        }
        return;
    }
    let started = Instant::now();
    let mut editor = Editor::open_existing(path).unwrap();
    println!("open bundle: {:?}", started.elapsed());
    let mut frame = pixelkit_raster::WindowBuffer::new(1100, 800);
    let started = Instant::now();
    editor.paint(&mut frame);
    println!(
        "first paint: {:?}; {} pages; {} words",
        started.elapsed(),
        editor.pager().pages(),
        editor.book().plain_text().split_whitespace().count()
    );
    assert_eq!(editor.pager().pages(), 200);
    let document = editor.document().unwrap();
    for (index, chapter) in editor
        .book()
        .parts()
        .iter()
        .flat_map(|part| part.chapters())
        .enumerate()
    {
        let text = chapter
            .sections()
            .iter()
            .flat_map(|section| section.blocks())
            .map(|block| block.text())
            .collect::<Vec<_>>()
            .join("\n");
        let expected = std::fs::read_to_string(
            path.parent()
                .unwrap()
                .join(format!("chapter-{}.txt", index + 1)),
        )
        .unwrap();
        assert_eq!(text, expected, "clipboard text for {}", chapter.title());
        let start = (1..=document.page_count())
            .find(|&page| {
                document
                    .page_texts(page)
                    .iter()
                    .any(|line| line.contains(chapter.title()))
            })
            .unwrap();
        println!(
            "{} starts at {start}; exact clipboard text retained",
            chapter.title()
        );
        assert_eq!(start, 1 + index as u32 * 30);
    }
    let generation = editor.layout_generation();
    for page in [1, 30, 31, 60, 61, 90, 91, 120, 121, 150, 151, 180, 181, 200] {
        let started = Instant::now();
        assert!(editor.seek_page(page));
        editor.paint(&mut frame);
        println!(
            "seek {page}: {:?}; scroll {}",
            started.elapsed(),
            editor.pager().scroll()
        );
        assert_eq!(editor.layout_generation(), generation);
    }
    for page in [1, 100, 200] {
        editor.seek_page(page);
        editor.paint(&mut frame);
        let started = Instant::now();
        editor.type_text("x");
        editor.paint(&mut frame);
        println!(
            "type at {page}: {:?}; {:?}",
            started.elapsed(),
            editor.last_layout()
        );
        editor.undo();
        editor.paint(&mut frame);
    }
    let started = Instant::now();
    let clone = editor.book().clone();
    println!("autosave snapshot clone: {:?}", started.elapsed());
    drop(clone);
    let all_text = editor.book().plain_text();
    let mut pasted = Editor::new_document("Single paste");
    pasted.paint(&mut frame);
    let started = Instant::now();
    pasted.paste_text(&all_text);
    let mutation = started.elapsed();
    pasted.paint(&mut frame);
    println!(
        "single {}-byte paste: mutation {mutation:?}; through paint {:?}; {} pages",
        all_text.len(),
        started.elapsed(),
        pasted.pager().pages()
    );
    assert_eq!(pasted.book().plain_text(), all_text);
    let started = Instant::now();
    pasted.undo();
    pasted.paint(&mut frame);
    println!("undo entire paste: {:?}", started.elapsed());
    assert_eq!(pasted.book().plain_text(), "");
    let started = Instant::now();
    pasted.redo();
    pasted.paint(&mut frame);
    println!("redo entire paste: {:?}", started.elapsed());
    assert_eq!(pasted.book().plain_text(), all_text);
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    editor
        .save_as_bundle(&dir.path().join("copy.legebook"))
        .unwrap();
    println!("save native bundle: {:?}", started.elapsed());
    assert_eq!(
        Editor::open_existing(&dir.path().join("copy.legebook"))
            .unwrap()
            .book()
            .plain_text(),
        all_text
    );
}
