//! A vertical scrollbar at the right edge of the page area, so a long
//! manuscript can be dragged through, and the view's place in it is always
//! visible. The thumb's size shows how much of the book one screen holds.

use crate::{Editor, Phase};

/// Scrollbar width in logical pixels.
pub const SCROLLBAR_W: f32 = 10.0;
/// Smallest thumb in logical pixels, so a 1000-page book stays grabbable.
const MIN_THUMB: f32 = 24.0;
/// Pages scrolled per reported wheel line: about four lines of body text
/// on a page that fits the window.
const PAGES_PER_WHEEL_LINE: f64 = 0.1;

/// Where the last paint put the scrollbar, in window pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarLayout {
    /// Track left edge.
    pub x: i32,
    /// Track top.
    pub y: i32,
    /// Track width.
    pub w: i32,
    /// Track height.
    pub h: i32,
    /// Thumb top.
    pub thumb_top: i32,
    /// Thumb height.
    pub thumb_h: i32,
    /// The scroll range the track spans, in pages.
    pub max_scroll: f64,
}

impl ScrollbarLayout {
    /// Lay out a track at `x`, from `y` for `h` pixels, over a document
    /// whose view sits at `scroll` of `max_scroll` pages and shows
    /// `viewport` pages at once.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        scroll: f64,
        max_scroll: f64,
        viewport: f64,
        scale: f32,
    ) -> Self {
        let h = h.max(1);
        let extent = (max_scroll + viewport).max(viewport.max(1e-6));
        let min_thumb = ((MIN_THUMB * scale).round() as i32).min(h);
        let thumb_h = ((f64::from(h) * viewport / extent).round() as i32).clamp(min_thumb, h);
        let travel = (h - thumb_h).max(0);
        let fraction = if max_scroll > 0.0 {
            (scroll / max_scroll).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            x,
            y,
            w,
            h,
            thumb_top: y + (f64::from(travel) * fraction).round() as i32,
            thumb_h,
            max_scroll,
        }
    }

    /// Whether the point is on the track.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32
            && x < (self.x + self.w) as f32
            && y >= self.y as f32
            && y < (self.y + self.h) as f32
    }

    /// Whether the point is on the thumb.
    pub fn on_thumb(&self, x: f32, y: f32) -> bool {
        self.contains(x, y)
            && y >= self.thumb_top as f32
            && y < (self.thumb_top + self.thumb_h) as f32
    }

    /// The scroll position that puts the thumb's top at `top`.
    pub fn scroll_for_thumb_top(&self, top: f32) -> f64 {
        let travel = (self.h - self.thumb_h).max(0) as f32;
        if travel <= 0.0 {
            return 0.0;
        }
        let fraction = ((top - self.y as f32) / travel).clamp(0.0, 1.0);
        f64::from(fraction) * self.max_scroll
    }
}

impl Editor {
    /// Scrollbar width in window pixels; zero in full screen or on a
    /// one-page book.
    pub(crate) fn scrollbar_width(&self) -> i32 {
        if self.fullscreen || self.pager.pages() <= 1 {
            0
        } else {
            (SCROLLBAR_W * self.ui_scale).round() as i32
        }
    }

    /// Where the last paint put the scrollbar.
    pub fn scrollbar(&self) -> Option<ScrollbarLayout> {
        self.scrollbar
    }

    /// Wheel or trackpad input from the window. `pixel_dy` is the platform's
    /// pixel delta (positive turns toward earlier pages, the way the wheel
    /// turns); `line_dy` is set when the platform reported lines. A bare
    /// wheel notch moves the view and leaves it there; a gesture with
    /// phases accumulates while it moves and settles on a page when it ends.
    pub fn scroll_wheel(&mut self, pixel_dy: f32, line_dy: Option<f32>, phase: Phase) {
        let pages = match line_dy {
            Some(lines) if lines != 0.0 => -f64::from(lines) * PAGES_PER_WHEEL_LINE,
            Some(_) => 0.0,
            None => -f64::from(pixel_dy) / self.row_pitch * f64::from(self.per_row),
        };
        match phase {
            Phase::Started => {
                self.gesture = Some(pages);
                self.pager.scroll_gesture(pages, Phase::Started);
            }
            Phase::Moved => match self.gesture.as_mut() {
                Some(total) => {
                    *total += pages;
                    let total = *total;
                    self.pager.scroll_gesture(total, Phase::Moved);
                }
                None => self.pager.scroll_by(pages),
            },
            Phase::Ended | Phase::Cancelled => match self.gesture.take() {
                Some(total) => self.pager.scroll_gesture(total + pages, phase),
                None => self.pager.scroll_by(pages),
            },
        }
    }

