//! Unobtrusive page measurement for the reader.
//!
//! The tool answers "is this really A4" without getting in the way: it is
//! off until `R` toggles it, lives entirely in the status bar plus a thin
//! overlay, and exits on `Esc`. All math runs in PDF points (1/72 inch) in
//! page display space, so readings are identical at any zoom; document space
//! is only the overlay's coordinate system.

use crate::document::PageIndex;
use crate::document::layout::PagePlacement;
use crate::geometry::{PointF, RectF};

/// Points per unit. A PDF point is 1/72 inch by definition; pixels are CSS
/// pixels at 96 dpi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeasureUnit {
    Points,
    #[default]
    Millimeters,
    Centimeters,
    Inches,
    Pixels,
}

impl MeasureUnit {
    pub const ALL: [Self; 5] = [
        Self::Points,
        Self::Millimeters,
        Self::Centimeters,
        Self::Inches,
        Self::Pixels,
    ];

    pub fn abbrev(self) -> &'static str {
        match self {
            Self::Points => "pt",
            Self::Millimeters => "mm",
            Self::Centimeters => "cm",
            Self::Inches => "in",
            Self::Pixels => "px",
        }
    }

    pub fn from_points(self, points: f64) -> f64 {
        match self {
            Self::Points => points,
            Self::Millimeters => points * 25.4 / 72.0,
            Self::Centimeters => points * 2.54 / 72.0,
            Self::Inches => points / 72.0,
            Self::Pixels => points * 96.0 / 72.0,
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Points => Self::Millimeters,
            Self::Millimeters => Self::Centimeters,
            Self::Centimeters => Self::Inches,
            Self::Inches => Self::Pixels,
            Self::Pixels => Self::Points,
        }
    }

    /// One-decimal reading with the unit suffix (`61.0 mm`).
    pub fn format(self, points: f64) -> String {
        format!("{:.1} {}", self.from_points(points), self.abbrev())
    }
}

/// One measured endpoint, in both spaces the tool needs: document space for
/// painting the overlay, page display space (PDF points from the page box's
/// top-left) for every reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasuredPoint {
    pub page: PageIndex,
    pub doc: PointF,
    pub display: PointF,
}

/// Page display coordinates of a document-space point. The placement maps
/// display space translated by the view-box origin (trim mode shows a subset
/// of the page box), so the inverse is a plain offset — no matrix needed.
pub fn doc_to_display(doc: PointF, placement: &PagePlacement) -> PointF {
    PointF {
        x: doc.x - placement.bounds.x + placement.view_box.x,
        y: doc.y - placement.bounds.y + placement.view_box.y,
    }
}

/// Document-space position of page display coordinates.
pub fn display_to_doc(display: PointF, placement: &PagePlacement) -> PointF {
    PointF {
        x: display.x - placement.view_box.x + placement.bounds.x,
        y: display.y - placement.view_box.y + placement.bounds.y,
    }
}

/// Snap a display-space point onto the page-box edges and corners when it is
/// within `tolerance` points of one. Each axis snaps independently, so a
/// corner wins only when both axes are near it.
pub fn snap_to_page_edges(display: PointF, page_box: RectF, tolerance: f64) -> PointF {
    let snap_axis = |value: f64, min: f64, max: f64| {
        if (value - min).abs() <= tolerance {
            min
        } else if (value - max).abs() <= tolerance {
            max
        } else {
            value
        }
    };
    PointF {
        x: snap_axis(display.x, page_box.x, page_box.right()),
        y: snap_axis(display.y, page_box.y, page_box.bottom()),
    }
}

/// Shift-lock a cursor to the anchor's row or column, whichever the raw
/// movement favors. Applied in display space after snapping.
pub fn shift_lock(anchor: PointF, cursor: PointF) -> PointF {
    if (cursor.x - anchor.x).abs() >= (cursor.y - anchor.y).abs() {
        PointF {
            x: cursor.x,
            y: anchor.y,
        }
    } else {
        PointF {
            x: anchor.x,
            y: cursor.y,
        }
    }
}

