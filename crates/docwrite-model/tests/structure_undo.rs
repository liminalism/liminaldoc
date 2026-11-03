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
fn chapters_are_optional_and_naming_the_initial_flow_does_not_add_an_empty_chapter() {
    let mut book = Book::new("Plain document");
    book.insert("Opening words.").unwrap();
    assert_eq!(titles(&book), [""]);
    assert!(book.chapter_openers().is_empty());
    assert!(!book.has_structure());
    let block = book.block_ids()[0];
    let chapter = book
        .start_chapter_at(Position::new(block, 0), "Opening")
        .unwrap();
    assert_eq!(book.parts()[0].chapters().len(), 1);
    assert_eq!(book.parts()[0].chapters()[0].id(), chapter);
    assert!(book.has_structure());
    assert_eq!(book.plain_text(), "Opening words.");
    book.undo().unwrap();
    assert_eq!(titles(&book), [""]);
    book.redo().unwrap();
    assert_eq!(titles(&book), ["Opening"]);
}

#[test]
fn a_chapter_boundary_at_a_paragraph_start_keeps_the_original_blocks_and_text() {
    let mut book = Book::new("Draft");
    book.insert("first\nsecond\nthird").unwrap();
    let ids = book.block_ids();
    book.start_chapter_at(Position::new(ids[1], 0), "Middle")
        .unwrap();
    assert_eq!(book.block_ids(), ids);
    assert_eq!(book.plain_text(), "first\nsecond\nthird");
    assert_eq!(
        book.parts()[0].chapters()[0].sections()[0].blocks().len(),
        1
    );
    assert_eq!(
        book.parts()[0].chapters()[1].sections()[0].blocks().len(),
        2
    );
    book.undo().unwrap();
    assert_eq!(book.parts()[0].chapters().len(), 1);
    assert_eq!(book.block_ids(), ids);
    book.redo().unwrap();
    assert_eq!(book.parts()[0].chapters().len(), 2);
    book.check_consistency().unwrap();
}

#[test]
fn splitting_a_chapter_preserves_text_marks_notes_and_following_sections_through_undo_and_reopen() {
    use docwrite_model::{Align, Mark, NoteKind};
    let mut book = Book::new("Draft");
    book.insert("alpha café omega\nsecond paragraph").unwrap();
    let original = book.block_ids()[0];
    book.set_selection(Selection {
        anchor: Position::new(original, 6),
        focus: Position::new(original, 16),
    })
    .unwrap();
    book.toggle_mark(Mark::Bold).unwrap();
    book.set_align(original, Some(Align::Center)).unwrap();
    let note = book
        .attach_note(NoteKind::Footnote, "Keep this note")
        .unwrap();
    let initial = book.parts()[0].chapters()[0].id();
    let section = book.add_section(initial, "Later section").unwrap();
    let before = book.plain_text();
    let named = book
        .start_chapter_at(Position::new(original, 11), "New chapter")
        .unwrap();
    assert_eq!(titles(&book), ["", "New chapter"]);
    let tail = &book.parts()[0].chapters()[1].sections()[0].blocks()[0];
    assert_eq!(tail.text(), "omega");
    assert!(tail.runs().iter().all(|run| run.marks.bold));
    assert_eq!(tail.align(), Some(Align::Center));
    assert_eq!(book.block(original).unwrap().note(), Some(note));
    assert_eq!(book.parts()[0].chapters()[1].sections()[1].id(), section);
    let split_text = book.plain_text();
    book.check_consistency().unwrap();
    book.undo().unwrap();
    assert_eq!(book.plain_text(), before);
    assert_eq!(titles(&book), [""]);
    book.redo().unwrap();
    assert_eq!(book.plain_text(), split_text);
    assert_eq!(book.parts()[0].chapters()[1].id(), named);
    let root = std::env::temp_dir().join(format!("docwrite-chapter-split-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("split.legebook");
    book.save_bundle(&path).unwrap();
    let reopened = Book::load_bundle(&path).unwrap();
    assert_eq!(reopened.plain_text(), split_text);
    assert_eq!(reopened.parts()[0].chapters()[1].id(), named);
    assert_eq!(reopened.note(note).unwrap().text(), "Keep this note");
    std::fs::remove_dir_all(root).unwrap();
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
    assert_eq!(titles(&book), ["Three", "", "Deux"]);

    book.undo().unwrap(); // reorder
    assert_eq!(titles(&book), ["", "Deux", "Three"]);
    book.undo().unwrap(); // add Three
    book.undo().unwrap(); // rename
    assert_eq!(titles(&book), ["", "Two"]);
    assert_eq!(book.parts()[0].chapters()[1].id(), second);
    book.undo().unwrap(); // text
    assert_eq!(book.plain_text(), "First.\n");
    book.undo().unwrap(); // add Two
    assert_eq!(titles(&book), [""]);
    book.undo().unwrap(); // First.
    assert_eq!(book.plain_text(), "");
    assert!(book.undo().is_err());

    while book.can_redo() {
        book.redo().unwrap();
    }
    assert_eq!(titles(&book), ["Three", "", "Deux"]);
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
    assert_eq!(titles(&book), [""]);
    book.check_consistency().unwrap();
    book.undo().unwrap();
    assert_eq!(titles(&book), ["", "Two"]);
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
