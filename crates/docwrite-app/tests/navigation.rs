//! Exact PageUp/PageDown and snap-at-gesture-end.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use docwrite_app::{Editor, Pager, Phase, PromptKind};
use docwrite_model::{Position, Selection};

fn manuscript() -> Editor {
    let mut editor = Editor::new_document("Long article");
    editor.type_text("An opening paragraph.\n");
    editor.type_text(&"A paragraph long enough to occupy several lines on the page. ".repeat(600));
    editor.refresh_layout();
    assert!(editor.pager().pages() >= 5);
    editor
}

fn caret_page(editor: &Editor) -> u32 {
    let focus = editor.book().selection().focus;
    let document = editor.document().unwrap();
    let paragraph = document.paragraph_of(focus.block.raw()).unwrap();
    let text = editor.book().block(focus.block).unwrap().text();
    let byte = text.chars().take(focus.offset).map(char::len_utf8).sum();
    document.page_of(paragraph, byte).unwrap()
}

#[test]
fn seeking_then_typing_edits_the_destination_page_and_reuses_layout() {
    let mut editor = manuscript();
    let generation = editor.layout_generation();
    for page in [1, 4, 2, editor.pager().pages()] {
        let before = editor.book().plain_text();
        assert!(editor.seek_page(page));
        editor.refresh_layout();
        assert_eq!(editor.pager().scroll(), f64::from(page - 1));
        assert_eq!(caret_page(&editor), page);
        assert_eq!(editor.book().plain_text(), before);
        assert_eq!(editor.layout_generation(), generation);
    }
    assert!(editor.seek_page(3));
    let selection = editor.book().selection();
    editor.type_text("HERE ");
    let text = editor.book().block(selection.focus.block).unwrap().text();
    assert!(
        text.chars()
            .skip(selection.focus.offset)
            .collect::<String>()
            .starts_with("HERE ")
    );
    editor.refresh_layout();
    assert_eq!(caret_page(&editor), 3);
    assert_eq!(editor.pager().scroll(), 2.0);
}

#[test]
fn page_prompt_validates_and_cancels_without_leaving_fullscreen() {
    let mut editor = manuscript();
    editor.seek_page(2);
    editor.set_fullscreen(true);
    editor.poll_fullscreen();
    editor.open_prompt(PromptKind::GoToPage);
    assert_eq!(editor.prompt_selected_text().as_deref(), Some("2"));
    let original = editor.book().selection();
    let text = editor.book().plain_text();
    for invalid in ["0", "99999999999999", "-1", "2.5", "no"] {
        editor.prompt_type(invalid);
        editor.commit_prompt();
        assert!(editor.prompt().unwrap().error.is_some());
        assert_eq!(editor.prompt_selected_text().as_deref(), Some(invalid));
        assert_eq!(editor.book().selection(), original);
        assert_eq!(editor.pager().scroll(), 1.0);
    }
    editor.prompt_backspace();
    editor.commit_prompt();
    assert!(
        editor.prompt().unwrap().error.is_some(),
        "empty input stays open"
    );
    editor.prompt_type("4");
    assert!(editor.prompt().unwrap().error.is_none());
    let mut frame = liminalkit_raster::WindowBuffer::new(900, 700);
    editor.paint(&mut frame);
    assert!(editor.is_fullscreen());
    assert!(editor.poll_fullscreen().is_none());
    assert!(
        editor.caret_area().unwrap().y < 34.0,
        "prompt caret is visible"
    );
    editor.commit_prompt();
    editor.paint(&mut frame);
    assert!(editor.prompt().is_none());
    assert_eq!(editor.pager().scroll(), 3.0);
    assert_eq!(caret_page(&editor), 4);
    assert!(editor.is_fullscreen());
    editor.open_prompt(PromptKind::GoToPage);
    editor.prompt_type("1");
    editor.cancel_prompt();
    assert_eq!(editor.pager().scroll(), 3.0);
    assert_eq!(editor.book().plain_text(), text);
}

#[test]
fn seek_shows_blank_recto_padding_without_bouncing_to_the_old_caret() {
    let mut editor = Editor::new_document("Blank verso");
    editor.type_text("First chapter");
    editor.new_chapter("Second");
    editor.type_text("Second chapter");
    editor.refresh_layout();
    assert_eq!(editor.pager().pages(), 3);
    assert!(editor.seek_page(2));
    editor.refresh_layout();
    assert_eq!(editor.pager().scroll(), 1.0);
    assert!(editor.document().unwrap().page_painted_lines(2).is_empty());
    editor.page_down();
    editor.refresh_layout();
    assert_eq!(editor.pager().scroll(), 2.0);
    assert_eq!(caret_page(&editor), 3);
    assert!(editor.seek_page(3));
    assert_eq!(
        caret_page(&editor),
        3,
        "chapter title is skipped for editable body"
    );
}

#[test]
fn page_down_and_page_up_land_on_page_tops_from_any_scroll() {
    let mut pager = Pager::new(400, false);
    pager.scroll_gesture(2.4, Phase::Moved);
    // Moved without Started treats delta as an absolute gesture from 0 when
    // origin is missing. Set a mid-page position by a started gesture.
    let mut pager = Pager::new(400, false);
    pager.scroll_gesture(0.0, Phase::Started);
    pager.scroll_gesture(2.4, Phase::Moved);
    assert!((pager.scroll() - 2.4).abs() < 1e-9);

    pager.page_down();
    assert_eq!(
        pager.scroll(),
        3.0,
        "PageDown from 2.4 is the next page top"
    );
    pager.page_up();
    assert_eq!(
        pager.scroll(),
        2.0,
        "PageUp from a page top is the previous top"
    );

    pager.scroll_gesture(0.0, Phase::Started);
    pager.scroll_gesture(0.2, Phase::Moved);
    assert!((pager.scroll() - 2.2).abs() < 1e-9);
    pager.page_up();
    assert_eq!(
        pager.scroll(),
        2.0,
        "PageUp from mid-page lands on this page top"
    );
}

