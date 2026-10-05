//! A [`DocumentEngine`] for Markdown documents.
//!
//! `MarkdownEngine::open` parses and lays out the whole document up front:
//! Markdown has no per-page object tree to compile lazily, so `compile_page`
//! only builds the search/selection substrate and the document-space raster
//! payload for one laid page. Raster workers re-shape each visible line at
//! device scale through the same cosmic-text path as the chrome renderer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cosmic_text::{Attrs, Buffer, Color, Family, Metrics, Shaping, Style, SwashCache, Weight};

use crate::document::engine::{
    CancellationFlag, CompiledArtifact, CompiledArtifacts, DocumentCompileWorker,
    DocumentDescriptor, DocumentEngine, DocumentEngineError, DocumentRasterWorker, PageGeometry,
    RasterPass, SemanticArtifact, TextArtifact,
};
use crate::document::features::{
    DocumentLink, LinkTarget, OutlineNode, OutlineSource, PageStructure,
};
use crate::document::tile::{TileDemand, TileKey, TileSurface, TileTier, ZoomBucket};
use crate::document::{DocumentId, PageIndex};
use crate::geometry::{Affine, PointF, RectF};
use crate::paint::PixelSurface;
use crate::text::{
    CharacterGeometry, TextSubstrate, cluster_lines, transform_characters_to_document,
};

use super::layout::{MdInk, MdLine, MdLink, MdPage, MdSpan, layout_document};
use super::model::parse_markdown;
use super::theme::{MdPalette, PAGE_HEIGHT, PAGE_WIDTH, palette_for};

/// First Markdown document id. PDF ids count up from 100 and synthetic and
/// empty engines use small constants; starting here keeps every engine's ids
/// disjoint for the life of the process.
static NEXT_MARKDOWN_ID: AtomicU64 = AtomicU64::new(1_000_000_000);

/// One laid page in document space: the raster payload `compile_page` builds.
#[derive(Debug, Clone)]
pub struct MarkdownPage {
    /// Document-space lines; spans keep layout byte offsets into `text`.
    pub lines: Vec<MarkdownLine>,
    /// Document-space background fills.
    pub fills: Vec<(RectF, MdInk)>,
    operation_count: usize,
}

/// One laid line in document space.
#[derive(Debug, Clone)]
pub struct MarkdownLine {
    pub x: f64,
    pub baseline: f64,
    pub top: f64,
    pub height: f64,
    pub width: f64,
    pub size: f32,
    pub text: String,
    pub spans: Vec<MdSpan>,
}

impl MarkdownPage {
    /// Count lines, spans, and fills for cache accounting.
    pub fn operation_count(&self) -> usize {
        self.operation_count
    }

    /// Rough retained bytes for the memory arbiter.
    pub fn estimated_bytes(&self) -> u64 {
        self.lines
            .iter()
            .map(|line| line.text.len() as u64 + line.spans.len() as u64 * 96 + 128)
            .sum::<u64>()
            + self.fills.len() as u64 * 64
    }
}

/// A Markdown document opened for reading.
#[derive(Debug, Clone)]
pub struct MarkdownEngine {
    descriptor: DocumentDescriptor,
    pages: Arc<Vec<MdPage>>,
    source: PathBuf,
}

impl MarkdownEngine {
    /// Parse and lay out a Markdown file.
    pub fn open(path: &Path) -> Result<Self, DocumentEngineError> {
        let bytes = std::fs::read(path).map_err(|error| {
            DocumentEngineError::Engine(format!("could not read {}: {error}", path.display()))
        })?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
        let blocks = parse_markdown(text);
        let pages = layout_document(&mut super::layout::MdFonts::new(), &blocks);
        let display_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Markdown document")
            .to_owned();
        Ok(Self::from_pages(display_name, path.to_path_buf(), pages))
    }

    /// Parse and lay out Markdown source without a file, for tests and tools.
    pub fn open_source(source_name: &str, text: &str) -> Self {
        let blocks = parse_markdown(text);
        let pages = layout_document(&mut super::layout::MdFonts::new(), &blocks);
        Self::from_pages(source_name.to_owned(), PathBuf::from(source_name), pages)
    }