    /// Scroll the view by `pages` and leave it there.
    pub fn scroll_by_pages(&mut self, pages: f64) {
        self.pager.scroll_by(pages);
    }

    pub(crate) fn scrollbar_pointer(&mut self, pressed: bool) -> bool {
        let (x, y) = self.cursor;
        if !pressed {
            return self.scroll_grab.take().is_some();
        }
        let Some(bar) = self.scrollbar else {
            return false;
        };
        if !bar.contains(x, y) {
            return false;
        }
        if bar.on_thumb(x, y) {
            self.scroll_grab = Some(y - bar.thumb_top as f32);
        } else if y < bar.thumb_top as f32 {
            self.pager.page_up();
        } else {
            self.pager.page_down();
        }
        true
    }

    pub(crate) fn scrollbar_drag(&mut self, y: f32) {
        let (Some(grab), Some(bar)) = (self.scroll_grab, self.scrollbar) else {
            return;
        };
        let scroll = bar.scroll_for_thumb_top(y - grab);
        self.pager.set_scroll(scroll);
        // Keep the thumb under the pointer between paints.
        self.scrollbar = Some(ScrollbarLayout::new(
            bar.x,
            bar.y,
            bar.w,
            bar.h,
            self.pager.scroll(),
            bar.max_scroll,
            self.viewport_pages(),
            self.ui_scale,
        ));
    }

    /// Whether the pointer is dragging the scrollbar thumb.
    pub fn scrollbar_dragging(&self) -> bool {
        self.scroll_grab.is_some()
    }

    /// How many pages the view shows at once, as of the last paint.
    pub(crate) fn viewport_pages(&self) -> f64 {
        let visible = (self.frame_height - self.toolbar_height()).max(1) as f64;
        (visible / self.row_pitch * f64::from(self.per_row)).max(0.05)
    }

