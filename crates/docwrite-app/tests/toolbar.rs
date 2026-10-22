//! The toolbar's buttons, its prompts, and Book Map navigation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use docwrite_app::{Action, Editor, PromptKind};
use docwrite_model::{Align, BlockKind, Mark, NoteKind, Position, Selection};

fn frame(editor: &mut Editor) {
    let mut buffer = pixelkit_raster::WindowBuffer::new(1400, 800);
    editor.paint(&mut buffer);
}

fn click(editor: &mut Editor, action: Action) {
    click_at_width(editor, action, 1400);
}

fn click_at_width(editor: &mut Editor, action: Action, width: i32) {
    let button = editor
        .toolbar_buttons(width)
        .into_iter()
        .find(|button| button.action == action)
        .unwrap_or_else(|| panic!("no {action:?} button"));
    editor.hover(
        (button.x + button.w / 2) as f32,
        (button.y + button.h / 2) as f32,
    );
    editor.pointer(true);
    editor.pointer(false);
}

#[test]
fn narrow_and_scaled_toolbars_keep_every_action_without_overlapping() {
    let mut editor = Editor::new();
    let expected: Vec<_> = editor
        .toolbar_buttons(2000)
        .iter()
        .map(|button| button.action)
        .collect();
    assert_eq!(expected.len(), 18);
    for (width, scale) in [(640, 1.0), (1100, 1.0), (1100, 1.5), (1280, 2.0)] {
        editor.set_ui_scale(scale);
        let buttons = editor.toolbar_buttons(width);
        assert_eq!(
            buttons
                .iter()
                .map(|button| button.action)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(buttons.iter().all(|button| button.y == buttons[0].y));
        assert!(
            buttons.last().unwrap().x + buttons.last().unwrap().w <= width - (70.0 * scale) as i32
        );
        assert!(buttons[0].h <= (20.0 * scale) as i32);
        for (i, button) in buttons.iter().enumerate() {
            assert!(button.x >= 0 && button.x + button.w <= width);
            for other in &buttons[i + 1..] {
                assert!(
                    button.x + button.w <= other.x
                        || other.x + other.w <= button.x
                        || button.y + button.h <= other.y
                        || other.y + other.h <= button.y,
                    "overlapping controls: {button:?}, {other:?}"
                );
            }
        }
    }
}

#[test]
fn compact_chapter_and_note_controls_work_and_pages_stay_below_them() {
    let mut editor = Editor::new();
    let mut buffer = pixelkit_raster::WindowBuffer::new(640, 800);
    editor.paint(&mut buffer);
    click_at_width(&mut editor, Action::NewChapter, 640);
    editor.prompt_type("Narrow window chapter");
    editor.commit_prompt();
    editor.paint(&mut buffer);
    assert_eq!(
        editor.book().parts()[0].chapters()[1].title(),
        "Narrow window chapter"
    );
    click_at_width(&mut editor, Action::RenameChapter, 640);
    assert_eq!(editor.prompt().unwrap().text, "Narrow window chapter");
    editor.prompt_type(" renamed");
    editor.commit_prompt();
    editor.type_text("Text beneath the compact toolbar.");
    editor.paint(&mut buffer);
    click_at_width(&mut editor, Action::Footnote, 640);
    editor.prompt_type("A visible note control.");
    editor.commit_prompt();
    editor.paint(&mut buffer);
    let focus = editor.book().selection().focus.block;
    let note = editor.book().block(focus).unwrap().note().unwrap();
    assert_eq!(
        editor.book().note(note).unwrap().text(),
        "A visible note control."
    );
    let buttons = editor.toolbar_buttons(640);
    let toolbar_bottom = buttons.last().unwrap().y + buttons.last().unwrap().h;
    assert!(editor.caret_area().unwrap().y > f64::from(toolbar_bottom));

    // Resizing back to a wide window restores one row and working hit targets.
    let mut wide = pixelkit_raster::WindowBuffer::new(2000, 1000);
    editor.paint(&mut wide);
    assert!(
        editor
            .toolbar_buttons(2000)
            .iter()
            .all(|button| button.y == 2)
    );
    click_at_width(&mut editor, Action::Contents, 2000);
    assert!(editor.book().has_contents());
}

#[test]
fn a_prompt_hides_toolbar_actions_and_fullscreen_has_no_buttons() {
    let mut editor = Editor::new();
    frame(&mut editor);
    let bold = editor
        .toolbar_buttons(1400)
        .into_iter()
        .find(|button| button.action == Action::Mark(Mark::Bold))
        .unwrap();
    editor.open_prompt(PromptKind::NewChapter);
    editor.prompt_type("Keep this prompt");
    assert!(editor.toolbar_buttons(1400).is_empty());
    editor.hover((bold.x + bold.w / 2) as f32, (bold.y + bold.h / 2) as f32);
    editor.pointer(true);
    editor.pointer(false);
    assert_eq!(editor.prompt().unwrap().text, "Keep this prompt");
    assert!(!editor.book().selection_has(Mark::Bold));
    editor.cancel_prompt();
    editor.set_fullscreen(true);
    assert!(editor.toolbar_buttons(1400).is_empty());
}

#[test]
fn buttons_format_the_selection_and_show_its_state() {
    let mut editor = Editor::new();
    frame(&mut editor);
    let first = editor.book().block_ids()[0];
    editor
        .book_mut()
        .set_selection(Selection {
            anchor: Position::new(first, 0),
            focus: Position::new(first, 4),
        })
        .unwrap();
    frame(&mut editor);
    let bold = |editor: &Editor| {
        editor
            .toolbar_buttons(1400)
            .into_iter()
            .find(|button| button.action == Action::Mark(Mark::Bold))
            .unwrap()
            .active
    };
    assert!(!bold(&editor));
    click(&mut editor, Action::Mark(Mark::Bold));
    assert!(editor.book().selection_has(Mark::Bold));
    assert!(bold(&editor), "the button shows the selection is bold");
    editor.undo();
    assert!(!editor.book().selection_has(Mark::Bold));

    assert_eq!(editor.current_style_label(), "Body");
    click(&mut editor, Action::CycleStyle);
    assert_eq!(editor.current_style_label(), "Subhead");
    let kind = editor.book().block(first).unwrap().kind().clone();
    assert_eq!(kind, BlockKind::Subhead);
    editor.set_block_kind_number(2);
    assert_eq!(editor.current_style_label(), "Block Quote");

    let before = editor
        .book()
        .paragraph_styles()
        .iter()
        .find(|style| style.name == "Body")
        .unwrap()
        .size_pt;
    click(&mut editor, Action::Larger);
    let after = editor
        .book()
        .paragraph_styles()
        .iter()
        .find(|style| style.name == "Body")
        .unwrap()
        .size_pt;
    assert_eq!(after, before + 0.5);
}

#[test]
fn prompts_add_and_rename_chapters_and_attach_notes() {
    let mut editor = Editor::new();
    frame(&mut editor);
    click(&mut editor, Action::NewChapter);
    assert_eq!(
        editor.prompt().map(|prompt| prompt.kind),
        Some(PromptKind::NewChapter)
    );
    editor.prompt_type("The Clerk");
    assert!(editor.commit_prompt());
    assert!(editor.prompt().is_none());
    let titles: Vec<String> = editor.book().parts()[0]
        .chapters()
        .iter()
        .map(|chapter| chapter.title().to_string())
        .collect();
    assert_eq!(
        titles,
        vec!["Chapter 1".to_string(), "The Clerk".to_string()]
    );
    editor.type_text("Words of the new chapter.");
    assert!(
        editor
            .book()
            .plain_text()
            .ends_with("Words of the new chapter.")
    );

    editor.open_prompt(PromptKind::RenameChapter);
    assert_eq!(
        editor.prompt().unwrap().text,
        "The Clerk",
        "rename starts from the title"
    );
    for _ in 0.."Clerk".len() {
        editor.prompt_backspace();
    }
    editor.prompt_type("Archivist");
    editor.commit_prompt();
    assert_eq!(
        editor.book().parts()[0].chapters()[1].title(),
        "The Archivist"
    );

    editor.open_prompt(PromptKind::Footnote);
    editor.prompt_type("A note on the archivist.");
    editor.commit_prompt();
    let focus = editor.book().selection().focus.block;
    let note = editor.book().block(focus).unwrap().note().unwrap();
    assert_eq!(editor.book().note(note).unwrap().kind(), NoteKind::Footnote);

    editor.open_prompt(PromptKind::Endnote);
    editor.prompt_type("ignored");
    assert!(editor.cancel_prompt());
    assert_eq!(
        editor.book().note(note).unwrap().text(),
        "A note on the archivist."
    );
}

#[test]
fn a_chapter_row_in_the_book_map_goes_to_the_chapter() {
    let mut editor = Editor::new();
    editor.type_text(&" Body text for the first chapter.".repeat(60));
    editor.new_chapter("Second");
    editor.type_text("Second chapter text.");
    frame(&mut editor);
    editor.jump_to_page(1);
    frame(&mut editor);
    assert_eq!(editor.pager().scroll(), 0.0);
    let rows = editor.map_rows();
    let second = rows.iter().position(|row| row.title == "Second").unwrap();
    let (x, y) = Editor::map_row_center(second);
    editor.hover(x, y);
    editor.pointer(true);
    editor.pointer(false);
    frame(&mut editor);
    assert!(
        editor.pager().scroll() >= 1.0,
        "the view moved to the second chapter"
    );
    let focus = editor.book().selection().focus.block;
    let text = editor.book().block(focus).unwrap().text();
    assert!(text.starts_with("Second chapter"));
}

#[test]
fn prompts_index_terms_and_build_the_bibliography() {
    let mut editor = Editor::new();
    editor.open_prompt(PromptKind::IndexTerm);
    editor.prompt_type("opening");
    editor.commit_prompt();
    assert_eq!(editor.book().index_terms()[0].term, "opening");

    editor.open_prompt(PromptKind::BibliographyEntry);
    editor.prompt_type("Arendt, Hannah; The Human Condition; 1958");
    editor.commit_prompt();
    let entry = &editor.book().bibliography()[0];
    assert_eq!(entry.author, "Arendt, Hannah");
    assert_eq!(entry.title, "The Human Condition");
    assert_eq!(entry.issued, "1958");
    assert_eq!(entry.key, "arendt1958");

    let dir = std::env::temp_dir().join(format!("docwrite-bib-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bib = dir.join("refs.bib");
    std::fs::write(
        &bib,
        "@book{weber1922,\n  author = {Weber, Max},\n  title = {Economy and Society},\n  year = {1922}\n}\n",
    )
    .unwrap();
    editor.open_prompt(PromptKind::ImportBibliography);
    editor.prompt_type(bib.to_str().unwrap());
    editor.commit_prompt();
    assert_eq!(
        editor.book().bibliography().len(),
        2,
        "{:?}",
        editor.book().bibliography()
    );
    frame(&mut editor);
    let document = editor.document().unwrap();
    let headings: Vec<String> = (1..=document.page_count())
        .filter_map(|page| document.page_texts(page).first().cloned())
        .collect();
    assert!(headings.iter().any(|h| h == "Bibliography"), "{headings:?}");
    assert!(headings.iter().any(|h| h == "Index"), "{headings:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_alignment_button_cycles_and_direct_keys_set_each_side() {
    let mut editor = Editor::new();
    frame(&mut editor);
    let first = editor.book().block_ids()[0];
    assert_eq!(editor.current_alignment(), None);
    assert_eq!(editor.current_alignment_label(), "Style");

    for (label, align) in [
        ("Left", Align::Left),
        ("Center", Align::Center),
        ("Right", Align::Right),
        ("Justify", Align::Justify),
    ] {
        click(&mut editor, Action::CycleAlign);
        assert_eq!(editor.current_alignment_label(), label);
        assert_eq!(editor.book().block(first).unwrap().align(), Some(align));
    }
    click(&mut editor, Action::CycleAlign);
    assert_eq!(editor.current_alignment(), None, "back to the style");

    editor.set_alignment(Align::Center);
    assert_eq!(
        editor.book().block(first).unwrap().align(),
        Some(Align::Center)
    );
    frame(&mut editor);
    let document = editor.document().unwrap();
    let paragraph = document.paragraph_of(first.raw()).unwrap();
    let centered = (1..=document.page_count())
        .flat_map(|page| document.page_painted_lines(page))
        .any(|line| line.paragraph == paragraph && line.indent > 10.0);
    assert!(centered, "the override centers the laid-out line");
    editor.undo();
    assert_eq!(editor.book().block(first).unwrap().align(), None);
    editor.clear_alignment();
    assert_eq!(editor.current_alignment(), None);
}
