//! Saves are validated, keep assets, and fall back to the last good generation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;

use docwrite_model::{Book, BundleOpen};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("docwrite-durable-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn assets_survive_every_save() {
    let root = scratch("assets");
    let path = root.join("book.legebook");
    let mut book = Book::new("Essay");
    book.insert("Text.").unwrap();
    book.save_bundle(&path).unwrap();
    fs::create_dir_all(path.join("assets")).unwrap();
    fs::write(path.join("assets/fig.png"), b"png-bytes").unwrap();
    book.insert(" More.").unwrap();
    book.save_bundle(&path).unwrap();
    book.save_bundle(&path).unwrap();
    assert_eq!(fs::read(path.join("assets/fig.png")).unwrap(), b"png-bytes");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_corrupt_live_generation_falls_back_to_the_old_one() {
    let root = scratch("corrupt");
    let path = root.join("book.legebook");
    let mut book = Book::new("Essay");
    book.insert("Good draft.").unwrap();
    book.save_bundle(&path).unwrap();
    // An interrupted swap: the old generation beside a damaged live one.
    fs::rename(&path, root.join(".book.legebook.old")).unwrap();
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("book.json"), "{ truncated").unwrap();
    match Book::open_bundle(&path).unwrap() {
        BundleOpen::Recovered(recovered) => assert_eq!(recovered.plain_text(), "Good draft."),
        other => panic!("expected recovery, got {other:?}"),
    }
    // Saving replaces the damaged generation and keeps a copy of it aside.
    book.save_bundle(&path).unwrap();
    assert!(root.join(".book.legebook.damaged").exists());
    assert!(!root.join(".book.legebook.old").exists());
    assert_eq!(
        Book::load_bundle(&path).unwrap().plain_text(),
        "Good draft."
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_corrupt_live_generation_without_a_fallback_is_an_error() {
    let root = scratch("nofallback");
    let path = root.join("book.legebook");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("book.json"), "nope").unwrap();
    assert!(Book::open_bundle(&path).is_err());
    let _ = fs::remove_dir_all(&root);
}
