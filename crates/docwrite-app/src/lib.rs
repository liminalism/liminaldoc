//! Editor behavior the window hosts: paging, snap, painting, and the latency budget.

pub mod autosave;
mod commands;
mod document_io;
mod map;
pub mod menu;
mod nav;
mod prompt;
pub mod recent;
pub mod recovery;
mod research;
mod scrollbar;
mod selection;
pub mod tabs;
mod toolbar;
mod trace;
mod view;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use commands::PageSlot;

pub use map::{BookMap, MapKind, MapRow, SIDEBAR_W};
pub use menu::{FileMenu, MenuItem};
pub use nav::{Pager, Phase};
pub use prompt::{Prompt, PromptMotion};
pub use recent::RecentFiles;
pub use research::SourceAction;
pub use scrollbar::{SCROLLBAR_W, ScrollbarLayout};
pub use toolbar::{Action, Button, PromptKind, TOOLBAR_H};
pub use trace::{FrameMetrics, InputTrace, ReplayStep, TraceCommand};
use view::Sidebar;
pub use view::{NIGHT, PAPER, PointerShape, SANZO_EARTH, SANZO_SEA, Theme, WARM, WORDPERFECT};

use docwrite_model::{Book, Direction, Motion, NoteId, Position, Selection};
use docwrite_typeset::{
    Document, EditReport, Face, GlyphAtlas, NOTE_MARK_CLUSTER, UpdateTiming, from_book_with,
    update_from_book_timed,
};
use map::BookMap as Map;

const DESK: u32 = PAPER.desk;
const PAGE_COLOR: u32 = PAPER.page;
/// Selection highlight painted behind selected glyphs in the paged view.
pub const SELECTION: u32 = PAPER.selection;

/// A footnote being typed at the foot of its page. The hotkey opens one,
/// typing goes to the note's body, and Enter returns to `anchor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FootnoteEdit {
    /// The note being typed.
    pub note: NoteId,
    /// The block the note hangs from.
    pub block: docwrite_model::BlockId,
    /// Where the caret was when the session opened; Enter restores it.
    pub anchor: Selection,
    /// Caret position in the note's body, in characters.
    pub cursor: usize,
}

/// Shaped chrome labels, by face, size and text.
///
/// The toolbar, page counter, running heads, folios, map rows and tab
/// strip are shaped through harfrust on every frame they are painted, and
/// every keystroke paints a frame. None of that text changes from one key
/// to the next, so the shaped glyphs are kept: a label costs a hash lookup
/// after its first frame. The cache is bounded and simply starts over when
/// it fills, which a session of ordinary editing never reaches.
#[derive(Debug, Default)]
pub(crate) struct LabelCache {
    entries: HashMap<(u64, u32), HashMap<String, ShapedLabel>>,
    count: usize,
}

/// One shaped label, shared between the cache and the frame drawing it.
type ShapedLabel = Arc<Vec<docwrite_typeset::Glyph>>;

impl LabelCache {
    const CAPACITY: usize = 2048;

    /// `text` shaped in `face` at `size` pixels, plain features.
    pub(crate) fn shaped(&mut self, face: &Face, text: &str, size: f32) -> ShapedLabel {
        let size = size.max(1.0);
        let bucket = (face.id(), size.to_bits());
        if let Some(glyphs) = self.entries.get(&bucket).and_then(|texts| texts.get(text)) {
            return Arc::clone(glyphs);
        }
        if self.count >= Self::CAPACITY {
            self.entries.clear();
            self.count = 0;
        }
        let glyphs = Arc::new(face.shape(text, size, &[]).unwrap_or_default());
        self.entries
            .entry(bucket)
            .or_default()
            .insert(text.to_owned(), Arc::clone(&glyphs));
        self.count += 1;
        glyphs
    }

    /// Advance width of `text` in `face` at `size` pixels.
    pub(crate) fn width(&mut self, face: &Face, text: &str, size: f32) -> f32 {
        self.shaped(face, text, size)
            .iter()
            .map(|glyph| glyph.x_advance)
            .sum()
    }
}

/// Where one painted frame spent its time. Zero for phases that did not run.
#[derive(Clone, Copy, Debug, Default)]
pub struct FramePhases {
    /// The whole layout refresh, incremental update included.
    pub layout: Duration,
    /// The incremental update's own breakdown.
    pub update: UpdateTiming,
    /// Painting the visible pages.
    pub pages: Duration,
    /// Painting the toolbar, map, scrollbar and menus.
    pub chrome: Duration,
    /// The whole paint.
    pub total: Duration,
}

/// Caret rectangle in window pixels, for the IME candidate window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretRect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// The manuscript and the page viewport the window draws.
pub struct Editor {
    book: Book,
    pager: Pager,
    /// The font family, regular first; empty when no font could be loaded.
    /// Loaded lazily on the first layout so opening a tab never waits on fonts.
    faces: Vec<Face>,
    /// Which discovered family `faces` holds, and its display name.
    family_index: usize,
    family_name: String,
    /// The laid-out book. Edits mark it stale; the next paint brings it up
    /// to date incrementally rather than laying the book out again.
    document: Option<Document>,
    layout_stale: bool,
    /// Set by an edit: the next layout refresh scrolls to the caret's page.
    follow_caret: bool,
    last_layout: Option<EditReport>,
    layout_generation: u64,
    atlas: GlyphAtlas,
    /// Shaped chrome labels.
    pub(crate) labels: LabelCache,
    /// Where the last frame spent its time.
    phases: FramePhases,
    preedit: String,
    caret: Option<CaretRect>,
    /// A soft line end shares a text position with the next line's start.
    line_end: Option<(Position, u32, usize)>,
    map: Map,
    cursor: (f32, f32),
    pressed_row: Option<usize>,
    text_pointer: selection::TextPointer,
    bundle: Option<PathBuf>,
    dirty: bool,
    /// Counts edits, so an untitled draft is written to recovery only when changed.
    revision: u64,
    recovered_revision: u64,
    /// Application-owned bundle that keeps an untitled draft across a crash.
    recovery: Option<PathBuf>,
    recovery_root: Option<PathBuf>,
    /// Last save or autosave attempt; typing does not restart this clock.
    last_save_attempt: Instant,
    autosave_request: Option<autosave::AutosaveInterval>,
    /// A background save is writing the bundle.
    saving: Arc<AtomicBool>,
    save_error: Arc<Mutex<Option<String>>>,
    /// The font file the face was parsed from, for PDF export.
    font_files: Vec<Vec<u8>>,
    /// Where the last paint put each visible page, for pointer hits.
    slots: Vec<PageSlot>,
    /// Physical pixels per logical pixel. Chrome is sized in logical pixels.
    ui_scale: f32,
    sidebar: Sidebar,
    fullscreen: bool,
    /// A full-screen change the window has not applied yet.
    fullscreen_request: Option<bool>,
    /// Caret page and position in inches (line from the top, position from
    /// the left edge), as of the last paint.
    caret_status: Option<(u32, f32, f32)>,
    /// Colors of the paint in progress.
    theme_now: Theme,
    view_theme: Theme,
    /// Two facing pages side by side.
    spread: bool,
    /// The last search, for find-again.
    last_find: Option<String>,
    /// The search text while a replace prompt asks for its replacement.
    replacing: Option<String>,
    /// A toolbar prompt taking typed text, when one is open.
    prompt: Option<Prompt>,
    /// Chapter and optional insertion point captured when its prompt opened.
    chapter_target: Option<(docwrite_model::ChapterId, Option<Position>)>,
    file_command: Option<tabs::TabCommand>,
    /// A footnote being typed at the foot of the page, when one is open.
    /// Prompts and footnote sessions never overlap.
    footnote: Option<FootnoteEdit>,
    /// Size of the last painted frame, for toolbar and pane hits.
    frame_width: i32,
    frame_height: i32,
    /// A source PDF open beside the manuscript.
    research: Option<research::Research>,
    /// Page count of the document the last [`Self::paint`] laid out.
    pages_painted: u32,
    /// The File menu, while it is open.
    menu: Option<menu::FileMenu>,
    /// Recently opened files the menu lists, newest first.
    recent: Vec<PathBuf>,
    /// Running total of a trackpad gesture, in pages, while one is under way.
    gesture: Option<f64>,
    /// Where the last paint put the scrollbar.
    scrollbar: Option<ScrollbarLayout>,
    /// Pointer offset from the thumb's top while the thumb is dragged.
    scroll_grab: Option<f32>,
    /// Height of one page row plus its gap, in window pixels, as of the
    /// last paint. Converts pixel scrolling into pages.
    row_pitch: f64,
    /// Pages per row the last paint drew (two in spread view).
    per_row: u32,
}

