//! Prompt editing must leave the manuscript alone until the command commits.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use docwrite_app::{Editor, PromptKind, PromptMotion};

#[test]
fn rename_edits_in_the_middle_and_changes_the_title_only_on_commit() {
    let mut editor = Editor::new();
    let manuscript = editor.book().plain_text();
    let selection = editor.book().selection();
    editor.open_prompt(PromptKind::RenameChapter);
    editor.prompt_move(PromptMotion::Home, false);
    editor.prompt_move(PromptMotion::Right, false);
    editor.prompt_type("h");
    assert_eq!(editor.prompt().unwrap().text, "Chhapter 1");
    editor.prompt_delete_forward();
    assert_eq!(editor.prompt().unwrap().text, "Chapter 1");
    editor.prompt_move(PromptMotion::End, false);
    editor.prompt_move(PromptMotion::Left, true);
    assert_eq!(editor.prompt_selected_text().as_deref(), Some("1"));
    editor.prompt_type("One");
    assert_eq!(editor.prompt().unwrap().text, "Chapter One");
    assert_eq!(editor.book().plain_text(), manuscript);
    assert_eq!(editor.book().selection(), selection);
    assert_eq!(editor.book().parts()[0].chapters()[0].title(), "Chapter 1");
    editor.commit_prompt();
    assert_eq!(
        editor.book().parts()[0].chapters()[0].title(),
        "Chapter One"
    );
    editor.open_prompt(PromptKind::RenameChapter);
    editor.prompt_select_all();
    editor.prompt_type("Discard this title");
    editor.cancel_prompt();
    assert_eq!(
        editor.book().parts()[0].chapters()[0].title(),
        "Chapter One"
    );
}

#[test]
fn selection_cut_and_deletion_preserve_complete_graphemes() {
    let mut editor = Editor::new();
    editor.open_prompt(PromptKind::Find);
    editor.prompt_type("Ae\u{301}👩‍💻Z");
    editor.prompt_move(PromptMotion::Left, true);
    assert_eq!(editor.prompt_cut().as_deref(), Some("Z"));
    editor.prompt_backspace();
    assert_eq!(editor.prompt().unwrap().text, "Ae\u{301}");
    editor.prompt_move(PromptMotion::Left, false);
    assert_eq!(editor.prompt().unwrap().caret(), 1);
    editor.prompt_delete_forward();
    assert_eq!(editor.prompt().unwrap().text, "A");
    editor.prompt_type("\u{301}");
    editor.prompt_backspace();
    assert_eq!(editor.prompt().unwrap().text, "");
    editor.prompt_type("🇮🇩!");
    editor.prompt_move(PromptMotion::Home, false);
    editor.prompt_delete_forward();
    assert_eq!(editor.prompt().unwrap().text, "!");

    editor.prompt_select_all();
    editor.prompt_type("");
    assert_eq!(editor.prompt_selected_text().as_deref(), Some("!"));
    editor.prompt_type("new\r\ntext\t");
    assert_eq!(editor.prompt().unwrap().text, "newtext");
    editor.prompt_move(PromptMotion::Home, true);
    editor.prompt_move(PromptMotion::Right, false);
    assert_eq!(editor.prompt().unwrap().caret(), "newtext".len());
    assert_eq!(editor.prompt_cut(), None);
    editor.prompt_select_all();
    editor.prompt_move(PromptMotion::Left, false);
    assert_eq!(editor.prompt().unwrap().caret(), 0);
    editor.prompt_backspace();
    assert_eq!(editor.prompt().unwrap().text, "newtext");
    editor.cancel_prompt();
    assert!(editor.prompt().is_none());
}

#[test]
fn long_prompt_selection_and_caret_are_painted_inside_the_field() {
    let mut editor = Editor::new();
    editor.open_prompt(PromptKind::RenameChapter);
    editor.prompt_select_all();
    editor.prompt_type(&"A very long chapter title with many words. ".repeat(20));
    for (width, scale) in [(640, 1.0), (1100, 1.5)] {
        editor.set_ui_scale(scale);
        editor.prompt_move(PromptMotion::End, false);
        let mut buffer = pixelkit_raster::WindowBuffer::new(width, 800);
        editor.paint(&mut buffer);
        let end = editor.caret_area().unwrap();
        assert!(end.x > 0.0 && end.x + end.width < f64::from(width));
        assert!(end.y > 0.0 && end.y + end.height <= f64::from(34.0 * scale));
        assert_eq!(
            buffer.pixels[end.y as usize * width as usize + end.x as usize],
            editor.theme().ink
        );

        editor.prompt_select_all();
        editor.paint(&mut buffer);
        let chrome_end = ((34.0 * scale).ceil() as usize) * width as usize;
        assert!(buffer.pixels[..chrome_end].contains(&editor.theme().selection));
        editor.prompt_move(PromptMotion::Home, false);
        editor.paint(&mut buffer);
        assert!(editor.caret_area().unwrap().x < end.x);
        assert!(!buffer.pixels[..chrome_end].contains(&editor.theme().selection));
    }
}
