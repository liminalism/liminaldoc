//! Markdown block model, parsed from a CommonMark event stream.
//!
//! A port of `MarkdownViewer.Wpf`'s `MarkdownDocumentService` block walk onto
//! pulldown-cmark events. The WPF viewer walks a Markdig AST and converts each
//! block (`ConvertBlock`) and inline (`AppendInline`); this module walks the
//! equivalent event stream and converts each container the same way: headings,
//! paragraphs, quotes, ordered and unordered lists with nesting, fenced and
//! indented code blocks, pipe tables with column alignment, thematic breaks,
//! HTML blocks rendered as muted monospace, and the full inline set
//! (emphasis, strong, strikethrough, code spans, links, images, task-list
//! markers, autolinks, inline HTML).
//!
//! Two WPF behaviors are preserved deliberately:
//!
//! - soft line breaks render as hard breaks (`UseSoftlineBreakAsHardlineBreak`
//!   in the WPF pipeline), so an author's line structure survives;
//! - images keep only their URL and alt text here; the WPF viewer loads the
//!   bitmap and falls back to muted italic alt text, and this port renders
//!   that fallback until image loading exists.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};

/// The CommonMark extensions this port enables, matching the WPF viewer's
/// advanced pipeline (tables, footnotes, strikethrough, task lists) plus
/// heading attributes, YAML front matter (parsed and then skipped, never
/// rendered), and math spans (rendered as code).
pub fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_MATH
}

/// One block of a parsed Markdown document.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// A `#`–`######` heading; `level` is clamped to 1–6.
    Heading {
        level: u8,
        inlines: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    Quote(Vec<Block>),
    List {
        ordered: bool,
        start: u64,
        items: Vec<Vec<Block>>,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Table {
        alignments: Vec<TableAlign>,
        head: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Rule,
    /// Raw HTML, rendered as muted monospace like the WPF viewer.
    Html(String),
}

/// One inline element inside a block.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    /// A line break. Soft breaks are hardened to match the WPF pipeline.
    Break,
    Code(String),
    /// Single `*`/`_` emphasis (italic).
    Emphasis(Vec<Inline>),
    /// Double `**`/`__` emphasis (bold).
    Strong(Vec<Inline>),
    /// `~~` strikethrough.
    Strike(Vec<Inline>),
    Link {
        url: String,
        title: String,
        children: Vec<Inline>,
    },
    /// The URL is kept for future image loading; `alt` is what renders today.
    Image {
        url: String,
        alt: String,
    },
    Task(bool),
    Html(String),
}

/// A pipe-table column alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAlign {
    Left,
    Center,
    Right,
}

impl TableAlign {
    fn from_pulldown(alignment: Alignment) -> Self {
        match alignment {
            Alignment::Center => Self::Center,
            Alignment::Right => Self::Right,
            Alignment::Left | Alignment::None => Self::Left,
        }
    }
}

/// Parse Markdown source into blocks.
pub fn parse_markdown(text: &str) -> Vec<Block> {
    let parser = Parser::new_ext(text, markdown_options());
    let mut events = parser.peekable();
    parse_blocks(&mut events)
}

type EventStream<'a> = std::iter::Peekable<Parser<'a>>;

fn parse_blocks(events: &mut EventStream<'_>) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut stray: Vec<Inline> = Vec::new();
    while events.peek().is_some() {
        match events.peek() {
            None | Some(Event::End(_)) => break,
            Some(Event::Start(_)) => {
                let Some(Event::Start(tag)) = events.next() else {
                    break;
                };
                flush_stray(&mut blocks, &mut stray);
                blocks.extend(parse_container(tag, events));
            }
            Some(Event::Rule) => {
                events.next();
                flush_stray(&mut blocks, &mut stray);
                blocks.push(Block::Rule);
            }
            Some(_) => {
                let Some(event) = events.next() else {
                    break;
                };
                // Inline events outside a block cannot happen from a
                // well-formed stream; keep the text rather than drop it.
                push_single_inline(&mut stray, event);
            }
        }
    }
    flush_stray(&mut blocks, &mut stray);
    blocks
}

