//! Editing, file and caret commands the window binds to keys and the pointer.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use docwrite_model::{Book, BundleOpen, Direction, Motion, NoteId, NoteKind, Position, Selection};
use docwrite_typeset::{Document, FootnoteLine, NOTE_MARK_CLUSTER, PaintedLine};

use crate::{Editor, FootnoteEdit, byte_at, focus_mark};

/// Where the last paint put one page, in window pixels.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PageSlot {
    pub page: u32,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f32,
}

impl Editor {
    /// Start a blank document that has no save destination yet.
    pub fn new_document(title: impl Into<String>) -> Self {
        Self::with_book(Book::new(title))
    }

    /// Open a native bundle or import a text-bearing standard format into an
    /// unsaved document. Imports never bind their source file as a save target.
    pub fn open_document(path: &Path) -> Result<Self, String> {
        if path.is_dir() || path.extension().is_some_and(|ext| ext == "legebook") {
            return Self::open_existing(path);
        }
        let book = crate::document_io::import_book(path)?;
        let mut editor = Self::with_book(book);
        editor.dirty = true;
        Ok(editor)
    }

    /// Save this manuscript to a new native bundle and bind later saves to it.
    /// Existing unrelated paths and interrupted save generations are refused.
    pub fn save_as_bundle(&mut self, path: &Path) -> Result<(), String> {
        self.wait_for_background_save()?;
        if self.bundle.as_deref() == Some(path) {
            self.dirty = true;
            return self.save_now();
        }
        match Book::open_bundle(path).map_err(|err| err.to_string())? {
            BundleOpen::New => {}
            BundleOpen::Existing(_) | BundleOpen::Recovered(_) => {
                return Err(format!("{} already contains a book", path.display()));
            }
        }
        self.book.save_bundle(path).map_err(|err| err.to_string())?;
        self.bundle = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    /// Open the bundle at `path`, or start a new book that will be saved
    /// there on the first save.
    pub fn open_or_create(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let (book, fresh) = match Book::open_bundle(&path).map_err(|err| err.to_string())? {
            BundleOpen::Existing(book) | BundleOpen::Recovered(book) => (book, false),
            BundleOpen::New => {
                let title = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Untitled".into());
                (Book::new(title), true)
            }
        };
        let mut editor = Self::with_book(book);
        editor.open_bundle(path);
        editor.dirty = fresh;
        Ok(editor)
    }

    /// Open an existing manuscript, including an interrupted save generation.
    pub fn open_existing(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let book = Book::load_bundle(&path).map_err(|err| err.to_string())?;
        let mut editor = Self::with_book(book);
        editor.open_bundle(path);
        Ok(editor)
    }

    /// The bundle this editor saves to, if any.
    pub fn bundle(&self) -> Option<&Path> {
        self.bundle.as_deref()
    }

    /// Whether an edit has landed since the last save started.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Keep polling until a background save has reported success or failure.
    pub fn needs_save_tick(&self) -> bool {
        self.dirty
            || self.saving.load(Ordering::Acquire)
            || self.save_error.lock().is_ok_and(|slot| slot.is_some())
    }

    /// Save now, after any background save in flight. For Cmd-S and exit.
    pub fn save_now(&mut self) -> Result<(), String> {
        self.wait_for_background_save()?;
        if let Some(error) = self.take_save_error() {
            self.dirty = true;
            return Err(error);
        }
        self.autosave()
    }

    /// Start a background save when the book has been dirty and untouched
    /// for `idle`. Serialization runs on its own thread against a copy of
    /// the book, so typing never waits on the disk. Returns whether a save
    /// started.
    pub fn autosave_if_idle(&mut self, idle: Duration) -> bool {
        if !self.dirty || self.saving.load(Ordering::Acquire) {
            return false;
        }
        if self.last_edit.is_some_and(|last| last.elapsed() < idle) {
            return false;
        }
        let Some(path) = self.bundle.clone() else {
            return false;
        };
        let mut copy = self.book.clone();
        let saving = self.saving.clone();
        let error = self.save_error.clone();
        saving.store(true, Ordering::Release);
        self.dirty = false;
        std::thread::spawn(move || {
            if let Err(err) = copy.save_bundle(&path)
                && let Ok(mut slot) = error.lock()
            {
                *slot = Some(err.to_string());
            }
            saving.store(false, Ordering::Release);
        });
        true
    }

    /// The error from the last background save, if it failed. Taking it
    /// marks the book dirty again so the next save retries.
    pub fn take_save_error(&mut self) -> Option<String> {
        let error = self.save_error.lock().ok().and_then(|mut slot| slot.take());
        if error.is_some() {
            self.dirty = true;
        }
        error
    }

    pub(crate) fn wait_for_background_save(&self) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.saving.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        if self.saving.load(Ordering::Acquire) {
            Err("background save is still running; no second save was started".into())
        } else {
            Ok(())
        }
    }

