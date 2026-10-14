//! How the window is arranged: the resizable Book Map, its collapsed tab,
//! and the full-screen writing mode.

use docwrite_typeset::Face;

use crate::{Editor, SIDEBAR_W, family_defs, load_family_def};

/// Colors one view paints with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Behind the pages.
    pub desk: u32,
    /// The paper.
    pub page: u32,
    /// Text and caret.
    pub ink: u32,
    /// Behind selected text.
    pub selection: u32,
    /// Between pages in full screen; the page edge in the page view.
    pub rule: u32,
    /// Window controls and side panels.
    pub chrome: u32,
    /// Text on window controls.
    pub chrome_ink: u32,
}

/// Lege viewer light palette.
pub const PAPER: Theme = Theme {
    desk: 0x00C9C9C9,
    page: 0x00FFFFFF,
    ink: 0x00181818,
    selection: 0x00568BD2,
    rule: 0x00909090,
    chrome: 0x00ECECEC,
    chrome_ink: 0x00181818,
};

/// Lege viewer night palette.
pub const NIGHT: Theme = Theme {
    desk: 0x00161616,
    page: 0x00252525,
    ink: 0x00D8D1C4,
    selection: 0x00638BC0,
    rule: 0x00484848,
    chrome: 0x00212121,
    chrome_ink: 0x00D8D1C4,
};

/// Lege viewer warm paper palette.
pub const WARM: Theme = Theme {
    desk: 0x00C9BFAA,
    page: 0x00F2E8D2,
    ..PAPER
};

/// Lege viewer Sanzo Earth palette.
pub const SANZO_EARTH: Theme = Theme {
    desk: 0x002C2825,
    page: 0x00F2E5D2,
    ink: 0x003A3430,
    selection: 0x00C78050,
    rule: 0x0098765B,
    chrome: 0x004C443D,
    chrome_ink: 0x00F7EEE2,
};

/// Lege viewer Sanzo Sea palette.
pub const SANZO_SEA: Theme = Theme {
    desk: 0x0017272C,
    page: 0x00E9F0E9,
    ink: 0x001D3035,
    selection: 0x004CA5A0,
    rule: 0x006A9294,
    chrome: 0x00264147,
    chrome_ink: 0x00E7F3EF,
};

/// Full screen: WordPerfect 5.1's blue field, light text, block in reverse.
pub const WORDPERFECT: Theme = Theme {
    desk: 0x0000_00AA,
    page: 0x0000_00AA,
    ink: 0x00E0_E0E0,
    selection: 0x0000_AAAA,
    rule: 0x0000_4ACC,
    chrome: 0x0000_00AA,
    chrome_ink: 0x00E0_E0E0,
};

/// Sidebar widths in logical pixels.
const SIDEBAR_MIN_OPEN: f32 = 96.0;
/// Released narrower than this, the sidebar folds into its tab.
const SIDEBAR_COLLAPSE_BELOW: f32 = 90.0;
const SIDEBAR_MAX: f32 = 640.0;
/// The drag handle's half-width around the sidebar's right edge.
const HANDLE: f32 = 5.0;
/// The tab a collapsed sidebar leaves at the window's left edge.
const TAB_W: f32 = 22.0;
const TAB_TOP: f32 = crate::toolbar::TOOLBAR_H + 4.0;
const TAB_H: f32 = 84.0;

/// The Book Map's width and whether it is folded away.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sidebar {
    /// Current width in logical pixels, live while dragging.
    pub width: f32,
    /// Width to restore when the tab is clicked.
    pub open_width: f32,
    pub collapsed: bool,
    pub dragging: bool,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            width: SIDEBAR_W as f32,
            open_width: SIDEBAR_W as f32,
            collapsed: false,
            dragging: false,
        }
    }
}

/// The pointer shape the window should show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerShape {
    /// The platform arrow.
    Arrow,
    /// An I-beam, over text.
    Text,
    /// A hand, over something to click.
    Hand,
    /// A column-resize arrow, over the Book Map's edge.
    ResizeColumn,
}

impl Editor {
    /// Whether the full-screen writing mode is on.
    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// Enter or leave full screen: borderless, no chrome, the page edge to
    /// edge in WordPerfect colors, with a status line.
    pub fn toggle_fullscreen(&mut self) {
        self.set_fullscreen(!self.fullscreen);
    }