    fn from_pages(display_name: String, source: PathBuf, pages: Vec<MdPage>) -> Self {
        let geometries: Vec<PageGeometry> = pages
            .iter()
            .map(|_| PageGeometry {
                crop: RectF {
                    x: 0.0,
                    y: 0.0,
                    width: PAGE_WIDTH,
                    height: PAGE_HEIGHT,
                },
                rotation: 0,
            })
            .collect();
        let mut anchors = HashMap::new();
        let mut outline = Vec::new();
        for (index, page) in pages.iter().enumerate() {
            let page_index = PageIndex(index as u32);
            for heading in &page.headings {
                anchors
                    .entry(heading.slug.clone())
                    .or_insert((page_index, heading.rect));
                outline.push(OutlineNode {
                    title: Arc::from(heading.title.as_str()),
                    page: page_index,
                    target_region: Some(heading.rect),
                    depth: u16::from(heading.level.saturating_sub(1)),
                    source: OutlineSource::Synthesized,
                });
            }
        }
        let mut page_links: Vec<Arc<[DocumentLink]>> = Vec::new();
        for page in &pages {
            let mut links = Vec::new();
            for (rect, target) in &page.links {
                let target = match target {
                    MdLink::External(url) => Some(LinkTarget::External(Arc::from(url.as_str()))),
                    MdLink::Anchor(slug) => {
                        resolve_anchor(&anchors, slug).map(|(page, region)| LinkTarget::Internal {
                            page,
                            target_region: Some(region),
                        })
                    }
                };
                if let Some(target) = target {
                    links.push(DocumentLink {
                        source_region: *rect,
                        target,
                    });
                }
            }
            page_links.push(links.into());
        }
        Self {
            descriptor: DocumentDescriptor {
                id: DocumentId(NEXT_MARKDOWN_ID.fetch_add(1, Ordering::Relaxed)),
                display_name,
                page_count: pages.len() as u32,
                page_geometries: geometries.into(),
                outline: outline.into(),
                page_links: page_links.into(),
            },
            pages: Arc::new(pages),
            source,
        }
    }
}

/// Resolve a `#anchor` against heading slugs, trying the raw anchor first and
/// then its GitHub slug form (authors write both `#Hello World` and
/// `#hello-world`).
fn resolve_anchor(
    anchors: &HashMap<String, (PageIndex, RectF)>,
    slug: &str,
) -> Option<(PageIndex, RectF)> {
    anchors
        .get(slug)
        .or_else(|| anchors.get(&slug.to_lowercase()))
        .or_else(|| anchors.get(&super::model::slug(slug)))
        .copied()
}

#[derive(Debug)]
struct MarkdownCompileWorker {
    engine: MarkdownEngine,
}

impl DocumentCompileWorker for MarkdownCompileWorker {
    fn compile_page(
        &mut self,
        page: PageIndex,
        page_to_doc: Affine,
        cancellation: &CancellationFlag,
    ) -> Result<Arc<CompiledArtifacts>, DocumentEngineError> {
        compile_markdown_page(&self.engine, page, page_to_doc, cancellation)
    }
}

struct MarkdownRasterWorker {
    engine: MarkdownEngine,
    fonts: RasterFonts,
}

impl std::fmt::Debug for MarkdownRasterWorker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MarkdownRasterWorker")
            .field("engine", &self.engine)
            .finish_non_exhaustive()
    }
}

/// Per-worker shaping context. One font-system scan per raster thread, mirroring
/// the chrome renderer's documented cost.
struct RasterFonts {
    font_system: cosmic_text::FontSystem,
    swash: SwashCache,
    regular: Option<String>,
    bold: Option<String>,
    mono: Option<String>,
}

impl RasterFonts {
    fn new() -> Self {
        let mut font_system = cosmic_text::FontSystem::new();
        let regular = super::layout::register_embedded_family(
            &mut font_system,
            pdf_font::StandardFont::Helvetica,
        );
        let bold = super::layout::register_embedded_family(
            &mut font_system,
            pdf_font::StandardFont::HelveticaBold,
        );
        let mono = super::layout::register_embedded_family(
            &mut font_system,
            pdf_font::StandardFont::Courier,
        );
        Self {
            font_system,
            swash: SwashCache::new(),
            regular,
            bold,
            mono,
        }
    }
}

