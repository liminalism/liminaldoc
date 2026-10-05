//! Markdown pagination and shaping.
//!
//! A port of the WPF viewer's rich rendering path (`BuildFlowDocument` plus
//! the `Build*` converters) onto the reader's page model. Blocks become laid
//! lines with style spans; lines flow into Letter pages; headings feed the
//! outline and `#anchor` links resolve against heading slugs.
//!
//! Layout runs in y-down coordinates with cosmic-text and flips to the
//! reader's y-up page space on output, so the rest of the viewer (tiling,
//! selection, search) sees Markdown pages exactly like PDF pages.
//!
//! Deliberate v1 limits, all noted where they bite: images render their alt
//! text (the WPF fallback path), code has no syntax highlighting, link URLs
//! are not validated here (the viewer allows only safe schemes at click
//! time), and table columns share the content width equally instead of
//! auto-sizing to content.

use std::collections::HashSet;

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, Weight};
use pdf_font::StandardFont;

use crate::geometry::RectF;

use super::model::{Block, Inline, TableAlign, plain_text, slug};
use super::theme::{
    BODY_SIZE, CELL_PAD_X, CELL_PAD_Y, CODE_BOTTOM, CODE_PAD_X, CODE_PAD_Y, CODE_SIZE, CODE_TOP,
    H1_TOP, HEADING_BOTTOM, HEADING_RULE_PADDING, HTML_SIZE, LIST_GAP, LIST_INDENT, MARGIN_X,
    MARGIN_Y, OTHER_HEADING_TOP, PAGE_HEIGHT, PAGE_WIDTH, PARAGRAPH_GAP, QUOTE_BAR, QUOTE_GAP,
    QUOTE_PAD_Y, QUOTE_PADDING, RULE_BOTTOM, RULE_TOP, TABLE_GAP, TIGHT_PARAGRAPH_GAP,
    heading_size,
};

/// Content width between the page margins.
pub const CONTENT_WIDTH: f64 = PAGE_WIDTH - MARGIN_X * 2.0;
/// Content height between the page margins.
pub const CONTENT_HEIGHT: f64 = PAGE_HEIGHT - MARGIN_Y * 2.0;

/// A semantic ink, resolved against the tile's palette at raster time so a
/// palette change never requires relayout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdInk {
    Paper,
    Ink,
    Muted,
    Link,
    CodeBg,
    InlineCodeBg,
    Rule,
    TableHeaderBg,
}

impl MdInk {
    /// Resolve to an XRGB pixel for `palette`.
    pub fn resolve(self, palette: &super::theme::MdPalette) -> u32 {
        match self {
            Self::Paper => palette.paper,
            Self::Ink => palette.ink,
            Self::Muted => palette.muted,
            Self::Link => palette.link,
            Self::CodeBg => palette.code_bg,
            Self::InlineCodeBg => palette.inline_code_bg,
            Self::Rule => palette.rule,
            Self::TableHeaderBg => palette.table_header_bg,
        }
    }
}

/// A link target: an external URL or a same-document `#anchor` slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdLink {
    External(String),
    Anchor(String),
}

impl MdLink {
    fn from_url(url: &str) -> Self {
        url.strip_prefix('#')
            .filter(|anchor| !anchor.is_empty())
            .map_or_else(
                || Self::External(url.to_owned()),
                |anchor| Self::Anchor(anchor.to_owned()),
            )
    }
}

/// One styled span of a laid line, as byte offsets into the line text.
#[derive(Debug, Clone)]
pub struct MdSpan {
    pub start: usize,
    pub end: usize,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub underline: bool,
    pub color: MdInk,
    pub bg: Option<MdInk>,
    pub mono: bool,
    pub link: Option<MdLink>,
}

/// One shaped glyph's horizontal extent, as byte offsets into the line text.
#[derive(Debug, Clone)]
pub struct MdGlyph {
    pub start: usize,
    pub end: usize,
    pub x: f32,
    pub w: f32,
}

/// One laid line in page space (y-up).
#[derive(Debug, Clone)]
pub struct MdLine {
    /// Page-space origin (left edge, y-up).
    pub x: f64,
    /// Page-space baseline (y-up).
    pub baseline: f64,
    /// Page-space top edge (y-up); the line spans `top - height..top`.
    pub top: f64,
    pub height: f64,
    pub width: f64,
    pub size: f32,
    pub text: String,
    pub spans: Vec<MdSpan>,
    pub glyphs: Vec<MdGlyph>,
}

/// A heading for the outline, in page space.
#[derive(Debug, Clone)]
pub struct MdHeading {
    pub title: String,
    pub level: u8,
    pub slug: String,
    pub rect: RectF,
}

/// One laid page in page space (y-up).
#[derive(Debug, Clone, Default)]
pub struct MdPage {
    pub lines: Vec<MdLine>,
    /// Background fills: code blocks, table headers, rules, quote bars, grid.
    pub fills: Vec<(RectF, MdInk)>,
    /// Clickable link regions in page space.
    pub links: Vec<(RectF, MdLink)>,
    pub headings: Vec<MdHeading>,
}

/// Shaping context: the font database plus the embedded families.
pub struct MdFonts {
    font_system: FontSystem,
    regular: Option<String>,
    bold: Option<String>,
    mono: Option<String>,
}

impl std::fmt::Debug for MdFonts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MdFonts")
            .field("regular", &self.regular)
            .field("bold", &self.bold)
            .field("mono", &self.mono)
            .finish_non_exhaustive()
    }
}