    /// Write the book as a typeset PDF at `path`.
    pub fn export_pdf_to(&mut self, path: &Path) -> Result<(), String> {
        self.ensure_fonts();
        if self.font_files.is_empty() {
            return Err("no font is loaded".to_string());
        }
        let pdf = docwrite_export::export_pdf_family(&self.book, &self.font_files)?;
        write_atomically(path, &pdf.bytes)
    }

    /// Write the book as Markdown at `path`.
    pub fn export_markdown_to(&self, path: &Path) -> Result<(), String> {
        write_atomically(
            path,
            docwrite_export::export_markdown(&self.book).as_bytes(),
        )
    }

    /// Export plain text without changing this tab's native save destination.
    pub fn export_text_to(&self, path: &Path) -> Result<(), String> {
        write_atomically(path, self.book.plain_text().as_bytes())
    }

    /// Write a DOCX copy without changing this tab's native save destination.
    pub fn export_docx_to(&self, path: &Path) -> Result<(), String> {
        crate::document_io::export_docx(&self.book, path)
    }

    /// Undo the last edit.
    pub fn undo(&mut self) {
        if self.book.undo().is_ok() {
            self.footnote_clamp();
            self.edited();
        }
    }

    /// Redo the last undone edit.
    pub fn redo(&mut self) {
        if self.book.redo().is_ok() {
            self.footnote_clamp();
            self.edited();
        }
    }

    /// The selected text, for the clipboard. `None` when nothing is selected.
    pub fn copy_text(&self) -> Option<String> {
        if self.book.selection().is_collapsed() {
            return None;
        }
        self.book.copy().ok().map(|fragment| fragment.plain_text())
    }

    /// Remove the selection and return its text, for the clipboard.
    pub fn cut_text(&mut self) -> Option<String> {
        if self.book.selection().is_collapsed() {
            return None;
        }
        let fragment = self.book.cut().ok()?;
        self.edited();
        Some(fragment.plain_text())
    }

    /// Replace the selection with `text`. Newlines start new paragraphs.
    pub fn paste_text(&mut self, text: &str) {
        if !text.is_empty() && self.book.paste_text(text).is_ok() {
            self.edited();
        }
    }

    /// Delete the selection, or the character after the caret.
    pub fn delete_forward(&mut self) {
        if self.book.delete_forward().is_ok() {
            self.edited();
        }
    }

    /// Move (or with `extend`, select) one word forward or backward.
    pub fn move_word(&mut self, forward: bool, extend: bool) {
        let motion = Motion::Word(if forward {
            Direction::Forward
        } else {
            Direction::Backward
        });
        let _ = if extend {
            self.book.extend_selection(motion)
        } else {
            self.book.move_caret(motion)
        };
        self.follow_caret = true;
    }

    /// Move to the start of the caret's paragraph.
    pub fn move_home(&mut self, extend: bool) {
        let focus = self.book.selection().focus;
        self.set_focus(Position::new(focus.block, 0), extend);
    }

    /// Move to the end of the caret's paragraph.
    pub fn move_end(&mut self, extend: bool) {
        let focus = self.book.selection().focus;
        let len = self.book.block_len(focus.block).unwrap_or(focus.offset);
        self.set_focus(Position::new(focus.block, len), extend);
    }

