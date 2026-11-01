//! The toolbar above the pages, and the one-line prompt it shows when a
//! command needs text (a chapter's name, a note).

use docwrite_model::{Align, BlockKind, Mark, NoteKind, Position, Selection};
use docwrite_typeset::Face;

use crate::{Editor, Prompt};

/// Height of one toolbar row in logical pixels.
pub const TOOLBAR_H: f32 = 24.0;

/// Paragraph kinds the style button cycles through, with their labels.
const KINDS: [(&str, BlockKind); 7] = [
    ("Body", BlockKind::Body),
    ("Subhead", BlockKind::Subhead),
    ("Block Quote", BlockKind::BlockQuote),
    ("Epigraph", BlockKind::Epigraph),
    ("Verse", BlockKind::Verse),
    ("Caption", BlockKind::Caption),
    ("Scene Break", BlockKind::SceneBreak),
];

/// Something the toolbar can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Open the File menu.
    FileMenu,
    /// A file command handled by the window host.
    File(crate::tabs::TabCommand),
    /// Toggle a character mark.
    Mark(Mark),
    /// Set the next paragraph style.
    CycleStyle,
    /// Set the next paragraph alignment.
    CycleAlign,
    /// Body text half a point smaller.
    Smaller,
    /// Body text half a point larger.
    Larger,
    /// Add a chapter.
    NewChapter,
    /// Rename the caret's chapter.
    RenameChapter,
    /// Attach a footnote.
    Footnote,
    /// Attach an endnote.
    Endnote,
    /// Open a source PDF.
    OpenSource,
    /// Show facing pages.
    Spread,
    /// Set or remove the table of contents.
    Contents,
}

/// What a prompt's text is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// A physical page number to navigate to.
    GoToPage,
    /// Application autosave interval in whole minutes.
    AutosaveInterval,
    /// The title of a new chapter.
    NewChapter,
    /// A new title for the caret's chapter.
    RenameChapter,
    /// The text of a footnote.
    Footnote,
    /// The text of an endnote.
    Endnote,
    /// The citation text for a source just captured.
    Citation,
    /// The path of a PDF to open beside the manuscript.
    OpenSource,
    /// Text to find.
    Find,
    /// What to replace; the prompt then asks what with.
    Replace,
    /// What to replace it with.
    ReplaceWith,
    /// A term to index the caret's paragraph under.
    IndexTerm,
    /// A bibliography entry as "Author; Title; Year".
    BibliographyEntry,
    /// A BibTeX (.bib) or CSL JSON file to import into the bibliography.
    ImportBibliography,
}

impl PromptKind {
    fn label(self) -> &'static str {
        match self {
            Self::GoToPage => "Go to page:",
            Self::AutosaveInterval => "Autosave minutes (15–120):",
            Self::NewChapter => "New chapter title:",
            Self::RenameChapter => "Rename chapter:",
            Self::Footnote => "Footnote:",
            Self::Endnote => "Endnote:",
            Self::Citation => "Cite as:",
            Self::OpenSource => "Open source PDF:",
            Self::Find => "Find:",
            Self::Replace => "Replace:",
            Self::ReplaceWith => "With:",
            Self::IndexTerm => "Index term:",
            Self::BibliographyEntry => "Author; Title; Year:",
            Self::ImportBibliography => "Import .bib or CSL JSON file:",
        }
    }
}

/// A toolbar button in window pixels.
#[derive(Clone, Debug)]
pub struct Button {
    /// What it does.
    pub action: Action,
    /// Its label.
    pub label: String,
    /// Lit: the state it toggles is on.
    pub active: bool,
    /// Left edge in window pixels.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

impl Editor {
    /// Toolbar height in window pixels; fullscreen reserves only an open prompt.
    pub fn toolbar_height(&self) -> i32 {
        if self.fullscreen && self.prompt.is_none() {
            0
        } else {
            (TOOLBAR_H * self.ui_scale).round() as i32
        }
    }