fn flush_stray(blocks: &mut Vec<Block>, stray: &mut Vec<Inline>) {
    if !stray.is_empty() {
        blocks.push(Block::Paragraph(std::mem::take(stray)));
    }
}

/// Convert one container start tag plus its contents. Returns a list because a
/// footnote definition renders as a marker paragraph followed by its blocks.
fn parse_container(tag: Tag<'_>, events: &mut EventStream<'_>) -> Vec<Block> {
    match tag {
        Tag::Paragraph => vec![Block::Paragraph(parse_inlines(events))],
        Tag::Heading { level, .. } => vec![Block::Heading {
            level: heading_level(level),
            inlines: parse_inlines(events),
        }],
        Tag::BlockQuote(_) => vec![Block::Quote(parse_inner_blocks(events))],
        Tag::CodeBlock(kind) => vec![parse_code_block(kind, events)],
        Tag::HtmlBlock => vec![Block::Html(parse_raw_text(events))],
        Tag::List(first) => vec![parse_list(first, events)],
        Tag::Item => {
            // A bare item outside a list cannot happen; keep its blocks.
            vec![Block::Quote(parse_inner_blocks(events))]
        }
        Tag::FootnoteDefinition(label) => {
            let mut blocks = vec![Block::Paragraph(vec![Inline::Text(format!("[^{label}]"))])];
            blocks.extend(parse_inner_blocks(events));
            blocks
        }
        Tag::Table(alignments) => vec![parse_table(alignments, events)],
        Tag::TableHead | Tag::TableRow | Tag::TableCell => {
            // Bare table parts cannot happen; keep their text as a paragraph.
            vec![Block::Paragraph(parse_inlines(events))]
        }
        Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. } => {
            // Inline containers at block level cannot happen; keep the text.
            vec![Block::Paragraph(parse_inlines(events))]
        }
        Tag::Superscript | Tag::Subscript => {
            // Not enabled in this port's options; flatten defensively.
            vec![Block::Paragraph(parse_inlines(events))]
        }
        Tag::DefinitionList | Tag::DefinitionListDefinition => {
            // Not enabled in this port's options; keep the blocks unstyled.
            parse_inner_blocks(events)
        }
        Tag::DefinitionListTitle => {
            vec![Block::Paragraph(vec![Inline::Strong(parse_inlines(
                events,
            ))])]
        }
        Tag::MetadataBlock(_) => {
            skip_container(events);
            Vec::new()
        }
    }
}

/// Parse blocks until the container's end tag, consuming the end tag.
fn parse_inner_blocks(events: &mut EventStream<'_>) -> Vec<Block> {
    let blocks = parse_blocks(events);
    consume_end(events);
    blocks
}

fn parse_inlines(events: &mut EventStream<'_>) -> Vec<Inline> {
    let mut inlines = Vec::new();
    while events.peek().is_some() {
        match events.peek() {
            None | Some(Event::End(_)) => break,
            Some(Event::Start(_)) => {
                let Some(Event::Start(tag)) = events.next() else {
                    break;
                };
                push_container_inline(&mut inlines, tag, events);
            }
            Some(_) => {
                let Some(event) = events.next() else {
                    break;
                };
                push_single_inline(&mut inlines, event);
            }
        }
    }
    consume_end(events);
    inlines
}

fn push_container_inline(inlines: &mut Vec<Inline>, tag: Tag<'_>, events: &mut EventStream<'_>) {
    match tag {
        Tag::Emphasis => inlines.push(Inline::Emphasis(parse_inlines(events))),
        Tag::Strong => inlines.push(Inline::Strong(parse_inlines(events))),
        Tag::Strikethrough => inlines.push(Inline::Strike(parse_inlines(events))),
        Tag::Link {
            dest_url, title, ..
        } => {
            let children = parse_inlines(events);
            let children = if children.is_empty() {
                vec![Inline::Text(dest_url.to_string())]
            } else {
                children
            };
            inlines.push(Inline::Link {
                url: dest_url.to_string(),
                title: title.to_string(),
                children,
            });
        }
        Tag::Image { dest_url, .. } => {
            let children = parse_inlines(events);
            inlines.push(Inline::Image {
                url: dest_url.to_string(),
                alt: plain_text(&children),
            });
        }
        // Block containers inside inline content cannot happen; flatten their
        // text so nothing is lost.
        _ => inlines.extend(parse_inlines(events)),
    }
}