impl DocumentRasterWorker for MarkdownRasterWorker {
    fn raster_tile(
        &mut self,
        artifacts: &CompiledArtifacts,
        bucket: ZoomBucket,
        demand: TileDemand,
        pass: RasterPass,
        generation: u64,
        cancellation: &CancellationFlag,
    ) -> Result<TileSurface, DocumentEngineError> {
        raster_markdown_tile(
            &self.engine,
            &mut self.fonts,
            artifacts,
            bucket,
            demand,
            pass,
            generation,
            cancellation,
        )
    }
}

impl DocumentEngine for MarkdownEngine {
    fn descriptor(&self) -> &DocumentDescriptor {
        &self.descriptor
    }

    fn create_compile_worker(&self) -> Box<dyn DocumentCompileWorker> {
        Box::new(MarkdownCompileWorker {
            engine: self.clone(),
        })
    }

    fn create_raster_worker(&self) -> Box<dyn DocumentRasterWorker> {
        Box::new(MarkdownRasterWorker {
            engine: self.clone(),
            fonts: RasterFonts::new(),
        })
    }

    fn source_path(&self) -> Option<&Path> {
        Some(&self.source)
    }
}

fn compile_markdown_page(
    engine: &MarkdownEngine,
    page: PageIndex,
    page_to_doc: Affine,
    cancellation: &CancellationFlag,
) -> Result<Arc<CompiledArtifacts>, DocumentEngineError> {
    if cancellation.is_cancelled() {
        return Err(DocumentEngineError::Cancelled);
    }
    let geometry = *engine
        .descriptor
        .page_geometries
        .get(page.0 as usize)
        .ok_or(DocumentEngineError::PageOutOfRange(page))?;
    let laid = engine
        .pages
        .get(page.0 as usize)
        .ok_or(DocumentEngineError::PageOutOfRange(page))?;

    let mut characters = Vec::new();
    let mut utf16 = Vec::new();
    for (line_index, line) in laid.lines.iter().enumerate() {
        push_line_characters(line, line_index as u32, &mut characters, &mut utf16);
        utf16.push('\n' as u16);
    }
    let lines = Arc::new(cluster_lines(page, &characters, page_to_doc));
    transform_characters_to_document(&mut characters, page_to_doc);
    let substrate = Arc::new(TextSubstrate {
        utf16: utf16.into(),
        characters: characters.into(),
        lines,
    });

    let mut raster_lines = Vec::with_capacity(laid.lines.len());
    let mut operation_count = 0usize;
    for line in &laid.lines {
        // Document space is y-down by viewer contract: the placement flips
        // the y-up top edge above the baseline, so no reordering is needed.
        let origin = page_to_doc.apply(PointF {
            x: line.x,
            y: line.baseline,
        });
        let top = page_to_doc.apply(PointF {
            x: line.x,
            y: line.top,
        });
        operation_count += 1 + line.spans.len();
        raster_lines.push(MarkdownLine {
            x: origin.x,
            baseline: origin.y,
            top: top.y,
            height: line.height,
            width: line.width,
            size: line.size,
            text: line.text.clone(),
            spans: line.spans.clone(),
        });
    }
    let mut fills = Vec::with_capacity(laid.fills.len());
    for (rect, ink) in &laid.fills {
        fills.push((page_to_doc.apply_rect(*rect), *ink));
        operation_count += 1;
    }
    let markdown = Arc::new(MarkdownPage {
        lines: raster_lines,
        fills,
        operation_count,
    });
    Ok(Arc::new(CompiledArtifacts {
        page,
        geometry,
        semantic: SemanticArtifact::Markdown(Arc::clone(&markdown)),
        compiled: CompiledArtifact::Markdown(markdown),
        text: TextArtifact::Synthetic(substrate),
        structure: PageStructure::page_box(geometry.display_box()),
        lowering_degraded: false,
    }))
}

