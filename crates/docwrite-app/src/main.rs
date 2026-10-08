//! `liminaldoc-writer`: the page-native book editor.
//!
//! ```text
//! liminaldoc-writer [--fullscreen] [--source S.pdf] [DOCUMENT]  open or start a document
//! liminaldoc-writer export BOOK.legebook OUT     write OUT as .pdf, .md, .txt or .docx
//! ```
//!
//! Without a display the window is not opened; the process says so and
//! exits. It does not pretend to have drawn.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use docwrite_app::PromptKind;
use docwrite_app::tabs::{DocumentTabs, TabCommand, TabStripLayout};
use docwrite_model::Mark;
use pixelkit_shell::clipboard::Clipboard;
use pixelkit_shell::{KeyEvent, KeyInput};
use rfd::FileDialog;

/// Typing has to pause this long before autosave writes the bundle.
const AUTOSAVE_IDLE: Duration = Duration::from_millis(1500);

fn main() {
    match run() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("liminaldoc-writer: {err}");
            std::process::exit(if err.is_display() { 2 } else { 1 });
        }
    }
}

struct RunError {
    display: bool,
    message: String,
}

impl RunError {
    fn display(message: impl Into<String>) -> Self {
        Self {
            display: true,
            message: message.into(),
        }
    }

    fn other(message: impl Into<String>) -> Self {
        Self {
            display: false,
            message: message.into(),
        }
    }