fn push_single_inline(inlines: &mut Vec<Inline>, event: Event<'_>) {
    match event {
        Event::Text(text) => inlines.push(Inline::Text(text.to_string())),
        Event::Code(code) => inlines.push(Inline::Code(code.to_string())),
        Event::Html(html) | Event::InlineHtml(html) => inlines.push(Inline::Html(html.to_string())),
        Event::SoftBreak | Event::HardBreak => inlines.push(Inline::Break),
        Event::TaskListMarker(checked) => inlines.push(Inline::Task(checked)),
        Event::FootnoteReference(label) => inlines.push(Inline::Text(format!("[^{label}]"))),
        Event::InlineMath(math) | Event::DisplayMath(math) => {
            inlines.push(Inline::Code(math.to_string()));
        }
        Event::Rule | Event::Start(_) | Event::End(_) => {}
    }
}

fn parse_code_block(kind: CodeBlockKind<'_>, events: &mut EventStream<'_>) -> Block {
    let language = match &kind {
        CodeBlockKind::Fenced(info) => info.split([' ', '\t']).find(|word| !word.is_empty()),
        CodeBlockKind::Indented => None,
    }
    .map(str::to_owned);
    let mut text = String::new();
    loop {
        match events.peek() {
            None | Some(Event::End(_)) => break,
            Some(Event::SoftBreak) | Some(Event::HardBreak) => {
                events.next();
                text.push('\n');
            }
            Some(_) => match events.next() {
                Some(Event::Text(chunk)) | Some(Event::Html(chunk)) => text.push_str(&chunk),
                Some(Event::Code(chunk)) => text.push_str(&chunk),
                _ => {}
            },
        }
    }
    consume_end(events);
    Block::Code {
        language,
        text: text.strip_suffix('\n').unwrap_or(&text).to_owned(),
    }
}

fn parse_raw_text(events: &mut EventStream<'_>) -> String {
    let mut text = String::new();
    loop {
        match events.next() {
            None | Some(Event::End(_)) => break,
            Some(Event::Text(chunk))
            | Some(Event::Html(chunk))
            | Some(Event::InlineHtml(chunk))
            | Some(Event::Code(chunk)) => text.push_str(&chunk),
            Some(Event::SoftBreak) | Some(Event::HardBreak) => text.push('\n'),
            Some(_) => {}
        }
    }
    text
}

fn parse_list(first: Option<u64>, events: &mut EventStream<'_>) -> Block {
    let mut items = Vec::new();
    loop {
        match events.peek() {
            None | Some(Event::End(_)) => break,
            Some(Event::Start(Tag::Item)) => {
                events.next();
                items.push(parse_inner_blocks(events));
            }
            // Anything else at item level is skipped to resynchronize.
            Some(_) => {
                events.next();
            }
        }
    }
    consume_end(events);
    Block::List {
        ordered: first.is_some(),
        start: first.unwrap_or(1),
        items,
    }
}

fn parse_table(alignments: Vec<Alignment>, events: &mut EventStream<'_>) -> Block {
    let alignments = alignments
        .into_iter()
        .map(TableAlign::from_pulldown)
        .collect();
    let mut head: Vec<Vec<Inline>> = Vec::new();
    let mut rows: Vec<Vec<Vec<Inline>>> = Vec::new();
    loop {
        match events.peek() {
            None | Some(Event::End(_)) => break,
            Some(Event::Start(Tag::TableHead)) => {
                events.next();
                // A GFM table has exactly one header row; anything more is
                // folded into the body so no cell is lost.
                let mut section = parse_table_section(events);
                if !section.is_empty() {
                    head = section.remove(0);
                }
                rows.extend(section);
                consume_end(events);
            }
            Some(Event::Start(Tag::TableRow)) => {
                rows.push(parse_table_row(events));
            }
            // Anything else at row level is skipped to resynchronize.
            Some(_) => {
                events.next();
            }
        }
    }
    consume_end(events);
    Block::Table {
        alignments,
        head,
        rows,
    }
}

