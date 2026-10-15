use std::time::{Duration, Instant};

use crate::document::{DocumentId, TileKey, TileTier, ViewportIntent};

/// Real PDF coverage in an already composed frame. Geometry placeholders and
/// stale variants never qualify as visible document content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PresentedCoverage {
    pub any: bool,
    pub covered: bool,
    pub exact: bool,
}

impl PresentedCoverage {
    pub fn from_images(
        document: DocumentId,
        intent: &ViewportIntent,
        images: impl Iterator<Item = TileKey>,
    ) -> Self {
        let images = images.collect::<std::collections::HashSet<_>>();
        let has_final = |demand: &crate::document::TileDemand| {
            images.contains(&demand.key(document, intent.bucket, TileTier::Final))
        };
        let has_preview = |demand: &crate::document::TileDemand| {
            images.iter().any(|key| {
                key.document == document
                    && key.page == demand.page
                    && key.variant == demand.variant
                    && key.tier == TileTier::Thumbnail
            })
        };
        let visible = &intent.visible_tiles;
        Self {
            any: visible
                .iter()
                .any(|demand| has_final(demand) || has_preview(demand)),
            covered: !visible.is_empty()
                && visible
                    .iter()
                    .all(|demand| has_final(demand) || has_preview(demand)),
            exact: !visible.is_empty() && visible.iter().all(has_final),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SeekTrace {
    pub generation: u64,
    pub input_received: Option<Instant>,
    pub intent_published: Option<Instant>,
    pub first_pixels_ready: Option<Instant>,
    pub first_pixels_presented: Option<Instant>,
    pub exact_viewport_presented: Option<Instant>,
}

impl SeekTrace {
    pub fn begin(&mut self, generation: u64, input_received: Instant) {
        *self = Self {
            generation,
            input_received: Some(input_received),
            ..Self::default()
        };
    }

    pub fn mark_intent_published(&mut self) {
        self.intent_published.get_or_insert_with(Instant::now);
    }

    pub fn mark_pixels_ready(&mut self) {
        self.first_pixels_ready.get_or_insert_with(Instant::now);
    }

    /// Returns `true` exactly once, when this generation first reaches an
    /// exact viewport. Callers use that edge to emit one seek report.
    pub fn mark_presented(&mut self, exact: bool) -> bool {
        let now = Instant::now();
        self.first_pixels_presented.get_or_insert(now);
        if exact && self.exact_viewport_presented.is_none() {
            self.exact_viewport_presented = Some(now);
            true
        } else {
            false
        }
    }

    pub fn input_to_first_present(&self) -> Option<Duration> {
        Some(
            self.first_pixels_presented?
                .duration_since(self.input_received?),
        )
    }

    pub fn input_to_exact_present(&self) -> Option<Duration> {
        Some(
            self.exact_viewport_presented?
                .duration_since(self.input_received?),
        )
    }

    pub fn report_line(&self, frame: &FrameMetrics) -> Option<String> {
        let input = self.input_received?;
        let intent = self.intent_published?;
        let pixels = self.first_pixels_ready?;
        let first_present = self.first_pixels_presented?;
        let exact_present = self.exact_viewport_presented?;
        Some(format!(
            "viewer-seek generation={} input_intent_us={} intent_pixels_us={} \
             pixels_present_us={} first_exact_us={} total_exact_us={} \
             compose_us={} present_us={} queues={}/{}/{}",
            self.generation,
            micros(intent.saturating_duration_since(input)),
            micros(pixels.saturating_duration_since(intent)),
            micros(first_present.saturating_duration_since(pixels)),
            micros(exact_present.saturating_duration_since(first_present)),
            micros(exact_present.saturating_duration_since(input)),
            frame.compose_time.as_micros(),
            frame.present_time.as_micros(),
            frame.compile_pending,
            frame.raster_pending,
            frame.in_flight,
        ))
    }
}

fn micros(duration: Duration) -> u128 {
    duration.as_micros()
}

#[derive(Debug, Clone)]
pub struct FrameMetrics {
    pub input_received: Option<Instant>,
    pub redraw_requested: Option<Instant>,
    pub frame_started: Option<Instant>,
    pub compose_finished: Option<Instant>,
    pub present_finished: Option<Instant>,
    pub compose_time: Duration,
    pub present_time: Duration,
    pub damaged_pixels: u64,
    pub copied_pixels: u64,
    pub page_blits: u32,
    pub allocations: u32,
    pub compile_pending: usize,
    pub raster_pending: usize,
    pub in_flight: usize,
    pub gpu_atlas_bytes: u64,
    pub gpu_atlas_uploads: u64,
    pub gpu_draw_calls: u32,
    pub gpu_vertices: u32,
}

impl Default for FrameMetrics {
    fn default() -> Self {
        Self {
            input_received: None,
            redraw_requested: None,
            frame_started: None,
            compose_finished: None,
            present_finished: None,
            compose_time: Duration::ZERO,
            present_time: Duration::ZERO,
            damaged_pixels: 0,
            copied_pixels: 0,
            page_blits: 0,
            allocations: 0,
            compile_pending: 0,
            raster_pending: 0,
            in_flight: 0,
            gpu_atlas_bytes: 0,
            gpu_atlas_uploads: 0,
            gpu_draw_calls: 0,
            gpu_vertices: 0,
        }
    }
}

impl FrameMetrics {
    pub fn begin_frame(&mut self) {
        self.frame_started = Some(Instant::now());
        self.damaged_pixels = 0;
        self.copied_pixels = 0;
        self.page_blits = 0;
        self.allocations = 0;
    }

    pub fn finish_compose(&mut self) {
        let now = Instant::now();
        self.compose_finished = Some(now);
        self.compose_time = self
            .frame_started
            .map_or(Duration::ZERO, |start| now - start);
    }

    pub fn finish_present(&mut self) {
        let now = Instant::now();
        self.present_finished = Some(now);
        self.present_time = self
            .compose_finished
            .map_or(Duration::ZERO, |compose| now - compose);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_coverage_requires_real_current_document_pixels_for_every_tile() {
        use crate::document::{PageIndex, TileCoord, TileDemand};
        use crate::geometry::{RectF, RectI};
        let document = DocumentId(42);
        let first = TileDemand {
            page: PageIndex(0),
            coord: TileCoord { x: 0, y: 0 },
            page_device_rect: RectI {
                x: 0,
                y: 0,
                width: 256,
                height: 256,
            },
            page_document_rect: RectF::ZERO,
            distance_from_viewport: 0.0,
            visible: true,
            page_view_box: RectF::ZERO,
            color_mode: crate::document::ColorMode::Original,
            variant: 7,
        };
        let second = TileDemand {
            coord: TileCoord { x: 1, y: 0 },
            ..first
        };
        let mut intent = ViewportIntent::empty();
        intent.visible_tiles = std::sync::Arc::from([first, second]);
        let final_key = |demand: TileDemand| demand.key(document, intent.bucket, TileTier::Final);
        let coverage = |keys: Vec<TileKey>| {
            PresentedCoverage::from_images(document, &intent, keys.into_iter())
        };
        let exact = [final_key(first), final_key(second)];
        assert_eq!(coverage(vec![]), PresentedCoverage::default());
        assert_eq!(
            coverage(vec![first.key(
                document,
                intent.bucket,
                TileTier::TextFirst
            )]),
            PresentedCoverage::default()
        );
        assert_eq!(
            coverage(vec![exact[0]]),
            PresentedCoverage {
                any: true,
                covered: false,
                exact: false
            }
        );
        assert_eq!(
            coverage(exact.to_vec()),
            PresentedCoverage {
                any: true,
                covered: true,
                exact: true
            }
        );
        let preview = first.key(document, intent.bucket, TileTier::Thumbnail);
        assert_eq!(
            coverage(vec![preview]),
            PresentedCoverage {
                any: true,
                covered: true,
                exact: false
            }
        );
        for stale in [
            TileKey {
                variant: 6,
                ..preview
            },
            TileKey {
                document: DocumentId(41),
                ..preview
            },
        ] {
            assert_eq!(coverage(vec![stale]), PresentedCoverage::default());
        }
    }

    #[test]
    fn seek_trace_records_each_stage_only_once() {
        let start = Instant::now();
        let mut trace = SeekTrace::default();
        trace.begin(7, start);
        trace.mark_intent_published();
        trace.mark_pixels_ready();
        assert!(!trace.mark_presented(false));
        let first = trace.first_pixels_presented;
        assert!(trace.mark_presented(true));
        assert!(!trace.mark_presented(true));
        assert_eq!(trace.generation, 7);
        assert_eq!(trace.first_pixels_presented, first);
        assert!(trace.input_to_first_present().is_some());
        assert!(trace.input_to_exact_present().is_some());
        assert!(
            trace
                .report_line(&FrameMetrics::default())
                .is_some_and(|line| line.contains("generation=7"))
        );
    }
}