    /// Move to the typeset line above or below, keeping the caret's
    /// horizontal position. Crosses page boundaries.
    pub fn move_line(&mut self, down: bool, extend: bool) {
        self.refresh_layout();
        let Some(target) = self.line_neighbor(down) else {
            return;
        };
        self.set_focus(target, extend);
    }

    /// Turn one page (`down`) or back one, moving the caret to the same
    /// line offset on the new page so repeated PageDown/PageUp pairs never
    /// drift. With `extend`, the selection grows to the new caret. At the
    /// end of the book the caret goes to the last (or first) line shown.
    pub fn page(&mut self, down: bool, extend: bool) {
        self.refresh_layout();
        let from = self
            .document
            .as_ref()
            .and_then(|document| focus_mark(&self.book, document))
            .and_then(|(paragraph, byte)| {
                let document = self.document.as_ref()?;
                let page = document.page_of(paragraph, byte)?;
                let lines = document.page_painted_lines(page);
                let index = lines
                    .iter()
                    .rposition(|line| line.paragraph == paragraph && line_start(line) <= byte)
                    .or_else(|| lines.iter().position(|line| line.paragraph == paragraph))?;
                let x = lines[index].indent + x_of(&lines[index], byte);
                Some((page, index, x))
            });
        if down {
            self.pager.page_down();
        } else {
            self.pager.page_up();
        }
        let (Some((page, index, x)), Some(document)) = (from, self.document.as_ref()) else {
            return;
        };
        let target = self.pager.page_top() + 1;
        let lines = document.page_painted_lines(target);
        if lines.is_empty() {
            return;
        }
        let index = if target == page {
            // Nowhere to turn to: the caret goes to the last or first line.
            if down { lines.len() - 1 } else { 0 }
        } else {
            index.min(lines.len() - 1)
        };
        // Chapter titles and generated headings are not editable blocks. Pick
        // the next editable line, then look back if the page ends in metadata.
        let position = lines[index..]
            .iter()
            .chain(lines[..index].iter().rev())
            .find_map(|line| {
                let byte = byte_at_x(line, x - line.indent, self.paragraph_len(line.paragraph));
                self.position_of(line.paragraph, byte)
            });
        if let Some(position) = position {
            self.set_focus(position, extend);
        }
    }

    /// Seek to a physical page (1-based), moving the caret to its first editable
    /// line. Generated or blank pages retain the selection but still show that
    /// exact page. Invalid numbers leave the view and selection unchanged.
    pub fn seek_page(&mut self, page: u32) -> bool {
        self.refresh_layout();
        if page == 0 || page > self.pager.pages() {
            return false;
        }
        self.abandon_footnote();
        let position = self.document.as_ref().and_then(|document| {
            document
                .page_painted_lines(page)
                .iter()
                .find_map(|line| self.position_of(line.paragraph, line_start(line)))
        });
        if let Some(position) = position {
            self.set_focus(position, false);
        }
        // Seeking is a viewport command even when the page has no editable text.
        self.follow_caret = false;
        self.pager.jump_to(page - 1);
        true
    }

    /// Put the caret at a window point on a page, as a click does.
    /// Returns whether the point was on a page.
    pub fn click_at(&mut self, x: f32, y: f32, extend: bool) -> bool {
        let Some(slot) = self
            .slots
            .iter()
            .find(|slot| {
                x >= slot.left as f32
                    && x < (slot.left + slot.width) as f32
                    && y >= slot.top as f32
                    && y < (slot.top + slot.height) as f32
            })
            .copied()
        else {
            return false;
        };
        let Some(document) = self.document.as_ref() else {
            return false;
        };
        let page_x = (x - slot.left as f32) / slot.scale;
        let page_y = (y - slot.top as f32) / slot.scale;
        let lines = document.page_painted_lines(slot.page);
        let Some(line) = lines.iter().min_by(|a, b| {
            let da = (a.baseline - a.em * 0.3 - page_y).abs();
            let db = (b.baseline - b.em * 0.3 - page_y).abs();
            da.total_cmp(&db)
        }) else {
            return true;
        };
        let inset = document.page_content_inset(slot.page);
        let byte = byte_at_x(
            line,
            page_x - inset - line.indent,
            self.paragraph_len(line.paragraph),
        );
        if let Some(position) = self.position_of(line.paragraph, byte) {
            self.set_focus(position, extend);
            // A click on a citation opens its source where it came from.
            self.follow_citation();
        }
        true
    }