    /// A file command requested by a toolbar click, for the host's dialogs.
    pub fn take_file_command(&mut self) -> Option<crate::tabs::TabCommand> {
        self.file_command.take()
    }

    fn page_counter_width(&self) -> i32 {
        let digits = self.pager.pages().to_string().len();
        ((70.0_f32).max((digits * 2 + 3) as f32 * 6.0 + 12.0) * self.ui_scale).ceil() as i32
    }

    /// The paragraph style name of the caret's block.
    pub fn current_style_label(&self) -> &'static str {
        let focus = self.book.selection().focus;
        let kind = self
            .book
            .block(focus.block)
            .map(|block| block.kind().clone());
        match kind {
            Ok(BlockKind::ChapterTitle) => "Chapter Title",
            Ok(BlockKind::Image { .. }) => "Image",
            Ok(BlockKind::BibliographyEntry) => "Bibliography",
            Ok(kind) => KINDS
                .iter()
                .find(|(_, candidate)| *candidate == kind)
                .map_or("Body", |(label, _)| label),
            Err(_) => "Body",
        }
    }

    /// The toolbar's buttons, laid out for a window `width` pixels wide.
    pub fn toolbar_buttons(&self, width: i32) -> Vec<Button> {
        if self.fullscreen || self.prompt.is_some() {
            return Vec::new();
        }
        use crate::tabs::TabCommand;
        let s = self.ui_scale;
        let compact = width as f32 / s < 900.0;
        let mut x = (4.0 * s).round() as i32;
        let mut out = Vec::new();
        let mut push = |action, label: String, active, wide: f32, narrow: f32| {
            let w = ((if compact { narrow } else { wide }) * s).round() as i32;
            out.push(Button {
                action,
                label,
                active,
                x,
                y: (2.0 * s) as i32,
                w,
                h: (20.0 * s) as i32,
            });
            x += w + s.round().max(1.0) as i32;
        };
        // Plain "File": the UI face has no triangle glyph.
        push(
            Action::FileMenu,
            "File".into(),
            self.menu.is_some(),
            36.0,
            30.0,
        );
        push(
            Action::File(TabCommand::Save),
            "Save".into(),
            false,
            36.0,
            30.0,
        );
        for (mark, label, wide, narrow) in [
            (Mark::Bold, "B", 20.0, 16.0),
            (Mark::Italic, "I", 20.0, 16.0),
            (Mark::SmallCaps, "Sc", 24.0, 20.0),
            (Mark::Superscript, "x²", 24.0, 20.0),
            (Mark::Subscript, "x₂", 24.0, 20.0),
        ] {
            push(
                Action::Mark(mark),
                label.into(),
                self.book.selection_has(mark),
                wide,
                narrow,
            );
        }
        let style = match self.current_style_label() {
            "Block Quote" if compact => "Quote",
            "Scene Break" if compact => "Break",
            "Chapter Title" if compact => "Title",
            "Bibliography" if compact => "Biblio",
            label => label,
        };
        push(Action::CycleStyle, format!("¶ {style}"), false, 82.0, 48.0);
        let align = self.current_alignment_label();
        push(
            Action::CycleAlign,
            if compact {
                align.into()
            } else {
                format!("Align {align}")
            },
            false,
            64.0,
            40.0,
        );
        push(Action::Smaller, "A-".into(), false, 24.0, 20.0);
        push(Action::Larger, "A+".into(), false, 24.0, 20.0);
        push(Action::NewChapter, "+ Ch".into(), false, 42.0, 30.0);
        push(
            Action::RenameChapter,
            if compact { "Ren" } else { "Rename" }.into(),
            false,
            48.0,
            30.0,
        );
        push(
            Action::Footnote,
            if compact { "Fn" } else { "Footnote" }.into(),
            false,
            54.0,
            26.0,
        );
        push(
            Action::Endnote,
            if compact { "En" } else { "Endnote" }.into(),
            false,
            54.0,
            26.0,
        );
        push(
            Action::OpenSource,
            if compact { "Src" } else { "Source" }.into(),
            false,
            48.0,
            30.0,
        );
        push(
            Action::Spread,
            if compact { "2pg" } else { "Spread" }.into(),
            self.spread,
            48.0,
            30.0,
        );
        push(
            Action::Contents,
            "TOC".into(),
            self.book.has_contents(),
            36.0,
            30.0,
        );
        out
    }