/// A two-point measurement. Same-page endpoints read in page display space;
/// a cross-page pair falls back to document-space deltas (document units are
/// points at scale one, so the length stays honest while dx/dy lose their
/// page frame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measurement {
    pub from: MeasuredPoint,
    pub to: MeasuredPoint,
}

impl Measurement {
    pub fn cross_page(self) -> bool {
        self.from.page != self.to.page
    }

    fn delta(self) -> (f64, f64) {
        if self.cross_page() {
            (
                self.to.doc.x - self.from.doc.x,
                self.to.doc.y - self.from.doc.y,
            )
        } else {
            (
                self.to.display.x - self.from.display.x,
                self.to.display.y - self.from.display.y,
            )
        }
    }

    pub fn dx_points(self) -> f64 {
        self.delta().0
    }

    pub fn dy_points(self) -> f64 {
        self.delta().1
    }

    pub fn length_points(self) -> f64 {
        let (dx, dy) = self.delta();
        dx.hypot(dy)
    }

    /// Clockwise angle in display space (y down): 0° points east, 90° south.
    pub fn angle_degrees(self) -> f64 {
        let (dx, dy) = self.delta();
        dy.atan2(dx).to_degrees()
    }
}

/// Transient tool state. Nothing here persists: a new document or `Esc`
/// resets it, and the unit falls back to millimetres.
#[derive(Debug, Clone, Default)]
pub struct MeasureState {
    pub active: bool,
    pub unit: MeasureUnit,
    pub anchor: Option<MeasuredPoint>,
    pub cursor: Option<MeasuredPoint>,
    pub pinned: Option<Measurement>,
    pub page_report: Option<String>,
}

impl MeasureState {
    pub fn toggle(&mut self) {
        self.active = !self.active;
        self.anchor = None;
        self.cursor = None;
        self.pinned = None;
        self.page_report = None;
    }

    /// Leave the tool. Reports whether it was active, so callers only
    /// repaint when something actually changed.
    pub fn exit(&mut self) -> bool {
        if !self.active {
            return false;
        }
        self.toggle();
        true
    }

    /// The measurement to paint and report: the live anchor-cursor pair, or
    /// the pinned one once the second click lands.
    pub fn live(&self) -> Option<Measurement> {
        if let (Some(from), Some(to)) = (self.anchor, self.cursor) {
            Some(Measurement { from, to })
        } else {
            self.pinned
        }
    }

    /// Record a canvas click. The first click anchors, the second pins the
    /// measurement, and a further click starts a fresh one.
    pub fn click(&mut self, point: MeasuredPoint) {
        match (self.anchor, self.cursor) {
            (Some(from), Some(_)) => {
                self.pinned = Some(Measurement { from, to: point });
                self.anchor = None;
                self.cursor = None;
            }
            (Some(_), None) => {
                self.cursor = Some(point);
                if let (Some(from), Some(to)) = (self.anchor, self.cursor) {
                    self.pinned = Some(Measurement { from, to });
                    self.anchor = None;
                    self.cursor = None;
                }
            }
            _ => {
                self.pinned = None;
                self.anchor = Some(point);
                // Seed the cursor so the overlay and the status bar show a
                // zero-length reading immediately, even before the next move.
                self.cursor = Some(point);
            }
        }
    }