impl MdFonts {
    /// Build a shaping context. The embedded base-14 programs are the primary
    /// faces (mirroring the chrome text renderer, including its discovered
    /// family names); system fonts supply every script they cannot cover.
    /// Roughly 60 ms once per owner from the system scan.
    pub fn new() -> Self {
        let mut font_system = FontSystem::new();
        let regular = register_embedded_family(&mut font_system, StandardFont::Helvetica);
        let bold = register_embedded_family(&mut font_system, StandardFont::HelveticaBold);
        let mono = register_embedded_family(&mut font_system, StandardFont::Courier);
        Self {
            font_system,
            regular,
            bold,
            mono,
        }
    }
}

impl Default for MdFonts {
    fn default() -> Self {
        Self::new()
    }
}

/// Register one embedded program and report the family name it landed under.
/// Same discovered-name rule as the chrome renderer: matching on the declared
/// family keeps a future program swap from silently changing the typeface.
pub(crate) fn register_embedded_family(
    font_system: &mut FontSystem,
    font: StandardFont,
) -> Option<String> {
    let before: HashSet<_> = font_system.db().faces().map(|face| face.id).collect();
    font_system
        .db_mut()
        .load_font_data(font.program_data().to_vec());
    font_system
        .db()
        .faces()
        .find(|face| !before.contains(&face.id))
        .and_then(|face| face.families.first().map(|(name, _)| name.clone()))
}

/// Lay out a whole document into pages.
pub fn layout_document(fonts: &mut MdFonts, blocks: &[Block]) -> Vec<MdPage> {
    let mut frags = Vec::new();
    for block in blocks {
        let mut frag = Frag::default();
        Layer {
            fonts: &mut *fonts,
            frag: &mut frag,
            indent: MARGIN_X,
            width: CONTENT_WIDTH,
            muted: false,
            gap: PARAGRAPH_GAP,
        }
        .layout_block(block, true);
        frags.push(frag);
    }
    paginate(frags)
}

/// One flattened style run: text plus its full style.
#[derive(Debug, Clone)]
struct StyledRun {
    text: String,
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    color: MdInk,
    bg: Option<MdInk>,
    mono: bool,
    link: Option<MdLink>,
}

/// Base style inherited from the enclosing block.
#[derive(Debug, Clone, Copy)]
struct StyleBase {
    bold: bool,
    color: MdInk,
    mono: bool,
}

/// The inherited style while flattening nested inlines.
#[derive(Debug, Clone)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    color: MdInk,
    bg: Option<MdInk>,
    mono: bool,
    link: Option<MdLink>,
}

impl InlineStyle {
    fn run(&self, text: String) -> StyledRun {
        StyledRun {
            text,
            bold: self.bold,
            italic: self.italic,
            strike: self.strike,
            underline: self.underline,
            color: self.color,
            bg: self.bg,
            mono: self.mono,
            link: self.link.clone(),
        }
    }
}

/// Flatten nested inlines into style runs, merging adjacent runs that share a
/// style so shaping sees clean span boundaries.
fn flatten_inlines(inlines: &[Inline], base: StyleBase) -> Vec<StyledRun> {
    fn walk(inlines: &[Inline], style: &InlineStyle, runs: &mut Vec<StyledRun>) {
        for inline in inlines {
            match inline {
                Inline::Text(text) => push_run(runs, style.run(text.clone())),
                Inline::Break => {
                    let mut broken = style.clone();
                    broken.link = None;
                    push_run(runs, broken.run("\n".to_owned()));
                }
                Inline::Code(text) => {
                    let mut coded = style.clone();
                    coded.underline = false;
                    coded.bg = Some(MdInk::InlineCodeBg);
                    coded.mono = true;
                    coded.link = None;
                    push_run(runs, coded.run(text.clone()));
                }
                Inline::Emphasis(children) => {
                    let mut emphasized = style.clone();
                    emphasized.italic = true;
                    walk(children, &emphasized, runs);
                }
                Inline::Strong(children) => {
                    let mut strong = style.clone();
                    strong.bold = true;
                    walk(children, &strong, runs);
                }
                Inline::Strike(children) => {
                    let mut struck = style.clone();
                    struck.strike = true;
                    walk(children, &struck, runs);
                }
                Inline::Link { url, children, .. } => {
                    let mut linked = style.clone();
                    linked.underline = true;
                    linked.color = MdInk::Link;
                    linked.link = Some(MdLink::from_url(url));
                    walk(children, &linked, runs);
                }
                Inline::Image { alt, .. } => {
                    let captioned = InlineStyle {
                        bold: false,
                        italic: true,
                        strike: false,
                        underline: false,
                        color: MdInk::Muted,
                        bg: None,
                        mono: false,
                        link: None,
                    };
                    push_run(
                        runs,
                        captioned.run(if alt.is_empty() {
                            "[image]".to_owned()
                        } else {
                            alt.clone()
                        }),
                    );
                }
                Inline::Task(checked) => {
                    let mut tasked = style.clone();
                    tasked.mono = true;
                    tasked.link = None;
                    push_run(
                        runs,
                        tasked.run(if *checked { "\u{2611} " } else { "\u{2610} " }.to_owned()),
                    );
                }
                Inline::Html(tag) => {
                    let mut html = style.clone();
                    html.color = MdInk::Muted;
                    html.link = None;
                    push_run(runs, html.run(tag.clone()));
                }
            }
        }
    }

    let mut runs = Vec::new();
    walk(
        inlines,
        &InlineStyle {
            bold: base.bold,
            italic: false,
            strike: false,
            underline: false,
            color: base.color,
            bg: None,
            mono: base.mono,
            link: None,
        },
        &mut runs,
    );
    runs
}

fn runs_equal(left: &StyledRun, right: &StyledRun) -> bool {
    left.bold == right.bold
        && left.italic == right.italic
        && left.strike == right.strike
        && left.underline == right.underline
        && left.color == right.color
        && left.bg == right.bg
        && left.mono == right.mono
        && left.link == right.link
}