    /// Run a toolbar action.
    pub fn run_action(&mut self, action: Action) {
        match action {
            Action::FileMenu => self.toggle_file_menu(),
            Action::File(command) => self.file_command = Some(command),
            Action::Mark(mark) => self.toggle_mark(mark),
            Action::CycleStyle => self.cycle_block_style(),
            Action::CycleAlign => self.cycle_alignment(),
            Action::Smaller => self.change_body_size(-0.5),
            Action::Larger => self.change_body_size(0.5),
            Action::NewChapter => self.open_prompt(PromptKind::NewChapter),
            Action::RenameChapter => self.open_prompt(PromptKind::RenameChapter),
            Action::Footnote => self.open_prompt(PromptKind::Footnote),
            Action::Endnote => self.open_prompt(PromptKind::Endnote),
            Action::OpenSource => self.open_prompt(PromptKind::OpenSource),
            Action::Spread => self.set_spread(!self.spread),
            Action::Contents => self.toggle_contents(),
        }
    }

    /// Set or remove the table of contents before the first chapter.
    pub fn toggle_contents(&mut self) {
        let on = !self.book.has_contents();
        self.book.set_contents(on);
        self.edited();
    }

    /// Toggle a character mark on the selection, or for what is typed next.
    pub fn toggle_mark(&mut self, mark: Mark) {
        let selected = !self.book.selection().is_collapsed();
        if self.book.toggle_mark(mark).is_ok() && selected {
            self.edited();
        }
    }

    /// Set every selected block (or the caret's) to paragraph kind `kind`.
    pub fn set_block_kind(&mut self, kind: BlockKind) {
        let selection = self.book.selection();
        let ids = self.book.block_ids();
        let at = |block| ids.iter().position(|id| *id == block);
        let (Some(a), Some(b)) = (at(selection.anchor.block), at(selection.focus.block)) else {
            return;
        };
        let (from, to) = (a.min(b), a.max(b));
        let mut changed = false;
        for id in &ids[from..=to] {
            if self.book.set_kind(*id, kind.clone()).is_ok() {
                changed = true;
            }
        }
        if changed {
            self.edited();
        }
    }

    /// Set the caret's block to the next paragraph kind in the style list.
    pub fn cycle_block_style(&mut self) {
        let current = self.current_style_label();
        let next = KINDS
            .iter()
            .position(|(label, _)| *label == current)
            .map_or(0, |index| (index + 1) % KINDS.len());
        self.set_block_kind(KINDS[next].1.clone());
    }

    /// Paragraph kinds by number, for the keyboard (0 = Body, 1 = Subhead, ...).
    pub fn set_block_kind_number(&mut self, number: usize) {
        if let Some((_, kind)) = KINDS.get(number) {
            self.set_block_kind(kind.clone());
        }
    }

    /// The caret block's direct alignment, if the alignment keys set one.
    pub fn current_alignment(&self) -> Option<Align> {
        let focus = self.book.selection().focus;
        self.book
            .block(focus.block)
            .ok()
            .and_then(|block| block.align())
    }