    fn line_neighbor(&self, down: bool) -> Option<Position> {
        let document = self.document.as_ref()?;
        let (paragraph, byte) = focus_mark(&self.book, document)?;
        let page = document.page_of(paragraph, byte)?;
        let lines = document.page_painted_lines(page);
        let index = lines
            .iter()
            .rposition(|line| line.paragraph == paragraph && line_start(line) <= byte)
            .or_else(|| lines.iter().position(|line| line.paragraph == paragraph))?;
        let current = &lines[index];
        let x = current.indent + x_of(current, byte);
        let target = if down {
            match lines.get(index + 1) {
                Some(line) => line.clone(),
                None => first_line_after(document, page)?,
            }
        } else {
            match index
                .checked_sub(1)
                .and_then(|previous| lines.get(previous))
            {
                Some(line) => line.clone(),
                None => last_line_before(document, page)?,
            }
        };
        let byte = byte_at_x(
            &target,
            x - target.indent,
            self.paragraph_len(target.paragraph),
        );
        self.position_of(target.paragraph, byte)
    }

    fn paragraph_len(&self, paragraph: usize) -> usize {
        self.document
            .as_ref()
            .and_then(|document| document.paragraphs().get(paragraph))
            .map_or(0, |paragraph| paragraph.text.len())
    }

    /// The model position of byte `byte` in layout paragraph `paragraph`.
    /// `None` for a paragraph that is not a block, such as a chapter title
    /// set from the chapter's metadata.
    fn position_of(&self, paragraph: usize, byte: usize) -> Option<Position> {
        let raw = self.document.as_ref()?.paragraphs().get(paragraph)?.id;
        let id = self
            .book
            .block_ids()
            .into_iter()
            .find(|id| id.raw() == raw)?;
        let text = self.book.block(id).ok()?.text();
        let byte = byte.min(text.len());
        let chars = text.get(..byte).map_or(0, |head| head.chars().count());
        Some(Position::new(id, chars))
    }

    fn set_focus(&mut self, position: Position, extend: bool) {
        let selection = if extend {
            Selection {
                anchor: self.book.selection().anchor,
                focus: position,
            }
        } else {
            Selection::collapsed(position)
        };
        let _ = self.book.set_selection(selection);
        self.follow_caret = true;
    }