fn parse_table_section(events: &mut EventStream<'_>) -> Vec<Vec<Vec<Inline>>> {
    // Head cells arrive bare (no TableRow wrapper) while body cells are
    // row-wrapped; accept both so neither form drops cells.
    let mut rows = Vec::new();
    let mut current: Option<Vec<Vec<Inline>>> = None;
    loop {
        match events.peek() {
            None | Some(Event::End(_)) => break,
            Some(Event::Start(Tag::TableRow)) => {
                if let Some(row) = current.take() {
                    rows.push(row);
                }
                rows.push(parse_table_row(events));
            }
            Some(Event::Start(Tag::TableCell)) => {
                events.next();
                current
                    .get_or_insert_with(Vec::new)
                    .push(parse_inlines(events));
            }
            Some(_) => {
                events.next();
            }
        }
    }
    if let Some(row) = current.take() {
        rows.push(row);
    }
    rows
}

fn parse_table_row(events: &mut EventStream<'_>) -> Vec<Vec<Inline>> {
    events.next(); // consume Start(TableRow)
    let mut cells = Vec::new();
    loop {
        match events.peek() {
            None | Some(Event::End(_)) => break,
            Some(Event::Start(Tag::TableCell)) => {
                events.next();
                cells.push(parse_inlines(events));
            }
            Some(_) => {
                events.next();
            }
        }
    }
    consume_end(events);
    cells
}

fn skip_container(events: &mut EventStream<'_>) {
    let mut depth = 0usize;
    for event in events.by_ref() {
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) if depth == 0 => break,
            Event::End(_) => depth -= 1,
            _ => {}
        }
    }
}

fn consume_end(events: &mut EventStream<'_>) {
    if matches!(events.peek(), Some(Event::End(_))) {
        events.next();
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// The printable text of an inline run, without styling.
pub fn plain_text(inlines: &[Inline]) -> String {
    let mut text = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(content) | Inline::Code(content) | Inline::Html(content) => {
                text.push_str(content)
            }
            Inline::Break => text.push('\n'),
            Inline::Emphasis(children) | Inline::Strong(children) | Inline::Strike(children) => {
                text.push_str(&plain_text(children))
            }
            Inline::Link { children, .. } => text.push_str(&plain_text(children)),
            Inline::Image { alt, .. } => text.push_str(alt),
            Inline::Task(checked) => text.push_str(if *checked { "[x] " } else { "[ ] " }),
        }
    }
    text
}