    /// Enter (`true`) or leave full screen.
    pub fn set_fullscreen(&mut self, on: bool) {
        if self.fullscreen != on {
            self.fullscreen = on;
            self.fullscreen_request = Some(on);
            self.follow_caret = true;
        }
    }

    /// A full-screen change for the window to apply, once.
    pub fn poll_fullscreen(&mut self) -> Option<bool> {
        self.fullscreen_request.take()
    }

    /// The colors of the current view.
    pub fn theme(&self) -> Theme {
        if self.fullscreen {
            WORDPERFECT
        } else {
            self.view_theme
        }
    }

    /// The family the book is set in, as named by discovery.
    pub fn font_family_name(&self) -> &str {
        &self.family_name
    }

    /// Set the book in the next loadable family (serif first). The whole
    /// book is shaped again in the new faces on the next paint.
    pub fn cycle_font_family(&mut self) {
        self.ensure_fonts();
        let defs = family_defs();
        let count = defs.len();
        if count == 0 {
            return;
        }
        for step in 1..=count {
            let index = (self.family_index + step) % count;
            let loaded = defs
                .get(index)
                .and_then(|def| load_family_def(def).map(|loaded| (def.name, loaded)));
            if let Some((name, loaded)) = loaded {
                let (faces, font_files): (Vec<Face>, Vec<Vec<u8>>) = loaded.into_iter().unzip();
                self.faces = faces;
                self.font_files = font_files;
                self.family_index = index;
                self.family_name = name.to_string();
                self.document = None;
                self.layout_stale = true;
                self.follow_caret = true;
                return;
            }
        }
    }

    /// Cycle through the five Lege viewer color modes.
    pub fn cycle_theme(&mut self) {
        self.view_theme = if self.view_theme == PAPER {
            NIGHT
        } else if self.view_theme == NIGHT {
            WARM
        } else if self.view_theme == WARM {
            SANZO_EARTH
        } else if self.view_theme == SANZO_EARTH {
            SANZO_SEA
        } else {
            PAPER
        };
    }

    /// Fold the Book Map into its tab, or open it again.
    pub fn toggle_sidebar(&mut self) {
        if self.sidebar.collapsed {
            self.expand_sidebar();
        } else {
            self.collapse_sidebar();
        }
    }

    /// Whether the Book Map is folded into its tab.
    pub fn sidebar_collapsed(&self) -> bool {
        self.sidebar.collapsed
    }

    /// The Book Map's width in logical pixels (zero when folded away).
    pub fn sidebar_logical_width(&self) -> f32 {
        if self.sidebar.collapsed {
            0.0
        } else {
            self.sidebar.width
        }
    }

    fn collapse_sidebar(&mut self) {
        if self.sidebar.width >= SIDEBAR_MIN_OPEN {
            self.sidebar.open_width = self.sidebar.width;
        }
        self.sidebar.collapsed = true;
        self.sidebar.width = 0.0;
    }

    fn expand_sidebar(&mut self) {
        self.sidebar.collapsed = false;
        self.sidebar.width = self.sidebar.open_width.max(SIDEBAR_MIN_OPEN);
    }

    /// The Book Map's width in window pixels; zero when folded or in full screen.
    pub(crate) fn sidebar_width(&self) -> i32 {
        if self.fullscreen || self.sidebar.collapsed {
            return 0;
        }
        (self.sidebar.width * self.ui_scale).round() as i32
    }

    fn near_sidebar_edge(&self, x: f32) -> bool {
        !self.fullscreen
            && !self.sidebar.collapsed
            && (x - self.sidebar_width() as f32).abs() <= HANDLE * self.ui_scale
    }

    fn on_tab(&self, x: f32, y: f32) -> bool {
        let s = self.ui_scale;
        !self.fullscreen
            && self.sidebar.collapsed
            && x < TAB_W * s
            && y >= TAB_TOP * s
            && y < (TAB_TOP + TAB_H) * s
    }

    /// Pointer motion while the sidebar edge is held resizes it. Returns
    /// whether the motion was consumed.
    pub(crate) fn drag_sidebar(&mut self, x: f32) -> bool {
        if !self.sidebar.dragging {
            return false;
        }
        self.sidebar.width = (x / self.ui_scale).clamp(0.0, SIDEBAR_MAX);
        true
    }