    /// Type one keypress's text, with smart punctuation: straight quotes
    /// curl open after a space or an opening bracket and closed otherwise
    /// (after a dash too, where speech is cut off), two hyphens make an em
    /// dash, and three periods an ellipsis. One undo reverts a substitution
    /// to what was typed.
    pub fn type_key(&mut self, text: &str) {
        let mut chars = text.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            self.type_text(text);
            return;
        };
        let before: Vec<char> = self.text_before_caret(2);
        let prev = before.last().copied();
        let opens = |prev: Option<char>| {
            prev.is_none_or(|p| p.is_whitespace() || "([{\u{201c}\u{2018}".contains(p))
        };
        let (replace, with) = match ch {
            '"' => (0, if opens(prev) { "\u{201c}" } else { "\u{201d}" }),
            '\'' => (0, if opens(prev) { "\u{2018}" } else { "\u{2019}" }),
            '-' if prev == Some('-') => (1, "\u{2014}"),
            '.' if before == ['.', '.'] => (2, "\u{2026}"),
            _ => (0, ""),
        };
        if with.is_empty() {
            self.type_text(text);
            return;
        }
        let selection = self.book.selection();
        if replace > 0 && selection.is_collapsed() && selection.focus.offset >= replace {
            let anchor = Position::new(selection.focus.block, selection.focus.offset - replace);
            let _ = self.book.set_selection(Selection {
                anchor,
                focus: selection.focus,
            });
        }
        self.type_text(with);
    }

    /// Up to `count` characters before the caret (or the selection's start),
    /// within its paragraph.
    fn text_before_caret(&self, count: usize) -> Vec<char> {
        let selection = self.book.selection();
        let at = if selection.anchor.block == selection.focus.block {
            Position::new(
                selection.focus.block,
                selection.anchor.offset.min(selection.focus.offset),
            )
        } else {
            selection.focus
        };
        let Ok(block) = self.book.block(at.block) else {
            return Vec::new();
        };
        let chars: Vec<char> = block.text().chars().take(at.offset).collect();
        chars[chars.len().saturating_sub(count)..].to_vec()
    }

    /// Select the next occurrence of `query` after the caret, remembering it
    /// for [`Self::find_again`]. Returns whether one was found.
    pub fn find(&mut self, query: &str) -> bool {
        self.last_find = Some(query.to_string());
        self.find_again()
    }

    /// Select the next occurrence of the last search.
    pub fn find_again(&mut self) -> bool {
        let Some(query) = self.last_find.clone() else {
            return false;
        };
        let found = self.book.find_next(&query);
        if found {
            self.follow_caret = true;
        }
        found
    }

    /// Replace every occurrence of `query` with `with`; one undo step.
    pub fn replace_all(&mut self, query: &str, with: &str) -> usize {
        match self.book.replace_all(query, with) {
            Ok(count) if count > 0 => {
                self.edited();
                count
            }
            _ => 0,
        }
    }

    /// Whether a footnote session is open: typing goes to the note's body.
    pub fn in_footnote(&self) -> bool {
        self.footnote.is_some()
    }

    /// The note being typed, if a session is open.
    pub fn footnote_note(&self) -> Option<NoteId> {
        self.footnote.map(|session| session.note)
    }

    /// The body being typed, if a session is open.
    pub fn footnote_body(&self) -> Option<String> {
        let session = self.footnote?;
        self.book.note(session.note).ok().map(|note| note.text())
    }

    /// Open a footnote session on the caret's block: a new empty footnote,
    /// or the block's footnote when it already has one (the caret starts at
    /// the end of its body). A block with an endnote keeps it; the hotkey
    /// never clobbers an endnote. Returns whether a session opened.
    pub fn start_footnote(&mut self) -> bool {
        self.cancel_prompt();
        self.refresh_layout();
        let anchor = self.book.selection();
        let block = anchor.focus.block;
        let existing = self.book.block(block).ok().and_then(|body| body.note());
        let (note, cursor) = match existing {
            Some(id) => match self.book.note(id) {
                Ok(note) if note.kind() == NoteKind::Footnote => (id, note.text().chars().count()),
                _ => return false,
            },
            None => match self.book.attach_note(NoteKind::Footnote, "") {
                Ok(id) => (id, 0),
                Err(_) => return false,
            },
        };
        self.footnote = Some(FootnoteEdit {
            note,
            block,
            anchor,
            cursor,
        });
        self.edited();
        self.show_footnote_page();
        true
    }

    /// End the session, keeping the typed text, and return to the anchor.
    /// Returns whether a session was open.
    pub fn end_footnote(&mut self) -> bool {
        let Some(session) = self.footnote.take() else {
            return false;
        };
        let _ = self.book.set_selection(session.anchor);
        self.follow_caret = true;
        true
    }

    /// End the session without returning: the pointer is going elsewhere.
    pub fn abandon_footnote(&mut self) {
        self.footnote = None;
    }

    /// Type into the open footnote. Newlines become spaces: a note is one
    /// paragraph, wrapped by the page.
    pub fn footnote_type(&mut self, text: &str) {
        self.footnote_clamp();
        let Some(session) = self.footnote else {
            return;
        };
        let mut clean = String::with_capacity(text.len());
        for ch in text.chars() {
            if ch == '\n' || ch == '\r' {
                clean.push(' ');
            } else if !ch.is_control() {
                clean.push(ch);
            }
        }
        if clean.is_empty() {
            return;
        }
        let Ok(note) = self.book.note(session.note) else {
            return;
        };
        let mut body = note.text();
        body.insert_str(byte_at(&body, session.cursor), &clean);
        if self.book.set_note_text(session.note, &body).is_ok() {
            if let Some(session) = self.footnote.as_mut() {
                session.cursor += clean.chars().count();
            }
            self.edited();
        }
    }

    /// Delete the footnote character before the caret.
    pub fn footnote_backspace(&mut self) {
        self.footnote_clamp();
        let Some(session) = self.footnote else {
            return;
        };
        if session.cursor == 0 {
            return;
        }
        let Ok(note) = self.book.note(session.note) else {
            return;
        };
        let mut body = note.text();
        let end = byte_at(&body, session.cursor);
        let start = byte_at(&body, session.cursor - 1);
        body.replace_range(start..end, "");
        if self.book.set_note_text(session.note, &body).is_ok() {
            if let Some(session) = self.footnote.as_mut() {
                session.cursor -= 1;
            }
            self.edited();
        }
    }

    /// Delete the footnote character after the caret.
    pub fn footnote_delete_forward(&mut self) {
        self.footnote_clamp();
        let Some(session) = self.footnote else {
            return;
        };
        let Ok(note) = self.book.note(session.note) else {
            return;
        };
        let body = note.text();
        let len = body.chars().count();
        if session.cursor >= len {
            return;
        }
        let mut next = body;
        let start = byte_at(&next, session.cursor);
        let end = byte_at(&next, session.cursor + 1);
        next.replace_range(start..end, "");
        if self.book.set_note_text(session.note, &next).is_ok() {
            self.edited();
        }
    }

    /// Move the footnote caret one character.
    pub fn footnote_move(&mut self, left: bool) {
        if let Some(session) = self.footnote.as_mut() {
            if left {
                session.cursor = session.cursor.saturating_sub(1);
            } else {
                session.cursor = session.cursor.saturating_add(1);
            }
        }
        self.footnote_clamp();
    }

    /// Move the footnote caret to the start of the note.
    pub fn footnote_home(&mut self) {
        if let Some(session) = self.footnote.as_mut() {
            session.cursor = 0;
        }
    }

    /// Move the footnote caret to the end of the note.
    pub fn footnote_end(&mut self) {
        self.footnote_clamp();
        if let Some(session) = self.footnote.as_mut() {
            let len = self
                .book
                .note(session.note)
                .map(|note| note.text().chars().count())
                .unwrap_or(0);
            session.cursor = len;
        }
    }

    /// Clamp the session cursor to the body; drop the session when its note
    /// is gone (undone away mid-session).
    fn footnote_clamp(&mut self) {
        let note = self.footnote.map(|session| session.note);
        let Some(note) = note else {
            return;
        };
        match self.book.note(note) {
            Ok(body) => {
                let len = body.text().chars().count();
                if let Some(session) = self.footnote.as_mut() {
                    session.cursor = session.cursor.min(len);
                }
            }
            Err(_) => self.footnote = None,
        }
    }

    /// The 1-based page holding the session's note, if it is laid out. A
    /// long note continues onto later pages; this is where it starts.
    pub(crate) fn footnote_page(&self) -> Option<u32> {
        self.footnote_page_in(self.document.as_ref()?)
    }

    /// [`Self::footnote_page`] against an explicit layout. Paint holds the
    /// document outside the editor while pages draw.
    pub(crate) fn footnote_page_in(&self, document: &Document) -> Option<u32> {
        let session = self.footnote?;
        let paragraph = document.paragraph_of(session.block.raw())?;
        let start = document.page_of(paragraph, 0)?;
        (start..=document.page_count()).find(|page| {
            document
                .page_footnote_lines(*page)
                .iter()
                .any(|line| line.paragraph == paragraph)
        })
    }

    /// Show the session's note. Called when the session opens; typing never
    /// moves the view.
    fn show_footnote_page(&mut self) {
        self.refresh_layout();
        if let Some(page) = self.footnote_page() {
            self.pager.jump_to(page - 1);
        }
    }

    /// The session's caret as (page, footnote line index on that page, byte
    /// in the line's text), for paint.
    pub(crate) fn footnote_caret_in(&self, document: &Document) -> Option<(u32, usize, usize)> {
        let session = self.footnote?;
        let paragraph = document.paragraph_of(session.block.raw())?;
        let body = self.book.note(session.note).ok()?.text();
        let page = self.footnote_page_in(document)?;
        let lines = document.page_footnote_lines(page);
        let ours: Vec<(usize, &FootnoteLine)> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.paragraph == paragraph)
            .collect();
        let (index, byte) = footnote_cursor_in_lines(&ours, &body, session.cursor)?;
        Some((page, index, byte))
    }

    /// Record an edit: the layout is stale, the view follows the caret, and
    /// autosave waits for typing to pause.
    pub(crate) fn edited(&mut self) {
        self.dirty = true;
        self.layout_stale = true;
        self.follow_caret = true;
        self.last_edit = Some(Instant::now());
    }
}