    fn is_display(&self) -> bool {
        self.display
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.display {
            write!(f, "display unavailable: {}", self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

fn run() -> Result<(), RunError> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let fullscreen = args.iter().any(|arg| arg == "--fullscreen");
    args.retain(|arg| arg != "--fullscreen");
    let source = args
        .iter()
        .position(|arg| arg == "--source")
        .and_then(|index| {
            args.remove(index);
            (index < args.len()).then(|| PathBuf::from(args.remove(index)))
        });
    match args.as_slice() {
        [command, book, out] if command == "export" => export(Path::new(book), Path::new(out)),
        [] => open(None, fullscreen, source),
        [book] if !book.starts_with('-') => open(Some(PathBuf::from(book)), fullscreen, source),
        _ => Err(RunError::other(
            "usage: liminaldoc-writer [--fullscreen] [--source SOURCE.pdf] [DOCUMENT] | liminaldoc-writer export BOOK.legebook OUT.pdf|OUT.md|OUT.txt|OUT.docx",
        )),
    }
}

fn export(book: &Path, out: &Path) -> Result<(), RunError> {
    let editor = docwrite_app::Editor::open_existing(book).map_err(RunError::other)?;
    match out.extension().and_then(|ext| ext.to_str()) {
        Some("pdf") => editor.export_pdf_to(out),
        Some("md") => editor.export_markdown_to(out),
        Some("txt") => editor.export_text_to(out),
        Some("docx") => editor.export_docx_to(out),
        _ => Err("the output must end in .pdf, .md, .txt or .docx".to_string()),
    }
    .map_err(RunError::other)?;
    println!("wrote {}", out.display());
    Ok(())
}

fn open(book: Option<PathBuf>, fullscreen: bool, source: Option<PathBuf>) -> Result<(), RunError> {
    if docwrite_app::display_unavailable() {
        return Err(RunError::display(
            "no display; the paged surface was not opened",
        ));
    }
    let mut editor = match book.as_deref() {
        Some(path) if path.exists() => docwrite_app::Editor::open_document(path),
        Some(path) if path.extension().is_some_and(|ext| ext == "legebook") => {
            docwrite_app::Editor::open_or_create(path)
        }
        Some(path) => Err(format!("{} does not exist", path.display())),
        None => Ok(docwrite_app::Editor::new_document("Untitled")),
    }
    .map_err(RunError::other)?;
    editor.set_fullscreen(fullscreen);
    if let Some(source) = source {
        editor.open_source(source).map_err(RunError::other)?;
    }
    let title = format!("{} — liminaldoc-writer", editor.book().title());
    let app = EditorApp {
        tabs: DocumentTabs::new(editor),
        clipboard: Clipboard::new(),
        announced: false,
        status: None,
        ime_composing: false,
        tab_height: 0,
        cursor: (0.0, 0.0),
        content_buffer: pixelkit_raster::WindowBuffer::new(1, 1),
        last_title: String::new(),
        ui_scale: 1.0,
    };
    let config = pixelkit_shell::WindowConfig::new(&title, 1100.0, 800.0);
    pixelkit_shell::run_app(app, config).map_err(|err| {
        let message = err.to_string();
        let lower = message.to_ascii_lowercase();
        if lower.contains("display") || lower.contains("window") || lower.contains("connection") {
            RunError::display(message)
        } else {
            RunError::other(message)
        }
    })
}

struct EditorApp {
    tabs: DocumentTabs,
    clipboard: Clipboard,
    announced: bool,
    /// The last save or export outcome, reported once on stderr.
    status: Option<String>,
    ime_composing: bool,
    tab_height: i32,
    cursor: (f32, f32),
    content_buffer: pixelkit_raster::WindowBuffer,
    last_title: String,
    ui_scale: f32,
}

impl EditorApp {
    /// Use the platform's complete text payload. PixelKit currently emits
    /// named Space as a character key with an empty payload on some hosts.
    fn input_text(&self, event: &KeyEvent) -> String {
        let modifiers = event.modifiers;
        if !event.pressed || modifiers.accel() || modifiers.control || self.ime_composing {
            return String::new();
        }
        let text = if !event.text.is_empty() {
            event.text.as_str()
        } else if event.physical == "Space"
            && event.key == KeyInput::Character(' ')
            && !modifiers.alt
            && !modifiers.logo
        {
            " "
        } else {
            ""
        };
        text.chars().filter(|ch| !ch.is_control()).collect()
    }

    fn report(&mut self, outcome: Result<String, String>) {
        let line = match outcome {
            Ok(done) => done,
            Err(err) => format!("error: {err}"),
        };
        eprintln!("liminaldoc-writer: {line}");
        self.status = Some(line);
    }

    fn new_document(&mut self) {
        let title = if self.tabs.len() == 0 {
            "Untitled".to_string()
        } else {
            format!("Untitled {}", self.tabs.len() + 1)
        };
        self.tabs.push(docwrite_app::Editor::new_document(title));
        self.status = None;
    }

    fn open_path(&mut self, path: &Path) {
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        {
            self.open_pdf_in_reader(path);
            return;
        }
        if let Some(index) = self.tabs.find_bundle(path) {
            self.tabs.select(index);
            self.status = None;
            return;
        }
        match docwrite_app::Editor::open_document(path) {
            Ok(editor) => {
                let imported = editor.bundle().is_none();
                self.tabs.push(editor);
                self.status = None;
                if imported {
                    self.report(Ok(format!(
                        "imported text from {}; save as a .legebook to keep editing",
                        path.display()
                    )));
                }
            }
            Err(err) => self.report(Err(err)),
        }
    }

    fn open_pdf_in_reader(&mut self, path: &Path) {
        let outcome = std::env::current_exe()
            .map(|writer| {
                writer.with_file_name(format!(
                    "liminaldoc-reader{}",
                    std::env::consts::EXE_SUFFIX
                ))
            })
            .map_err(|error| format!("could not locate Liminaldoc Reader: {error}"))
            .and_then(|reader| {
                Command::new(&reader).arg(path).spawn().map_err(|error| {
                    format!(
                        "could not open PDF in Liminaldoc Reader ({}): {error}; build the full workspace first",
                        reader.display()
                    )
                })
            })
            .map(|_| format!("opened {} in Liminaldoc Reader", path.display()));
        self.report(outcome);
    }

    fn open_file_dialog(&mut self) {
        let path = FileDialog::new()
            .set_title("Open document")
            .add_filter(
                "Documents",
                &["txt", "md", "markdown", "odt", "docx", "pdf"],
            )
            .pick_file();
        if let Some(path) = path {
            self.open_path(&path);
        }
    }

    fn open_book_dialog(&mut self) {
        if let Some(path) = FileDialog::new()
            .set_title("Open .legebook folder")
            .pick_folder()
        {
            self.open_path(&path);
        }
    }

    fn save_as_dialog(&mut self) -> bool {
        let name = self.tabs.book().title().to_string();
        let path = FileDialog::new()
            .set_title("Save as")
            .add_filter("Lege book", &["legebook"])
            .add_filter("PDF", &["pdf"])
            .add_filter("Word document", &["docx"])
            .add_filter("Markdown", &["md"])
            .add_filter("Plain text", &["txt"])
            .set_file_name(name)
            .save_file();
        let Some(mut path) = path else {
            return false;
        };
        if path.extension().is_none() {
            path.set_extension("legebook");
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let result = match extension.as_str() {
            "legebook" => self.tabs.save_as_bundle(&path),
            "pdf" => self.tabs.export_pdf_to(&path),
            "docx" => self.tabs.export_docx_to(&path),
            "md" => self.tabs.export_markdown_to(&path),
            "txt" => self.tabs.export_text_to(&path),
            _ => Err("save filename must end in .legebook, .pdf, .docx, .md or .txt".into()),
        };
        match result {
            Ok(()) => {
                self.report(Ok(format!("saved {}", path.display())));
                true
            }
            Err(err) => {
                self.report(Err(err));
                false
            }
        }
    }

    fn save_active(&mut self) -> bool {
        if self.tabs.bundle().is_none() {
            return self.save_as_dialog() && self.tabs.bundle().is_some();
        }
        match self.tabs.save_now() {
            Ok(()) => {
                self.report(Ok("saved".into()));
                true
            }
            Err(err) => {
                self.report(Err(err));
                false
            }
        }
    }

    fn close_active_tab(&mut self) {
        let needs_save = self.tabs.is_dirty()
            || (self.tabs.bundle().is_none() && !self.tabs.book().plain_text().is_empty());
        if needs_save && !self.save_active() {
            return;
        }
        self.tabs.close_active();
        self.status = None;
    }

    fn backup_current_on_exit(&self, index: usize) -> Result<PathBuf, String> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = home.join("Documents").join("Liminaldoc Recovery");
        std::fs::create_dir_all(&root).map_err(|err| err.to_string())?;
        let title: String = self
            .tabs
            .book()
            .title()
            .chars()
            .filter(|ch| ch.is_alphanumeric() || *ch == '-' || *ch == '_')
            .take(40)
            .collect();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|err| err.to_string())?
            .as_nanos();
        let path = root.join(format!("{title}-{stamp}-{index}.legebook"));
        let mut book = self.tabs.book().clone();
        book.save_bundle(&path).map_err(|err| err.to_string())?;
        Ok(path)
    }

    fn run_tab_command(&mut self, command: TabCommand) {
        match command {
            TabCommand::Select(index) => {
                self.tabs.select(index);
                self.status = None;
            }
            TabCommand::New => self.new_document(),
            TabCommand::Open => self.open_file_dialog(),
            TabCommand::OpenBook => self.open_book_dialog(),
            TabCommand::SaveAs => {
                self.save_as_dialog();
            }
        }
    }

    /// Commands bound to the platform accelerator (Cmd on macOS, Ctrl
    /// elsewhere), matched by physical key so Option does not change them.
    fn accelerator(&mut self, event: &KeyEvent) -> bool {
        let shift = event.modifiers.shift;
        let alt = event.modifiers.alt;
        match event.physical.as_str() {
            "KeyZ" if shift => self.tabs.redo(),
            "KeyZ" => self.tabs.undo(),
            "KeyY" => self.tabs.redo(),
            "KeyC" => {
                if let Some(text) = self.tabs.copy_text() {
                    self.clipboard.set_text(text);
                }
            }
            "KeyX" => {
                if let Some(text) = self.tabs.cut_text() {
                    self.clipboard.set_text(text);
                }
            }
            "KeyV" => {
                if let Some(text) = self.clipboard.text() {
                    self.tabs.paste_text(&text);
                }
            }
            "KeyS" if shift => {
                self.save_as_dialog();
            }
            "KeyS" => {
                self.save_active();
            }
            "KeyB" if !alt => self.tabs.toggle_mark(Mark::Bold),
            "KeyI" if !alt => self.tabs.toggle_mark(Mark::Italic),
            "KeyH" if shift => self.tabs.toggle_mark(Mark::SmallCaps),
            "Equal" if shift => self.tabs.toggle_mark(Mark::Superscript),
            "Minus" if shift => self.tabs.toggle_mark(Mark::Subscript),
            "KeyN" if shift => self.tabs.open_prompt(PromptKind::NewChapter),
            "KeyN" if !alt => self.new_document(),
            "KeyO" if shift => self.tabs.open_prompt(PromptKind::OpenSource),
            "KeyO" if alt => self.open_book_dialog(),
            "KeyO" => self.open_file_dialog(),
            "KeyW" => self.close_active_tab(),
            "KeyP" if shift => {
                let on = !self.tabs.is_spread();
                self.tabs.set_spread(on);
            }
            "KeyR" if shift => self.tabs.open_prompt(PromptKind::RenameChapter),
            "KeyN" if alt => self.tabs.open_prompt(PromptKind::Footnote),
            "KeyF" if alt => self.tabs.open_prompt(PromptKind::Replace),
            "KeyE" if alt => self.tabs.open_prompt(PromptKind::Endnote),
            "KeyT" if alt => self.tabs.toggle_contents(),
            "KeyI" if alt => self.tabs.open_prompt(PromptKind::IndexTerm),
            "KeyB" if alt && shift => self.tabs.open_prompt(PromptKind::ImportBibliography),
            "KeyB" if alt => self.tabs.open_prompt(PromptKind::BibliographyEntry),
            "KeyF" if shift => self.tabs.toggle_fullscreen(),
            "KeyT" if shift => self.tabs.cycle_theme(),
            "KeyF" => self.tabs.open_prompt(PromptKind::Find),
            "KeyG" => {
                self.tabs.find_again();
            }
            "Backslash" => self.tabs.toggle_sidebar(),
            digit if alt && digit.starts_with("Digit") => {
                if let Ok(number) = digit.trim_start_matches("Digit").parse::<usize>() {
                    self.tabs.set_block_kind_number(number);
                }
            }
            _ => return false,
        }
        true
    }

    /// Keys while a toolbar prompt is open. Returns whether the key was used.
    fn prompt_key(&mut self, event: &KeyEvent) -> bool {
        if self.tabs.prompt().is_none() {
            return false;
        }
        if event.modifiers.accel() && event.physical == "KeyV" {
            if let Some(text) = self.clipboard.text() {
                let text: String = text.chars().filter(|ch| !ch.is_control()).collect();
                self.tabs.prompt_type(&text);
            }
            return true;
        }
        match event.key {
            KeyInput::Enter => {
                self.tabs.commit_prompt();
            }
            KeyInput::Escape => {
                self.tabs.cancel_prompt();
            }
            KeyInput::Backspace => {
                self.tabs.prompt_backspace();
            }
            _ => {
                let text = self.input_text(event);
                self.tabs.prompt_type(&text);
            }
        }
        true
    }
}

impl pixelkit_shell::PixelApp for EditorApp {
    fn render(&mut self, buffer: &mut pixelkit_raster::WindowBuffer, scale: pixelkit_shell::Scale) {
        self.ui_scale = scale.0;
        self.tabs.set_ui_scale(scale.0);
        let width = buffer.width;
        let height = buffer.height;
        self.tab_height = if self.tabs.is_fullscreen() {
            0
        } else {
            TabStripLayout::new(width as i32, scale.0, self.tabs.len()).height
        };
        let content_height = height.saturating_sub(self.tab_height as u32).max(1);
        self.content_buffer.resize(width, content_height);
        self.tabs.paint(&mut self.content_buffer);
        let offset = self.tab_height as usize * width as usize;
        let copy = self
            .content_buffer
            .pixels
            .len()
            .min(buffer.pixels.len().saturating_sub(offset));
        buffer.pixels[offset..offset + copy].copy_from_slice(&self.content_buffer.pixels[..copy]);
        if self.tab_height > 0 {
            let labels = self.tabs.labels();
            let active = self.tabs.active_index();
            self.tabs.paint_tab_strip(buffer, &labels, active);
        }
        if self.announced {
            return;
        }
        self.announced = true;
        let (desk, page, ink) = docwrite_app::Editor::census(&buffer.pixels);
        eprintln!(
            "paged surface: {} pages, desk pixels {desk}, page pixels {page}, ink pixels {ink}",
            self.tabs.pages_painted()
        );
    }

    fn poll_fullscreen(&mut self) -> Option<bool> {
        self.tabs.poll_fullscreen()
    }

    fn poll_title(&mut self) -> Option<String> {
        let label = self.tabs.labels()[self.tabs.active_index()].clone();
        let title = match &self.status {
            Some(status) => format!(
                "{label} — {} — liminaldoc-writer",
                status.chars().take(80).collect::<String>()
            ),
            None => format!("{label} — liminaldoc-writer"),
        };
        if title == self.last_title {
            None
        } else {
            self.last_title = title.clone();
            Some(title)
        }
    }

    fn cursor_shape(&self) -> pixelkit_shell::CursorShape {
        match self.tabs.pointer_shape() {
            docwrite_app::PointerShape::Arrow => pixelkit_shell::CursorShape::Default,
            docwrite_app::PointerShape::Text => pixelkit_shell::CursorShape::Text,
            docwrite_app::PointerShape::Hand => pixelkit_shell::CursorShape::Pointer,
            docwrite_app::PointerShape::ResizeColumn => {
                pixelkit_shell::CursorShape::ResizeHorizontal
            }
        }
    }

    fn animation_interval(&self) -> Option<Duration> {
        self.tabs
            .iter()
            .any(|editor| editor.needs_save_tick())
            .then_some(Duration::from_millis(500))
    }

    fn tick(&mut self) {
        let mut errors = Vec::new();
        for editor in self.tabs.iter_mut() {
            editor.autosave_if_idle(AUTOSAVE_IDLE);
            if let Some(err) = editor.take_save_error() {
                errors.push(err);
            }
        }
        for err in errors {
            self.report(Err(format!("autosave failed: {err}")));
        }
    }

    fn on_exit(&mut self) {
        for index in 0..self.tabs.len() {
            self.tabs.select(index);
            let has_unsaved_text =
                self.tabs.is_dirty() || !self.tabs.book().plain_text().is_empty();
            let saved = if self.tabs.bundle().is_none() {
                !has_unsaved_text || self.save_as_dialog()
            } else {
                match self.tabs.save_now() {
                    Ok(()) => true,
                    Err(err) => {
                        eprintln!("liminaldoc-writer: could not save on exit: {err}");
                        false
                    }
                }
            };
            if !saved {
                match self.backup_current_on_exit(index) {
                    Ok(path) => {
                        eprintln!("liminaldoc-writer: recovery copy at {}", path.display());
                        let _ = rfd::MessageDialog::new()
                            .set_title("Recovery copy saved")
                            .set_description(format!(
                                "The manuscript was saved at {}",
                                path.display()
                            ))
                            .show();
                    }
                    Err(err) => {
                        eprintln!("liminaldoc-writer: recovery copy failed: {err}");
                        let _ = rfd::MessageDialog::new()
                            .set_title("Could not save manuscript")
                            .set_description(format!("Save and recovery both failed: {err}"))
                            .show();
                    }
                }
            }
        }
    }

    fn on_key_event(&mut self, event: &KeyEvent) {
        if !event.pressed {
            return;
        }
        if event.modifiers.control && event.key == KeyInput::Tab {
            self.tabs.next(event.modifiers.shift);
            self.status = None;
            return;
        }
        if self.prompt_key(event) {
            return;
        }
        let modifiers = event.modifiers;
        if modifiers.accel() && self.accelerator(event) {
            return;
        }
        let shift = modifiers.shift;
        // Word motion: Option on macOS, Ctrl elsewhere.
        let word = if cfg!(target_os = "macos") {
            modifiers.alt
        } else {
            modifiers.control
        };
        match event.key {
            KeyInput::Function(11) => self.tabs.toggle_fullscreen(),
            KeyInput::Escape if self.tabs.is_fullscreen() => self.tabs.set_fullscreen(false),
            KeyInput::Left if word => self.tabs.move_word(false, shift),
            KeyInput::Right if word => self.tabs.move_word(true, shift),
            KeyInput::Left if modifiers.logo => self.tabs.move_home(shift),
            KeyInput::Right if modifiers.logo => self.tabs.move_end(shift),
            KeyInput::Left if shift => self.tabs.extend_left(),
            KeyInput::Right if shift => self.tabs.extend_right(),
            KeyInput::Left => self.tabs.move_left(),
            KeyInput::Right => self.tabs.move_right(),
            KeyInput::Up => self.tabs.move_line(false, shift),
            KeyInput::Down => self.tabs.move_line(true, shift),
            KeyInput::Home => self.tabs.move_home(shift),
            KeyInput::End => self.tabs.move_end(shift),
            KeyInput::PageDown => self.tabs.page_down(),
            KeyInput::PageUp => self.tabs.page_up(),
            KeyInput::Backspace => self.tabs.backspace(),
            KeyInput::Delete => self.tabs.delete_forward(),
            KeyInput::Enter => self.tabs.type_text("\n"),
            _ => {
                let text = self.input_text(event);
                if !text.is_empty() {
                    self.tabs.type_key(&text);
                }
            }
        }
    }

    fn on_cursor(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        self.tabs.hover(x, y - self.tab_height as f32);
    }

    fn on_mouse(&mut self, button: pixelkit_shell::MouseButton, pressed: bool) {
        if button == pixelkit_shell::MouseButton::Left {
            if self.tab_height > 0 && self.cursor.1 < self.tab_height as f32 {
                if pressed {
                    let layout = TabStripLayout::new(
                        self.content_buffer.width as i32,
                        self.ui_scale,
                        self.tabs.len(),
                    );
                    if let Some(command) = layout.hit(self.cursor.0 as i32, self.cursor.1 as i32) {
                        self.run_tab_command(command);
                    }
                } else {
                    self.tabs.pointer(false);
                }
                return;
            }
            self.tabs.pointer(pressed);
        }
    }

    fn on_ime(&mut self, ime: &pixelkit_shell::ImeEvent) {
        match ime {
            pixelkit_shell::ImeEvent::Commit(text) => {
                self.ime_composing = false;
                if self.tabs.prompt().is_some() {
                    self.tabs.set_preedit("");
                    self.tabs.prompt_type(text);
                } else {
                    self.tabs.commit_ime(text);
                }
            }
            pixelkit_shell::ImeEvent::Preedit(text, _) => {
                self.ime_composing = !text.is_empty();
                self.tabs.set_preedit(text);
            }
            pixelkit_shell::ImeEvent::Disabled => {
                self.ime_composing = false;
                self.tabs.set_preedit("");
            }
            pixelkit_shell::ImeEvent::Enabled => {}
        }
    }

    fn ime_cursor_area(&self) -> Option<pixelkit_shell::ImeCursorArea> {
        self.tabs
            .caret_area()
            .map(|caret| pixelkit_shell::ImeCursorArea {
                x: caret.x,
                y: caret.y + self.tab_height as f64,
                width: caret.width,
                height: caret.height,
            })
    }

    fn on_scroll_event(&mut self, event: &pixelkit_shell::ScrollEvent) {
        let phase = match event.phase {
            pixelkit_shell::GesturePhase::Started => docwrite_app::Phase::Started,
            pixelkit_shell::GesturePhase::Moved => docwrite_app::Phase::Moved,
            pixelkit_shell::GesturePhase::Ended => docwrite_app::Phase::Ended,
            pixelkit_shell::GesturePhase::Cancelled => docwrite_app::Phase::Cancelled,
        };
        self.tabs
            .scroll_gesture(-(event.pixel_dy as f64) / 400.0, phase);
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use docwrite_model::{Position, Selection};
    use pixelkit_shell::{Modifiers, PixelApp};

    fn app() -> EditorApp {
        EditorApp {
            tabs: DocumentTabs::new(docwrite_app::Editor::with_book(docwrite_model::Book::new(
                "Test",
            ))),
            clipboard: Clipboard::new(),
            announced: false,
            status: None,
            ime_composing: false,
            tab_height: 0,
            cursor: (0.0, 0.0),
            content_buffer: pixelkit_raster::WindowBuffer::new(1, 1),
            last_title: String::new(),
            ui_scale: 1.0,
        }
    }

    fn space(pressed: bool, repeat: bool, modifiers: Modifiers) -> KeyEvent {
        KeyEvent {
            key: KeyInput::Character(' '),
            text: String::new(),
            physical: "Space".into(),
            pressed,
            repeat,
            modifiers,
        }
    }

    #[test]
    fn host_space_event_inserts_once_on_press_and_replaces_selection() {
        let mut app = app();
        app.tabs.type_text("oneXtwo");
        let block = app.tabs.book().block_ids()[0];
        app.tabs
            .book_mut()
            .set_selection(Selection {
                anchor: Position::new(block, 3),
                focus: Position::new(block, 4),
            })
            .unwrap();
        app.on_key_event(&space(true, false, Modifiers::default()));
        assert_eq!(app.tabs.book().plain_text(), "one two");
        app.on_key_event(&space(false, false, Modifiers::default()));
        assert_eq!(app.tabs.book().plain_text(), "one two");
        app.on_key_event(&space(
            true,
            true,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        ));
        assert_eq!(app.tabs.book().plain_text(), "one  two");
        app.on_key_event(&space(
            true,
            false,
            Modifiers {
                logo: true,
                ..Modifiers::default()
            },
        ));
        assert_eq!(app.tabs.book().plain_text(), "one  two");
    }

    #[test]
    fn prompt_receives_ime_commit_and_space_without_editing_manuscript() {
        let mut app = app();
        app.tabs.open_prompt(PromptKind::Find);
        app.on_ime(&pixelkit_shell::ImeEvent::Preedit("あ".into(), None));
        app.on_key_event(&space(true, false, Modifiers::default()));
        assert_eq!(app.tabs.prompt().unwrap().text, "");
        app.on_ime(&pixelkit_shell::ImeEvent::Commit("あ".into()));
        app.on_key_event(&space(true, false, Modifiers::default()));
        assert_eq!(app.tabs.prompt().unwrap().text, "あ ");
        assert_eq!(app.tabs.book().plain_text(), "");
    }

    #[test]
    fn empty_dead_key_is_ignored_and_complete_text_payload_is_inserted() {
        let mut app = app();
        app.on_key_event(&KeyEvent {
            key: KeyInput::Character('\u{00b4}'),
            text: String::new(),
            physical: "Quote".into(),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        });
        assert_eq!(app.tabs.book().plain_text(), "");
        app.on_key_event(&KeyEvent {
            key: KeyInput::Character('e'),
            text: "e\u{0301}".into(),
            physical: "KeyE".into(),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        });
        assert_eq!(app.tabs.book().plain_text(), "e\u{0301}");
    }

    #[test]
    fn new_and_clickable_tabs_keep_distinct_manuscripts() {
        let mut app = app();
        app.tabs.type_text("first");
        app.run_tab_command(TabCommand::New);
        app.tabs.type_text("second");
        assert_eq!(app.tabs.len(), 2);
        let mut buffer = pixelkit_raster::WindowBuffer::new(1100, 800);
        app.render(&mut buffer, pixelkit_shell::Scale::ONE);
        assert!(app.tab_height > 0);
        app.on_cursor(20.0, 12.0);
        app.on_mouse(pixelkit_shell::MouseButton::Left, true);
        assert_eq!(app.tabs.active_index(), 0);
        assert_eq!(app.tabs.book().plain_text(), "first");
        app.tabs.select(1);
        assert_eq!(app.tabs.book().plain_text(), "second");
    }
}