    /// Press or release on the sidebar's edge or tab. Returns whether the
    /// pointer event was consumed.
    pub(crate) fn sidebar_pointer(&mut self, pressed: bool) -> bool {
        let (x, y) = self.cursor;
        if pressed {
            if self.on_tab(x, y) {
                self.expand_sidebar();
                return true;
            }
            if self.near_sidebar_edge(x) {
                self.sidebar.dragging = true;
                return true;
            }
            return false;
        }
        if !self.sidebar.dragging {
            return false;
        }
        self.sidebar.dragging = false;
        if self.sidebar.width < SIDEBAR_COLLAPSE_BELOW {
            self.collapse_sidebar();
        } else {
            self.sidebar.open_width = self.sidebar.width;
        }
        true
    }

    /// The pointer shape for where the pointer is now.
    pub fn pointer_shape(&self) -> PointerShape {
        let (x, y) = self.cursor;
        if !self.sidebar.dragging && y < self.toolbar_height() as f32 {
            return if self.button_at(x, y, self.frame_width).is_some() {
                PointerShape::Hand
            } else {
                PointerShape::Arrow
            };
        }
        if self.sidebar.dragging || self.near_sidebar_edge(x) {
            PointerShape::ResizeColumn
        } else if self.on_tab(x, y) || (x < self.sidebar_width() as f32) {
            PointerShape::Hand
        } else if self.slots.iter().any(|slot| {
            x >= slot.left as f32
                && x < (slot.left + slot.width) as f32
                && y >= slot.top as f32
                && y < (slot.top + slot.height) as f32
        }) {
            PointerShape::Text
        } else {
            PointerShape::Arrow
        }
    }

    /// The folded sidebar's tab: a pull handle at the window's left edge.
    pub(crate) fn paint_tab(&self, painter: &mut pixelkit_raster::Painter<'_>) {
        let s = self.ui_scale;
        let rect = pixelkit_raster::Rect::new(
            0,
            (TAB_TOP * s) as i32,
            (TAB_W * s) as i32,
            (TAB_H * s) as i32,
        );
        painter.fill_rect(rect, self.theme_now.chrome);
        painter.fill_rect(
            pixelkit_raster::Rect::new(
                rect.x + rect.w - (2.0 * s) as i32,
                rect.y,
                (2.0 * s) as i32,
                rect.h,
            ),
            self.theme_now.rule,
        );
        // Three short rules: the map, folded.
        for row in 0..3 {
            painter.fill_rect(
                pixelkit_raster::Rect::new(
                    (6.0 * s) as i32,
                    rect.y + (30.0 * s) as i32 + (row as f32 * 8.0 * s) as i32,
                    (9.0 * s) as i32,
                    (2.0 * s).max(1.0) as i32,
                ),
                PAPER.ink,
            );
        }
    }

    /// WordPerfect's bottom line: the book on the left, where the caret is
    /// on the right, in inches from the page's top-left corner.
    pub(crate) fn paint_status_line(
        &mut self,
        painter: &mut pixelkit_raster::Painter<'_>,
        face: &Face,
        width: i32,
        height: i32,
    ) {
        let s = self.ui_scale;
        let bar = (22.0 * s) as i32;
        painter.fill_rect(
            pixelkit_raster::Rect::new(0, height - bar, width, bar),
            WORDPERFECT.desk,
        );
        let size = 11.0 * s;
        let baseline = height - (7.0 * s) as i32;
        let title = self.book.title().to_string();
        self.paint_label(painter, face, &title, (16.0 * s) as i32, baseline, size);
        let status = match self.caret_status {
            Some((page, line, pos)) => {
                format!(
                    "Pg {page}/{}  Ln {line:.2}\"  Pos {pos:.2}\"",
                    self.pager.pages()
                )
            }
            None => format!("Pg {}/{}", self.pager.page_top() + 1, self.pager.pages()),
        };
        let text_width: f32 = face
            .shape(&status, size, &[])
            .map(|glyphs| glyphs.iter().map(|glyph| glyph.x_advance).sum())
            .unwrap_or(0.0);
        self.paint_label(
            painter,
            face,
            &status,
            width - text_width as i32 - (16.0 * s) as i32,
            baseline,
            size,
        );
    }
}