#[test]
fn enlarged_final_page_can_scroll_to_its_bottom_and_page_up_returns_to_its_top() {
    let mut pager = Pager::new(2, false);
    pager.set_view_tail(0.2);
    pager.jump_to(1);
    pager.scroll_gesture(0.0, Phase::Started);
    pager.scroll_gesture(0.2, Phase::Ended);
    assert!((pager.scroll() - 1.2).abs() < 1e-9);
    assert_eq!(pager.page_top(), 1);
    pager.page_up();
    assert_eq!(pager.scroll(), 1.0);
}

#[test]
fn spread_mode_moves_by_spread_and_a_gesture_snaps() {
    let mut pager = Pager::new(20, true);
    pager.page_down();
    assert_eq!(pager.scroll(), 2.0);
    pager.page_down();
    assert_eq!(pager.scroll(), 4.0);
    pager.page_up();
    assert_eq!(pager.scroll(), 2.0);

    pager.scroll_gesture(0.0, Phase::Started);
    pager.scroll_gesture(1.2, Phase::Moved);
    assert!(
        (pager.scroll() - 3.2).abs() < 1e-9,
        "the gesture stays continuous"
    );
    pager.scroll_gesture(1.2, Phase::Ended);
    assert_eq!(pager.scroll(), 4.0, "3.2 snaps to the nearer spread");

    let mut single = Pager::new(20, false);
    single.scroll_gesture(0.0, Phase::Started);
    single.scroll_gesture(2.4, Phase::Ended);
    assert_eq!(single.scroll(), 2.0, "2.4 snaps to page 2");
    single.scroll_gesture(0.0, Phase::Started);
    single.scroll_gesture(0.6, Phase::Ended);
    assert_eq!(single.scroll(), 3.0, "2.6 snaps to page 3");
}

#[test]
fn the_window_editor_types_into_the_book_and_turns_pages() {
    let mut editor = Editor::new();
    let before = editor.book().plain_text();
    editor.type_text(" Hello");
    assert!(editor.book().plain_text().ends_with(" Hello"));
    assert_ne!(editor.book().plain_text(), before);
    // A one-page book has nowhere to page to.
    editor.refresh_layout();
    assert_eq!(editor.pager().pages(), 1);
    editor.page_down();
    assert_eq!(editor.pager().scroll(), 0.0);

    // Enough text for several pages: the pager follows the laid-out book.
    let paragraph = "A paragraph long enough to wrap over a few lines of the page. ".repeat(4);
    for _ in 0..40 {
        editor.type_text(&format!("\n{paragraph}"));
    }
    editor.refresh_layout();
    let pages = editor.pager().pages();
    assert!(pages >= 3, "forty paragraphs fill {pages} pages");
    assert_eq!(
        editor.pager().scroll(),
        f64::from(pages - 1),
        "typing follows the caret onto the last page"
    );
    editor.jump_to_page(1);
    editor.page_down();
    assert_eq!(editor.pager().scroll(), 1.0);
    editor.page_up();
    assert_eq!(editor.pager().scroll(), 0.0);
    editor.jump_to_page(pages);
    assert_eq!(editor.pager().scroll(), f64::from(pages - 1));
    editor.page_down();
    assert_eq!(
        editor.pager().scroll(),
        f64::from(pages - 1),
        "no page past the last"
    );
    editor.jump_to_page(1);

    let before = editor.book().plain_text().to_string();
    editor.set_preedit("未");
    assert_eq!(editor.book().plain_text(), before);
    assert_eq!(editor.preedit(), "未");
    editor.commit_ime("未");
    assert!(editor.book().plain_text().contains('未'));
    assert!(editor.preedit().is_empty());

    let ink = editor.painted_ink(900, 700);
    assert!(
        ink > 80,
        "the open page paints the manuscript, ink pixels {ink}"
    );
    assert!(
        editor.pages_painted() >= 1,
        "paint laid out {} pages",
        editor.pages_painted()
    );
    assert!(editor.caret_area().is_some(), "the caret is on the page");
}

#[test]
fn paging_keeps_the_caret_line_so_round_trips_never_drift() {
    let mut editor = Editor::new();
    let paragraph = "A paragraph long enough to wrap over a few lines of the page. ".repeat(4);
    for _ in 0..40 {
        editor.type_text(&format!("\n{paragraph}"));
    }
    editor.refresh_layout();
    let pages = editor.pager().pages();
    assert!(pages >= 3, "forty paragraphs fill {pages} pages");

    // The caret starts at the beginning of the second line of page one.
    editor.jump_to_page(1);
    let first = editor.book().block_ids()[0];
    editor
        .book_mut()
        .set_selection(Selection::collapsed(Position::new(first, 0)))
        .unwrap();
    editor.move_line(true, false);
    let home = editor.book().selection();
    let home_page = editor.pager().page_top();

    for _ in 0..pages - 1 {
        editor.page_down();
    }
    assert!(editor.pager().page_top() > home_page);
    for _ in 0..pages - 1 {
        editor.page_up();
    }
    assert_eq!(editor.pager().page_top(), home_page);
    assert_eq!(
        editor.book().selection(),
        home,
        "three pages down and back lands on the same line"
    );

    // Shift extends the selection to the new caret instead of moving it.
    editor.page(true, true);
    let selection = editor.book().selection();
    assert_eq!(selection.anchor, home.focus);
    assert_ne!(selection.focus, home.focus);
}