    /// The alignment button's label for the caret's block.
    pub fn current_alignment_label(&self) -> &'static str {
        match self.current_alignment() {
            None => "Style",
            Some(Align::Left) => "Left",
            Some(Align::Justify) => "Justify",
            Some(Align::Center) => "Center",
            Some(Align::Right) => "Right",
        }
    }

    /// Set every selected block (or the caret's) to `align`, directly. The
    /// paragraph style still decides every block without an override.
    pub fn set_alignment(&mut self, align: Align) {
        let selection = self.book.selection();
        let ids = self.book.block_ids();
        let at = |block| ids.iter().position(|id| *id == block);
        let (Some(a), Some(b)) = (at(selection.anchor.block), at(selection.focus.block)) else {
            return;
        };
        let (from, to) = (a.min(b), a.max(b));
        let mut changed = false;
        for id in &ids[from..=to] {
            if self.book.set_align(*id, Some(align)).is_ok() {
                changed = true;
            }
        }
        if changed {
            self.edited();
        }
    }

    /// Drop the direct alignment of every selected block (or the caret's),
    /// so the paragraph style decides again.
    pub fn clear_alignment(&mut self) {
        let selection = self.book.selection();
        let ids = self.book.block_ids();
        let at = |block| ids.iter().position(|id| *id == block);
        let (Some(a), Some(b)) = (at(selection.anchor.block), at(selection.focus.block)) else {
            return;
        };
        let (from, to) = (a.min(b), a.max(b));
        let mut changed = false;
        for id in &ids[from..=to] {
            if self.book.set_align(*id, None).is_ok() {
                changed = true;
            }
        }
        if changed {
            self.edited();
        }
    }

    /// Step the caret's block through Style, Left, Center, Right, Justify.
    pub fn cycle_alignment(&mut self) {
        match self.current_alignment() {
            None => self.set_alignment(Align::Left),
            Some(Align::Left) => self.set_alignment(Align::Center),
            Some(Align::Center) => self.set_alignment(Align::Right),
            Some(Align::Right) => self.set_alignment(Align::Justify),
            Some(Align::Justify) => self.clear_alignment(),
        }
    }

    /// Make the body text `delta` points larger (smaller when negative);
    /// the first paragraph follows, and leading keeps its proportion.
    pub fn change_body_size(&mut self, delta: f32) {
        for name in ["Body", "First Paragraph"] {
            let Some(mut style) = self
                .book
                .paragraph_styles()
                .iter()
                .find(|style| style.name == name)
                .cloned()
            else {
                continue;
            };
            let size = (style.size_pt + delta).clamp(6.0, 36.0);
            style.leading_pt *= size / style.size_pt;
            style.size_pt = size;
            let _ = self.book.set_paragraph_style(style);
        }
        self.edited();
    }

    /// Show a prompt in the toolbar; typing goes to it until Enter or Esc.
    pub fn open_prompt(&mut self, kind: PromptKind) {
        // Prompts and footnote sessions never overlap: the caret goes home.
        self.end_footnote();
        if kind == PromptKind::GoToPage {
            self.refresh_layout();
        }
        let text = match kind {
            PromptKind::GoToPage => (self.pager.page_top() + 1)
                .min(self.pager.pages())
                .to_string(),
            PromptKind::RenameChapter => self.current_chapter_title().unwrap_or_default(),
            PromptKind::Find | PromptKind::Replace => self.last_find.clone().unwrap_or_default(),
            _ => String::new(),
        };
        if self.fullscreen && !matches!(kind, PromptKind::GoToPage | PromptKind::AutosaveInterval) {
            self.set_fullscreen(false);
        }
        self.prompt = Some(Prompt::new(kind, text));
        if kind == PromptKind::GoToPage {
            self.prompt_select_all();
        }
    }

    /// The prompt being typed into, if any.
    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    /// Close the prompt without acting.
    pub fn cancel_prompt(&mut self) -> bool {
        self.prompt.take().is_some()
    }

    /// Act on the prompt's text. Invalid numeric settings remain open for correction.
    pub fn commit_prompt(&mut self) -> bool {
        let Some(mut prompt) = self.prompt.take() else {
            return false;
        };
        let text = prompt.text.trim().to_string();
        match prompt.kind {
            PromptKind::AutosaveInterval => match crate::autosave::AutosaveInterval::parse(&text) {
                Ok(interval) => self.autosave_request = Some(interval),
                Err(error) => {
                    prompt.error = Some(error);
                    self.prompt = Some(prompt);
                    self.prompt_select_all();
                }
            },
            PromptKind::GoToPage => {
                let page = text.parse::<u32>().ok();
                if !page.is_some_and(|page| self.seek_page(page)) {
                    prompt.error = Some(format!("Enter a page from 1 to {}:", self.pager.pages()));
                    self.prompt = Some(prompt);
                    self.prompt_select_all();
                }
            }
            PromptKind::NewChapter => {
                let title = if text.is_empty() {
                    "Untitled".to_string()
                } else {
                    text
                };
                self.new_chapter(&title);
            }
            PromptKind::RenameChapter => {
                if let Some(id) = self.current_chapter()
                    && self.book.rename_chapter(id, text).is_ok()
                {
                    self.edited();
                }
            }
            PromptKind::Citation => self.set_last_citation(&text),
            PromptKind::IndexTerm => {
                if !text.is_empty() {
                    let block = self.book.selection().focus.block;
                    self.book.add_index_term(text, block);
                    self.edited();
                }
            }
            PromptKind::BibliographyEntry => {
                let mut fields = text.split(';').map(str::trim);
                let author = fields.next().unwrap_or("").to_string();
                let title = fields.next().unwrap_or("").to_string();
                let issued = fields.next().unwrap_or("").to_string();
                if !title.is_empty() || !author.is_empty() {
                    let key = format!(
                        "{}{}",
                        author
                            .split([',', ' '])
                            .next()
                            .unwrap_or("ref")
                            .to_lowercase(),
                        issued
                    );
                    self.book
                        .add_bibliography(docwrite_model::BibliographyEntry {
                            key,
                            kind: "book".into(),
                            title,
                            author,
                            issued,
                        });
                    self.edited();
                }
            }
            PromptKind::ImportBibliography => {
                let path = text.trim_matches(|ch| ch == '"' || ch == '\'');
                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        let result = if path.ends_with(".bib") {
                            self.book.import_bibtex(&content)
                        } else {
                            self.book.import_csl_json(&content)
                        };
                        match result {
                            Ok(count) => {
                                eprintln!(
                                    "liminaldoc-writer: imported {count} bibliography entries"
                                );
                                self.edited();
                            }
                            Err(err) => eprintln!("liminaldoc-writer: {path}: {err}"),
                        }
                    }
                    Err(err) => eprintln!("liminaldoc-writer: {path}: {err}"),
                }
            }
            PromptKind::Find => {
                if !text.is_empty() {
                    self.find(&text);
                }
            }
            PromptKind::Replace => {
                if !text.is_empty() {
                    self.last_find = Some(text.clone());
                    self.replacing = Some(text);
                    self.open_prompt(PromptKind::ReplaceWith);
                }
            }
            PromptKind::ReplaceWith => {
                if let Some(query) = self.replacing.take() {
                    // The replacement is taken as typed, spaces included.
                    self.replace_all(&query, &prompt.text);
                }
            }
            PromptKind::OpenSource => {
                let path = text.trim_matches(|ch| ch == '"' || ch == '\'');
                if let Err(err) = self.open_source(path) {
                    eprintln!("liminaldoc-writer: {err}");
                }
            }
            PromptKind::Footnote | PromptKind::Endnote if !text.is_empty() => {
                let kind = if prompt.kind == PromptKind::Footnote {
                    NoteKind::Footnote
                } else {
                    NoteKind::Endnote
                };
                if self.book.attach_note(kind, &text).is_ok() {
                    self.edited();
                }
            }
            _ => {}
        }
        true
    }

    /// Add a chapter after the caret's chapter's part and put the caret in it.
    pub fn new_chapter(&mut self, title: &str) {
        let part = self
            .current_part()
            .or_else(|| self.book.parts().first().map(|part| part.id()));
        let Some(part) = part else {
            return;
        };
        if self.book.add_chapter(part, title).is_err() {
            return;
        }
        let chapter = self
            .book
            .parts()
            .iter()
            .find(|candidate| candidate.id() == part)
            .and_then(|part| part.chapters().last());
        let first = chapter.and_then(|chapter| {
            chapter
                .sections()
                .iter()
                .flat_map(|section| section.blocks())
                .next()
                .map(|block| block.id())
        });
        if let Some(block) = first {
            let _ = self
                .book
                .set_selection(Selection::collapsed(Position::new(block, 0)));
        }
        self.edited();
    }

    fn current_chapter(&self) -> Option<docwrite_model::ChapterId> {
        let focus = self.book.selection().focus.block;
        self.book.parts().iter().find_map(|part| {
            part.chapters()
                .iter()
                .find(|chapter| {
                    chapter
                        .sections()
                        .iter()
                        .any(|section| section.blocks().iter().any(|block| block.id() == focus))
                })
                .map(|chapter| chapter.id())
        })
    }

    fn current_part(&self) -> Option<docwrite_model::PartId> {
        let chapter = self.current_chapter()?;
        self.book
            .parts()
            .iter()
            .find(|part| {
                part.chapters()
                    .iter()
                    .any(|candidate| candidate.id() == chapter)
            })
            .map(|part| part.id())
    }

    fn current_chapter_title(&self) -> Option<String> {
        let chapter = self.current_chapter()?;
        self.book
            .parts()
            .iter()
            .flat_map(|part| part.chapters())
            .find(|candidate| candidate.id() == chapter)
            .map(|chapter| chapter.title().to_string())
    }

    /// Put the caret on the first line of chapter `chapter` and show its page.
    pub fn go_to_chapter(&mut self, chapter: docwrite_model::ChapterId) {
        let first = self
            .book
            .parts()
            .iter()
            .flat_map(|part| part.chapters())
            .find(|candidate| candidate.id() == chapter)
            .and_then(|chapter| {
                chapter
                    .sections()
                    .iter()
                    .flat_map(|section| section.blocks())
                    .next()
                    .map(|block| block.id())
            });
        if let Some(block) = first {
            let _ = self
                .book
                .set_selection(Selection::collapsed(Position::new(block, 0)));
            self.follow_caret = true;
            self.refresh_layout();
        }
    }

    /// The toolbar button under a window point.
    pub(crate) fn button_at(&self, x: f32, y: f32, width: i32) -> Option<Action> {
        self.toolbar_buttons(width)
            .into_iter()
            .find(|button| {
                x >= button.x as f32
                    && x < (button.x + button.w) as f32
                    && y >= button.y as f32
                    && y < (button.y + button.h) as f32
            })
            .map(|button| button.action)
    }

    /// Draw the toolbar (or the open prompt) across the top of the pages.
    pub(crate) fn paint_toolbar(
        &mut self,
        painter: &mut liminalkit_raster::Painter<'_>,
        face: &Face,
        width: i32,
        pages: u32,
    ) {
        let s = self.ui_scale;
        let left = 0;
        let height = self.toolbar_height();
        painter.fill_rect(
            liminalkit_raster::Rect::new(left, 0, width - left, height),
            self.theme_now.chrome,
        );
        painter.fill_rect(
            liminalkit_raster::Rect::new(
                left,
                height - (1.0 * s).max(1.0) as i32,
                width - left,
                (1.0 * s).max(1.0) as i32,
            ),
            self.theme_now.rule,
        );
        let size = if width as f32 / s < 900.0 { 10.0 } else { 11.0 } * s;
        let baseline = (16.0 * s) as i32;
        if let Some(prompt) = self.prompt.clone() {
            let label = prompt.error.clone().unwrap_or_else(|| {
                if prompt.kind == PromptKind::GoToPage {
                    format!("Go to page (1–{}):", self.pager.pages())
                } else {
                    prompt.kind.label().to_string()
                }
            });
            self.paint_label(
                painter,
                face,
                &label,
                left + (6.0 * s) as i32,
                baseline,
                size,
            );
            let label_w: f32 = face
                .shape(&label, size, &[])
                .map(|glyphs| glyphs.iter().map(|glyph| glyph.x_advance).sum())
                .unwrap_or(0.0);
            let field_x = left + (12.0 * s) as i32 + label_w as i32;
            painter.fill_rect(
                liminalkit_raster::Rect::new(
                    field_x,
                    (2.0 * s) as i32,
                    width - field_x - (6.0 * s) as i32,
                    (20.0 * s) as i32,
                ),
                self.theme_now.page,
            );
            let text_width = |end: usize| -> f32 {
                face.shape(&prompt.text[..end], size, &[])
                    .map(|glyphs| glyphs.iter().map(|glyph| glyph.x_advance).sum())
                    .unwrap_or(0.0)
            };
            let inset = (4.0 * s) as i32;
            let inner_x = field_x + inset;
            let inner_w = (width - (6.0 * s) as i32 - inset - inner_x).max(1);
            let caret_w = s.round().max(1.0) as i32;
            let caret_offset = text_width(prompt.caret()).round() as i32;
            let scroll = (caret_offset + caret_w - inner_w).max(0);
            let text_x = inner_x - scroll;
            let text_top = baseline - size.ceil() as i32;
            let text_h = size.ceil() as i32 + (4.0 * s) as i32;
            painter.push_clip(liminalkit_raster::Rect::new(
                inner_x,
                (2.0 * s) as i32,
                inner_w,
                (20.0 * s) as i32,
            ));
            let selected = prompt.selection();
            if !selected.is_empty() {
                let start = text_width(selected.start).round() as i32;
                let end = text_width(selected.end).round() as i32;
                painter.fill_rect(
                    liminalkit_raster::Rect::new(text_x + start, text_top, end - start, text_h),
                    self.theme_now.selection,
                );
            }
            self.paint_page_label(painter, face, &prompt.text, text_x, baseline, size);
            let caret_x = text_x + caret_offset;
            painter.fill_rect(
                liminalkit_raster::Rect::new(caret_x, text_top, caret_w, text_h),
                self.theme_now.ink,
            );
            painter.pop_clip();
            self.caret = Some(crate::CaretRect {
                x: f64::from(caret_x),
                y: f64::from(text_top),
                width: f64::from(caret_w),
                height: f64::from(text_h),
            });
            return;
        }
        for button in self.toolbar_buttons(width) {
            let fill = if button.active {
                self.theme_now.selection
            } else {
                self.theme_now.chrome
            };
            painter.fill_rect(
                liminalkit_raster::Rect::new(button.x, button.y, button.w, button.h),
                fill,
            );
            let label_w: f32 = face
                .shape(&button.label, size, &[])
                .map(|glyphs| glyphs.iter().map(|glyph| glyph.x_advance).sum())
                .unwrap_or(0.0);
            let x = button.x + ((button.w as f32 - label_w) / 2.0) as i32;
            painter.push_clip(liminalkit_raster::Rect::new(
                button.x, button.y, button.w, button.h,
            ));
            self.paint_label(painter, face, &button.label, x, baseline, size);
            painter.pop_clip();
        }
        if pages > 0 {
            let top = self.pager.page_top();
            let status = if self.spread {
                // Spread k shows pages 2k and 2k + 1; page 1 stands alone.
                match (top, (top + 1).min(pages)) {
                    (0, _) => format!("1 / {pages}"),
                    (left, right) if right > left => format!("{left}\u{2013}{right} / {pages}"),
                    (left, _) => format!("{left} / {pages}"),
                }
            } else {
                format!("{} / {pages}", top + 1)
            };
            let status_w: f32 = face
                .shape(&status, size, &[])
                .map(|glyphs| glyphs.iter().map(|glyph| glyph.x_advance).sum())
                .unwrap_or(0.0);
            let x = width - status_w as i32 - (5.0 * s) as i32;
            let reserved = self.page_counter_width();
            painter.fill_rect(
                liminalkit_raster::Rect::new(width - reserved, 0, reserved, height - 1),
                self.theme_now.chrome,
            );
            self.paint_label(painter, face, &status, x, baseline, size);
        }
    }
}