fn push_run(runs: &mut Vec<StyledRun>, run: StyledRun) {
    if run.text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut()
        && runs_equal(last, &run)
    {
        last.text.push_str(&run.text);
    } else {
        runs.push(run);
    }
}

/// A laid fragment: one top-level block before pagination, in y-down
/// coordinates relative to the fragment top.
#[derive(Debug, Default)]
struct Frag {
    lines: Vec<LaidLine>,
    fills: Vec<(RectF, MdInk)>,
    links: Vec<(RectF, MdLink)>,
    headings: Vec<MdHeadingDown>,
    height: f64,
    /// Headings keep at least one following line on the same page.
    keep_with_next: bool,
    /// Line-index ranges that must not split across pages (table rows).
    atomic: Vec<(usize, usize)>,
}

#[derive(Debug, Clone)]
struct LaidLine {
    x: f64,
    baseline: f64,
    top: f64,
    height: f64,
    width: f64,
    size: f32,
    text: String,
    spans: Vec<MdSpan>,
    glyphs: Vec<MdGlyph>,
}

#[derive(Debug, Clone)]
struct MdHeadingDown {
    title: String,
    level: u8,
    slug: String,
    rect: RectF,
}

fn flip_rect(rect: RectF) -> RectF {
    RectF {
        x: rect.x,
        y: PAGE_HEIGHT - rect.y - rect.height,
        width: rect.width,
        height: rect.height,
    }
}

/// Block layout state: the shaping context, the fragment being built, and the
/// inherited indent, width, muting, and paragraph gap.
struct Layer<'a, 'f> {
    fonts: &'a mut MdFonts,
    frag: &'f mut Frag,
    indent: f64,
    width: f64,
    muted: bool,
    gap: f64,
}

impl Layer<'_, '_> {
    fn cursor(&self) -> f64 {
        self.frag.height
    }

    fn base(&self) -> StyleBase {
        StyleBase {
            bold: false,
            color: if self.muted { MdInk::Muted } else { MdInk::Ink },
            mono: false,
        }
    }

    fn layout_block(&mut self, block: &Block, top_level: bool) {
        match block {
            Block::Heading { level, inlines } => self.layout_heading(*level, inlines, top_level),
            Block::Paragraph(inlines) => {
                self.layout_runs(inlines, BODY_SIZE, self.base(), self.gap);
            }
            Block::Quote(blocks) => self.layout_quote(blocks),
            Block::List {
                ordered,
                start,
                items,
            } => {
                self.layout_list(*ordered, *start, items);
            }
            Block::Code { language: _, text } => self.layout_code(text),
            Block::Table {
                alignments,
                head,
                rows,
            } => self.layout_table(alignments, head, rows),
            Block::Rule => {
                let y = self.cursor() + RULE_TOP;
                self.frag.fills.push((
                    RectF {
                        x: self.indent,
                        y,
                        width: self.width,
                        height: 1.0,
                    },
                    MdInk::Rule,
                ));
                self.frag.height = y + 1.0 + RULE_BOTTOM;
            }
            Block::Html(text) => {
                let base = StyleBase {
                    bold: false,
                    color: MdInk::Muted,
                    mono: true,
                };
                self.layout_text_runs(text, HTML_SIZE, base, self.gap);
            }
        }
    }

    fn layout_heading(&mut self, level: u8, inlines: &[Inline], top_level: bool) {
        let size = heading_size(level);
        let top_margin = if level == 1 {
            H1_TOP
        } else {
            OTHER_HEADING_TOP
        };
        let start = self.cursor() + top_margin;
        self.frag.height = start;
        let base = StyleBase {
            bold: true,
            color: if self.muted { MdInk::Muted } else { MdInk::Ink },
            mono: false,
        };
        let first_line = self.frag.lines.len();
        self.layout_runs(inlines, size, base, 0.0);
        let heading_rect = RectF {
            x: self.indent,
            y: start,
            width: self.width,
            height: self.frag.height - start,
        };
        if level <= 2 {
            let rule_y = self.frag.height + HEADING_RULE_PADDING - 1.0;
            self.frag.fills.push((
                RectF {
                    x: self.indent,
                    y: rule_y,
                    width: self.width,
                    height: 1.0,
                },
                MdInk::Rule,
            ));
            self.frag.height = rule_y + 1.0;
        }
        self.frag.height += HEADING_BOTTOM;
        let title = plain_text(inlines);
        self.frag.headings.push(MdHeadingDown {
            title: title.clone(),
            level,
            slug: slug(&title),
            rect: heading_rect,
        });
        // Bold the heading spans explicitly: the base already marks them bold,
        // and this keeps the record honest if the base ever changes.
        for line in &mut self.frag.lines[first_line..] {
            for span in &mut line.spans {
                span.bold = true;
            }
        }
        if top_level {
            self.frag.keep_with_next = true;
        }
    }

    fn layout_quote(&mut self, blocks: &[Block]) {
        let start = self.cursor();
        self.frag.height = start + QUOTE_PAD_Y;
        let bar_x = self.indent;
        let inner_indent = self.indent + QUOTE_BAR + QUOTE_PADDING;
        let inner_width = self.width - (QUOTE_BAR + QUOTE_PADDING);
        let outer_indent = std::mem::replace(&mut self.indent, inner_indent);
        let outer_width = std::mem::replace(&mut self.width, inner_width.max(1.0));
        let outer_muted = std::mem::replace(&mut self.muted, true);
        for block in blocks {
            self.layout_block(block, false);
        }
        self.indent = outer_indent;
        self.width = outer_width;
        self.muted = outer_muted;
        self.frag.height += QUOTE_PAD_Y;
        self.frag.fills.push((
            RectF {
                x: bar_x,
                y: start,
                width: QUOTE_BAR,
                height: self.frag.height - start,
            },
            MdInk::Rule,
        ));
        self.frag.height += QUOTE_GAP;
    }