    /// One-line status bar text while the tool is active, or `None` when it
    /// is off. The page-size report (key `A`) replaces the hover line so the
    /// two never fight over the same strip.
    pub fn status_text(&self) -> Option<String> {
        if !self.active {
            return None;
        }
        if let Some(report) = &self.page_report {
            return Some(format!("Measure · {report} · Esc exit"));
        }
        if let Some(measured) = self.live() {
            let across = if measured.cross_page() {
                " · across pages"
            } else {
                ""
            };
            return Some(format!(
                "Measure · L {} (dx {}, dy {}, ∠ {:.0}°) · click to pin, U units, A page size, Esc exit{across}",
                self.unit.format(measured.length_points()),
                self.unit.format(measured.dx_points()),
                self.unit.format(measured.dy_points()),
                measured.angle_degrees(),
            ));
        }
        if let Some(cursor) = self.cursor {
            return Some(format!(
                "Measure · {} · X {} Y {} (page {}) · click to anchor, U units, A page size, Esc exit",
                self.unit.abbrev(),
                self.unit.format(cursor.display.x),
                self.unit.format(cursor.display.y),
                cursor.page.0 + 1,
            ));
        }
        Some(
            "Measure · hover for X/Y, click two points for length · U units, A page size, Esc exit"
                .to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn point(page: u32, x: f64, y: f64) -> MeasuredPoint {
        MeasuredPoint {
            page: PageIndex(page),
            doc: PointF { x, y },
            display: PointF { x, y },
        }
    }

    #[test]
    fn units_convert_from_pdf_points() {
        assert!((MeasureUnit::Millimeters.from_points(72.0) - 25.4).abs() < 1e-9);
        assert!((MeasureUnit::Inches.from_points(72.0) - 1.0).abs() < 1e-9);
        assert!((MeasureUnit::Pixels.from_points(72.0) - 96.0).abs() < 1e-9);
        assert_eq!(
            MeasureUnit::Millimeters.format(61.0 * 72.0 / 25.4),
            "61.0 mm"
        );
        assert_eq!(MeasureUnit::Points.cycle(), MeasureUnit::Millimeters);
        assert_eq!(MeasureUnit::Pixels.cycle(), MeasureUnit::Points);
    }

    #[test]
    fn a4_page_reports_210_by_297_mm() {
        // A4 is 595.276 × 841.89 points; the card from the motivating post is
        // 61 × 91 mm. Both must read back within a tenth.
        let width = MeasureUnit::Millimeters.from_points(595.276);
        let height = MeasureUnit::Millimeters.from_points(841.89);
        assert!((width - 210.0).abs() < 0.1, "{width}");
        assert!((height - 297.0).abs() < 0.1, "{height}");
    }

    #[test]
    fn measurement_reads_legs_and_angle() {
        let measured = Measurement {
            from: point(0, 10.0, 20.0),
            to: point(0, 40.0, 60.0),
        };
        assert_eq!(measured.dx_points(), 30.0);
        assert_eq!(measured.dy_points(), 40.0);
        assert_eq!(measured.length_points(), 50.0);
        assert!((measured.angle_degrees() - 53.13).abs() < 0.01);
        assert!(!measured.cross_page());
    }

    #[test]
    fn snap_hugs_edges_and_lock_holds_axes() {
        let page_box = RectF {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 200.0,
        };
        let snapped = snap_to_page_edges(PointF { x: 99.0, y: 50.0 }, page_box, 2.0);
        assert_eq!(snapped.x, 100.0);
        assert_eq!(snapped.y, 50.0);
        let locked = shift_lock(PointF { x: 10.0, y: 10.0 }, PointF { x: 40.0, y: 14.0 });
        assert_eq!(locked, PointF { x: 40.0, y: 10.0 });
    }

    #[test]
    fn clicks_anchor_then_pin_then_restart() {
        let mut state = MeasureState {
            active: true,
            ..MeasureState::default()
        };
        state.click(point(0, 0.0, 0.0));
        assert!(state.anchor.is_some());
        assert!(state.pinned.is_none());
        state.cursor = Some(point(0, 3.0, 4.0));
        state.click(point(0, 3.0, 4.0));
        let pinned = state.pinned.expect("pinned measurement");
        assert_eq!(pinned.length_points(), 5.0);
        assert!(state.anchor.is_none());
        state.click(point(0, 9.0, 9.0));
        assert!(state.anchor.is_some());
        assert!(state.pinned.is_none());
    }

    #[test]
    fn status_stays_quiet_until_toggled() {
        let idle = MeasureState::default();
        assert_eq!(idle.status_text(), None);
        let mut active = MeasureState {
            active: true,
            ..MeasureState::default()
        };
        let hover = active.status_text().expect("hover line");
        assert!(hover.contains("hover for X/Y"), "{hover}");
        active.page_report = Some("Page 1 · 210.0 × 297.0 mm".to_owned());
        let report = active.status_text().expect("report line");
        assert!(report.contains("210.0"), "{report}");
    }
}
