//! Structural edits are undo transactions; typing bursts fold into one step.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use docwrite_model::{Book, Direction, Motion, Position, Selection};

fn titles(book: &Book) -> Vec<String> {
    book.parts()
        .iter()
        .flat_map(|part| part.chapters())
        .map(|chapter| chapter.title().to_string())
        .collect()
}

#[test]
fn chapter_operations_undo_and_redo_with_text_edits_in_order() {
    let mut book = Book::new("Essay");
    let part = book.parts()[0].id();
    book.insert("First.").unwrap();
    let second = book.add_chapter(part, "Two").unwrap();
    let block = book.parts()[0].chapters()[1].sections()[0].blocks()[0].id();
    book.set_selection(Selection::collapsed(Position::new(block, 0)))
        .unwrap();
    book.insert("Second.").unwrap();
    book.rename_chapter(second, "Deux").unwrap();
    book.add_chapter(part, "Three").unwrap();
    book.reorder_chapter(2, 0).unwrap();
    assert_eq!(titles(&book), ["Three", "Chapter 1", "Deux"]);

    book.undo().unwrap(); // reorder
    assert_eq!(titles(&book), ["Chapter 1", "Deux", "Three"]);
    book.undo().unwrap(); // add Three
    book.undo().unwrap(); // rename
    assert_eq!(titles(&book), ["Chapter 1", "Two"]);
    assert_eq!(book.parts()[0].chapters()[1].id(), second);
    book.undo().unwrap(); // text
    assert_eq!(book.plain_text(), "First.\n");
    book.undo().unwrap(); // add Two
    assert_eq!(titles(&book), ["Chapter 1"]);
    book.undo().unwrap(); // First.
    assert_eq!(book.plain_text(), "");
    assert!(book.undo().is_err());

    while book.can_redo() {
        book.redo().unwrap();
    }
    assert_eq!(titles(&book), ["Three", "Chapter 1", "Deux"]);
    assert_eq!(book.plain_text(), "\nFirst.\nSecond.");
    book.check_consistency().unwrap();
}

#[test]
fn delete_chapter_undoes_with_blocks_and_selection() {
    let mut book = Book::new("Essay");
    let part = book.parts()[0].id();
    let two = book.add_chapter(part, "Two").unwrap();
    let block = book.parts()[0].chapters()[1].sections()[0].blocks()[0].id();
    book.set_selection(Selection::collapsed(Position::new(block, 0)))
        .unwrap();
    book.insert("Keep me.").unwrap();
    let before = book.selection();
    book.delete_chapter(two).unwrap();
    assert_eq!(titles(&book), ["Chapter 1"]);
    book.check_consistency().unwrap();
    book.undo().unwrap();
    assert_eq!(titles(&book), ["Chapter 1", "Two"]);
    assert_eq!(book.plain_text(), "\nKeep me.");
    assert_eq!(book.selection(), before);
    assert!(book.delete_chapter(two).is_ok());
    let last = book.parts()[0].chapters()[0].id();
    assert!(book.delete_chapter(last).is_err());
}

#[test]
fn template_change_and_new_part_are_undoable() {
    let mut book = Book::new("Essay");
    let chapter = book.parts()[0].chapters()[0].id();
    let mut template = book.chapter_templates()[0].clone();
    template.name = "Plain".into();
    book.add_chapter_template(template).unwrap();
    book.apply_template(chapter, "Plain").unwrap();
    book.add_part("Back matter");
    assert_eq!(book.parts().len(), 2);
    book.undo().unwrap();
    assert_eq!(book.parts().len(), 1);
    book.undo().unwrap();
    assert_eq!(book.parts()[0].chapters()[0].template(), "Chapter");
    book.redo().unwrap();
    assert_eq!(book.parts()[0].chapters()[0].template(), "Plain");
}

#[test]
fn a_typing_burst_is_one_undo_step_per_word() {
    let mut book = Book::new("Essay");
    for ch in "hello world".chars() {
        book.type_text(&ch.to_string()).unwrap();
    }
    book.undo().unwrap();
    assert_eq!(book.plain_text(), "hello ");
    book.undo().unwrap();
    assert_eq!(book.plain_text(), "");
    book.redo().unwrap();
    book.redo().unwrap();
    assert_eq!(book.plain_text(), "hello world");
}

#[test]
fn breaks_paste_and_caret_motion_end_a_burst() {
    let mut book = Book::new("Essay");
    book.type_text("a").unwrap();
    book.type_text("b").unwrap();
    book.type_text("\n").unwrap();
    book.type_text("c").unwrap();
    book.insert("PASTE").unwrap();
    book.type_text("d").unwrap();
    book.move_caret(Motion::Char(Direction::Backward)).unwrap();
    book.type_text("e").unwrap();
    book.undo().unwrap(); // e
    book.undo().unwrap(); // d
    book.undo().unwrap(); // paste
    assert_eq!(book.plain_text(), "ab\nc");
    book.undo().unwrap(); // c
    book.undo().unwrap(); // newline
    assert_eq!(book.plain_text(), "ab");
    book.undo().unwrap(); // ab
    assert_eq!(book.plain_text(), "");
}