    fn layout_list(&mut self, ordered: bool, start: u64, items: &[Vec<Block>]) {
        for (index, item) in items.iter().enumerate() {
            let marker = if ordered {
                format!("{}. ", start.saturating_add(index as u64))
            } else {
                "• ".to_owned()
            };
            self.layout_list_item(&marker, item);
        }
        self.frag.height += LIST_GAP;
    }

    fn layout_list_item(&mut self, marker: &str, item: &[Block]) {
        let item_indent = self.indent;
        let marker_run = StyledRun {
            text: marker.to_owned(),
            bold: false,
            italic: false,
            strike: false,
            underline: false,
            color: if self.muted { MdInk::Muted } else { MdInk::Ink },
            bg: None,
            mono: false,
            link: None,
        };
        let mut first = true;
        let mut hang = LIST_INDENT;
        for block in item {
            if first && let Block::Paragraph(inlines) = block {
                let mut runs = vec![marker_run.clone()];
                runs.extend(flatten_inlines(inlines, self.base()));
                let before = self.frag.lines.len();
                self.shape_runs(runs, BODY_SIZE, item_indent, self.width, 0.0);
                // The hanging indent is measured, not guessed: wrapped lines
                // start where the marker text ends.
                if let Some(line) = self.frag.lines.get(before) {
                    let marker_bytes = marker.len();
                    let end = line
                        .glyphs
                        .iter()
                        .filter(|glyph| glyph.start < marker_bytes)
                        .map(|glyph| f64::from(glyph.x + glyph.w))
                        .fold(0.0_f64, f64::max);
                    if end > 0.0 {
                        hang = end;
                    }
                    for line in &mut self.frag.lines[before + 1..] {
                        line.x += hang;
                    }
                }
                self.frag.height += TIGHT_PARAGRAPH_GAP;
                first = false;
                continue;
            }
            if first {
                // An item that opens with a nested block still shows its
                // marker on its own line, like the WPF virtualized path.
                self.shape_runs(
                    vec![marker_run.clone()],
                    BODY_SIZE,
                    item_indent,
                    self.width,
                    0.0,
                );
                self.frag.height += TIGHT_PARAGRAPH_GAP;
                first = false;
            }
            let outer_indent = std::mem::replace(&mut self.indent, item_indent + hang);
            let nested_width = (self.width - hang).max(1.0);
            let outer_width = std::mem::replace(&mut self.width, nested_width);
            let outer_gap = std::mem::replace(&mut self.gap, TIGHT_PARAGRAPH_GAP);
            self.layout_block(block, false);
            self.indent = outer_indent;
            self.width = outer_width;
            self.gap = outer_gap;
        }
        if first {
            self.shape_runs(vec![marker_run], BODY_SIZE, item_indent, self.width, 0.0);
            self.frag.height += TIGHT_PARAGRAPH_GAP;
        }
    }

    fn layout_code(&mut self, text: &str) {
        let start = self.cursor() + CODE_TOP;
        self.frag.height = start + CODE_PAD_Y;
        let base = StyleBase {
            bold: false,
            color: if self.muted { MdInk::Muted } else { MdInk::Ink },
            mono: true,
        };
        let outer_indent = self.indent + CODE_PAD_X;
        let outer_width = (self.width - CODE_PAD_X * 2.0).max(1.0);
        let saved_indent = std::mem::replace(&mut self.indent, outer_indent);
        let saved_width = std::mem::replace(&mut self.width, outer_width);
        self.layout_text_runs(text, CODE_SIZE, base, 0.0);
        self.indent = saved_indent;
        self.width = saved_width;
        self.frag.height += CODE_PAD_Y;
        // Fills always paint beneath text, so order among fills only matters
        // where they overlap; the code background goes first.
        self.frag.fills.insert(
            0,
            (
                RectF {
                    x: saved_indent,
                    y: start,
                    width: saved_width,
                    height: self.frag.height - start,
                },
                MdInk::CodeBg,
            ),
        );
        self.frag.height += CODE_BOTTOM;
    }