    pub(crate) fn paint_scrollbar(&mut self, painter: &mut pixelkit_raster::Painter<'_>) {
        let w = self.scrollbar_width();
        if w == 0 {
            self.scrollbar = None;
            return;
        }
        let y = self.toolbar_height();
        let h = (self.frame_height - y).max(1);
        let x = self.frame_width - self.pane_width() - w;
        let bar = ScrollbarLayout::new(
            x,
            y,
            w,
            h,
            self.pager.scroll(),
            self.pager.max_scroll(),
            self.viewport_pages(),
            self.ui_scale,
        );
        let theme = self.theme_now;
        painter.fill_rect(pixelkit_raster::Rect::new(x, y, w, h), theme.chrome);
        let inset = self.ui_scale.round().max(1.0) as i32;
        painter.fill_rect(
            pixelkit_raster::Rect::new(
                x + inset,
                bar.thumb_top + inset,
                (w - 2 * inset).max(1),
                (bar.thumb_h - 2 * inset).max(1),
            ),
            if self.scroll_grab.is_some() {
                theme.chrome_ink
            } else {
                theme.rule
            },
        );
        self.scrollbar = Some(bar);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn long_book() -> Editor {
        let mut editor = Editor::new_document("Long");
        editor.paste_text(&"Dense text that fills page after page of the book. ".repeat(1500));
        editor.move_document_start(false);
        editor.refresh_layout();
        assert!(
            editor.pager().pages() > 10,
            "{} pages",
            editor.pager().pages()
        );
        // The first paragraph may follow a generated page; start from the top.
        editor.scroll_by_pages(-editor.pager().scroll());
        assert_eq!(editor.pager().scroll(), 0.0);
        editor
    }

    #[test]
    fn wheel_notches_scroll_freely_and_gestures_total_then_settle() {
        let mut editor = long_book();
        // X11 wheel: every notch is a Moved with one line, no Started/Ended.
        for _ in 0..5 {
            editor.scroll_wheel(-24.0, Some(-1.0), Phase::Moved);
        }
        assert!(
            (editor.pager().scroll() - 0.5).abs() < 1e-9,
            "{}",
            editor.pager().scroll()
        );
        editor.scroll_wheel(24.0, Some(1.0), Phase::Moved);
        assert!((editor.pager().scroll() - 0.4).abs() < 1e-9);
        // A trackpad gesture reports per-event pixel steps; they add up, and
        // the view settles on the nearer page when the fingers lift.
        let mut frame = pixelkit_raster::WindowBuffer::new(1100, 800);
        editor.paint(&mut frame);
        let pitch = editor.row_pitch as f32;
        editor.scroll_by_pages(-editor.pager().scroll());
        editor.scroll_wheel(-pitch * 0.3, None, Phase::Started);
        editor.scroll_wheel(-pitch * 0.3, None, Phase::Moved);
        editor.scroll_wheel(-pitch * 0.3, None, Phase::Moved);
        assert!(
            (editor.pager().scroll() - 0.9).abs() < 1e-6,
            "{}",
            editor.pager().scroll()
        );
        editor.scroll_wheel(0.0, None, Phase::Ended);
        assert_eq!(editor.pager().scroll(), 1.0);
        // A later bare notch is not mistaken for part of the finished gesture.
        editor.scroll_wheel(-24.0, Some(-1.0), Phase::Moved);
        assert!((editor.pager().scroll() - 1.1).abs() < 1e-9);
    }

    #[test]
    fn scrollbar_thumb_drags_the_view_and_track_clicks_page() {
        let mut editor = long_book();
        let mut frame = pixelkit_raster::WindowBuffer::new(1100, 800);
        editor.paint(&mut frame);
        let bar = editor.scrollbar().unwrap();
        assert_eq!(bar.x + bar.w, 1100, "the track hugs the right edge");
        assert_eq!(bar.y, editor.toolbar_height());
        let pages = editor.pager().pages();
        // Drag the thumb to the bottom of the track.
        let grab_y = (bar.thumb_top + bar.thumb_h / 2) as f32;
        editor.hover((bar.x + bar.w / 2) as f32, grab_y);
        assert_eq!(editor.pointer_shape(), crate::PointerShape::Arrow);
        editor.pointer(true);
        assert!(editor.scrollbar_dragging());
        editor.hover((bar.x + bar.w / 2) as f32, (bar.y + bar.h) as f32 + 50.0);
        assert_eq!(editor.pager().page_top() + 1, pages);
        editor.pointer(false);
        assert!(!editor.scrollbar_dragging());
        // Halfway up lands near the middle, and the caret stays put.
        let selection = editor.book().selection();
        editor.hover((bar.x + bar.w / 2) as f32, grab_y);
        editor.paint(&mut frame);
        let bar = editor.scrollbar().unwrap();
        editor.hover((bar.x + 1) as f32, (bar.thumb_top + 1) as f32);
        editor.pointer(true);
        editor.hover((bar.x + 1) as f32, (bar.y + bar.h / 2) as f32);
        editor.pointer(false);
        let middle = editor.pager().scroll();
        assert!(
            middle > f64::from(pages) * 0.3 && middle < f64::from(pages) * 0.7,
            "{middle}"
        );
        assert_eq!(editor.book().selection(), selection);
        // Clicking the track above the thumb pages up; below pages down.
        editor.paint(&mut frame);
        let bar = editor.scrollbar().unwrap();
        let before = editor.pager().scroll();
        // Page up from inside a page lands on its top; from a top, one back.
        let up = if before.fract() > 1e-9 {
            before.floor()
        } else {
            before - 1.0
        };
        editor.hover((bar.x + 1) as f32, (bar.thumb_top - 2) as f32);
        editor.pointer(true);
        editor.pointer(false);
        assert_eq!(editor.pager().scroll(), up);
        editor.hover((bar.x + 1) as f32, (bar.thumb_top + bar.thumb_h + 2) as f32);
        editor.pointer(true);
        editor.pointer(false);
        assert_eq!(editor.pager().scroll(), up + 1.0);
        // Pages never sit under the track.
        editor.paint(&mut frame);
        assert!(
            editor
                .slots
                .iter()
                .all(|slot| slot.left + slot.width <= bar.x)
        );
        // One page: no scrollbar.
        let mut short = Editor::new_document("Short");
        short.paint(&mut frame);
        assert!(short.scrollbar().is_none());
    }

    #[test]
    fn thumb_tracks_scroll_and_maps_back() {
        let bar = ScrollbarLayout::new(990, 24, 10, 776, 0.0, 199.0, 1.0, 1.0);
        assert_eq!(bar.thumb_top, 24);
        assert_eq!(bar.thumb_h, 24, "minimum thumb on a 200-page book");
        let end = ScrollbarLayout::new(990, 24, 10, 776, 199.0, 199.0, 1.0, 1.0);
        assert_eq!(end.thumb_top + end.thumb_h, 24 + 776);
        let mid = bar.scroll_for_thumb_top(24.0 + (776.0 - 24.0) / 2.0);
        assert!((mid - 99.5).abs() < 1e-9);
        assert_eq!(bar.scroll_for_thumb_top(0.0), 0.0);
        assert_eq!(bar.scroll_for_thumb_top(10_000.0), 199.0);
        let short = ScrollbarLayout::new(0, 0, 10, 100, 0.0, 1.0, 1.0, 1.0);
        assert_eq!(short.thumb_h, 50, "two pages: the thumb is half the track");
        let one = ScrollbarLayout::new(0, 0, 10, 100, 0.0, 0.0, 1.0, 1.0);
        assert_eq!(one.thumb_h, 100);
        assert_eq!(one.scroll_for_thumb_top(50.0), 0.0);
    }
}