fn first_line_after(document: &docwrite_typeset::Document, page: u32) -> Option<PaintedLine> {
    (page + 1..=document.page_count())
        .find_map(|next| document.page_painted_lines(next).into_iter().next())
}

fn last_line_before(document: &docwrite_typeset::Document, page: u32) -> Option<PaintedLine> {
    (1..page)
        .rev()
        .find_map(|previous| document.page_painted_lines(previous).into_iter().last())
}

/// Map a footnote body offset in characters onto one of the note's
/// laid-out lines: (line index on its page, byte in the line's text). The
/// laid-out note starts with its number and an en space; a wrap drops its
/// break space, which the body keeps. Past the end clamps to the last line.
fn footnote_cursor_in_lines(
    lines: &[(usize, &FootnoteLine)],
    body: &str,
    cursor: usize,
) -> Option<(usize, usize)> {
    let (_, first) = lines.first()?;
    let prefix_chars = first
        .text
        .find('\u{2002}')
        .map_or(0, |at| first.text[..at].chars().count() + 1);
    let body_chars: Vec<char> = body.chars().collect();
    let mut consumed = 0usize;
    for (position, (index, line)) in lines.iter().enumerate() {
        if position > 0
            && body_chars
                .get(consumed)
                .is_some_and(|ch| ch.is_whitespace())
        {
            consumed += 1;
        }
        let line_chars = line.text.chars().count();
        let cap = line_chars.saturating_sub(if position == 0 { prefix_chars } else { 0 });
        if cursor <= consumed + cap {
            let within = if position == 0 { prefix_chars } else { 0 } + (cursor - consumed);
            return Some((*index, byte_at(&line.text, within)));
        }
        consumed += cap;
    }
    let (index, line) = lines.last()?;
    Some((*index, line.text.len()))
}