impl Editor {
    /// An editor on a new, one-paragraph book (it seeds one sample
    /// paragraph). Fixtures that need empty input should use
    /// [`Self::with_book`] with `Book::new` instead.
    pub fn new() -> Self {
        let mut book = Book::new("Untitled");
        let _ = book.insert("The book opens on a page.");
        Self::with_book(book)
    }

    /// Open `book` in the editor. Fonts load and layout happens on the
    /// first paint, so opening never waits on either.
    pub fn with_book(book: Book) -> Self {
        Self {
            book,
            pager: Pager::new(1, false),
            faces: Vec::new(),
            family_index: 0,
            family_name: "Default".into(),
            document: None,
            layout_stale: true,
            follow_caret: false,
            last_layout: None,
            layout_generation: 0,
            atlas: GlyphAtlas::new(),
            labels: LabelCache::default(),
            phases: FramePhases::default(),
            preedit: String::new(),
            caret: None,
            line_end: None,
            map: Map::new(),
            cursor: (0.0, 0.0),
            pressed_row: None,
            text_pointer: selection::TextPointer::default(),
            bundle: None,
            dirty: false,
            revision: 0,
            recovered_revision: 0,
            recovery: None,
            recovery_root: None,
            last_save_attempt: Instant::now(),
            autosave_request: None,
            saving: Arc::new(AtomicBool::new(false)),
            save_error: Arc::new(Mutex::new(None)),
            font_files: Vec::new(),
            slots: Vec::new(),
            ui_scale: 1.0,
            sidebar: Sidebar::default(),
            fullscreen: false,
            fullscreen_request: None,
            caret_status: None,
            theme_now: PAPER,
            view_theme: PAPER,
            spread: false,
            last_find: None,
            replacing: None,
            prompt: None,
            chapter_target: None,
            file_command: None,
            footnote: None,
            frame_width: 0,
            frame_height: 0,
            research: None,
            pages_painted: 0,
            menu: None,
            recent: Vec::new(),
            gesture: None,
            scrollbar: None,
            scroll_grab: None,
            row_pitch: 800.0,
            per_row: 1,
        }
    }

    /// Pages in the document the last [`Self::paint`] drew. Zero before the first paint.
    pub fn pages_painted(&self) -> u32 {
        self.pages_painted
    }

    /// Count desk, page, and ink pixels using the colors [`Self::paint`] writes.
    pub fn census(pixels: &[u32]) -> (usize, usize, usize) {
        let mut desk = 0usize;
        let mut page = 0usize;
        let mut ink = 0usize;
        for pixel in pixels {
            if *pixel == DESK {
                desk += 1;
            } else if *pixel == PAGE_COLOR {
                page += 1;
            } else {
                ink += 1;
            }
        }
        (desk, page, ink)
    }