/// Build one laid line's substrate characters from its shaped glyphs. Chars
/// without glyphs (spaces) advance a pen by the line's mean advance so
/// selection stays aligned with the rendered text.
fn push_line_characters(
    line: &MdLine,
    object_id: u32,
    characters: &mut Vec<CharacterGeometry>,
    utf16: &mut Vec<u16>,
) {
    let char_count = line.text.chars().count().max(1) as f64;
    let fallback_advance = (line.width / char_count).max(1.0);
    let mut pen = line.x;
    for (byte, ch) in line.text.char_indices() {
        let covering = line
            .glyphs
            .iter()
            .find(|glyph| glyph.start <= byte && byte < glyph.end)
            .map(|glyph| (line.x + f64::from(glyph.x), f64::from(glyph.w).max(0.5)));
        let (x, width) = covering.unwrap_or_else(|| {
            let x = pen;
            pen += fallback_advance;
            (x, fallback_advance)
        });
        if covering.is_some() {
            pen = (x + width).max(pen);
        }
        let bold = line
            .spans
            .iter()
            .any(|span| span.bold && span.start <= byte && byte < span.end);
        let index = utf16.len();
        let mut encoded = [0; 2];
        let utf16_len = ch.encode_utf16(&mut encoded).len();
        utf16.extend_from_slice(&encoded[..utf16_len]);
        characters.push(CharacterGeometry {
            unicode: Some(ch),
            origin: PointF {
                x,
                y: line.baseline,
            },
            bounds: RectF {
                x,
                y: line.top - line.height,
                width,
                height: line.height,
            },
            nominal_height: line.height,
            font_size: f64::from(line.size),
            bold,
            object_id,
            char_index: index,
            utf16_len,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn raster_markdown_tile(
    engine: &MarkdownEngine,
    fonts: &mut RasterFonts,
    artifacts: &CompiledArtifacts,
    bucket: ZoomBucket,
    demand: TileDemand,
    pass: RasterPass,
    generation: u64,
    cancellation: &CancellationFlag,
) -> Result<TileSurface, DocumentEngineError> {
    if cancellation.is_cancelled() {
        return Err(DocumentEngineError::Cancelled);
    }
    let CompiledArtifact::Markdown(page) = &artifacts.compiled else {
        return Err(DocumentEngineError::Engine(
            "markdown raster worker received a non-markdown page".to_owned(),
        ));
    };
    let palette = palette_for(demand.color_mode);
    let width = demand.page_device_rect.width;
    let height = demand.page_device_rect.height;
    let stride = width as usize;
    let mut pixels = vec![MdInk::Paper.resolve(&palette); stride.saturating_mul(height as usize)];
    let scale = bucket.scale();
    let rect = demand.page_document_rect;

    for (fill, ink) in &page.fills {
        let x0 = ((fill.x - rect.x) * scale).floor() as i32;
        let y0 = ((fill.y - rect.y) * scale).floor() as i32;
        let x1 = ((fill.x + fill.width - rect.x) * scale).ceil() as i32;
        let y1 = ((fill.y + fill.height - rect.y) * scale).ceil() as i32;
        fill_rect_i32(
            &mut pixels,
            stride,
            width,
            height,
            x0,
            y0,
            x1,
            y1,
            ink.resolve(&palette),
        );
    }

    for line in &page.lines {
        let line_top = (line.top - rect.y) * scale;
        let line_bottom = (line.top + line.height - rect.y) * scale;
        if line_bottom < 0.0 || line_top > f64::from(height) {
            continue;
        }
        if cancellation.is_cancelled() {
            return Err(DocumentEngineError::Cancelled);
        }
        draw_markdown_line(
            fonts,
            &mut pixels,
            stride,
            width,
            height,
            &palette,
            line,
            &rect,
            scale,
        );
    }

    let tier = match pass {
        RasterPass::Thumbnail => TileTier::Thumbnail,
        RasterPass::Draft => TileTier::Draft,
        RasterPass::TextFirst => TileTier::TextFirst,
        RasterPass::Final => TileTier::Final,
    };
    Ok(TileSurface {
        key: TileKey {
            document: engine.descriptor.id,
            page: demand.page,
            bucket,
            coord: demand.coord,
            tier,
            variant: demand.variant,
        },
        generation,
        page_device_rect: demand.page_device_rect,
        page_document_rect: demand.page_document_rect,
        pixels: PixelSurface {
            width,
            height,
            stride,
            pixels: pixels.into(),
        },
        degraded: false,
    })
}

#[allow(clippy::too_many_arguments)]
fn draw_markdown_line(
    fonts: &mut RasterFonts,
    pixels: &mut [u32],
    stride: usize,
    width: u32,
    height: u32,
    palette: &MdPalette,
    line: &MarkdownLine,
    tile_rect: &RectF,
    scale: f64,
) {
    if line.text.is_empty() {
        return;
    }
    let size_px = (f64::from(line.size) * scale).max(1.0) as f32;
    let metrics = Metrics::new(size_px, size_px * super::theme::LINE_HEIGHT_FACTOR);
    let mut buffer = Buffer::new(&mut fonts.font_system, metrics);
    let spans: Vec<(&str, Attrs<'_>)> = line
        .spans
        .iter()
        .filter_map(|span| {
            let text = line.text.get(span.start..span.end)?;
            if text.is_empty() {
                return None;
            }
            let family = if span.mono {
                fonts.mono.as_deref().or(fonts.regular.as_deref())
            } else if span.bold {
                fonts.bold.as_deref().or(fonts.regular.as_deref())
            } else {
                fonts.regular.as_deref()
            };
            let attrs = Attrs::new()
                .family(match family {
                    Some(name) => Family::Name(name),
                    None if span.mono => Family::Monospace,
                    None => Family::SansSerif,
                })
                .weight(if span.bold {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                })
                .style(if span.italic {
                    Style::Italic
                } else {
                    Style::Normal
                })
                .color(span_color(span.color.resolve(palette)));
            Some((text, attrs))
        })
        .collect();
    if spans.is_empty() {
        return;
    }
    // Unbounded width: the line was already wrapped at layout, so shaping
    // must never re-wrap it here.
    let runs = {
        let mut borrowed = buffer.borrow_with(&mut fonts.font_system);
        borrowed.set_size(None, None);
        borrowed.set_rich_text(spans, &Attrs::new(), Shaping::Advanced, None);
        borrowed.shape_until_scroll(true);
        borrowed
            .layout_runs()
            .map(|run| {
                (
                    run.line_top,
                    run.line_y,
                    run.line_height,
                    run.glyphs
                        .iter()
                        .map(|glyph| (glyph.start, glyph.end, glyph.x, glyph.w))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    let base_x = ((line.x - tile_rect.x) * scale) as f32;
    let base_y = ((line.top - tile_rect.y) * scale) as f32;
    let line_height_px = (line.height * scale) as f32;
    for (run_top, _run_baseline, _run_height, glyphs) in &runs {
        let origin_x = base_x.round() as i32;
        let origin_y = (base_y + run_top).round() as i32;
        // Inline backgrounds first, then strike/underline resolved against
        // the device-scale glyphs, then the glyphs themselves.
        for span in &line.spans {
            let Some((x0, x1)) = span_device_range(glyphs, span) else {
                continue;
            };
            if let Some(bg) = span.bg {
                fill_rect_i32(
                    pixels,
                    stride,
                    width,
                    height,
                    origin_x + x0.round() as i32,
                    origin_y,
                    origin_x + x1.round() as i32,
                    origin_y + line_height_px.round() as i32,
                    bg.resolve(palette),
                );
            }
            let color = span.color.resolve(palette);
            let thickness = scale.max(1.0).round() as i32;
            if span.strike {
                let y = origin_y + (line_height_px * 0.55).round() as i32;
                fill_rect_i32(
                    pixels,
                    stride,
                    width,
                    height,
                    origin_x + x0.round() as i32,
                    y,
                    origin_x + x1.round() as i32,
                    y + thickness,
                    color,
                );
            }
            if span.underline {
                let y = origin_y + (line_height_px * 0.85).round() as i32;
                fill_rect_i32(
                    pixels,
                    stride,
                    width,
                    height,
                    origin_x + x0.round() as i32,
                    y,
                    origin_x + x1.round() as i32,
                    y + thickness,
                    color,
                );
            }
        }
    }
    buffer.draw(
        &mut fonts.font_system,
        &mut fonts.swash,
        Color::rgb(0, 0, 0),
        |x, y, glyph_width, glyph_height, glyph_color| {
            // Per-span colors were set in `Attrs`; the draw color only tints
            // spans that kept the default. Blend every pixel the same way.
            let argb = glyph_color.0;
            for row in 0..glyph_height as i32 {
                let target_y = base_y.round() as i32 + y + row;
                if target_y < 0 || target_y >= height as i32 {
                    continue;
                }
                for column in 0..glyph_width as i32 {
                    let target_x = base_x.round() as i32 + x + column;
                    if target_x < 0 || target_x >= width as i32 {
                        continue;
                    }
                    let index = target_y as usize * stride + target_x as usize;
                    pixels[index] = blend_xrgb(pixels[index], argb);
                }
            }
        },
    );
}

/// Device-space x-range of a span from device-scale glyphs.
fn span_device_range(glyphs: &[(usize, usize, f32, f32)], span: &MdSpan) -> Option<(f32, f32)> {
    let mut x0 = f32::MAX;
    let mut x1 = f32::MIN;
    for (start, end, x, w) in glyphs {
        if *end > span.start && *start < span.end {
            x0 = x0.min(*x);
            x1 = x1.max(*x + *w);
        }
    }
    (x0 <= x1).then_some((x0, x1))
}

fn span_color(xrgb: u32) -> Color {
    Color::rgb(
        ((xrgb >> 16) & 0xff) as u8,
        ((xrgb >> 8) & 0xff) as u8,
        (xrgb & 0xff) as u8,
    )
}

#[allow(clippy::too_many_arguments)]
fn fill_rect_i32(
    pixels: &mut [u32],
    stride: usize,
    width: u32,
    height: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
) {
    let left = x0.max(0).min(width as i32) as usize;
    let top = y0.max(0).min(height as i32) as usize;
    let right = x1.max(0).min(width as i32) as usize;
    let bottom = y1.max(0).min(height as i32) as usize;
    for row in top..bottom {
        pixels[row * stride + left..row * stride + right].fill(color);
    }
}

fn blend_xrgb(destination: u32, source_argb: u32) -> u32 {
    let alpha = (source_argb >> 24) & 0xff;
    let inverse = 255 - alpha;
    let channel = |shift: u32| {
        let source = (source_argb >> shift) & 0xff;
        let destination = (destination >> shift) & 0xff;
        (source * alpha + destination * inverse + 127) / 255
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::geometry::RectI;
    use crate::text::SearchIndex;

    const SAMPLE: &str = "# Shopping\n\nBuy *milk* and `eggs`.\n\n- [ ] milk\n- [x] eggs\n\n[store](https://example.com/)\n";

    #[test]
    fn open_source_builds_pages_outline_and_links() {
        let engine = MarkdownEngine::open_source("sample.md", SAMPLE);
        assert!(engine.descriptor().page_count >= 1);
        assert_eq!(engine.descriptor().outline.len(), 1);
        assert_eq!(engine.descriptor().outline[0].title.as_ref(), "Shopping");
        let links: usize = engine
            .descriptor()
            .page_links
            .iter()
            .map(|page| page.len())
            .sum();
        assert_eq!(links, 1);
    }

    #[test]
    fn anchors_resolve_to_heading_pages() {
        let engine = MarkdownEngine::open_source(
            "anchors.md",
            "# Top\n\nSee [below](#details).\n\n## Details\n",
        );
        let internal = engine
            .descriptor()
            .page_links
            .iter()
            .flat_map(|page| page.iter())
            .find(|link| matches!(&link.target, LinkTarget::Internal { .. }));
        assert!(
            internal.is_some(),
            "the #details link must resolve internally"
        );
    }

    /// Compile through a production placement: document space is y-down by
    /// viewer contract, and only a flipping transform exercises the real path.
    fn compile_first_page(
        engine: &MarkdownEngine,
    ) -> (Arc<CompiledArtifacts>, crate::document::PageLayoutIndex) {
        let layout = crate::document::PageLayoutIndex::build(
            &engine.descriptor().page_geometries,
            &crate::theme::Theme::default().metrics,
        );
        let placement = layout
            .placement(PageIndex(0))
            .expect("first page placement")
            .clone();
        let artifacts = engine
            .compile_page(
                PageIndex(0),
                placement.page_to_doc,
                &CancellationFlag::default(),
            )
            .expect("compile markdown page");
        (artifacts, layout)
    }

    #[test]
    fn compiled_substrate_is_searchable() {
        let engine = MarkdownEngine::open_source("sample.md", SAMPLE);
        let (artifacts, _) = compile_first_page(&engine);
        assert!(!artifacts.text.substrate().characters.is_empty());
        assert!(artifacts.compiled.operation_count() > 0);
        let mut index = SearchIndex::default();
        index.insert(PageIndex(0), Arc::clone(artifacts.text.substrate()));
        let hits = index.search_exact("milk", 8);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn raster_tile_paints_text_over_paper() {
        let engine = MarkdownEngine::open_source("sample.md", SAMPLE);
        let (artifacts, layout) = compile_first_page(&engine);
        let placement = layout
            .placement(PageIndex(0))
            .expect("first page placement");
        let demand = TileDemand {
            page: PageIndex(0),
            coord: crate::document::tile::TileCoord { x: 0, y: 0 },
            page_device_rect: RectI {
                x: 0,
                y: 0,
                width: 256,
                height: 256,
            },
            page_document_rect: RectF {
                x: placement.bounds.x,
                y: placement.bounds.y,
                width: 256.0,
                height: 256.0,
            },
            distance_from_viewport: 0.0,
            visible: true,
            page_view_box: placement.view_box,
            color_mode: crate::document::ColorMode::Original,
            variant: 0,
        };
        let tile = engine
            .raster_tile(
                &artifacts,
                ZoomBucket::ONE,
                demand,
                RasterPass::Final,
                1,
                &CancellationFlag::default(),
            )
            .expect("raster markdown tile");
        let paper = MdInk::Paper.resolve(&palette_for(crate::document::ColorMode::Original));
        assert!(
            tile.pixels.pixels.iter().any(|pixel| *pixel != paper),
            "the tile must contain ink"
        );
    }

    #[test]
    fn wrapped_text_stays_inside_the_content_edge() {
        let engine = MarkdownEngine::open_source(
            "wrap.md",
            "Note: kitty already copied (`~/.config/kitty/` populated, matches old). No files copied by this survey.\n",
        );
        let (artifacts, layout) = compile_first_page(&engine);
        let placement = layout
            .placement(PageIndex(0))
            .expect("first page placement");
        let demand = TileDemand {
            page: PageIndex(0),
            coord: crate::document::tile::TileCoord { x: 0, y: 0 },
            page_device_rect: RectI {
                x: 0,
                y: 0,
                width: PAGE_WIDTH as u32,
                height: 256,
            },
            page_document_rect: RectF {
                x: placement.bounds.x,
                y: placement.bounds.y,
                width: PAGE_WIDTH,
                height: 256.0,
            },
            distance_from_viewport: 0.0,
            visible: true,
            page_view_box: placement.view_box,
            color_mode: crate::document::ColorMode::Original,
            variant: 0,
        };
        let tile = engine
            .raster_tile(
                &artifacts,
                ZoomBucket::ONE,
                demand,
                RasterPass::Final,
                1,
                &CancellationFlag::default(),
            )
            .expect("raster markdown tile");
        let paper = MdInk::Paper.resolve(&palette_for(crate::document::ColorMode::Original));
        // Content ends at the left margin plus the content width; past a few
        // pixels of antialiasing slack only paper may remain.
        let edge = (PAGE_WIDTH - crate::document::markdown::theme::MARGIN_X) as usize + 4;
        let mut ink = 0usize;
        for (index, pixel) in tile.pixels.pixels.iter().enumerate() {
            if index % tile.pixels.stride >= edge && *pixel != paper {
                ink += 1;
            }
        }
        assert_eq!(ink, 0, "wrapped text must not spill past the margins");
    }
}