/// A GitHub-style heading slug for `#anchor` links.
pub fn slug(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut previous_dash = true;
    for ch in title.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                slug.push(lower);
            }
            previous_dash = false;
        } else if (ch.is_whitespace() || ch == '-' || ch == '_') && !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use super::*;

    fn parse_one(text: &str) -> Block {
        let blocks = parse_markdown(text);
        assert_eq!(blocks.len(), 1, "expected one block in {text:?}");
        blocks.into_iter().next().expect("one block")
    }

    #[test]
    fn heading_levels_and_paragraph_text() {
        assert!(matches!(
            parse_one("# Title\n"),
            Block::Heading { level: 1, .. }
        ));
        assert!(matches!(
            parse_one("###### Deep\n"),
            Block::Heading { level: 6, .. }
        ));
        let Block::Paragraph(inlines) = parse_one("Hello *world*.\n") else {
            panic!("expected a paragraph");
        };
        assert_eq!(plain_text(&inlines), "Hello world.");
    }

    #[test]
    fn soft_breaks_harden_like_the_wpf_pipeline() {
        let Block::Paragraph(inlines) = parse_one("first\nsecond\n") else {
            panic!("expected a paragraph");
        };
        assert!(inlines.contains(&Inline::Break));
        assert_eq!(plain_text(&inlines), "first\nsecond");
    }

    #[test]
    fn inline_styles_nest() {
        let Block::Paragraph(inlines) = parse_one("**bold `code` and *ital* and ~~gone~~**\n")
        else {
            panic!("expected a paragraph");
        };
        let [Inline::Strong(children)] = inlines.as_slice() else {
            panic!("expected one strong run, got {inlines:?}");
        };
        assert!(
            children
                .iter()
                .any(|child| matches!(child, Inline::Code(_)))
        );
        assert!(
            children
                .iter()
                .any(|child| matches!(child, Inline::Emphasis(_)))
        );
        assert!(
            children
                .iter()
                .any(|child| matches!(child, Inline::Strike(_)))
        );
    }

    #[test]
    fn links_images_and_autolinks() {
        let Block::Paragraph(inlines) =
            parse_one("[text](https://example.com/a) ![alt](pic.png)\n")
        else {
            panic!("expected a paragraph");
        };
        assert!(matches!(
            &inlines[0],
            Inline::Link { url, .. } if url == "https://example.com/a"
        ));
        assert!(matches!(
            &inlines[2],
            Inline::Image { url, alt } if url == "pic.png" && alt == "alt"
        ));
        let Block::Paragraph(auto) = parse_one("<https://example.com/>\n") else {
            panic!("expected a paragraph");
        };
        assert!(matches!(
            &auto[0],
            Inline::Link { url, .. } if url == "https://example.com/"
        ));
    }

    #[test]
    fn task_markers_survive() {
        let Block::List { items, .. } = parse_one("- [ ] open\n- [x] done\n") else {
            panic!("expected a list");
        };
        assert_eq!(items.len(), 2);
        let first = plain_text(
            items[0]
                .iter()
                .find_map(|block| match block {
                    Block::Paragraph(inlines) => Some(inlines.clone()),
                    _ => None,
                })
                .expect("item paragraph")
                .as_slice(),
        );
        assert_eq!(first, "[ ] open");
    }

    #[test]
    fn ordered_lists_keep_their_start_and_nesting() {
        let Block::List {
            ordered,
            start,
            items,
        } = parse_one("3. first\n4. second\n   - nested\n")
        else {
            panic!("expected a list");
        };
        assert!(ordered);
        assert_eq!(start, 3);
        assert_eq!(items.len(), 2);
        assert!(
            items[1]
                .iter()
                .any(|block| matches!(block, Block::List { ordered: false, .. })),
            "expected a nested unordered list in {items:?}"
        );
    }

    #[test]
    fn fenced_code_keeps_its_language() {
        let Block::Code { language, text } = parse_one("```rust\nfn main() {}\n```\n") else {
            panic!("expected a code block");
        };
        assert_eq!(language.as_deref(), Some("rust"));
        assert_eq!(text, "fn main() {}");
        assert!(matches!(parse_one("    indented\n"), Block::Code { .. }));
    }

    #[test]
    fn tables_keep_alignment_and_header() {
        let Block::Table {
            alignments,
            head,
            rows,
        } = parse_one("| a | b |\n| :-- | --: |\n| 1 | 2 |\n")
        else {
            panic!("expected a table");
        };
        assert_eq!(alignments, vec![TableAlign::Left, TableAlign::Right]);
        assert_eq!(head.len(), 2);
        assert_eq!(plain_text(&head[0]), "a");
        assert_eq!(rows.len(), 1);
        assert_eq!(plain_text(&rows[0][1]), "2");
    }

    #[test]
    fn quotes_rules_and_html_blocks() {
        let blocks = parse_markdown("> quoted\n\n---\n\n<div>x</div>\n");
        assert!(matches!(blocks[0], Block::Quote(_)));
        assert!(matches!(blocks[1], Block::Rule));
        assert!(matches!(blocks[2], Block::Html(_)));
    }

    #[test]
    fn front_matter_is_skipped_and_footnotes_render() {
        let blocks = parse_markdown("---\ntitle: x\n---\n\nText[^a].\n\n[^a]: Note.\n");
        assert!(
            !plain_text(&[Inline::Text(
                blocks
                    .iter()
                    .map(|block| match block {
                        Block::Paragraph(inlines) => plain_text(inlines),
                        _ => String::new(),
                    })
                    .collect::<String>(),
            )])
            .contains("title: x"),
            "front matter must not render: {blocks:?}"
        );
        let dumped = format!("{blocks:?}");
        assert!(dumped.contains("[^a]"), "footnote markers kept: {dumped}");
    }

    #[test]
    fn heading_slugs_match_github_rules() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("  Multiple   Spaces  "), "multiple-spaces");
        assert_eq!(slug("C++ & Rust"), "c-rust");
    }
}