    /// Physical pixels per logical pixel on the window's display.
    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = scale.max(0.5);
    }

    /// Remember where the pointer is, in window pixels. The window calls
    /// this on cursor motion.
    pub fn hover(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        if self.text_pointer.drag.is_some() {
            self.drag_text();
            return;
        }
        self.menu_hover(x, y);
        self.scrollbar_drag(y);
        self.drag_sidebar(x);
        self.research_drag(x, y);
    }

    /// Press or release the pointer. A drag from one chapter row to another
    /// reorders the book. A click collapses the part or chapter under the pointer.
    pub fn pointer(&mut self, pressed: bool) {
        self.pointer_with_shift(pressed, false);
    }

    /// Press or release the pointer, preserving Shift-click selection.
    pub fn pointer_with_shift(&mut self, pressed: bool, shift: bool) {
        if !pressed && self.text_pointer.drag.is_some() {
            self.drag_text();
            let follow = self
                .text_pointer
                .drag
                .take()
                .is_some_and(|drag| drag.citation);
            if follow && self.book.selection().is_collapsed() {
                self.follow_citation();
            }
            return;
        }
        // A click goes where it lands; it never returns to a footnote anchor.
        self.abandon_footnote();
        if self.menu_pointer(pressed) {
            return;
        }
        if self.scrollbar_pointer(pressed) {
            return;
        }
        if self.sidebar.dragging && self.sidebar_pointer(pressed) {
            return;
        }
        if self.cursor.1 < self.toolbar_height() as f32 {
            if pressed
                && let Some(action) = self.button_at(self.cursor.0, self.cursor.1, self.frame_width)
            {
                self.run_action(action);
            }
            if !pressed {
                self.pressed_row = None;
            }
            return;
        }
        if self.sidebar_pointer(pressed) {
            return;
        }
        if self.research_pointer(pressed) {
            return;
        }
        if self.cursor.0 >= self.sidebar_width() as f32 {
            if pressed {
                self.begin_text_selection(shift);
            }
            return;
        }
        let rows = self.map.rows(&self.book);
        let hit = self.map.row_at(
            self.cursor.0 / self.ui_scale,
            self.cursor.1 / self.ui_scale,
            rows.len(),
        );
        if pressed {
            self.pressed_row = hit;
            return;
        }
        let Some(from) = self.pressed_row.take() else {
            return;
        };
        let Some(to) = hit else {
            return;
        };
        if from == to {
            // A chapter row takes you to the chapter; a part row folds.
            let chapter = rows
                .get(from)
                .filter(|row| row.kind == MapKind::Chapter)
                .and_then(|row| row.chapter_id);
            match chapter {
                Some(id) => self.go_to_chapter(id),
                None => self.map.activate(&self.book, from),
            }
        } else if self.map.drag_chapter(&mut self.book, from, to).is_ok() {
            self.edited();
        }
    }

    /// Sidebar rows in the order the window paints them.
    pub fn map_rows(&self) -> Vec<MapRow> {
        self.map.rows(&self.book)
    }

    /// Center of a sidebar row, matching [`BookMap::row_center`].
    pub fn map_row_center(index: usize) -> (f32, f32) {
        BookMap::row_center(index)
    }

    /// Bind autosave and snapshots to `path`. Typing does not write the bundle.
    pub fn open_bundle(&mut self, path: impl Into<PathBuf>) {
        self.drop_recovery();
        self.bundle = Some(path.into());
        self.last_save_attempt = Instant::now();
    }

    /// Write the bundle if an edit has landed since the last save.
    pub fn autosave(&mut self) -> Result<(), String> {
        self.wait_for_background_save()?;
        let Some(path) = self.bundle.clone() else {
            return Err("no bundle".into());
        };
        if !self.dirty {
            return Ok(());
        }
        self.book
            .save_bundle(&path)
            .map_err(|err| err.to_string())?;
        self.dirty = false;
        self.last_save_attempt = Instant::now();
        Ok(())
    }

    /// Store a named snapshot of the manuscript as it is now.
    pub fn save_named_snapshot(&mut self, name: &str) -> Result<(), String> {
        let path = self.bundle.clone().ok_or_else(|| "no bundle".to_string())?;
        self.wait_for_background_save()?;
        self.book
            .save_snapshot(&path, name)
            .map_err(|err| err.to_string())
    }

    /// Replace the open manuscript with a named snapshot.
    pub fn restore_named_snapshot(&mut self, name: &str) -> Result<(), String> {
        let path = self.bundle.clone().ok_or_else(|| "no bundle".to_string())?;
        self.wait_for_background_save()?;
        self.book = Book::load_snapshot(&path, name).map_err(|err| err.to_string())?;
        self.dirty = false;
        self.layout_stale = true;
        Ok(())
    }

    /// Move the caret one character back.
    pub fn move_left(&mut self) {
        self.line_end = None;
        let _ = self.book.move_caret(Motion::Char(Direction::Backward));
        self.follow_caret = true;
    }

    /// Move the caret one character on.
    pub fn move_right(&mut self) {
        self.line_end = None;
        let _ = self.book.move_caret(Motion::Char(Direction::Forward));
        self.follow_caret = true;
    }

    /// Extend the selection one character back.
    pub fn extend_left(&mut self) {
        self.line_end = None;
        let _ = self
            .book
            .extend_selection(Motion::Char(Direction::Backward));
        self.follow_caret = true;
    }

    /// Extend the selection one character on.
    pub fn extend_right(&mut self) {
        self.line_end = None;
        let _ = self.book.extend_selection(Motion::Char(Direction::Forward));
        self.follow_caret = true;
    }

    /// Insert text the input method finished composing.
    pub fn commit_ime(&mut self, text: &str) {
        self.preedit.clear();
        if !text.is_empty() && self.book.insert(text).is_ok() {
            self.edited();
        }
    }

    /// Show an in-progress composition without changing the manuscript.
    pub fn set_preedit(&mut self, text: &str) {
        self.preedit = text.to_string();
    }

    /// The composition the window is showing, if any.
    pub fn preedit(&self) -> &str {
        &self.preedit
    }

    /// Caret rectangle from the last [`Self::paint`], in window pixels.
    pub fn caret_area(&self) -> Option<CaretRect> {
        self.caret
    }

    /// Draw the window's document tabs and file commands above the page view.
    pub fn paint_tab_strip(
        &mut self,
        buffer: &mut liminalkit_raster::WindowBuffer,
        labels: &[String],
        active: usize,
        top: i32,
    ) {
        self.ensure_fonts();
        let layout = tabs::TabStripLayout::new(buffer.width as i32, self.ui_scale, labels.len());
        let width = buffer.width as i32;
        let mut painter = liminalkit_raster::Painter::new(buffer);
        painter.fill_rect(
            liminalkit_raster::Rect::new(0, top, width, layout.height),
            self.theme_now.chrome,
        );
        let Some(face) = self.faces.first() else {
            return;
        };
        let atlas = &mut self.atlas;
        let labels_cache = &mut self.labels;
        let ink = self.theme_now.chrome_ink;
        let size = 11.0 * self.ui_scale;
        let baseline = top + (14.0 * self.ui_scale) as i32;
        for (index, label) in labels.iter().enumerate() {
            let x = index as i32 * layout.tab_width;
            painter.fill_rect(
                liminalkit_raster::Rect::new(
                    x,
                    top,
                    layout.tab_width.saturating_sub(1),
                    layout.height,
                ),
                if index == active {
                    self.theme_now.selection
                } else {
                    self.theme_now.chrome
                },
            );
            let limit = (layout.tab_width / (6.0 * self.ui_scale).max(1.0) as i32).max(0) as usize;
            let shown: String = label.chars().take(limit.saturating_sub(2)).collect();
            painter.push_clip(liminalkit_raster::Rect::new(
                x,
                top,
                layout.tab_width,
                layout.height,
            ));
            paint_tab_label(
                atlas,
                labels_cache,
                &mut painter,
                face,
                &shown,
                x + (5.0 * self.ui_scale) as i32,
                baseline,
                size,
                ink,
            );
            painter.pop_clip();
        }
    }

    /// Draw the desk, the page stack, and the manuscript on those pages.
    pub fn paint(&mut self, buffer: &mut liminalkit_raster::WindowBuffer) {
        // Layout first: it can move the view to follow the caret.
        let frame_started = Instant::now();
        self.refresh_layout();
        self.phases.layout = frame_started.elapsed();
        let pages_started = Instant::now();
        self.caret = None;
        self.caret_status = None;
        self.slots.clear();
        let theme = self.theme();
        self.theme_now = theme;
        for pixel in buffer.pixels.iter_mut() {
            *pixel = theme.desk;
        }
        let width = buffer.width as i32;
        let height = buffer.height as i32;
        if width <= 0 || height <= 0 {
            return;
        }
        self.frame_width = width;
        self.frame_height = height;
        let content_left = self.sidebar_width().min(width - 1);
        let content_w = (width - content_left - self.pane_width() - self.scrollbar_width()).max(1);
        let (trim_w, trim_h) = self
            .document
            .as_ref()
            .map(|document| document.geometry())
            .map_or((432.0, 648.0), |geometry| {
                (geometry.page_width, geometry.page_height)
            });
        let bar = self.toolbar_height();
        let per_row = if self.spread && !self.fullscreen {
            2
        } else {
            1
        };
        let status_h = if self.fullscreen {
            (22.0 * self.ui_scale) as i32
        } else {
            0
        };
        let gap = (3.0 * self.ui_scale).round().max(1.0) as i32;
        let max_w = ((content_w - 2 * gap) / per_row).max(1);
        let max_h = (height - bar - status_h - 2 * gap).max(1);
        let (page_w, page_h) = if self.fullscreen {
            fit_page_size(trim_w, trim_h, max_w, max_h)
        } else {
            fit_page_size(trim_w, trim_h, max_w, i32::MAX)
        };
        self.row_pitch = f64::from((page_h + gap).max(1));
        self.per_row = per_row as u32;
        let visible_page_height = (height - bar - gap).max(1);
        let tail = ((page_h - visible_page_height).max(0) as f64
            / f64::from((page_h + gap).max(1)))
            * f64::from(per_row);
        self.pager.set_view_tail(tail);
        // In spread view a row holds a verso and a recto: pages 2|3, 4|5, ...,
        // with page 1 alone on the right, as in a bound book.
        let scroll = self.pager.scroll() / f64::from(per_row);
        let mut origin = bar + gap - (scroll.fract() * f64::from(page_h + gap)) as i32;
        let first = scroll.floor() as i32;
        if self.footnote.is_some()
            && self.footnote_page().is_some_and(|page| {
                let row = if per_row == 2 { page / 2 } else { page - 1 };
                row as i32 == first
            })
        {
            // The zoomed page is taller than the view: lift the stack so
            // the footnote at its foot (and the session caret) shows. The
            // pager stays page-aligned; ending the session drops the view
            // back exactly where the hotkey found it.
            origin -= (origin + page_h - height).max(0);
        }
        // Build the hit geometry before painting, so edge scrolling can
        // extend a held selection before its highlight is drawn.
        let count = self
            .document
            .as_ref()
            .map_or(1, |document| document.page_count()) as i32;
        for slot in -1..4 {
            let row = first + slot;
            if row < 0 {
                continue;
            }
            let top = origin + slot * (page_h + gap);
            let row_left = content_left + (content_w - page_w * per_row) / 2;
            for column in 0..per_row {
                let number = if per_row == 2 {
                    row * 2 + column
                } else {
                    row + 1
                };
                if number < 1 || number > count {
                    continue;
                }
                self.slots.push(PageSlot {
                    page: number as u32,
                    left: row_left + column * page_w,
                    top,
                    width: page_w,
                    height: page_h,
                    scale: (page_w as f32 / trim_w.max(1.0))
                        .min(page_h as f32 / trim_h.max(1.0))
                        .max(0.05),
                });
            }
        }
        self.drag_text();
        let document = self.document.take();
        // Selection and caret are the same for every page of the frame.
        let selected = document
            .as_ref()
            .map(|document| selected_spans(&self.book, document))
            .unwrap_or_default();
        let focus = document
            .as_ref()
            .and_then(|document| focus_mark(&self.book, document));
        self.pages_painted = document.as_ref().map(|laid| laid.page_count()).unwrap_or(0);
        let last_page = if document.is_some() {
            self.pages_painted as i32
        } else {
            1
        };
        let mut painter = liminalkit_raster::Painter::new(buffer);
        // In full screen the status line owns the bottom strip: pages stop
        // above it instead of sliding underneath.
        if status_h > 0 {
            painter.push_clip(liminalkit_raster::Rect::new(0, 0, width, height - status_h));
        }
        for slot in -1..4 {
            let row = first + slot;
            if row < 0 {
                continue;
            }
            let top = origin + slot * (page_h + gap);
            // A row entirely above or below the window has nothing to show;
            // painting it would still walk every glyph of its pages.
            if top + page_h + gap <= bar || top >= height {
                continue;
            }
            let row_left = content_left + (content_w - page_w * per_row) / 2;
            if self.fullscreen {
                painter.fill_rect(
                    liminalkit_raster::Rect::new(0, top - gap, width, gap),
                    theme.rule,
                );
            }
            for column in 0..per_row {
                let number = if per_row == 2 {
                    row * 2 + column
                } else {
                    row + 1
                };
                if number < 1 || number > last_page {
                    continue;
                }
                let left = row_left + column * page_w;
                painter.fill_rect(
                    liminalkit_raster::Rect::new(left, top, page_w, page_h),
                    theme.page,
                );
                if let Some(document) = document.as_ref() {
                    self.paint_page(
                        &mut painter,
                        document,
                        number as u32,
                        left,
                        top,
                        page_w,
                        page_h,
                        &selected,
                        focus,
                    );
                }
            }
            if per_row == 2 {
                // The gutter between the two pages.
                painter.fill_rect(
                    liminalkit_raster::Rect::new(row_left + page_w - 1, top, 2, page_h),
                    theme.rule,
                );
            }
        }
        if status_h > 0 {
            painter.pop_clip();
        }
        self.phases.pages = pages_started.elapsed();
        let chrome_started = Instant::now();
        let face = document.as_ref().map(|document| document.face());
        if self.fullscreen {
            if let Some(face) = face {
                self.paint_status_line(&mut painter, face, width, height);
                if self.prompt.is_some() {
                    self.paint_toolbar(&mut painter, face, width, self.pages_painted);
                }
            }
        } else {
            self.paint_scrollbar(&mut painter);
            if let Some(face) = face {
                self.paint_research(&mut painter, face, width, height);
                let pages = self.pages_painted;
                self.paint_toolbar(&mut painter, face, width, pages);
            }
            if !self.has_map() {
            } else if self.sidebar.collapsed {
                self.paint_tab(&mut painter);
            } else {
                self.paint_sidebar(&mut painter, height, face);
            }
        }
        if let Some(face) = face {
            // The menu sits over everything, the status line included.
            self.paint_file_menu(&mut painter, face);
        }
        self.document = document;
        self.phases.chrome = chrome_started.elapsed();
        self.phases.total = frame_started.elapsed();
    }

    /// Where the most recent [`Self::paint`] spent its time.
    pub fn last_frame_phases(&self) -> FramePhases {
        self.phases
    }

    /// Load the current family when no faces are held yet. Layout and
    /// export both go through here, so opening never waits on fonts.
    pub(crate) fn ensure_fonts(&mut self) {
        if !self.faces.is_empty() {
            return;
        }
        let defs = family_defs();
        let count = defs.len();
        // Prefer a family that can display the manuscript's scripts. A font
        // with Latin coverage alone otherwise turns Thai imports into boxes.
        let mut characters = std::collections::BTreeSet::new();
        for block in self.book.blocks() {
            characters.extend(block.text().chars().filter(|c| !c.is_control()));
        }
        let probe: String = characters.into_iter().collect();
        let mut fallback = None;
        for step in 0..count {
            let index = (self.family_index + step) % count.max(1);
            if let Some(def) = defs.get(index)
                && let Some(loaded) = load_family_def(def)
            {
                let covered = loaded
                    .first()
                    .and_then(|(face, _)| face.shape(&probe, 12.0, &[]).ok())
                    .is_some_and(|glyphs| glyphs.iter().all(|g| g.id != 0));
                let (faces, font_files): (Vec<Face>, Vec<Vec<u8>>) = loaded.into_iter().unzip();
                if !covered && def.name != "Custom" {
                    if fallback.is_none() {
                        fallback = Some((faces, font_files, index, def.name));
                    }
                    continue;
                }
                let name = def.name;
                self.faces = faces;
                self.font_files = font_files;
                self.family_index = index;
                self.family_name = name.to_string();
                return;
            }
        }
        if let Some((faces, font_files, index, name)) = fallback {
            self.faces = faces;
            self.font_files = font_files;
            self.family_index = index;
            self.family_name = name.to_string();
        }
    }

    /// Bring the layout up to date with the book. A no-op when nothing
    /// changed since the last call. Only the edited paragraphs are shaped
    /// again, and pagination stops where the old page breaks line up.
    pub fn refresh_layout(&mut self) {
        self.ensure_fonts();
        if !self.layout_stale && self.document.is_some() {
            self.follow_caret_now();
            return;
        }
        self.layout_generation += 1;
        let report = match self.document.as_mut() {
            Some(document) => {
                update_from_book_timed(document, &self.book)
                    .ok()
                    .map(|(report, timing)| {
                        self.phases.update = timing;
                        report
                    })
            }
            None => None,
        };
        let report = match report {
            Some(report) => Some(report),
            None => {
                // First layout, or an update that failed: lay out afresh.
                let faces: Option<Vec<Face>> = self
                    .faces
                    .iter()
                    .map(|face| face.duplicate().ok())
                    .collect();
                let fresh = faces
                    .filter(|faces| !faces.is_empty())
                    .and_then(|faces| from_book_with(&self.book, faces).ok());
                let report = fresh.as_ref().map(|document| EditReport {
                    pages_laid_out: (1..=document.page_count()).collect(),
                    page_count: document.page_count(),
                    paragraphs_shaped: document.paragraphs().len(),
                });
                self.document = fresh;
                report
            }
        };
        if let Some(document) = self.document.as_ref() {
            self.pager.set_pages(document.page_count());
        }
        self.last_layout = report;
        self.layout_stale = false;
        self.follow_caret_now();
    }

    /// Scroll to the caret's page if an edit or caret move asked for it.
    fn follow_caret_now(&mut self) {
        if !std::mem::take(&mut self.follow_caret) {
            return;
        }
        let Some(document) = self.document.as_ref() else {
            return;
        };
        let page = self
            .line_end
            .filter(|(position, _, _)| *position == self.book.selection().focus)
            .map(|(_, page, _)| page)
            .or_else(|| {
                focus_mark(&self.book, document)
                    .and_then(|(paragraph, byte)| document.page_of(paragraph, byte))
            });
        let shown = |page: u32| {
            if self.pager.is_spread() {
                page / 2 == self.pager.page_top() / 2
            } else {
                self.pager.page_top() + 1 == page
            }
        };
        if let Some(page) = page {
            if !shown(page) {
                self.pager.jump_to(page - 1);
            } else if !self.spread
                && !self.fullscreen
                && let Some(slot) = self.slots.iter().find(|slot| slot.page == page)
                && slot.height > self.frame_height - self.toolbar_height()
                && let Some((_, line)) = self.caret_line()
            {
                let scale = slot.width as f32 / document.geometry().page_width;
                let y = slot.top as f32 + line.baseline * scale;
                let upper = self.toolbar_height() as f32 + line.em * scale;
                let lower = self.frame_height as f32 - line.em * scale;
                let delta = if y > lower {
                    y - lower
                } else if y < upper {
                    y - upper
                } else {
                    0.0
                };
                self.pager
                    .scroll_by(f64::from(delta) / self.row_pitch * f64::from(self.per_row));
            }
        }
    }

    /// What the most recent layout refresh did.
    pub fn last_layout(&self) -> Option<&EditReport> {
        self.last_layout.as_ref()
    }

    /// Number of layout passes, excluding navigation and repaint of cached pages.
    pub fn layout_generation(&self) -> u64 {
        self.layout_generation
    }

    /// The laid-out book, as of the last paint or [`Self::refresh_layout`].
    pub fn document(&self) -> Option<&Document> {
        self.document.as_ref()
    }

    /// Show facing pages side by side (or one page at a time). PageUp and
    /// PageDown then move a spread at a time.
    pub fn set_spread(&mut self, on: bool) {
        if self.spread == on {
            return;
        }
        self.spread = on;
        let top = self.pager.page_top();
        let pages = self.pager.pages();
        self.pager = Pager::new(pages, on);
        self.pager.jump_to(top);
        self.follow_caret = true;
    }

    /// Whether facing pages are shown side by side.
    pub fn is_spread(&self) -> bool {
        self.spread
    }

    /// Scroll so 1-based `page` is at the top of the view.
    pub fn jump_to_page(&mut self, page: u32) {
        self.refresh_layout();
        self.pager.jump_to(page.saturating_sub(1));
    }

    /// How many painted pixels equal `color`.
    pub fn pixels_of(&mut self, width: u32, height: u32, color: u32) -> usize {
        let mut buffer = liminalkit_raster::WindowBuffer::new(width, height);
        self.paint(&mut buffer);
        buffer
            .pixels
            .iter()
            .filter(|pixel| **pixel == color)
            .count()
    }

    /// How many pixels of the painted window are neither the desk nor the page.
    pub fn painted_ink(&mut self, width: u32, height: u32) -> usize {
        let mut buffer = liminalkit_raster::WindowBuffer::new(width, height);
        self.paint(&mut buffer);
        buffer
            .pixels
            .iter()
            .filter(|pixel| **pixel != DESK && **pixel != PAGE_COLOR)
            .count()
    }

    // A page's position and size, plus what to draw it with.
    #[allow(clippy::too_many_arguments)]
    fn paint_page(
        &mut self,
        painter: &mut liminalkit_raster::Painter<'_>,
        document: &Document,
        page: u32,
        left: i32,
        top: i32,
        page_w: i32,
        page_h: i32,
        selected: &[(usize, usize, usize)],
        focus: Option<(usize, usize)>,
    ) {
        if page == 0 || page > document.page_count() {
            return;
        }
        let geometry = document.geometry();
        let scale = (page_w as f32 / geometry.page_width.max(1.0))
            .min(page_h as f32 / geometry.page_height.max(1.0))
            .max(0.05);
        painter.push_clip(liminalkit_raster::Rect::new(left, top, page_w, page_h));
        if let Some(head) = document.page_running_head(page) {
            self.paint_page_label(
                painter,
                document.face(),
                &head,
                left + (document.page_content_inset(page) * scale) as i32,
                top + (12.0 * scale) as i32,
                10.0 * scale,
            );
        }
        if let Some(folio) = document.page_folio(page) {
            self.paint_page_label(
                painter,
                document.face(),
                &folio,
                left + page_w / 2,
                top + page_h - (18.0 * scale) as i32,
                10.0 * scale,
            );
        }
        let lines = document.page_painted_lines(page);
        let inset = document.page_content_inset(page);
        let mut caret_x = left + (inset * scale) as i32;
        let mut caret_y = top + geometry.margin_top as i32;
        let mut saw_focus = false;
        let face = document.face();
        let line_end = self
            .line_end
            .filter(|(position, _, _)| *position == self.book.selection().focus);
        for line in lines.iter() {
            let baseline = top + (line.baseline * scale) as i32;
            let start_x = left as f32 + (inset + line.indent) * scale;
            let line_start = line
                .glyphs
                .iter()
                .filter(|glyph| glyph.cluster < NOTE_MARK_CLUSTER)
                .map(|glyph| glyph.cluster as usize)
                .min()
                .unwrap_or(0);
            if let Some((paragraph, focus_byte)) = focus
                && paragraph == line.paragraph
                && line_start <= focus_byte
                && line_end
                    .is_none_or(|(_, end_page, start)| end_page == page && line_start == start)
            {
                // Lines come in order, so the last line of the paragraph that
                // starts at or before the caret is the caret's line.
                saw_focus = true;
                let before: f32 = line
                    .glyphs
                    .iter()
                    .filter(|glyph| (glyph.cluster as usize) < focus_byte)
                    .map(|glyph| glyph.x_advance * scale)
                    .sum();
                caret_x = (start_x + before).round() as i32;
                caret_y = baseline - (line.em * scale * 0.8) as i32;
            }
            let mut pen = start_x;
            for glyph in &line.glyphs {
                let advance = glyph.x_advance * scale;
                if span_covers(selected, line.paragraph, glyph.cluster as usize) {
                    painter.fill_rect(
                        liminalkit_raster::Rect::new(
                            pen.round() as i32,
                            baseline - (line.em * scale) as i32,
                            (advance.round() as i32).max(1),
                            (geometry.leading * scale).max(1.0) as i32,
                        ),
                        self.theme_now.selection,
                    );
                }
                let size = (if glyph.em > 0.0 {
                    glyph.em
                } else {
                    geometry.font_size
                }) * scale;
                draw_glyph(
                    &mut self.atlas,
                    painter,
                    document.face_at(glyph.face),
                    glyph.id,
                    size,
                    pen + glyph.x_offset * scale,
                    baseline - (glyph.y_offset * scale).round() as i32,
                    self.theme_now.ink,
                );
                pen += advance;
            }
        }
        if saw_focus && self.footnote.is_none() && self.prompt.is_none() {
            let height = (geometry.font_size * scale).max(1.0) as i32;
            if !self.preedit.is_empty() {
                // The composition is drawn at the caret, underlined, and does
                // not reflow the page: it is not in the manuscript yet.
                let preedit = self.preedit.clone();
                let size = geometry.font_size * scale;
                let before = caret_x;
                let baseline = caret_y + (height as f32 * 0.8) as i32;
                if let Ok(glyphs) = face.shape(&preedit, size.max(1.0), &[]) {
                    for glyph in glyphs {
                        draw_glyph(
                            &mut self.atlas,
                            painter,
                            face,
                            glyph.id,
                            size.max(1.0),
                            caret_x as f32,
                            baseline,
                            self.theme_now.ink,
                        );
                        caret_x += glyph.x_advance.round() as i32;
                    }
                }
                painter.fill_rect(
                    liminalkit_raster::Rect::new(
                        before,
                        baseline + 2,
                        (caret_x - before).max(1),
                        1,
                    ),
                    self.theme_now.ink,
                );
            }
            painter.fill_rect(
                liminalkit_raster::Rect::new(caret_x, caret_y, 2, height),
                self.theme_now.ink,
            );
            self.caret = Some(CaretRect {
                x: caret_x as f64,
                y: caret_y as f64,
                width: 2.0,
                height: height as f64,
            });
            let inches = |pixels: i32| pixels as f32 / scale / 72.0;
            self.caret_status =
                Some((page, inches(caret_y + height - top), inches(caret_x - left)));
        }
        // Footnotes: a short rule, then the lines at footnote size.
        if let Some((rule_y, rule_w)) = document.page_footnote_rule(page) {
            painter.fill_rect(
                liminalkit_raster::Rect::new(
                    left + (inset * scale) as i32,
                    top + (rule_y * scale) as i32,
                    (rule_w * scale) as i32,
                    (0.6 * scale).max(1.0) as i32,
                ),
                self.theme_now.ink,
            );
        }
        let footnote_lines = document.page_footnote_lines(page);
        for note in &footnote_lines {
            let mut pen = left as f32 + inset * scale;
            let baseline = top + (note.baseline * scale) as i32;
            for glyph in &note.glyphs {
                draw_glyph(
                    &mut self.atlas,
                    painter,
                    face,
                    glyph.id,
                    glyph.em * scale,
                    pen + glyph.x_offset * scale,
                    baseline - (glyph.y_offset * scale).round() as i32,
                    self.theme_now.ink,
                );
                pen += glyph.x_advance * scale;
            }
        }
        // The open footnote's caret, at the session's offset in the note.
        if let Some((caret_page, line_index, byte)) = self.footnote_caret_in(document)
            && caret_page == page
            && let Some(note) = footnote_lines.get(line_index)
        {
            let baseline = top + (note.baseline * scale) as i32;
            let before: f32 = note
                .glyphs
                .iter()
                .filter(|glyph| (glyph.cluster as usize) < byte)
                .map(|glyph| glyph.x_advance * scale)
                .sum();
            let mut caret_x = (left as f32 + inset * scale + before).round() as i32;
            let height = (note.em * scale).max(1.0) as i32;
            let caret_y = baseline - (height as f32 * 0.8) as i32;
            if !self.preedit.is_empty() {
                let preedit = self.preedit.clone();
                let size = note.em * scale;
                let start = caret_x;
                if let Ok(glyphs) = face.shape(&preedit, size.max(1.0), &[]) {
                    for glyph in glyphs {
                        draw_glyph(
                            &mut self.atlas,
                            painter,
                            face,
                            glyph.id,
                            size.max(1.0),
                            caret_x as f32,
                            baseline,
                            self.theme_now.ink,
                        );
                        caret_x += glyph.x_advance.round() as i32;
                    }
                }
                painter.fill_rect(
                    liminalkit_raster::Rect::new(start, baseline + 2, (caret_x - start).max(1), 1),
                    self.theme_now.ink,
                );
            }
            painter.fill_rect(
                liminalkit_raster::Rect::new(caret_x, caret_y, 2, height),
                self.theme_now.ink,
            );
            self.caret = Some(CaretRect {
                x: caret_x as f64,
                y: caret_y as f64,
                width: 2.0,
                height: height as f64,
            });
            let inches = |pixels: i32| pixels as f32 / scale / 72.0;
            self.caret_status =
                Some((page, inches(caret_y + height - top), inches(caret_x - left)));
        }
        painter.pop_clip();
    }

    fn paint_label(
        &mut self,
        painter: &mut liminalkit_raster::Painter<'_>,
        face: &Face,
        text: &str,
        x: i32,
        y: i32,
        size: f32,
    ) {
        self.paint_label_with_color(painter, face, text, x, y, size, self.theme_now.chrome_ink);
    }

    fn paint_page_label(
        &mut self,
        painter: &mut liminalkit_raster::Painter<'_>,
        face: &Face,
        text: &str,
        x: i32,
        y: i32,
        size: f32,
    ) {
        self.paint_label_with_color(painter, face, text, x, y, size, self.theme_now.ink);
    }

    fn paint_label_with_color(
        &mut self,
        painter: &mut liminalkit_raster::Painter<'_>,
        face: &Face,
        text: &str,
        x: i32,
        y: i32,
        size: f32,
        color: u32,
    ) {
        let glyphs = self.labels.shaped(face, text, size);
        let mut pen = x;
        for glyph in glyphs.iter() {
            draw_glyph(
                &mut self.atlas,
                painter,
                face,
                glyph.id,
                size.max(1.0),
                pen as f32,
                y,
                color,
            );
            pen += glyph.x_advance.round() as i32;
        }
    }

    fn paint_sidebar(
        &mut self,
        painter: &mut liminalkit_raster::Painter<'_>,
        height: i32,
        face: Option<&Face>,
    ) {
        let s = self.ui_scale;
        let width = self.sidebar_width();
        painter.fill_rect(
            liminalkit_raster::Rect::new(
                0,
                self.toolbar_height(),
                width,
                height - self.toolbar_height(),
            ),
            self.theme_now.chrome,
        );
        for (index, row) in self.map.rows(&self.book).iter().enumerate() {
            let (_, y) = BookMap::row_center(index);
            let y = y * s;
            let (color, indent) = match row.kind {
                MapKind::Chapter => (self.theme_now.chrome, 8.0),
                MapKind::Section => (self.theme_now.chrome, 16.0),
                MapKind::FrontMatter | MapKind::Part | MapKind::BackMatter => {
                    (self.theme_now.rule, 4.0)
                }
            };
            painter.fill_rect(
                liminalkit_raster::Rect::new(
                    (4.0 * s) as i32,
                    (y - 9.0 * s) as i32,
                    width - (8.0 * s) as i32,
                    (18.0 * s) as i32,
                ),
                color,
            );
            if let Some(face) = face {
                painter.push_clip(liminalkit_raster::Rect::new(
                    0,
                    0,
                    width - (12.0 * s) as i32,
                    height,
                ));
                self.paint_label(
                    painter,
                    face,
                    &row.title,
                    (indent * s) as i32,
                    (y + 4.0 * s) as i32,
                    11.0 * s,
                );
                painter.pop_clip();
            }
        }
    }

    /// The manuscript.
    pub fn book(&self) -> &Book {
        &self.book
    }

    /// Mutable access to the manuscript. The layout is refreshed on the
    /// next paint.
    pub fn book_mut(&mut self) -> &mut Book {
        self.edited();
        &mut self.book
    }

    /// The page viewport.
    pub fn pager(&self) -> &Pager {
        &self.pager
    }

    /// Insert `text` at the caret, replacing any selection. Newlines start paragraphs.
    pub fn type_text(&mut self, text: &str) {
        if self.book.type_text(text).is_ok() {
            self.edited();
        }
    }

    /// Delete the selection, or the character before the caret.
    pub fn backspace(&mut self) {
        if self.book.delete_backward().is_ok() {
            self.edited();
        }
    }

    /// Show the next page (or spread), keeping the caret's line offset.
    pub fn page_down(&mut self) {
        self.page(true, false);
    }

    /// Show the previous page (or spread), keeping the caret's line offset.
    pub fn page_up(&mut self) {
        self.page(false, false);
    }

    /// Scroll by `delta` pages in a gesture; `delta` is the gesture's total
    /// from its start, and the view settles on a page when it ends. Window
    /// input goes through [`Self::scroll_wheel`], which does the totalling.
    pub fn scroll_gesture(&mut self, delta: f64, phase: Phase) {
        self.gesture = None;
        self.pager.scroll_gesture(delta, phase);
    }
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("bundle", &self.bundle)
            .field("blocks", &self.book.block_ids().len())
            .field("pages", &self.pages_painted)
            .field("dirty", &self.dirty)
            .field("fullscreen", &self.fullscreen)
            .field("spread", &self.spread)
            .finish_non_exhaustive()
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

