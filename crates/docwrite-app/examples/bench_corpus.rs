//! Write the long-manuscript corpus the reader and writer benchmarks use.
//!
//! `cargo run --release -p docwrite-app --example bench_corpus -- OUT_DIR [CHAPTERS]`
//!
//! Produces, deterministically:
//! - `book.legebook`: a saved manuscript of `CHAPTERS` chapters (default 100,
//!   about 1,000 pages at the editor's default geometry);
//! - `book.pdf`: the same manuscript exported through the writer's PDF path,
//!   so the reader can be measured on a real typeset book;
//! - `book.md`: a Markdown rendering with headings, paragraphs, a list and a
//!   table per chapter, for the reader's Markdown engine.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use docwrite_model::{Book, ChapterStart, Position, Selection};

const PROSE: [&str; 4] = [
    "The archive kept its letters in bundles tied with string, and each bundle carried a \
date in a hand that changed over the years from careful to hurried. Reading them in order \
was like watching a clerk grow old, one season at a time, until the last bundle stopped \
mid-sentence.",
    "Nobody had catalogued the second room. Its shelves held ledgers whose spines had been \
relabelled so often that the paper had worn through to the board, and the clerk who knew \
which label was current had retired before anyone thought to ask.",
    "She worked by the window because the lamp hummed. The hum was not loud, but it sat \
exactly where her attention wanted to rest, and after an hour she would find that she had \
read the same page three times without taking in a word of it.",
    "The dates did not agree. The index said 1911; the string said older; the ink on the \
outermost letter had faded to the brown of weak tea, which the conservator put at forty \
years earlier than the catalogue allowed.",
];

fn noto() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../liminalkit/crates/liminalkit-text/test-fonts/NotoSans-Regular.ttf");
    std::fs::read(&path).expect("NotoSans-Regular.ttf beside the workspace")
}

fn chapter_body(chapter: usize, paragraphs: usize) -> String {
    (0..paragraphs)
        .map(|n| {
            format!(
                "{}.{}. {}",
                chapter + 1,
                n + 1,
                PROSE[(chapter + n) % PROSE.len()]
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_book(chapters: usize, paragraphs: usize) -> Book {
    let mut book = Book::new("The Archive");
    let mut template = book.chapter_templates()[0].clone();
    template.start = ChapterStart::NextPage;
    book.set_chapter_template(template).expect("template");
    book.insert(&chapter_body(0, paragraphs)).expect("insert");
    let part = book.parts()[0].id();
    for chapter in 1..chapters {
        book.add_chapter(part, format!("Chapter {}", chapter + 1))
            .expect("chapter");
        let fresh = *book.block_ids().last().expect("block");
        book.set_selection(Selection::collapsed(Position::new(fresh, 0)))
            .expect("select");
        book.insert(&chapter_body(chapter, paragraphs))
            .expect("insert");
    }
    book
}

fn markdown(chapters: usize, paragraphs: usize) -> String {
    let mut out = String::from("# The Archive\n\n");
    for chapter in 0..chapters {
        let _ = writeln!(out, "## Chapter {}\n", chapter + 1);
        for n in 0..paragraphs {
            let prose = PROSE[(chapter + n) % PROSE.len()];
            if n % 7 == 3 {
                let _ = writeln!(out, "> {prose}\n");
            } else if n % 11 == 5 {
                let _ = writeln!(
                    out,
                    "- first point of {}.{}\n- second point with *emphasis*\n- third point with `code`\n",
                    chapter + 1,
                    n + 1
                );
            } else {
                let _ = writeln!(out, "{}.{}. {prose}\n", chapter + 1, n + 1);
            }
        }
        let _ = writeln!(
            out,
            "| Bundle | Date | Letters |\n|---|---|---|\n| {} | 1911 | 24 |\n| {} | 1890 | 7 |\n",
            chapter * 2 + 1,
            chapter * 2 + 2
        );
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(
        args.first()
            .expect("usage: bench_corpus OUT_DIR [CHAPTERS]"),
    );
    let chapters: usize = args.get(1).map_or(100, |n| n.parse().expect("chapters"));
    let paragraphs = 40;
    std::fs::create_dir_all(dir).expect("out dir");

    let started = Instant::now();
    let mut book = build_book(chapters, paragraphs);
    println!(
        "built {chapters} chapters, {} blocks in {:?}",
        book.block_ids().len(),
        started.elapsed()
    );

    let started = Instant::now();
    let bundle = dir.join("book.legebook");
    book.save_bundle(&bundle).expect("save bundle");
    println!("wrote {} in {:?}", bundle.display(), started.elapsed());

    let started = Instant::now();
    let export = docwrite_export::export_pdf(&book, &noto()).expect("export pdf");
    let pdf = dir.join("book.pdf");
    std::fs::write(&pdf, &export.bytes).expect("write pdf");
    println!(
        "wrote {} ({} bytes) in {:?}",
        pdf.display(),
        export.bytes.len(),
        started.elapsed()
    );

    let md = dir.join("book.md");
    let text = markdown(chapters, paragraphs);
    std::fs::write(&md, &text).expect("write markdown");
    println!("wrote {} ({} bytes)", md.display(), text.len());
}