    fn layout_table(
        &mut self,
        alignments: &[TableAlign],
        head: &[Vec<Inline>],
        rows: &[Vec<Vec<Inline>>],
    ) {
        let columns = rows
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(head.len())
            .max(1);
        let column_width = self.width / columns as f64;
        let table_x = self.indent;
        let table_top = self.cursor();
        let mut row_ranges: Vec<(usize, usize)> = Vec::new();

        let mut layout_row = |cells: &[Vec<Inline>], header: bool, layer: &mut Layer<'_, '_>| {
            let row_start_line = layer.frag.lines.len();
            // Shape every cell, then align rows by the tallest cell.
            let mut cell_lines: Vec<Vec<LaidLine>> = Vec::new();
            let mut row_height: f64 = 0.0;
            for column in 0..columns {
                let inlines: Vec<Inline> = cells.get(column).map_or_else(
                    || vec![Inline::Text(String::new())],
                    |cell| {
                        if cell.is_empty() {
                            vec![Inline::Text(String::new())]
                        } else {
                            cell.clone()
                        }
                    },
                );
                let base = StyleBase {
                    bold: header,
                    color: layer.base().color,
                    mono: false,
                };
                let runs = flatten_inlines(&inlines, base);
                let mut families = LayerFamilies::from_fonts(layer.fonts);
                let shaped = shape_paragraph(
                    &mut layer.fonts.font_system,
                    &mut families,
                    &runs,
                    BODY_SIZE,
                    (column_width - CELL_PAD_X * 2.0).max(1.0) as f32,
                );
                let align = alignments.get(column).copied().unwrap_or(TableAlign::Left);
                let cell_x = table_x + column as f64 * column_width + CELL_PAD_X;
                let inner = (column_width - CELL_PAD_X * 2.0).max(0.0);
                let mut lines = Vec::new();
                let mut y = layer.cursor() + CELL_PAD_Y;
                for run in shaped {
                    let run_width = f64::from(run.width);
                    let offset = match align {
                        TableAlign::Left => 0.0,
                        TableAlign::Center => ((inner - run_width) / 2.0).max(0.0),
                        TableAlign::Right => (inner - run_width).max(0.0),
                    };
                    lines.push(run.into_line(cell_x + offset, y, BODY_SIZE));
                    y += lines.last().map_or(0.0, |line: &LaidLine| line.height);
                }
                row_height = row_height.max(y - layer.cursor() + CELL_PAD_Y);
                cell_lines.push(lines);
            }
            row_height = row_height.max(BODY_SIZE as f64 * 1.35);
            for lines in &cell_lines {
                layer.frag.lines.extend(lines.iter().cloned());
            }
            layer.frag.height += row_height;
            if header {
                layer.frag.fills.push((
                    RectF {
                        x: table_x,
                        y: layer.cursor() - row_height,
                        width: layer.width,
                        height: row_height,
                    },
                    MdInk::TableHeaderBg,
                ));
            }
            row_ranges.push((row_start_line, layer.frag.lines.len()));
        };

        if !head.is_empty() {
            layout_row(head, true, self);
        }
        for row in rows {
            layout_row(row, false, self);
        }

        // Grid: outer top/left plus per-cell right/bottom, like the WPF table.
        let table_bottom = self.cursor();
        let mut grid: Vec<(RectF, MdInk)> = Vec::new();
        grid.push((
            RectF {
                x: table_x,
                y: table_top,
                width: self.width,
                height: 1.0,
            },
            MdInk::Rule,
        ));
        grid.push((
            RectF {
                x: table_x,
                y: table_top,
                width: 1.0,
                height: table_bottom - table_top,
            },
            MdInk::Rule,
        ));
        let mut y = table_top;
        for (start, end) in &row_ranges {
            let row_bottom = self.frag.lines[*start..*end]
                .iter()
                .map(|line| line.top + line.height)
                .fold(y, f64::max)
                + CELL_PAD_Y;
            for column in 0..columns {
                grid.push((
                    RectF {
                        x: table_x + (column + 1) as f64 * column_width - 1.0,
                        y,
                        width: 1.0,
                        height: row_bottom - y,
                    },
                    MdInk::Rule,
                ));
            }
            grid.push((
                RectF {
                    x: table_x,
                    y: row_bottom - 1.0,
                    width: self.width,
                    height: 1.0,
                },
                MdInk::Rule,
            ));
            y = row_bottom;
        }
        // Grid paints beneath text and header fills.
        grid.extend(std::mem::take(&mut self.frag.fills));
        self.frag.fills = grid;
        self.frag.atomic.extend(row_ranges);
        self.frag.height += TABLE_GAP;
    }

    /// Shape inline runs into laid lines at the cursor, then advance by the
    /// text height plus `gap`.
    fn layout_runs(&mut self, inlines: &[Inline], size: f32, base: StyleBase, gap: f64) {
        let runs = flatten_inlines(inlines, base);
        self.shape_runs(runs, size, self.indent, self.width, gap);
    }

    /// Shape literal text (code, HTML) with a uniform base style.
    fn layout_text_runs(&mut self, text: &str, size: f32, base: StyleBase, gap: f64) {
        let runs = vec![StyledRun {
            text: text.to_owned(),
            bold: base.bold,
            italic: false,
            strike: false,
            underline: false,
            color: base.color,
            bg: None,
            mono: base.mono,
            link: None,
        }];
        self.shape_runs(runs, size, self.indent, self.width, gap);
    }

    fn shape_runs(&mut self, runs: Vec<StyledRun>, size: f32, x: f64, width: f64, gap: f64) {
        // Families clone out first: the `Attrs` borrows below must not alias
        // the font-system borrow.
        let mut families = LayerFamilies::from_fonts(self.fonts);
        let shaped = shape_paragraph(
            &mut self.fonts.font_system,
            &mut families,
            &runs,
            size,
            width.max(1.0) as f32,
        );
        let mut y = self.cursor();
        for run in shaped {
            let line = run.into_line(x, y, size);
            self.collect_link_runs(&line);
            y += line.height;
            self.frag.lines.push(line);
        }
        self.frag.height = y + gap;
    }

    /// Record clickable regions for the link spans of a laid line.
    fn collect_link_runs(&mut self, line: &LaidLine) {
        for span in &line.spans {
            let Some(link) = span.link.clone() else {
                continue;
            };
            let mut x0 = f64::MAX;
            let mut x1 = f64::MIN;
            for glyph in &line.glyphs {
                if glyph.end > span.start && glyph.start < span.end {
                    x0 = x0.min(line.x + f64::from(glyph.x));
                    x1 = x1.max(line.x + f64::from(glyph.x + glyph.w));
                }
            }
            if x0 <= x1 {
                self.frag.links.push((
                    RectF {
                        x: x0,
                        y: line.top,
                        width: (x1 - x0).max(1.0),
                        height: line.height,
                    },
                    link,
                ));
            }
        }
    }
}

/// The family names a shaping call needs, cloned out of [`MdFonts`] so the
/// `Attrs` borrows below never alias the font-system borrow.
struct LayerFamilies {
    regular: Option<String>,
    bold: Option<String>,
    mono: Option<String>,
}