/// Byte offset where `line` starts in its paragraph.
fn line_start(line: &PaintedLine) -> usize {
    line.glyphs
        .iter()
        .filter(|glyph| glyph.cluster < NOTE_MARK_CLUSTER)
        .map(|glyph| glyph.cluster as usize)
        .min()
        .unwrap_or(0)
}

/// Pen position of byte `byte` along `line`, from the line's start.
fn x_of(line: &PaintedLine, byte: usize) -> f32 {
    line.glyphs
        .iter()
        .filter(|glyph| glyph.cluster < NOTE_MARK_CLUSTER && (glyph.cluster as usize) < byte)
        .map(|glyph| glyph.x_advance)
        .sum()
}

/// The byte nearest pen position `x` on `line`. `len` is the paragraph's
/// length in bytes.
fn byte_at_x(line: &PaintedLine, x: f32, len: usize) -> usize {
    let mut pen = 0.0;
    let mut last_start = line_start(line);
    for glyph in line
        .glyphs
        .iter()
        .filter(|glyph| glyph.cluster < NOTE_MARK_CLUSTER)
    {
        if pen + glyph.x_advance / 2.0 > x {
            return glyph.cluster as usize;
        }
        pen += glyph.x_advance;
        last_start = glyph.cluster as usize;
    }
    // Past the end: a paragraph's last line ends at the paragraph's end;
    // any other line keeps the caret before its last glyph, on this line.
    if line.ends_paragraph { len } else { last_start }
}

pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} has no file name", path.display()))?;
    let temporary = path.with_file_name(format!(".{}.partial", name.to_string_lossy()));
    std::fs::write(&temporary, bytes).map_err(|err| err.to_string())?;
    std::fs::rename(&temporary, path).map_err(|err| err.to_string())
}
