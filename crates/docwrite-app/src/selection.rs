//! Mouse selection over the same laid-out text used for painting.

use std::time::{Duration, Instant};

use docwrite_model::{Position, Selection};
use unicode_segmentation::UnicodeSegmentation;

use crate::Editor;

#[derive(Clone, Copy, Debug)]
enum Unit {
    Character,
    Word,
    Paragraph,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TextDrag {
    start: Selection,
    unit: Unit,
    pub citation: bool,
}

#[derive(Default)]
pub(crate) struct TextPointer {
    pub drag: Option<TextDrag>,
    last_click: Option<(Instant, (f32, f32), u8)>,
}

impl Editor {
    pub(crate) fn begin_text_selection(&mut self, extend: bool) {
        let (x, y) = self.cursor;
        let Some(position) = self.text_position_at(x, y, false) else {
            self.text_pointer.last_click = None;
            return;
        };
        self.cancel_prompt();
        self.line_end = None;
        let now = Instant::now();
        let count = self
            .text_pointer
            .last_click
            .filter(|(time, point, _)| {
                now.duration_since(*time) <= Duration::from_millis(500)
                    && (point.0 - x).abs() <= 4.0 * self.ui_scale
                    && (point.1 - y).abs() <= 4.0 * self.ui_scale
                    && !extend
            })
            .map_or(1, |(_, _, count)| count % 3 + 1);
        self.text_pointer.last_click = Some((now, (x, y), count));
        let unit = match count {
            2 => Unit::Word,
            3 => Unit::Paragraph,
            _ => Unit::Character,
        };
        let start = if extend {
            Selection {
                anchor: self.book.selection().anchor,
                focus: position,
            }
        } else {
            self.selection_unit(position, unit)
        };
        let _ = self.book.set_selection(start);
        self.follow_caret = false;
        self.text_pointer.drag = Some(TextDrag {
            start,
            unit,
            citation: !extend && count == 1,
        });
    }

    fn selection_unit(&self, position: Position, unit: Unit) -> Selection {
        let Ok(block) = self.book.block(position.block) else {
            return Selection::collapsed(position);
        };
        let text = block.text();
        let (start, end) = match unit {
            Unit::Character => return Selection::collapsed(position),
            Unit::Paragraph => (0, text.chars().count()),
            Unit::Word => {
                let mut offset = 0;
                let mut range = (position.offset, position.offset);
                for segment in text.split_word_bounds() {
                    let end = offset + segment.chars().count();
                    range = (offset, end);
                    if position.offset < end {
                        break;
                    }
                    offset = end;
                }
                range
            }
        };
        Selection {
            anchor: Position::new(position.block, start),
            focus: Position::new(position.block, end),
        }
    }

    pub(crate) fn drag_text(&mut self) {
        let Some(mut drag) = self.text_pointer.drag else {
            return;
        };
        // Clamp to the viewport before choosing a page. Cached slots also
        // contain offscreen pages; dragging below the window must not jump
        // straight into one of those pages.
        let (x, y) = self.cursor;
        let y = y.clamp(
            self.toolbar_height() as f32,
            (self.frame_height - 1).max(self.toolbar_height()) as f32,
        );
        let Some(position) = self.text_position_at(x, y, true) else {
            return;
        };
        let target = self.selection_unit(position, drag.unit);
        let ids = self.book.block_ids();
        let order = |position: Position| {
            (
                ids.iter().position(|id| *id == position.block),
                position.offset,
            )
        };
        let selection = if matches!(drag.unit, Unit::Character) {
            Selection {
                anchor: drag.start.anchor,
                focus: position,
            }
        } else if order(target.anchor) < order(drag.start.anchor) {
            Selection {
                anchor: drag.start.focus,
                focus: target.anchor,
            }
        } else {
            Selection {
                anchor: drag.start.anchor,
                focus: target.focus,
            }
        };
        if selection != self.book.selection() {
            drag.citation = false;
            self.text_pointer.last_click = None;
        }
        let _ = self.book.set_selection(selection);
        self.text_pointer.drag = Some(drag);
        // Mouse motion follows the viewport. Caret following would snap the
        // view to a page boundary on every drag event.
        self.follow_caret = false;
    }

    /// Cancel a held pointer after focus loss or a document switch.
    pub fn cancel_pointer(&mut self) {
        self.text_pointer = TextPointer::default();
        self.pressed_row = None;
        self.scroll_grab = None;
        self.sidebar.dragging = false;
    }

    /// Whether the held text selection needs scrolling at the window edge.
    pub fn selection_needs_scroll(&self) -> bool {
        let edge = 16.0 * self.ui_scale;
        self.text_pointer.drag.is_some()
            && (self.cursor.1 < self.toolbar_height() as f32 + edge
                || self.cursor.1 >= self.frame_height as f32 - edge)
    }

    /// Advance an edge drag. Painting then extends the selection into the
    /// newly visible text, even while the mouse remains stationary.
    pub fn selection_scroll_tick(&mut self) {
        if !self.selection_needs_scroll() {
            return;
        }
        // Start inside the window: some desktop backends stop delivering
        // cursor coordinates once the pointer has left the surface.
        let edge = 16.0 * self.ui_scale;
        let top = self.toolbar_height() as f32 + edge;
        let bottom = self.frame_height as f32 - edge;
        let distance = if self.cursor.1 < top {
            self.cursor.1 - top
        } else {
            self.cursor.1 - bottom + 1.0
        };
        let pixels = distance.signum() * distance.abs().clamp(8.0, 80.0) * self.ui_scale;
        self.pager
            .scroll_by(f64::from(pixels) / self.row_pitch * f64::from(self.per_row));
        self.follow_caret = false;
    }
}