impl LayerFamilies {
    fn from_fonts(fonts: &MdFonts) -> Self {
        Self {
            regular: fonts.regular.clone(),
            bold: fonts.bold.clone(),
            mono: fonts.mono.clone(),
        }
    }
}

/// One shaped run: a wrapped visual line with run-relative byte offsets.
struct ShapedRun {
    text: String,
    spans: Vec<MdSpan>,
    glyphs: Vec<MdGlyph>,
    baseline: f32,
    top: f32,
    height: f32,
    width: f32,
}

impl ShapedRun {
    fn into_line(self, x: f64, y: f64, size: f32) -> LaidLine {
        LaidLine {
            x,
            baseline: y + f64::from(self.baseline - self.top),
            top: y,
            height: f64::from(self.height),
            width: f64::from(self.width),
            size,
            text: self.text,
            spans: self.spans,
            glyphs: self.glyphs,
        }
    }
}

/// Shape style runs into wrapped visual lines. Colors resolve against the
/// light palette: ink never affects metrics, and the raster pass re-resolves
/// every span against the tile's own palette.
fn shape_paragraph(
    font_system: &mut FontSystem,
    families: &mut LayerFamilies,
    runs: &[StyledRun],
    size: f32,
    width: f32,
) -> Vec<ShapedRun> {
    let full: String = runs.iter().map(|run| run.text.as_str()).collect();
    if full.is_empty() {
        return Vec::new();
    }
    let mut spans: Vec<(&str, Attrs<'_>)> = Vec::new();
    let mut line_spans: Vec<MdSpan> = Vec::new();
    let mut offset = 0usize;
    for run in runs {
        let family = if run.mono {
            families.mono.as_deref().or(families.regular.as_deref())
        } else if run.bold {
            families.bold.as_deref().or(families.regular.as_deref())
        } else {
            families.regular.as_deref()
        };
        let attrs = Attrs::new()
            .family(match family {
                Some(name) => Family::Name(name),
                None if run.mono => Family::Monospace,
                None => Family::SansSerif,
            })
            .weight(if run.bold {
                Weight::BOLD
            } else {
                Weight::NORMAL
            })
            .style(if run.italic {
                Style::Italic
            } else {
                Style::Normal
            });
        spans.push((run.text.as_str(), attrs));
        let end = offset + run.text.len();
        line_spans.push(MdSpan {
            start: offset,
            end,
            bold: run.bold,
            italic: run.italic,
            strike: run.strike,
            underline: run.underline,
            color: run.color,
            bg: run.bg,
            mono: run.mono,
            link: run.link.clone(),
        });
        offset = end;
    }

    let metrics = Metrics::new(size, size * super::theme::LINE_HEIGHT_FACTOR);
    let mut buffer = Buffer::new(font_system, metrics);
    {
        let mut borrowed = buffer.borrow_with(font_system);
        borrowed.set_size(Some(width), None);
        let default = Attrs::new();
        borrowed.set_rich_text(spans, &default, Shaping::Advanced, None);
        borrowed.shape_until_scroll(true);
    }
    // BufferLine byte offsets in `full`: rich text splits lines exactly at
    // `\n`, and glyph cluster indices restart at each line.
    let mut line_starts = vec![0usize];
    for (index, _) in full.match_indices('\n') {
        line_starts.push(index + 1);
    }
    let mut shaped = Vec::new();
    for run in buffer.layout_runs() {
        // `LayoutRun.text` is the whole buffer line, not the run's fragment:
        // the visible portion is exactly what the run's glyphs cover.
        if run.glyphs.is_empty() {
            continue;
        }
        let base = run
            .glyphs
            .iter()
            .map(|glyph| glyph.start)
            .min()
            .unwrap_or(0);
        let end = run
            .glyphs
            .iter()
            .map(|glyph| glyph.end)
            .max()
            .unwrap_or(base);
        let line_base = line_starts.get(run.line_i).copied().unwrap_or(0);
        let abs_base = line_base + base;
        let abs_end = line_base + end;
        let Some(text) = full.get(abs_base..abs_end) else {
            continue;
        };
        let mut glyphs = Vec::with_capacity(run.glyphs.len());
        for glyph in run.glyphs {
            glyphs.push(MdGlyph {
                start: glyph.start.saturating_sub(base),
                end: glyph.end.saturating_sub(base),
                x: glyph.x,
                w: glyph.w,
            });
        }
        let mut run_spans = Vec::new();
        for span in &line_spans {
            if span.end > abs_base && span.start < abs_end {
                run_spans.push(MdSpan {
                    start: span.start.saturating_sub(abs_base),
                    end: span.end.min(abs_end).saturating_sub(abs_base),
                    ..span.clone()
                });
            }
        }
        shaped.push(ShapedRun {
            text: text.to_owned(),
            spans: run_spans,
            glyphs,
            baseline: run.line_y,
            top: run.line_top,
            height: run.line_height,
            width: run.line_w,
        });
    }
    shaped
}

/// Paginate fragments into y-up pages.
fn paginate(frags: Vec<Frag>) -> Vec<MdPage> {
    let mut pages: Vec<MdPage> = Vec::new();
    let mut current = MdPageDown::default();
    let mut index = 0;
    while index < frags.len() {
        let frag = &frags[index];
        let remaining = CONTENT_HEIGHT - (current.height - MARGIN_Y);
        // A heading keeps at least one following line on its page.
        let keep_next_extra = if frag.keep_with_next {
            frags
                .get(index + 1)
                .and_then(|next| next.lines.first())
                .map_or(0.0, |line| line.height.min(BODY_SIZE as f64 * 1.35))
        } else {
            0.0
        };
        if frag.height + keep_next_extra <= remaining {
            current.push_frag(frag, 0, frag.lines.len());
            index += 1;
            continue;
        }
        if current.is_empty() {
            // Even a fresh page cannot hold it whole; split across pages.
            if frag.lines.is_empty() {
                current.push_frag(frag, 0, 0);
                index += 1;
                continue;
            }
            let mut start = 0;
            while start < frag.lines.len() {
                let room = CONTENT_HEIGHT - (current.height - MARGIN_Y);
                let end = split_end(frag, start, room);
                current.push_frag(frag, start, end);
                start = end;
                if start < frag.lines.len() {
                    pages.push(current.finish());
                    current = MdPageDown::default();
                }
            }
            index += 1;
        } else {
            pages.push(current.finish());
            current = MdPageDown::default();
        }
    }
    if !current.is_empty() {
        pages.push(current.finish());
    }
    if pages.is_empty() {
        pages.push(MdPage::default());
    }
    pages
}

/// Last line index (exclusive) of a fragment that fits in `room`, honoring
/// atomic ranges unless a single range alone overflows the page.
fn split_end(frag: &Frag, start: usize, room: f64) -> usize {
    let base = frag.lines.get(start).map_or(0.0, |line| line.top);
    let mut end = start;
    for (line_index, line) in frag.lines.iter().enumerate().skip(start) {
        if line.top + line.height - base > room && line_index > start {
            break;
        }
        end = line_index + 1;
    }
    if end == start && start < frag.lines.len() {
        end = start + 1;
    }
    // Pull the break back out of atomic ranges (table rows) when the page
    // still holds something; a range taller than the page splits anyway.
    for (range_start, range_end) in &frag.atomic {
        if *range_start < end && end < *range_end && *range_start > start {
            end = *range_start;
        }
    }
    end
}

/// A page under construction in y-down coordinates.
#[derive(Debug, Default)]
struct MdPageDown {
    lines: Vec<LaidLine>,
    fills: Vec<(RectF, MdInk)>,
    links: Vec<(RectF, MdLink)>,
    headings: Vec<MdHeadingDown>,
    height: f64,
}

impl MdPageDown {
    fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.fills.is_empty()
    }

    /// Append fragment lines `[start, end)` at the cursor, clipping fills,
    /// links, and headings to the kept line range. Line-less fragments
    /// (rules) carry only fills and are placed whole.
    fn push_frag(&mut self, frag: &Frag, start: usize, end: usize) {
        let origin = self.height_or_top();
        if frag.lines.is_empty() {
            for (rect, ink) in &frag.fills {
                self.fills.push((shift_rect(*rect, origin), *ink));
            }
            for (rect, link) in &frag.links {
                self.links.push((shift_rect(*rect, origin), link.clone()));
            }
            for heading in &frag.headings {
                self.headings.push(MdHeadingDown {
                    title: heading.title.clone(),
                    level: heading.level,
                    slug: heading.slug.clone(),
                    rect: shift_rect(heading.rect, origin),
                });
            }
            self.height = origin + frag.height;
            return;
        }
        if start >= end {
            return;
        }
        let end = end.min(frag.lines.len());
        let whole = start == 0 && end == frag.lines.len();
        // Whole fragments keep their internal margins; a split restarts its
        // kept lines at the page top.
        let frag_base = if whole {
            0.0
        } else {
            frag.lines.get(start).map_or(0.0, |line| line.top)
        };
        let dy = origin - frag_base;
        for line in &frag.lines[start..end] {
            let mut moved = line.clone();
            moved.top += dy;
            moved.baseline += dy;
            self.lines.push(moved);
        }
        // Whole-fragment placement shifts decorations untouched: clipping them
        // to the kept lines' extent would trim block pads (code backgrounds,
        // quote pads, table borders). Only a real page split clips.
        if whole {
            for (rect, ink) in &frag.fills {
                self.fills.push((shift_rect(*rect, dy), *ink));
            }
            for (rect, link) in &frag.links {
                self.links.push((shift_rect(*rect, dy), link.clone()));
            }
            for heading in &frag.headings {
                self.headings.push(MdHeadingDown {
                    title: heading.title.clone(),
                    level: heading.level,
                    slug: heading.slug.clone(),
                    rect: shift_rect(heading.rect, dy),
                });
            }
            self.height = origin + frag.height;
            return;
        }
        let top = frag.lines.get(start).map_or(0.0, |line| line.top);
        let bottom = frag
            .lines
            .get(end.saturating_sub(1))
            .map_or(top, |line| line.top + line.height);
        for (rect, ink) in &frag.fills {
            if let Some(clipped) = clip_vertical(*rect, top, bottom, dy) {
                self.fills.push((clipped, *ink));
            }
        }
        for (rect, link) in &frag.links {
            if let Some(clipped) = clip_vertical(*rect, top, bottom, dy) {
                self.links.push((clipped, link.clone()));
            }
        }
        for heading in &frag.headings {
            if let Some(clipped) = clip_vertical(heading.rect, top, bottom, dy) {
                self.headings.push(MdHeadingDown {
                    title: heading.title.clone(),
                    level: heading.level,
                    slug: heading.slug.clone(),
                    rect: clipped,
                });
            }
        }
        let frag_bottom = if end < frag.lines.len() {
            bottom
        } else {
            frag.height
        };
        self.height = origin + (frag_bottom - frag_base);
    }

    fn height_or_top(&self) -> f64 {
        if self.is_empty() {
            MARGIN_Y
        } else {
            self.height
        }
    }

    fn finish(self) -> MdPage {
        // Edge flips, not box flips: a y-down top edge T becomes the y-up top
        // edge PAGE_HEIGHT - T (larger y is higher in y-up). RectF fills below
        // keep the codebase convention where RectF.y is the y-up bottom edge.
        let flip_line = |line: LaidLine| MdLine {
            x: line.x,
            baseline: PAGE_HEIGHT - line.baseline,
            top: PAGE_HEIGHT - line.top,
            height: line.height,
            width: line.width,
            size: line.size,
            text: line.text,
            spans: line.spans,
            glyphs: line.glyphs,
        };
        MdPage {
            lines: self.lines.into_iter().map(flip_line).collect(),
            fills: self
                .fills
                .into_iter()
                .map(|(rect, ink)| (flip_rect(rect), ink))
                .collect(),
            links: self
                .links
                .into_iter()
                .map(|(rect, link)| (flip_rect(rect), link))
                .collect(),
            headings: self
                .headings
                .into_iter()
                .map(|heading| MdHeading {
                    title: heading.title,
                    level: heading.level,
                    slug: heading.slug,
                    rect: flip_rect(heading.rect),
                })
                .collect(),
        }
    }
}

