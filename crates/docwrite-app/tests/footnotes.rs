//! The footnote hotkey: it moves to the footnote line, typing fills the
//! note, and Enter returns to the document line.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use docwrite_app::{Editor, PromptKind};
use docwrite_model::{NoteKind, Position, Selection};

fn frame(editor: &mut Editor) {
    let mut buffer = pixelkit_raster::WindowBuffer::new(1100, 800);
    editor.paint(&mut buffer);
}

#[test]
fn the_hotkey_types_a_line_and_enter_returns() {
    let mut editor = Editor::new();
    editor.type_text("A line with a claim.");
    let anchor = editor.book().selection();
    frame(&mut editor);

    assert!(editor.start_footnote());
    assert!(editor.in_footnote());
    editor.footnote_type("The source for the claim.");
    frame(&mut editor);
    assert_eq!(
        editor.footnote_body().as_deref(),
        Some("The source for the claim.")
    );
    let caret = editor
        .caret_area()
        .expect("the footnote caret paints at the foot of the page");
    assert!(
        caret.y + caret.height <= 800.0,
        "the session lifts the note into view: {caret:?}"
    );

    assert!(editor.end_footnote());
    assert!(!editor.in_footnote());
    assert_eq!(editor.book().selection(), anchor, "Enter goes home");
    let block = anchor.focus.block;
    let note = editor.book().block(block).unwrap().note().unwrap();
    assert_eq!(editor.book().note(note).unwrap().kind(), NoteKind::Footnote);
    assert_eq!(
        editor.book().note(note).unwrap().text(),
        "The source for the claim."
    );
    frame(&mut editor);
    assert!(
        editor.caret_area().is_some(),
        "the document caret is back on its line"
    );
}

#[test]
fn reopening_a_footnote_continues_from_the_end() {
    let mut editor = Editor::new();
    editor.type_text("A line.");
    assert!(editor.start_footnote());
    editor.footnote_type("First half.");
    assert!(editor.end_footnote());

    assert!(editor.start_footnote(), "the block already has a footnote");
    editor.footnote_type(" Second half.");
    assert!(editor.end_footnote());
    let block = editor.book().selection().focus.block;
    let note = editor.book().block(block).unwrap().note().unwrap();
    assert_eq!(
        editor.book().note(note).unwrap().text(),
        "First half. Second half."
    );
}

#[test]
fn footnote_edits_move_within_the_note_and_fold_into_one_undo() {
    let mut editor = Editor::new();
    editor.type_text("A line.");
    assert!(editor.start_footnote());
    editor.footnote_type("abc");
    editor.footnote_move(true);
    editor.footnote_backspace();
    assert_eq!(editor.footnote_body().as_deref(), Some("ac"));
    editor.footnote_home();
    editor.footnote_type("z");
    assert_eq!(editor.footnote_body().as_deref(), Some("zac"));
    editor.footnote_end();
    editor.footnote_delete_forward();
    assert_eq!(editor.footnote_body().as_deref(), Some("zac"));
    editor.footnote_move(true);
    editor.footnote_delete_forward();
    assert_eq!(editor.footnote_body().as_deref(), Some("za"));
    assert!(editor.end_footnote());

    editor.undo();
    assert_eq!(
        editor.footnote_body(),
        None,
        "undo closes over the whole typed note"
    );
    let block = editor.book().selection().focus.block;
    let note = editor.book().block(block).unwrap().note().unwrap();
    assert_eq!(editor.book().note(note).unwrap().text(), "");
    editor.redo();
    let note = editor.book().block(block).unwrap().note().unwrap();
    assert_eq!(editor.book().note(note).unwrap().text(), "za");
}

#[test]
fn an_endnote_keeps_its_slot() {
    let mut editor = Editor::new();
    editor.type_text("A line.");
    editor.open_prompt(PromptKind::Endnote);
    editor.prompt_type("An endnote.");
    editor.commit_prompt();
    assert!(
        !editor.start_footnote(),
        "the hotkey never clobbers an endnote"
    );
    assert!(!editor.in_footnote());
}

#[test]
fn a_click_abandons_without_returning_and_a_prompt_returns() {
    let mut editor = Editor::new();
    editor.type_text("First line.\nSecond line.\nThird line.");
    // The Book Map only shows once the book has a structure.
    editor.new_chapter("Two");
    editor.go_to_chapter(editor.book().parts()[0].chapters()[0].id());
    editor.move_document_end(false);
    let anchor = editor.book().selection();
    frame(&mut editor);
    assert!(editor.start_footnote());
    editor.footnote_type("A note.");
    // A click on the chapter row goes where it lands.
    let rows = editor.map_rows();
    let chapter = rows
        .iter()
        .position(|row| row.title == "Chapter 1")
        .unwrap();
    let (x, y) = Editor::map_row_center(chapter);
    editor.hover(x, y);
    editor.pointer(true);
    editor.pointer(false);
    assert!(!editor.in_footnote());
    assert_ne!(
        editor.book().selection().focus.block,
        anchor.focus.block,
        "the click wins over the anchor"
    );
    let note = editor
        .book()
        .block(anchor.focus.block)
        .unwrap()
        .note()
        .unwrap();
    assert_eq!(
        editor.book().note(note).unwrap().text(),
        "A note.",
        "abandoning keeps the typed words"
    );

    // A prompt returns to the anchor instead.
    editor
        .book_mut()
        .set_selection(Selection::collapsed(Position::new(anchor.focus.block, 0)))
        .unwrap();
    assert!(editor.start_footnote());
    editor.open_prompt(PromptKind::Find);
    assert!(!editor.in_footnote());
    assert_eq!(
        editor.book().selection().focus.block,
        anchor.focus.block,
        "the prompt sends the caret home"
    );
}