/// Blend one glyph with its pen at `x` on `baseline`. The fractional part of
/// `x` selects a subpixel raster.
#[allow(clippy::too_many_arguments)]
fn draw_glyph(
    atlas: &mut GlyphAtlas,
    painter: &mut liminalkit_raster::Painter<'_>,
    face: &Face,
    glyph: u16,
    size: f32,
    x: f32,
    baseline: i32,
    color: u32,
) {
    let whole = x.floor();
    let _ = atlas.with_glyph(face, glyph, size, x - whole, |bitmap| {
        if bitmap.width == 0 {
            return;
        }
        for (row, cells) in bitmap.coverage.chunks(bitmap.width as usize).enumerate() {
            painter.blend_coverage_row(
                whole as i32 + bitmap.left,
                baseline - bitmap.top + row as i32,
                color,
                cells,
            );
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn paint_tab_label(
    atlas: &mut GlyphAtlas,
    labels: &mut LabelCache,
    painter: &mut liminalkit_raster::Painter<'_>,
    face: &Face,
    text: &str,
    x: i32,
    baseline: i32,
    size: f32,
    color: u32,
) {
    let glyphs = labels.shaped(face, text, size);
    let mut pen = x;
    for glyph in glyphs.iter() {
        draw_glyph(
            atlas, painter, face, glyph.id, size, pen as f32, baseline, color,
        );
        pen += glyph.x_advance.round() as i32;
    }
}

/// Keypress-to-present budget. A few milliseconds, inside one 60 Hz frame.
pub const KEYPRESS_BUDGET: Duration = Duration::from_millis(8);

/// One discoverable font family: candidate files per style, in order.
/// No file is touched until the family loads.
pub(crate) struct FamilyDef {
    name: &'static str,
    files: [Vec<std::path::PathBuf>; 4],
}

/// Families the editor can set books in, serif first: `LEGE_DOCWRITE_FONT`
/// (with optional `_BOLD`, `_ITALIC`, `_BOLD_ITALIC`) overrides everything,
/// then Georgia, Times New Roman, Noto Serif, FreeSerif, Noto Sans and Lato
/// from their usual system and user directories, then the bundled test Noto.
/// Only TrueType is listed: PDF export embeds TrueType today.
pub(crate) fn family_defs() -> Vec<FamilyDef> {
    let mut defs = Vec::new();
    if let Some(regular) = std::env::var_os("LEGE_DOCWRITE_FONT") {
        let variant = |name: &str| {
            std::env::var_os(name)
                .map(std::path::PathBuf::from)
                .into_iter()
                .collect()
        };
        defs.push(FamilyDef {
            name: "Custom",
            files: [
                vec![regular.into()],
                variant("LEGE_DOCWRITE_FONT_BOLD"),
                variant("LEGE_DOCWRITE_FONT_ITALIC"),
                variant("LEGE_DOCWRITE_FONT_BOLD_ITALIC"),
            ],
        });
    }
    let supplemental = std::path::Path::new("/System/Library/Fonts/Supplemental");
    for name in ["Georgia", "Times New Roman"] {
        let file = |style: &str| vec![supplemental.join(format!("{name}{style}.ttf"))];
        defs.push(FamilyDef {
            name: if name == "Georgia" {
                "Georgia"
            } else {
                "Times New Roman"
            },
            files: [
                file(""),
                file(" Bold"),
                file(" Italic"),
                file(" Bold Italic"),
            ],
        });
    }
    let mut roots = vec![std::path::PathBuf::from("/usr/share/fonts")];
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        roots.push(home.join(".local/share/fonts"));
        roots.push(home.join(".fonts"));
    }
    let under = |dir: &str, file: &str| {
        roots
            .iter()
            .map(|root| root.join(dir).join(file))
            .collect::<Vec<_>>()
    };
    for (name, dir, stems) in [
        (
            "Noto Serif",
            "truetype/noto",
            [
                "NotoSerif-Regular",
                "NotoSerif-Bold",
                "NotoSerif-Italic",
                "NotoSerif-BoldItalic",
            ],
        ),
        (
            "FreeSerif",
            "truetype/freefont",
            [
                "FreeSerif",
                "FreeSerifBold",
                "FreeSerifItalic",
                "FreeSerifBoldItalic",
            ],
        ),
        (
            "Noto Sans",
            "truetype/noto",
            [
                "NotoSans-Regular",
                "NotoSans-Bold",
                "NotoSans-Italic",
                "NotoSans-BoldItalic",
            ],
        ),
        (
            "Lato",
            "truetype/lato",
            [
                "Lato-Regular",
                "Lato-Bold",
                "Lato-Italic",
                "Lato-BoldItalic",
            ],
        ),
    ] {
        let [regular, bold, italic, bold_italic] = stems;
        defs.push(FamilyDef {
            name,
            files: [
                under(dir, &format!("{regular}.ttf")),
                under(dir, &format!("{bold}.ttf")),
                under(dir, &format!("{italic}.ttf")),
                under(dir, &format!("{bold_italic}.ttf")),
            ],
        });
    }
    let noto = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../liminalkit/crates/liminalkit-text/test-fonts/NotoSans-Regular.ttf");
    defs.push(FamilyDef {
        name: "Noto Sans",
        files: [vec![noto], Vec::new(), Vec::new(), Vec::new()],
    });
    defs
}

/// Load one family: regular, bold, italic, bold italic, as faces and their
/// files. A style with no readable file is the regular face again. `None`
/// when even the regular style has no readable file.
pub(crate) fn load_family_def(def: &FamilyDef) -> Option<Vec<(Face, Vec<u8>)>> {
    let read = |paths: &[std::path::PathBuf]| -> Option<(Face, Vec<u8>)> {
        paths.iter().find_map(|path| {
            let bytes = std::fs::read(path).ok()?;
            Face::parse(bytes.clone()).ok().map(|face| (face, bytes))
        })
    };
    let regular = read(&def.files[0])?;
    let mut faces = Vec::with_capacity(4);
    for style in &def.files[1..] {
        let face = read(style).or_else(|| {
            regular
                .0
                .duplicate()
                .ok()
                .map(|face| (face, regular.1.clone()))
        });
        if let Some(face) = face {
            faces.push(face);
        }
    }
    faces.insert(0, regular);
    Some(faces)
}

pub(crate) fn byte_at(text: &str, chars: usize) -> usize {
    text.chars().take(chars).map(|ch| ch.len_utf8()).sum()
}

/// The caret as (layout paragraph index, byte offset in that paragraph).
fn focus_mark(book: &Book, document: &Document) -> Option<(usize, usize)> {
    let focus = book.selection().focus;
    let index = document.paragraph_of(focus.block.raw())?;
    let text = book.block(focus.block).ok()?.text();
    Some((index, byte_at(&text, focus.offset)))
}

/// Selected byte ranges as (layout paragraph index, start, end).
fn selected_spans(book: &Book, document: &Document) -> Vec<(usize, usize, usize)> {
    let ids = book.block_ids();
    let selection = book.selection();
    let anchor_i = ids.iter().position(|id| *id == selection.anchor.block);
    let focus_i = ids.iter().position(|id| *id == selection.focus.block);
    let (Some(anchor_i), Some(focus_i)) = (anchor_i, focus_i) else {
        return Vec::new();
    };
    let (from_i, from_off, to_i, to_off) =
        if (anchor_i, selection.anchor.offset) <= (focus_i, selection.focus.offset) {
            (
                anchor_i,
                selection.anchor.offset,
                focus_i,
                selection.focus.offset,
            )
        } else {
            (
                focus_i,
                selection.focus.offset,
                anchor_i,
                selection.anchor.offset,
            )
        };
    if from_i == to_i && from_off == to_off {
        return Vec::new();
    }
    let mut spans = Vec::new();
    for index in from_i..=to_i {
        let Some(id) = ids.get(index).copied() else {
            continue;
        };
        let Ok(len) = book.block_len(id) else {
            continue;
        };
        let start = if index == from_i {
            from_off.min(len)
        } else {
            0
        };
        let end = if index == to_i { to_off.min(len) } else { len };
        if start >= end {
            continue;
        }
        let Ok(block) = book.block(id) else {
            continue;
        };
        let Some(paragraph) = document.paragraph_of(id.raw()) else {
            continue;
        };
        spans.push((
            paragraph,
            byte_at(&block.text(), start),
            byte_at(&block.text(), end),
        ));
    }
    spans
}

fn span_covers(spans: &[(usize, usize, usize)], paragraph: usize, byte: usize) -> bool {
    spans
        .iter()
        .any(|(index, start, end)| *index == paragraph && byte >= *start && byte < *end)
}

/// Fit the physical page into both available dimensions with one scale.
fn fit_page_size(trim_w: f32, trim_h: f32, max_w: i32, max_h: i32) -> (i32, i32) {
    let scale = (max_w.max(1) as f32 / trim_w.max(1.0)).min(max_h.max(1) as f32 / trim_h.max(1.0));
    (
        (trim_w * scale).floor().max(1.0) as i32,
        (trim_h * scale).floor().max(1.0) as i32,
    )
}

#[cfg(test)]
mod page_fit_tests {
    use super::fit_page_size;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn physical_pages_fit_below_controls_at_normal_and_high_dpi_sizes() {
        let mut editor = super::Editor::new_document("Page visibility");
        editor.paste_text(&"A complete page must be readable before Page Down. ".repeat(1200));
        editor.refresh_layout();
        let pages = editor.pager().pages();
        let generation = editor.layout_generation();
        for (width, height, scale) in [
            (640, 480, 1.0),
            (1100, 800, 1.0),
            (2200, 1375, 2.0),
            (2560, 1600, 2.0),
        ] {
            editor.set_ui_scale(scale);
            for fullscreen in [false, true] {
                editor.set_fullscreen(fullscreen);
                let mut frame = liminalkit_raster::WindowBuffer::new(width, height);
                for page in [1, pages / 2, pages] {
                    editor.seek_page(page);
                    editor.paint(&mut frame);
                    let slot = editor.slots.iter().find(|slot| slot.page == page).unwrap();
                    let bottom = height as i32 - if fullscreen { (22.0 * scale) as i32 } else { 0 };
                    assert!(slot.top >= editor.toolbar_height());
                    if fullscreen {
                        assert!(slot.top + slot.height <= bottom);
                    } else {
                        assert!(
                            slot.width
                                >= width as i32
                                    - editor.sidebar_width()
                                    - editor.pane_width()
                                    - editor.scrollbar_width()
                                    - (8.0 * scale) as i32
                        );
                    }
                    assert!(slot.left >= 0 && slot.left + slot.width <= width as i32);
                    assert_eq!(
                        editor.layout_generation(),
                        generation,
                        "resizing chrome must not re-typeset the book"
                    );
                    if !fullscreen {
                        let counter = (70.0 * scale) as usize;
                        let ink = (0..editor.toolbar_height() as usize - 2)
                            .flat_map(|y| {
                                &frame.pixels[y * width as usize + width as usize - counter
                                    ..(y + 1) * width as usize]
                            })
                            .any(|&pixel| {
                                pixel != editor.theme().chrome && pixel != editor.theme().rule
                            });
                        assert!(ink, "page counter visible at {width}px, scale {scale}");
                    }
                }
            }
        }
    }

    #[test]
    fn page_and_spread_keep_trim_aspect_when_canvas_is_narrow() {
        // 1100x800, default 220px Book Map and 460px research pane.
        assert_eq!(fit_page_size(432.0, 648.0, 388, 728), (388, 582));
        assert_eq!(fit_page_size(432.0, 648.0, 194, 728), (194, 291));
        // With the pane closed, available height is the limit.
        assert_eq!(fit_page_size(432.0, 648.0, 848, 728), (485, 728));
    }
}

/// True when opening a window is not expected to succeed.
pub fn display_unavailable() -> bool {
    if std::env::var_os("LEGE_DOCWRITE_HEADLESS").is_some() {
        return true;
    }
    cfg!(unix)
        && !cfg!(target_os = "macos")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
}

impl Drop for Editor {
    /// Closing a draft that was saved or discarded removes its recovery
    /// bundle. A panic keeps it, so the next launch can offer it.
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.drop_recovery();
        }
    }
}

mod pages;