/// Shift a y-down rect from fragment coordinates to page coordinates.
fn shift_rect(rect: RectF, origin: f64) -> RectF {
    RectF {
        x: rect.x,
        y: rect.y + origin,
        width: rect.width,
        height: rect.height,
    }
}

/// Clip a y-down rect to `[top, bottom)` and shift it by `dy`.
fn clip_vertical(rect: RectF, top: f64, bottom: f64, dy: f64) -> Option<RectF> {
    let y0 = rect.y.max(top);
    let y1 = (rect.y + rect.height).min(bottom);
    if y1 <= y0 {
        return None;
    }
    Some(RectF {
        x: rect.x,
        y: y0 + dy,
        width: rect.width,
        height: y1 - y0,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::super::model::parse_markdown;
    use super::*;

    fn laid(source: &str) -> Vec<MdPage> {
        let mut fonts = MdFonts::new();
        layout_document(&mut fonts, &parse_markdown(source))
    }

    #[test]
    fn a_short_document_fits_on_one_page_with_an_outline() {
        let pages = laid("# Title\n\nHello *world*.\n");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].headings.len(), 1);
        assert_eq!(pages[0].headings[0].title, "Title");
        assert_eq!(pages[0].headings[0].slug, "title");
        let text: String = pages[0]
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Title"), "heading text laid out: {text:?}");
        assert!(text.contains("Hello"), "paragraph text laid out: {text:?}");
    }

    #[test]
    fn long_documents_paginate() {
        let mut source = String::new();
        for index in 0..400 {
            source.push_str(&format!("Paragraph {index} with enough words to wrap across the page width several times over.\n\n"));
        }
        let pages = laid(&source);
        assert!(
            pages.len() > 3,
            "expected several pages, got {}",
            pages.len()
        );
        for page in &pages {
            assert!(!page.lines.is_empty(), "no page may be empty");
        }
    }

    #[test]
    fn links_and_code_survive_layout() {
        let pages = laid("[click](https://example.com/) and `code`.\n\n```rust\nlet x = 1;\n```\n");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].links.len(), 1);
        assert!(matches!(
            &pages[0].links[0].1,
            MdLink::External(url) if url == "https://example.com/"
        ));
        assert!(
            pages[0].fills.iter().any(|(_, ink)| *ink == MdInk::CodeBg),
            "code block background fill present"
        );
    }

    #[test]
    fn tables_lay_out_with_grid_and_header() {
        let pages = laid("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n");
        assert_eq!(pages.len(), 1);
        assert!(
            pages[0]
                .fills
                .iter()
                .any(|(_, ink)| *ink == MdInk::TableHeaderBg),
            "header background present"
        );
        let rules = pages[0]
            .fills
            .iter()
            .filter(|(_, ink)| *ink == MdInk::Rule)
            .count();
        assert!(rules >= 6, "grid lines present, got {rules}");
    }

    #[test]
    fn wrapped_lines_partition_the_paragraph_without_duplication() {
        // `LayoutRun.text` is the whole buffer line, not the run fragment;
        // slicing it blindly repeats the full text on every wrapped line.
        let source = "Note: kitty already copied (`~/.config/kitty/` populated, matches old). No files copied by this survey.";
        let pages = laid(&format!("{source}\n"));
        assert_eq!(pages.len(), 1);
        let lines = &pages[0].lines;
        assert!(
            lines.len() >= 2,
            "the paragraph must wrap, got {} line(s)",
            lines.len()
        );
        for line in lines {
            assert!(
                line.text.len() < source.len(),
                "no wrapped line may carry the full paragraph: {:?}",
                line.text
            );
            for span in &line.spans {
                assert!(
                    span.end <= line.text.len(),
                    "span {span:?} exceeds {:?}",
                    line.text
                );
            }
        }
        // Compare against the parsed plain text, not the raw source: code
        // spans render without their backticks.
        let inlines = super::super::model::parse_markdown(&format!("{source}\n"))
            .into_iter()
            .flat_map(|block| match block {
                Block::Paragraph(inlines) => inlines,
                _ => Vec::new(),
            })
            .collect::<Vec<_>>();
        let parsed = plain_text(&inlines);
        let reassembled: Vec<&str> = lines
            .iter()
            .flat_map(|line| line.text.split_whitespace())
            .collect();
        let expected: Vec<&str> = parsed.split_whitespace().collect();
        assert_eq!(reassembled, expected, "wrapped lines reassemble exactly");
    }

    #[test]
    fn table_cell_text_appears_exactly_once() {
        let pages = laid(
            "| alpha beta gamma delta epsilon zeta eta theta |\n|---|\n| one two three four five six seven eight nine ten |\n",
        );
        let text: String = pages[0]
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            text.matches("epsilon").count(),
            1,
            "header cell rendered once: {text:?}"
        );
        assert_eq!(
            text.matches("seven").count(),
            1,
            "body cell rendered once: {text:?}"
        );
    }
}
