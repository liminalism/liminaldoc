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

use docwrite_app::autosave::{self, AutosaveInterval};
use docwrite_app::tabs::{DocumentTabs, TabCommand, TabStripLayout};
use docwrite_app::{PromptKind, PromptMotion, RecentFiles};
use docwrite_model::{Align, Mark};
use liminalkit_shell::KeyInput;
use liminalkit_shell::clipboard::Clipboard;
use liminalkit_windowing::KeyEvent;
use rfd::FileDialog;

fn main() {
    match run() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("liminaldoc-writer: {err}");
            std::process::exit(if err.is_display() { 2 } else { 1 });
        }
    }
}

// On Linux the bundled portal backend cannot distinguish cancellation from
// portal failure and starts a second chooser. Use the native Zenity chooser
// when available, keeping cancellation as a final result.
#[cfg(target_os = "linux")]
fn linux_file_dialog(args: &[&str]) -> Option<Option<PathBuf>> {
    let output = Command::new("zenity")
        .arg("--file-selection")
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return Some(None);
    }
    let mut bytes = output.stdout;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    use std::os::unix::ffi::OsStringExt;
    Some((!bytes.is_empty()).then(|| PathBuf::from(std::ffi::OsString::from_vec(bytes))))
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
    let mut editor = docwrite_app::Editor::open_existing(book).map_err(RunError::other)?;
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
    let recent_path = RecentFiles::settings_path();
    let recent = match recent_path.as_deref().map(RecentFiles::load) {
        Some(Ok(recent)) => recent,
        Some(Err(error)) => {
            eprintln!("liminaldoc-writer: cannot load recent files: {error}");
            RecentFiles::new()
        }
        None => RecentFiles::new(),
    };
    let autosave_settings = autosave::settings_path();
    let (autosave_interval, status) =
        match autosave_settings.as_deref().map(autosave::load_interval) {
            Some(Err(error)) => {
                let message = format!("Cannot load autosave setting; using 15 minutes: {error}");
                eprintln!("{message}");
                (AutosaveInterval::default(), Some(message))
            }
            Some(Ok(interval)) => (interval, None),
            None => (AutosaveInterval::default(), None),
        };
    let mut app = EditorApp {
        tabs: DocumentTabs::new(editor),
        clipboard: Clipboard::new(),
        announced: false,
        status,
        autosave_interval,
        autosave_settings,
        ime_composing: false,
        tab_height: 0,
        cursor: (0.0, 0.0),
        content_buffer: liminalkit_raster::WindowBuffer::new(1, 1),
        last_title: String::new(),
        ui_scale: 1.0,
        recent,
        recent_path,
    };
    app.offer_recovered_drafts();
    match book {
        Some(path) => app.remember(&path),
        None => app.sync_recent(),
    }
    let mut config =
        liminalkit_windowing::WindowConfig::new(&title, 1100.0, 800.0).min_size(640.0, 400.0);
    // Same string a liminaldoc-writer desktop file would use for StartupWMClass.
    config.app_id = Some("liminaldoc-writer".to_owned());
    liminalkit_windowing::run_app(app, config).map_err(|err| {
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
    autosave_interval: AutosaveInterval,
    autosave_settings: Option<PathBuf>,
    tabs: DocumentTabs,
    clipboard: Clipboard,
    announced: bool,
    /// The last save or export outcome, reported once on stderr.
    status: Option<String>,
    ime_composing: bool,
    tab_height: i32,
    cursor: (f32, f32),
    content_buffer: liminalkit_raster::WindowBuffer,
    last_title: String,
    ui_scale: f32,
    /// Files opened or saved recently, newest first, for the File menu.
    recent: RecentFiles,
    /// Where `recent` is kept between runs; `None` without a settings directory.
    recent_path: Option<PathBuf>,
}

impl EditorApp {
    /// Hand the recent list to every open tab's menu.
    fn sync_recent(&mut self) {
        let paths = self.recent.existing();
        for editor in self.tabs.iter_mut() {
            editor.set_recent_files(paths.clone());
        }
    }

    /// Put `path` at the front of the recent list and persist it. Failure to
    /// write the list is reported on stderr only; it never blocks the open.
    fn remember(&mut self, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        self.recent.touch(path);
        self.persist_recent();
    }

    fn forget(&mut self, path: &Path) {
        self.recent.remove(path);
        self.persist_recent();
    }

    fn persist_recent(&mut self) {
        if let Some(store) = self.recent_path.as_deref()
            && let Err(error) = self.recent.save(store)
        {
            eprintln!("liminaldoc-writer: cannot save recent files: {error}");
        }
        self.sync_recent();
    }

    fn open_recent(&mut self, index: usize) {
        let Some(path) = self.recent.existing().get(index).cloned() else {
            return;
        };
        if !path.exists() {
            self.forget(&path);
            self.report(Err(format!("{} no longer exists", path.display())));
            return;
        }
        self.open_path(&path);
    }

    /// Open a document and, when it opened, remember it.
    fn open_path(&mut self, path: &Path) {
        if self.open_path_now(path) {
            self.remember(path);
        } else {
            let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            if self.recent.paths().contains(&absolute) && !path.exists() {
                self.forget(&absolute);
            }
        }
    }
    fn apply_autosave_request(&mut self) {
        let Some(interval) = self.tabs.take_autosave_request() else {
            return;
        };
        let result = self
            .autosave_settings
            .as_deref()
            .ok_or_else(|| "No user settings directory is available".to_string())
            .and_then(|path| autosave::save_interval(path, interval));
        match result {
            Ok(()) => {
                self.autosave_interval = interval;
                self.status = Some(format!("Autosave every {} minutes", interval.minutes()));
            }
            Err(error) => {
                self.tabs.open_autosave_prompt(interval);
                self.tabs
                    .set_prompt_error(format!("Cannot save setting: {error}"));
            }
        }
    }

    /// Use the platform's complete text payload. liminalkit currently emits
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

    /// Unsaved drafts an earlier run left in the recovery directory open as
    /// untitled tabs. They stay in recovery until saved under a name or closed.
    fn offer_recovered_drafts(&mut self) {
        let Some(root) = docwrite_app::recovery::default_root() else {
            return;
        };
        let mut opened = 0usize;
        for path in docwrite_app::recovery::pending(&root) {
            match docwrite_app::Editor::open_recovered(&path) {
                Ok(editor) => {
                    self.tabs.push(editor);
                    opened += 1;
                }
                Err(error) => eprintln!(
                    "liminaldoc-writer: cannot recover {}: {error}",
                    path.display()
                ),
            }
        }
        if opened > 0 {
            let noun = if opened == 1 { "draft" } else { "drafts" };
            self.status = Some(format!(
                "Recovered {opened} unsaved {noun}. Save As to keep {}.",
                if opened == 1 { "it" } else { "them" }
            ));
        }
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

    /// Open a document in a tab (or bring its tab forward). Returns whether
    /// the writer now shows it; a PDF goes to the reader and is not kept.
    fn open_path_now(&mut self, path: &Path) -> bool {
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        {
            self.open_pdf_in_reader(path);
            return false;
        }
        if let Some(index) = self.tabs.find_bundle(path) {
            self.tabs.select(index);
            self.status = None;
            return true;
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
                true
            }
            Err(err) => {
                self.report(Err(err));
                false
            }
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
        #[cfg(target_os = "linux")]
        if let Some(path) = linux_file_dialog(&[
            "--title=Open document",
            "--file-filter=Documents | *.txt *.md *.markdown *.odt *.docx *.pdf",
        ]) {
            if let Some(path) = path {
                self.open_path(&path);
            }
            return;
        }
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
        #[cfg(target_os = "linux")]
        if let Some(path) = linux_file_dialog(&["--title=Open .legebook folder", "--directory"]) {
            if let Some(path) = path {
                self.open_path(&path);
            }
            return;
        }
        if let Some(path) = FileDialog::new()
            .set_title("Open .legebook folder")
            .pick_folder()
        {
            self.open_path(&path);
        }
    }

    fn save_as_dialog(&mut self) -> bool {
        let name = self.tabs.book().title().to_string();
        #[cfg(target_os = "linux")]
        let path = linux_file_dialog(&[
            "--title=Save as",
            "--save",
            "--confirm-overwrite",
            &format!("--filename={name}"),
        ]);
        #[cfg(not(target_os = "linux"))]
        let path: Option<Option<PathBuf>> = None;
        let path = path.unwrap_or_else(|| {
            FileDialog::new()
                .set_title("Save as")
                .add_filter("Lege book", &["legebook"])
                .add_filter("PDF", &["pdf"])
                .add_filter("Word document", &["docx"])
                .add_filter("Markdown", &["md"])
                .add_filter("Plain text", &["txt"])
                .set_file_name(name)
                .save_file()
        });
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
                if extension == "legebook" {
                    self.remember(&path);
                }
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
            TabCommand::Save => {
                self.save_active();
            }
            TabCommand::OpenRecent(index) => self.open_recent(index),
            TabCommand::ClearRecent => {
                self.recent.clear();
                self.persist_recent();
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
            "KeyS" if alt => {
                self.tabs.open_autosave_prompt(self.autosave_interval);
            }
            "KeyS" if shift => {
                self.save_as_dialog();
            }
            "KeyS" => {
                self.save_active();
            }
            "KeyB" if !alt => self.tabs.toggle_mark(Mark::Bold),
            "KeyI" if !alt => self.tabs.toggle_mark(Mark::Italic),
            "KeyL" => self.tabs.set_alignment(Align::Left),
            "KeyE" if !alt => self.tabs.set_alignment(Align::Center),
            "KeyR" if !shift => self.tabs.set_alignment(Align::Right),
            "KeyJ" => self.tabs.set_alignment(Align::Justify),
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
            "KeyN" if alt => {
                self.tabs.start_footnote();
            }
            "KeyF" if alt => self.tabs.open_prompt(PromptKind::Replace),
            "KeyE" if alt => self.tabs.open_prompt(PromptKind::Endnote),
            "KeyT" if alt => self.tabs.toggle_contents(),
            "KeyT" if shift => self.tabs.cycle_theme(),
            "KeyT" => {
                self.tabs.cycle_font_family();
                let name = self.tabs.font_family_name().to_string();
                self.report(Ok(format!("font {name}")));
            }
            "KeyI" if alt => self.tabs.open_prompt(PromptKind::IndexTerm),
            "KeyB" if alt && shift => self.tabs.open_prompt(PromptKind::ImportBibliography),
            "KeyB" if alt => self.tabs.open_prompt(PromptKind::BibliographyEntry),
            "KeyF" if shift => self.tabs.toggle_fullscreen(),
            "KeyF" => self.tabs.open_prompt(PromptKind::Find),
            "KeyG" if shift => {
                self.tabs.find_again();
            }
            "KeyG" => self.tabs.open_prompt(PromptKind::GoToPage),
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

    /// Keys while a footnote session is open. Returns whether the key was
    /// used; anything unused ends the session and runs as a normal key.
    fn footnote_key(&mut self, event: &KeyEvent) -> bool {
        if event.modifiers.accel() {
            // Paste, undo and redo stay in the note; every other
            // accelerator leaves footnote mode and runs normally.
            match event.physical.as_str() {
                "KeyV" => {
                    if let Some(text) = self.clipboard.text() {
                        let text: String = text
                            .chars()
                            .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
                            .collect();
                        self.tabs.footnote_type(&text);
                    }
                    return true;
                }
                "KeyZ" => {
                    if event.modifiers.shift {
                        self.tabs.redo();
                    } else {
                        self.tabs.undo();
                    }
                    return true;
                }
                "KeyY" => {
                    self.tabs.redo();
                    return true;
                }
                _ => return false,
            }
        }
        match event.key {
            KeyInput::Enter => {
                self.tabs.end_footnote();
            }
            KeyInput::Escape => {
                self.tabs.end_footnote();
            }
            KeyInput::Backspace => self.tabs.footnote_backspace(),
            KeyInput::Delete => self.tabs.footnote_delete_forward(),
            KeyInput::Left => self.tabs.footnote_move(true),
            KeyInput::Right => self.tabs.footnote_move(false),
            KeyInput::Home => self.tabs.footnote_home(),
            KeyInput::End => self.tabs.footnote_end(),
            KeyInput::Up | KeyInput::Down | KeyInput::Tab => {}
            _ => {
                let text = self.input_text(event);
                if text.is_empty() {
                    // A dead key commits nothing; anything else unknown exits.
                    return matches!(event.key, KeyInput::Character(_));
                }
                self.tabs.footnote_type(&text);
            }
        }
        true
    }

    /// Keys while a toolbar prompt is open. Returns whether the key was used.
    fn prompt_key(&mut self, event: &KeyEvent) -> bool {
        if self.tabs.prompt().is_none() {
            return false;
        }
        if event.modifiers.accel() {
            match event.physical.as_str() {
                "KeyA" => self.tabs.prompt_select_all(),
                "KeyC" => {
                    if let Some(text) = self.tabs.prompt_selected_text() {
                        self.clipboard.set_text(text);
                    }
                }
                "KeyX" => {
                    if let Some(text) = self.tabs.prompt_selected_text()
                        && self.clipboard.set_text(text)
                    {
                        self.tabs.prompt_cut();
                    }
                }
                "KeyV" => {
                    if let Some(text) = self.clipboard.text() {
                        self.tabs.prompt_type(&text);
                    }
                }
                _ => {}
            }
        }
        let shift = event.modifiers.shift;
        match event.key {
            KeyInput::Enter => {
                self.tabs.commit_prompt();
                self.apply_autosave_request();
            }
            KeyInput::Escape => {
                self.tabs.cancel_prompt();
            }
            KeyInput::Backspace => {
                self.tabs.prompt_backspace();
            }
            KeyInput::Delete => {
                self.tabs.prompt_delete_forward();
            }
            KeyInput::Left if event.modifiers.logo => {
                self.tabs.prompt_move(PromptMotion::Home, shift)
            }
            KeyInput::Right if event.modifiers.logo => {
                self.tabs.prompt_move(PromptMotion::End, shift)
            }
            KeyInput::Left => self.tabs.prompt_move(PromptMotion::Left, shift),
            KeyInput::Right => self.tabs.prompt_move(PromptMotion::Right, shift),
            KeyInput::Home => self.tabs.prompt_move(PromptMotion::Home, shift),
            KeyInput::End => self.tabs.prompt_move(PromptMotion::End, shift),
            _ => {
                let text = self.input_text(event);
                self.tabs.prompt_type(&text);
            }
        }
        true
    }
}

impl liminalkit_windowing::PixelApp for EditorApp {
    fn on_native_window(&mut self, window: &winit::window::Window) {
        if let Some(icon) = window_icon() {
            window.set_window_icon(Some(icon));
        }
    }

    fn render(
        &mut self,
        buffer: &mut liminalkit_raster::WindowBuffer,
        scale: liminalkit_shell::Scale,
    ) {
        self.ui_scale = scale.0;
        self.tabs.set_ui_scale(scale.0);
        let width = buffer.width;
        let height = buffer.height;
        self.tab_height = if self.tabs.is_fullscreen() || self.tabs.len() == 1 {
            0
        } else {
            TabStripLayout::new(width as i32, scale.0, self.tabs.len()).height
        };
        let content_height = height.saturating_sub(self.tab_height as u32).max(1);
        self.content_buffer.resize(width, content_height);
        self.tabs.paint(&mut self.content_buffer);
        let top = self.tabs.toolbar_height();
        let split = (top as usize * width as usize).min(self.content_buffer.pixels.len());
        buffer.pixels[..split].copy_from_slice(&self.content_buffer.pixels[..split]);
        let offset = self.tab_height as usize * width as usize;
        let end = (self.content_buffer.pixels.len() + offset).min(buffer.pixels.len());
        buffer.pixels[split + offset..end]
            .copy_from_slice(&self.content_buffer.pixels[split..end - offset]);
        if self.tab_height > 0 {
            let labels = self.tabs.labels();
            let active = self.tabs.active_index();
            self.tabs.paint_tab_strip(buffer, &labels, active, top);
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

    fn cursor_shape(&self) -> liminalkit_windowing::CursorShape {
        match self.tabs.pointer_shape() {
            docwrite_app::PointerShape::Arrow => liminalkit_windowing::CursorShape::Default,
            docwrite_app::PointerShape::Text => liminalkit_windowing::CursorShape::Text,
            docwrite_app::PointerShape::Hand => liminalkit_windowing::CursorShape::Pointer,
            docwrite_app::PointerShape::ResizeColumn => {
                liminalkit_windowing::CursorShape::ResizeHorizontal
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
            editor.autosave_if_due(self.autosave_interval);
            if let Some(err) = editor.take_save_error() {
                errors.push(err);
            }
        }
        for err in errors {
            self.report(Err(format!("autosave failed: {err}")));
        }
    }

    fn on_exit(&mut self) {
        if std::thread::panicking() {
            for index in 0..self.tabs.len() {
                self.tabs.select(index);
                if self.tabs.is_dirty() {
                    let _ = self.backup_current_on_exit(index);
                }
            }
            return;
        }
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
        if self.tabs.menu_key(&event.key) {
            if let Some(command) = self.tabs.take_file_command() {
                self.run_tab_command(command);
            }
            return;
        }
        // Alt+F opens the File menu where Alt is a menu key; on macOS
        // Option types characters, so the menu is the toolbar's.
        if !cfg!(target_os = "macos")
            && event.modifiers.alt
            && !event.modifiers.accel()
            && !event.modifiers.control
            && event.physical == "KeyF"
        {
            self.tabs.toggle_file_menu();
            return;
        }
        if self.prompt_key(event) {
            return;
        }
        if self.tabs.in_footnote() {
            if self.footnote_key(event) {
                return;
            }
            self.tabs.end_footnote();
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
            KeyInput::Function(3) => {
                self.tabs.find_again();
            }
            KeyInput::Escape if self.tabs.is_fullscreen() => self.tabs.set_fullscreen(false),
            KeyInput::Left if word => self.tabs.move_word(false, shift),
            KeyInput::Right if word => self.tabs.move_word(true, shift),
            KeyInput::Left if modifiers.logo => self.tabs.move_home(shift),
            KeyInput::Right if modifiers.logo => self.tabs.move_end(shift),
            KeyInput::Left if shift => self.tabs.extend_left(),
            KeyInput::Right if shift => self.tabs.extend_right(),
            KeyInput::Left => self.tabs.move_left(),
            KeyInput::Right => self.tabs.move_right(),
            KeyInput::Up if modifiers.logo && cfg!(target_os = "macos") => {
                self.tabs.move_document_start(shift)
            }
            KeyInput::Down if modifiers.logo && cfg!(target_os = "macos") => {
                self.tabs.move_document_end(shift)
            }
            KeyInput::Up => self.tabs.move_line(false, shift),
            KeyInput::Down => self.tabs.move_line(true, shift),
            KeyInput::Home if word => self.tabs.move_document_start(shift),
            KeyInput::End if word => self.tabs.move_document_end(shift),
            KeyInput::Home => self.tabs.move_home(shift),
            KeyInput::End => self.tabs.move_end(shift),
            KeyInput::PageDown => self.tabs.page(true, shift),
            KeyInput::PageUp => self.tabs.page(false, shift),
            KeyInput::Backspace => self.tabs.backspace(),
            KeyInput::Delete => self.tabs.delete_forward(),
            KeyInput::Enter => self.tabs.type_text("\n"),
            KeyInput::Tab if !modifiers.alt && !modifiers.logo && !modifiers.control => {
                self.tabs.type_text("\t")
            }
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
        let top = self.tabs.toolbar_height() as f32;
        let offset = if y >= top + self.tab_height as f32 {
            self.tab_height as f32
        } else {
            0.0
        };
        self.tabs.hover(x, y - offset);
    }

    fn on_mouse(&mut self, button: liminalkit_shell::MouseButton, pressed: bool) {
        if button == liminalkit_shell::MouseButton::Left {
            let top = self.tabs.toolbar_height() as f32;
            if self.tab_height > 0
                && self.cursor.1 >= top
                && self.cursor.1 < top + self.tab_height as f32
            {
                if pressed {
                    let layout = TabStripLayout::new(
                        self.content_buffer.width as i32,
                        self.ui_scale,
                        self.tabs.len(),
                    );
                    if let Some(command) =
                        layout.hit(self.cursor.0 as i32, (self.cursor.1 - top) as i32)
                    {
                        self.run_tab_command(command);
                    }
                } else {
                    self.tabs.pointer(false);
                }
                return;
            }
            self.tabs.pointer(pressed);
            if let Some(command) = self.tabs.take_file_command() {
                self.run_tab_command(command);
            }
        }
    }

    fn on_ime(&mut self, ime: &liminalkit_windowing::ImeEvent) {
        match ime {
            liminalkit_windowing::ImeEvent::Commit(text) => {
                self.ime_composing = false;
                if self.tabs.prompt().is_some() {
                    self.tabs.set_preedit("");
                    self.tabs.prompt_type(text);
                } else if self.tabs.in_footnote() {
                    self.tabs.set_preedit("");
                    self.tabs.footnote_type(text);
                } else {
                    self.tabs.commit_ime(text);
                }
            }
            liminalkit_windowing::ImeEvent::Preedit(text, _) => {
                self.ime_composing = !text.is_empty();
                self.tabs.set_preedit(text);
            }
            liminalkit_windowing::ImeEvent::Disabled => {
                self.ime_composing = false;
                self.tabs.set_preedit("");
            }
            liminalkit_windowing::ImeEvent::Enabled => {}
        }
    }

    fn ime_cursor_area(&self) -> Option<liminalkit_windowing::ImeCursorArea> {
        self.tabs
            .caret_area()
            .map(|caret| liminalkit_windowing::ImeCursorArea {
                x: caret.x,
                y: caret.y
                    + if caret.y >= self.tabs.toolbar_height() as f64 {
                        self.tab_height as f64
                    } else {
                        0.0
                    },
                width: caret.width,
                height: caret.height,
            })
    }

    fn on_scroll_event(&mut self, event: &liminalkit_windowing::ScrollEvent) {
        let phase = match event.phase {
            liminalkit_windowing::GesturePhase::Started => docwrite_app::Phase::Started,
            liminalkit_windowing::GesturePhase::Moved => docwrite_app::Phase::Moved,
            liminalkit_windowing::GesturePhase::Ended => docwrite_app::Phase::Ended,
            liminalkit_windowing::GesturePhase::Cancelled => docwrite_app::Phase::Cancelled,
        };
        self.tabs.scroll_wheel(event.pixel_dy, event.line_dy, phase);
    }
}

/// Embedded launcher art. `None` if the PNG cannot be decoded; the window
/// still opens.
fn window_icon() -> Option<winit::window::Icon> {
    let bitmap = liminalkit_raster::png::decode(include_bytes!("../assets/icon.png")).ok()?;
    // A 256px icon does not survive winit's X11 property write. 128px is what
    // the title bar and taskbar display.
    let (width, height, pixels) = icon_pixels(&bitmap, 128);
    let mut rgba = Vec::with_capacity(pixels.len() * 4);
    for pixel in &pixels {
        rgba.extend_from_slice(&[
            ((pixel >> 16) & 255) as u8,
            ((pixel >> 8) & 255) as u8,
            (pixel & 255) as u8,
            (pixel >> 24) as u8,
        ]);
    }
    winit::window::Icon::from_rgba(rgba, width, height).ok()
}

fn icon_pixels(bitmap: &liminalkit_raster::Bitmap, side: u32) -> (u32, u32, Vec<u32>) {
    let side = side.min(bitmap.width.min(bitmap.height)).max(1);
    if bitmap.width == side && bitmap.height == side {
        return (side, side, bitmap.pixels.clone());
    }
    let mut out = vec![0; (side * side) as usize];
    for y in 0..side {
        let y0 = y * bitmap.height / side;
        let y1 = ((y + 1) * bitmap.height / side).max(y0 + 1);
        for x in 0..side {
            let x0 = x * bitmap.width / side;
            let x1 = ((x + 1) * bitmap.width / side).max(x0 + 1);
            let mut alpha = 0u32;
            let mut red = 0u32;
            let mut green = 0u32;
            let mut blue = 0u32;
            let mut count = 0u32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let pixel = bitmap.pixels[(sy * bitmap.width + sx) as usize];
                    alpha += pixel >> 24;
                    red += (pixel >> 16) & 255;
                    green += (pixel >> 8) & 255;
                    blue += pixel & 255;
                    count += 1;
                }
            }
            out[(y * side + x) as usize] =
                (alpha / count) << 24 | (red / count) << 16 | (green / count) << 8 | (blue / count);
        }
    }
    (side, side, out)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod icon_tests {
    #[test]
    fn writer_icon_is_a_transparent_rounded_square() {
        let bitmap = liminalkit_raster::png::decode(include_bytes!("../assets/icon.png"))
            .expect("writer icon");
        assert_eq!((bitmap.width, bitmap.height), (256, 256));
        assert_eq!(bitmap.pixels[0] >> 24, 0, "corner stays transparent");
        let top = bitmap.pixels[(18 * 256 + 128) as usize];
        let red = (top >> 16) & 255;
        let blue = top & 255;
        assert!(blue > red, "writer icon is the blue one, got {top:#x}");
        assert!(super::window_icon().is_some());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod input_tests {
    use super::*;
    use docwrite_model::{Position, Selection};
    use liminalkit_shell::Modifiers;
    use liminalkit_windowing::PixelApp;

    fn app() -> EditorApp {
        EditorApp {
            autosave_interval: AutosaveInterval::default(),
            autosave_settings: None,
            tabs: DocumentTabs::new(docwrite_app::Editor::with_book(docwrite_model::Book::new(
                "Test",
            ))),
            clipboard: Clipboard::new(),
            announced: false,
            status: None,
            ime_composing: false,
            tab_height: 0,
            cursor: (0.0, 0.0),
            content_buffer: liminalkit_raster::WindowBuffer::new(1, 1),
            last_title: String::new(),
            ui_scale: 1.0,
            recent: RecentFiles::new(),
            recent_path: None,
        }
    }

    #[test]
    fn tab_inserts_a_tab_in_the_manuscript() {
        let mut app = app();
        let mut event = space(true, false, Modifiers::default());
        event.key = KeyInput::Tab;
        app.on_key_event(&event);
        assert_eq!(app.tabs.book().plain_text(), "\t");
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
    fn autosave_setting_prompt_validates_persists_and_keeps_fullscreen() {
        let mut app = app();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autosave-minutes");
        app.autosave_settings = Some(path.clone());
        app.tabs.set_fullscreen(true);
        app.on_key_event(&KeyEvent {
            key: KeyInput::Character('s'),
            text: "s".into(),
            physical: "KeyS".into(),
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                control: true,
                logo: true,
                alt: true,
                ..Modifiers::default()
            },
        });
        assert_eq!(app.tabs.prompt_selected_text().as_deref(), Some("15"));
        assert!(app.tabs.is_fullscreen());
        let enter = KeyEvent {
            key: KeyInput::Enter,
            text: String::new(),
            physical: "Enter".into(),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        };
        app.tabs.prompt_type("14");
        app.on_key_event(&enter);
        assert!(app.tabs.prompt().unwrap().error.is_some());
        assert!(!path.exists());
        app.tabs.prompt_type("120");
        app.on_key_event(&enter);
        assert!(app.tabs.prompt().is_none());
        assert!(app.tabs.is_fullscreen());
        assert_eq!(app.autosave_interval.minutes(), 120);
        assert_eq!(
            autosave::load_interval(&path).unwrap(),
            app.autosave_interval
        );
        assert!(!app.tabs.is_dirty());

        app.tabs.open_autosave_prompt(app.autosave_interval);
        app.tabs.prompt_type("30");
        app.tabs.cancel_prompt();
        app.apply_autosave_request();
        assert_eq!(app.autosave_interval.minutes(), 120);

        app.autosave_settings = Some(path.join("blocked"));
        app.tabs.open_autosave_prompt(app.autosave_interval);
        app.tabs.prompt_type("60");
        app.on_key_event(&enter);
        assert!(app.tabs.prompt().unwrap().error.is_some());
        assert_eq!(app.autosave_interval.minutes(), 120);
        assert_eq!(autosave::load_interval(&path).unwrap().minutes(), 120);
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
        app.on_ime(&liminalkit_windowing::ImeEvent::Preedit("あ".into(), None));
        app.on_key_event(&space(true, false, Modifiers::default()));
        assert_eq!(app.tabs.prompt().unwrap().text, "");
        app.on_ime(&liminalkit_windowing::ImeEvent::Commit("あ".into()));
        app.on_key_event(&space(true, false, Modifiers::default()));
        assert_eq!(app.tabs.prompt().unwrap().text, "あ ");
        assert_eq!(app.tabs.book().plain_text(), "");
    }

    #[test]
    fn prompt_keyboard_edits_and_replaces_selection_without_touching_the_book() {
        let mut app = app();
        app.tabs.open_prompt(PromptKind::RenameChapter);
        let send = |app: &mut EditorApp, key, physical: &str, modifiers| {
            app.on_key_event(&KeyEvent {
                key,
                text: String::new(),
                physical: physical.into(),
                pressed: true,
                repeat: false,
                modifiers,
            });
        };
        send(&mut app, KeyInput::Home, "Home", Modifiers::default());
        send(
            &mut app,
            KeyInput::Right,
            "ArrowRight",
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(app.tabs.prompt_selected_text().as_deref(), Some("C"));
        send(&mut app, KeyInput::Delete, "Delete", Modifiers::default());
        assert_eq!(app.tabs.prompt().unwrap().text, "hapter 1");
        send(
            &mut app,
            KeyInput::Character('a'),
            "KeyA",
            Modifiers {
                control: true,
                logo: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(app.tabs.prompt_selected_text().as_deref(), Some("hapter 1"));
        app.on_ime(&liminalkit_windowing::ImeEvent::Commit("新しい章".into()));
        assert_eq!(app.tabs.prompt().unwrap().text, "新しい章");
        send(&mut app, KeyInput::Left, "ArrowLeft", Modifiers::default());
        send(
            &mut app,
            KeyInput::Backspace,
            "Backspace",
            Modifiers::default(),
        );
        assert_eq!(app.tabs.prompt().unwrap().text, "新し章");
        assert_eq!(app.tabs.book().plain_text(), "");
        assert_eq!(
            app.tabs.book().parts()[0].chapters()[0].title(),
            "Chapter 1"
        );
        send(&mut app, KeyInput::Enter, "Enter", Modifiers::default());
        assert_eq!(app.tabs.book().parts()[0].chapters()[0].title(), "新し章");
        assert!(app.tabs.prompt().is_none());
    }

    #[test]
    fn page_seek_shortcut_and_page_keys_navigate_the_active_manuscript() {
        let mut app = app();
        app.tabs
            .type_text(&"A long article needs simple navigation. ".repeat(900));
        app.tabs.refresh_layout();
        app.tabs.seek_page(1);
        let text = app.tabs.book().plain_text();
        app.on_key_event(&KeyEvent {
            key: KeyInput::Character('g'),
            text: "g".into(),
            physical: "KeyG".into(),
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                control: true,
                logo: true,
                ..Modifiers::default()
            },
        });
        assert_eq!(app.tabs.prompt().unwrap().kind, PromptKind::GoToPage);
        app.on_ime(&liminalkit_windowing::ImeEvent::Commit("3".into()));
        for (key, expected) in [
            (KeyInput::Enter, 2.0),
            (KeyInput::PageDown, 3.0),
            (KeyInput::PageUp, 2.0),
        ] {
            app.on_key_event(&KeyEvent {
                key,
                text: String::new(),
                physical: String::new(),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::default(),
            });
            app.tabs.refresh_layout();
            assert_eq!(app.tabs.pager().scroll(), expected);
        }
        assert_eq!(app.tabs.book().plain_text(), text);
        assert!(app.tabs.prompt().is_none());
    }

    #[test]
    fn f3_repeats_find_without_opening_page_seek() {
        let mut app = app();
        app.tabs.type_text("needle and another needle");
        assert!(app.tabs.find("needle"));
        let first = app.tabs.book().selection();
        app.on_key_event(&KeyEvent {
            key: KeyInput::Function(3),
            text: String::new(),
            physical: "F3".into(),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        });
        assert_ne!(app.tabs.book().selection(), first);
        assert!(app.tabs.prompt().is_none());
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
        let mut buffer = liminalkit_raster::WindowBuffer::new(1100, 800);
        app.render(&mut buffer, liminalkit_shell::Scale::ONE);
        assert!(app.tab_height > 0);
        app.on_cursor(20.0, app.tabs.toolbar_height() as f32 + 10.0);
        app.on_mouse(liminalkit_shell::MouseButton::Left, true);
        assert_eq!(app.tabs.active_index(), 0);
        assert_eq!(app.tabs.book().plain_text(), "first");
        app.tabs.select(1);
        assert_eq!(app.tabs.book().plain_text(), "second");
    }

    #[test]
    fn compact_file_row_and_tab_offsets_keep_mouse_and_ime_aligned() {
        let mut app = app();
        let mut buffer = liminalkit_raster::WindowBuffer::new(2200, 1400);
        let scale = liminalkit_shell::Scale::new(2.0);
        app.render(&mut buffer, scale);
        assert_eq!(
            app.tab_height, 0,
            "one manuscript uses the native title bar"
        );
        let file = app
            .tabs
            .toolbar_buttons(2200)
            .into_iter()
            .find(|button| button.action == docwrite_app::Action::FileMenu)
            .unwrap();
        app.on_cursor((file.x + file.w / 2) as f32, (file.y + file.h / 2) as f32);
        app.on_mouse(liminalkit_shell::MouseButton::Left, true);
        app.on_mouse(liminalkit_shell::MouseButton::Left, false);
        let menu = app.tabs.file_menu().unwrap().clone();
        let (top, h) = menu.row_rect(0).unwrap();
        assert_eq!(menu.items[0].command, Some(TabCommand::New));
        app.on_cursor((menu.x + 20) as f32, (top + h / 2) as f32);
        app.on_mouse(liminalkit_shell::MouseButton::Left, true);
        app.on_mouse(liminalkit_shell::MouseButton::Left, false);
        assert_eq!(app.tabs.len(), 2);
        app.tabs.type_text("second manuscript");
        app.render(&mut buffer, scale);
        assert_eq!(app.tabs.toolbar_height(), 48);
        assert_eq!(app.tab_height, 40);
        let ime = app.ime_cursor_area().unwrap();
        assert_eq!(
            buffer.pixels[ime.y as usize * 2200 + ime.x as usize],
            app.tabs.theme().ink
        );
        app.on_cursor(12.0, 60.0);
        app.on_mouse(liminalkit_shell::MouseButton::Left, true);
        app.on_mouse(liminalkit_shell::MouseButton::Left, false);
        assert_eq!(app.tabs.active_index(), 0);
        app.tabs.open_prompt(PromptKind::GoToPage);
        app.render(&mut buffer, scale);
        let ime = app.ime_cursor_area().unwrap();
        assert!(
            ime.y + ime.height <= 48.0,
            "prompt IME stays in the first row"
        );
        assert_eq!(
            buffer.pixels[ime.y as usize * 2200 + ime.x as usize],
            app.tabs.theme().ink
        );
    }
}
