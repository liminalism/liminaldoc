//! Layout a [`Book`](docwrite_model::Book) with its masters, styles, and notes.

use std::collections::HashMap;
use std::sync::Arc;

use docwrite_model::{Block, BlockKind, Book, ChapterStart, NoteKind};

use crate::engine::{
    Alignment, Document, EditReport, Face, Geometry, LayoutHints, PageRules, Paragraph,
    ParagraphStyle, StyledRun,
};
use crate::error::TypesetError;

/// The Notes section's paragraphs: its heading is this id, and each endnote
/// is this bit with the note's id. Block ids never set it.
pub const ENDNOTES_ID: u64 = 1 << 62;

/// Paragraph ids with this bit set are chapter titles set from chapter
/// metadata; the rest of the id is the chapter's id. Block ids never set it.
pub const CHAPTER_TITLE_ID: u64 = 1 << 63;

/// Paginate `book` with the chapter template's page master and paragraph styles.
pub fn from_book(book: &Book, face: Face) -> Result<Document, TypesetError> {
    from_book_with(book, vec![face])
}

/// Paginate `book` with a font family: regular, bold, italic and bold
/// italic, in that order. Styles the family lacks are set in regular.
pub fn from_book_with(book: &Book, faces: Vec<Face>) -> Result<Document, TypesetError> {
    let (geometry, paragraphs, hints, _) = layout_inputs(book, &HashMap::new(), None);
    let mut document = Document::new_with(faces, geometry, paragraphs, hints)?;
    document.style_signature = style_signature(book);
    settle_contents(&mut document, book)?;
    Ok(document)
}

/// Bring `document` up to date with `book` after an edit.
///
/// Only paragraphs whose text, style or note changed are shaped again, and
/// pagination stops once the page breaks line up with the previous layout.
/// A change of page master or body size lays out the whole book.
pub fn update_from_book(document: &mut Document, book: &Book) -> Result<EditReport, TypesetError> {
    update_from_book_timed(document, book).map(|(report, _)| report)
}

/// Where an update spent its time, for the editor's frame trace.
#[derive(Clone, Copy, Debug, Default)]
pub struct UpdateTiming {
    /// Rebuilding the paragraph list from the book.
    pub inputs: std::time::Duration,
    /// Matching, reshaping and repaginating.
    pub apply: std::time::Duration,
    /// Settling contents page numbers, when the book has a contents.
    pub settle: std::time::Duration,
}

/// [`update_from_book`], reporting where the time went.
pub fn update_from_book_timed(
    document: &mut Document,
    book: &Book,
) -> Result<(EditReport, UpdateTiming), TypesetError> {
    let started = std::time::Instant::now();
    let pages = document.contents_pages.clone();
    let signature = style_signature(book);
    let styles_unchanged = document.style_signature == signature;
    document.style_signature = signature;
    let (paragraphs_before, by_id) = document.reuse_source();
    let (geometry, paragraphs, hints, reused) = layout_inputs(
        book,
        &pages,
        Some(Previous {
            paragraphs: paragraphs_before,
            by_id,
            styles_unchanged,
        }),
    );
    let inputs = started.elapsed();
    let started = std::time::Instant::now();
    let mut report = document.apply_with_reuse(geometry, paragraphs, hints, Some(&reused))?;
    let apply = started.elapsed();
    let started = std::time::Instant::now();
    if let Some(more) = settle_contents(document, book)? {
        report.pages_laid_out.extend(more.pages_laid_out);
        report.pages_laid_out.sort_unstable();
        report.pages_laid_out.dedup();
        report.paragraphs_shaped += more.paragraphs_shaped;
        report.page_count = more.page_count;
    }
    let settle = started.elapsed();
    Ok((
        report,
        UpdateTiming {
            inputs,
            apply,
            settle,
        },
    ))
}

/// The Bibliography section's heading is this id; its entries add a number.
pub const BIBLIOGRAPHY_ID: u64 = 1 << 59;

/// The Index section's heading is this id; its entries add a number.
pub const INDEX_ID: u64 = 1 << 58;

/// The Contents section's heading is this id; each entry is this bit with
/// its chapter's id. Block ids never set it.
pub const CONTENTS_ID: u64 = 1 << 61;

/// Set the table of contents' page numbers from where the chapters landed,
/// laying out again until they stop changing (entries of the same length
/// change nothing else). Returns what the last extra pass did, if any.
fn settle_contents(
    document: &mut Document,
    book: &Book,
) -> Result<Option<EditReport>, TypesetError> {
    if !book.has_contents() && !has_index(book) {
        document.contents_pages.clear();
        return Ok(None);
    }
    let mut last = None;
    for _ in 0..3 {
        let pages = chapter_pages(document, book);
        if pages == document.contents_pages {
            break;
        }
        document.contents_pages = pages.clone();
        let (paragraphs_before, by_id) = document.reuse_source();
        let (geometry, paragraphs, hints, reused) = layout_inputs(
            book,
            &pages,
            Some(Previous {
                paragraphs: paragraphs_before,
                by_id,
                styles_unchanged: true,
            }),
        );
        last = Some(document.apply_with_reuse(geometry, paragraphs, hints, Some(&reused))?);
    }
    Ok(last)
}

/// Index entries' page numbers are keyed by this bit with the block's id.
const INDEXED_BLOCK: u64 = 1 << 60;

fn has_index(book: &Book) -> bool {
    book.index_terms()
        .iter()
        .any(|term| !term.term.starts_with("cite:"))
}

/// The page each chapter opens on, by chapter id, and the page each indexed
/// block starts on, by `INDEXED_BLOCK | block id`.
fn chapter_pages(document: &Document, book: &Book) -> HashMap<u64, u32> {
    let mut pages = HashMap::new();
    for term in book.index_terms() {
        if term.term.starts_with("cite:") {
            continue;
        }
        if let Some(page) = document
            .paragraph_of(term.block.raw())
            .and_then(|index| document.page_of(index, 0))
        {
            pages.insert(INDEXED_BLOCK | term.block.raw(), page);
        }
    }
    for chapter in book.parts().iter().flat_map(|part| part.chapters()) {
        let opener = document
            .paragraph_of(CHAPTER_TITLE_ID | chapter.id().raw())
            .or_else(|| {
                chapter
                    .sections()
                    .iter()
                    .flat_map(|section| section.blocks())
                    .next()
                    .and_then(|block| document.paragraph_of(block.id().raw()))
            });
        if let Some(page) = opener.and_then(|index| document.page_of(index, 0)) {
            pages.insert(chapter.id().raw(), page);
        }
    }
    pages
}

/// What an update can hand back unchanged: the previous paragraph list,
/// which unchanged paragraphs are moved out of (a placeholder stays), its
/// id index, and whether the book's style definitions are as they were.
struct Previous<'a> {
    paragraphs: &'a mut Vec<Paragraph>,
    by_id: &'a crate::hash::SmallKeyMap<u64, usize>,
    styles_unchanged: bool,
}

/// The book's style, template and master definitions, as one string to
/// compare between layouts. A few dozen short records; cheaper to format
/// than to compare every paragraph's composed style.
fn style_signature(book: &Book) -> String {
    format!(
        "{:?}|{:?}|{:?}",
        book.paragraph_styles(),
        book.chapter_templates(),
        book.page_masters()
    )
}

/// The paginator's inputs for `book`, plus, for each paragraph, the index
/// in `previous` it was taken from when nothing about it changed.
///
/// Typing touches one block, but every keystroke rebuilds this list, and
/// on a thousand-page manuscript that used to mean copying every block's
/// text out of its rope and allocating its style, runs and running head
/// again — a third of the keypress budget spent constructing paragraphs
/// the paginator was then going to recognise as identical and discard.
/// Now a block whose text, style, runs and note match its previous
/// paragraph takes that paragraph over (no copy), and the paginator is
/// told so instead of comparing.
fn layout_inputs(
    book: &Book,
    contents_pages: &HashMap<u64, u32>,
    mut previous: Option<Previous<'_>>,
) -> (Geometry, Vec<Paragraph>, LayoutHints, Vec<Option<usize>>) {
    let mut reused: Vec<Option<usize>> = Vec::new();
    // One model style and its mapped form per style name: every block of
    // a long book used to clone both from the book's style list again.
    let mut styles: HashMap<&str, (docwrite_model::ParagraphStyle, ParagraphStyle)> =
        HashMap::new();
    let mut scratch = ParagraphStyle::default();
    let template = book
        .chapter_templates()
        .iter()
        .find(|template| template.name == "Chapter")
        .or_else(|| book.chapter_templates().first());
    let master_name = template
        .map(|template| template.master.as_str())
        .unwrap_or("Right Body");
    let master = book
        .page_masters()
        .iter()
        .find(|master| master.name == master_name)
        .or_else(|| book.page_masters().first());
    let body = named_style(
        book,
        template
            .map(|template| template.body_style.as_str())
            .unwrap_or("Body"),
    );
    let geometry = match master {
        Some(master) => Geometry {
            page_width: master.width_pt,
            page_height: master.height_pt,
            margin_top: master.margin_top,
            margin_bottom: master.margin_bottom,
            margin_inner: master.margin_inner,
            margin_outer: master.margin_outer,
            font_size: body.size_pt,
            leading: body.leading_pt,
        },
        None => Geometry::one_line_pages(),
    };
    // One set of page rules per chapter template: its master's margins and
    // furniture inside the book's trim, and its opener-folio choice.
    let masters: Vec<PageRules> = book
        .chapter_templates()
        .iter()
        .map(|template| {
            let own = book
                .page_masters()
                .iter()
                .find(|master| master.name == template.master)
                .or(master);
            PageRules {
                margin_top: own.map_or(geometry.margin_top, |m| m.margin_top),
                margin_bottom: own.map_or(geometry.margin_bottom, |m| m.margin_bottom),
                margin_inner: own.map_or(geometry.margin_inner, |m| m.margin_inner),
                margin_outer: own.map_or(geometry.margin_outer, |m| m.margin_outer),
                folio: own.is_some_and(|m| m.folio),
                running_head: own.is_some_and(|m| m.running_head),
                hide_opener_folio: !template.show_opener_folio,
            }
        })
        .collect();
    let default_master = book
        .chapter_templates()
        .iter()
        .position(|candidate| template.is_some_and(|template| template.name == candidate.name))
        .unwrap_or(0);
    let mut paragraphs = Vec::new();
    let mut recto_at = Vec::new();
    let mut break_at = Vec::new();
    let mut master_at = Vec::new();
    let mut heads = Vec::new();
    let mut footnote_count = 0usize;
    let mut endnotes: Vec<(u64, String)> = Vec::new();
    if book.has_contents() {
        // Contents: a heading, then each titled chapter with its page number
        // after a tab, which takes the line's slack to push it flush right.
        let mut title = map_style(&named_style(book, "Chapter Title"));
        title.name = "Chapter Title".into();
        title.keep_with_next = true;
        title.space_after = title.space_after.max(title.leading);
        paragraphs.push(Paragraph {
            id: CONTENTS_ID,
            revision: 0,
            text: "Contents".into(),
            style: title,
            ..Paragraph::default()
        });
        reused.push(None);
        recto_at.push(false);
        break_at.push(false);
        master_at.push(default_master);
        heads.push(Arc::from(""));
        let entry = map_style(&named_style(book, "Contents Entry"));
        for chapter in book.parts().iter().flat_map(|part| part.chapters()) {
            if chapter.title().is_empty() {
                continue;
            }
            let page = contents_pages
                .get(&chapter.id().raw())
                .map_or_else(|| "0".to_string(), u32::to_string);
            paragraphs.push(Paragraph {
                id: CONTENTS_ID | chapter.id().raw(),
                revision: 0,
                text: format!("{}\t{page}", chapter.title()),
                style: entry.clone(),
                ..Paragraph::default()
            });
            reused.push(None);
            recto_at.push(false);
            break_at.push(false);
            master_at.push(default_master);
            heads.push(Arc::from(""));
        }
    }
    for part in book.parts() {
        for chapter in part.chapters() {
            let template = book
                .chapter_templates()
                .iter()
                .find(|template| template.name == chapter.template());
            let recto = template.is_some_and(|template| template.start == ChapterStart::NextRecto);
            let master_index = book
                .chapter_templates()
                .iter()
                .position(|candidate| candidate.name == chapter.template())
                .unwrap_or(default_master);
            let head: Arc<str> = Arc::from(chapter.title());
            let chapter_start = paragraphs.len();
            let mut first_body = true;
            // The chapter's title is metadata for the Book Map, contents and
            // running heads. It is set on the page only when the template
            // asks (`show_title`) and the author has not written a Chapter
            // Title block of their own, so a title never prints twice.
            let titled = chapter
                .sections()
                .iter()
                .flat_map(|section| section.blocks())
                .next()
                .is_some_and(|block| matches!(block.kind(), BlockKind::ChapterTitle));
            if template.is_some_and(|template| template.show_title) && !titled && !head.is_empty() {
                let title_style = template
                    .map(|template| template.title_style.as_str())
                    .unwrap_or("Chapter Title");
                let mut style = map_style(&named_style(book, title_style));
                style.name = "Chapter Title".into();
                style.keep_with_next = true;
                style.space_after = style.space_after.max(style.leading);
                paragraphs.push(Paragraph {
                    id: CHAPTER_TITLE_ID | chapter.id().raw(),
                    revision: 0,
                    text: head.to_string(),
                    style,
                    note: None,
                    note_is_endnote: false,
                    note_mark: String::new(),
                    runs: Vec::new(),
                });
                reused.push(None);
                recto_at.push(false);
                break_at.push(false);
                master_at.push(master_index);
                heads.push(Arc::clone(&head));
            }
            for section in chapter.sections() {
                for block in section.blocks() {
                    let style_name = match block.kind() {
                        BlockKind::ChapterTitle => template
                            .map(|template| template.title_style.as_str())
                            .unwrap_or("Chapter Title"),
                        BlockKind::Image { .. } => "Image",
                        BlockKind::Caption => "Caption",
                        BlockKind::Subhead => "Subhead",
                        BlockKind::BlockQuote => "Block Quote",
                        BlockKind::Epigraph => "Epigraph",
                        BlockKind::Verse => "Verse",
                        BlockKind::SceneBreak => "Scene Break",
                        BlockKind::BibliographyEntry => "Bibliography Entry",
                        _ if first_body => {
                            first_body = false;
                            template
                                .map(|template| template.first_paragraph_style.as_str())
                                .unwrap_or("First Paragraph")
                        }
                        _ => template
                            .map(|template| template.body_style.as_str())
                            .unwrap_or("Body"),
                    };
                    let (model_style, base_style) = styles.entry(style_name).or_insert_with(|| {
                        let model = named_style(book, style_name);
                        let mapped = map_style(&model);
                        (model, mapped)
                    });
                    let id = block.id().raw();
                    let is_table = matches!(block.kind(), BlockKind::TableCell { .. });
                    let effective_name: &str = match block.kind() {
                        BlockKind::Image { .. } => "Image",
                        BlockKind::Caption => "Caption",
                        _ => style_name,
                    };
                    // Footnotes are numbered through the book and set at the
                    // foot of their page; endnotes are numbered separately and
                    // set in the Notes section after the last chapter.
                    let mut note = None;
                    let mut note_mark = String::new();
                    if let Some(note_id) = block.note()
                        && let Ok(body) = book.note(note_id)
                    {
                        if body.kind() == NoteKind::Endnote {
                            endnotes.push((note_id.raw(), body.text()));
                            note_mark = endnotes.len().to_string();
                        } else {
                            footnote_count += 1;
                            note_mark = footnote_count.to_string();
                            note = Some(format!("{footnote_count}\u{2002}{}", body.text()));
                        }
                    }
                    let ornament = matches!(block.kind(), BlockKind::SceneBreak)
                        && block.text().trim().is_empty();
                    // The block's style is composed in `scratch`, whose name
                    // buffer is reused block after block; it is cloned out
                    // only for a paragraph that is actually rebuilt. When the
                    // style definitions are unchanged it is not composed at
                    // all for a block whose paragraph can be kept: the
                    // revision stands for everything the block contributes
                    // and the style name for everything its position does.
                    let fast = previous.as_ref().is_some_and(|p| p.styles_unchanged) && !is_table;
                    let mut composed = false;
                    if !fast {
                        compose_style(&mut scratch, base_style, block, &geometry);
                        composed = true;
                    }
                    let kept = previous.as_mut().and_then(|previous| {
                        let old = *previous.by_id.get(&id)?;
                        let candidate = previous.paragraphs.get(old)?;
                        if candidate.id != id || candidate.revision != block.revision() {
                            return None;
                        }
                        debug_assert!(
                            ornament || block.text_eq(&candidate.text),
                            "block {id} changed without a new revision"
                        );
                        let same = candidate.note == note
                            && !candidate.note_is_endnote
                            && candidate.note_mark == note_mark
                            && if fast {
                                candidate.style.name == effective_name
                            } else {
                                candidate.style == scratch
                            };
                        same.then(|| {
                            (
                                old,
                                std::mem::replace(
                                    &mut previous.paragraphs[old],
                                    Paragraph::placeholder(),
                                ),
                            )
                        })
                    });
                    match kept {
                        Some((old, paragraph)) => {
                            paragraphs.push(paragraph);
                            reused.push(Some(old));
                        }
                        None => {
                            if !composed {
                                compose_style(&mut scratch, base_style, block, &geometry);
                            }
                            let mut text = block.text();
                            if ornament {
                                // An empty scene break shows as a centered ornament.
                                text = "*\u{2003}*\u{2003}*".into();
                            }
                            let runs = styled_runs(
                                &text,
                                block.runs(),
                                model_style.bold,
                                model_style.italic,
                            );
                            paragraphs.push(Paragraph {
                                id,
                                revision: block.revision(),
                                text,
                                style: scratch.clone(),
                                note,
                                note_is_endnote: false,
                                note_mark,
                                runs,
                            });
                            reused.push(None);
                        }
                    }
                    recto_at.push(false);
                    break_at.push(false);
                    master_at.push(master_index);
                    heads.push(Arc::clone(&head));
                }
            }
            // Every chapter opens a page; NextRecto also skips to a recto.
            if chapter_start > 0
                && let Some(flag) = break_at.get_mut(chapter_start)
            {
                *flag = true;
            }
            if recto && let Some(flag) = recto_at.get_mut(chapter_start) {
                *flag = true;
            }
        }
    }
    let mut back_matter: Vec<(u64, &str, Vec<Paragraph>)> = Vec::new();
    if !book.bibliography().is_empty() {
        // The bibliography: entries by author, the title in italic.
        let entry = map_style(&named_style(book, "Bibliography Entry"));
        let mut entries: Vec<_> = book.bibliography().iter().collect();
        entries.sort_by_key(|entry| entry.author.to_lowercase());
        let mut items = Vec::new();
        for (number, record) in entries.into_iter().enumerate() {
            let lead = if record.author.is_empty() {
                String::new()
            } else {
                format!("{}. ", record.author.trim_end_matches('.'))
            };
            let title = record.title.trim_end_matches('.').to_string();
            let tail = if record.issued.is_empty() {
                ".".to_string()
            } else {
                format!(". {}.", record.issued)
            };
            let text = format!("{lead}{title}{tail}");
            let runs = vec![StyledRun {
                start: lead.len(),
                end: lead.len() + title.len(),
                italic: true,
                ..StyledRun::default()
            }];
            items.push(Paragraph {
                id: BIBLIOGRAPHY_ID | (number as u64 + 1),
                revision: 0,
                text,
                style: entry.clone(),
                runs,
                ..Paragraph::default()
            });
        }
        back_matter.push((BIBLIOGRAPHY_ID, "Bibliography", items));
    }
    if has_index(book) {
        // The index: each term, then the pages it is on, flush right.
        let entry = map_style(&named_style(book, "Contents Entry"));
        let mut terms: Vec<(String, Vec<u32>)> = Vec::new();
        for term in book.index_terms() {
            if term.term.starts_with("cite:") {
                continue;
            }
            let page = contents_pages
                .get(&(INDEXED_BLOCK | term.block.raw()))
                .copied()
                .unwrap_or(0);
            match terms
                .iter_mut()
                .find(|(name, _)| name.eq_ignore_ascii_case(&term.term))
            {
                Some((_, pages)) => pages.push(page),
                None => terms.push((term.term.clone(), vec![page])),
            }
        }
        terms.sort_by_key(|(name, _)| name.to_lowercase());
        let items = terms
            .into_iter()
            .enumerate()
            .map(|(number, (name, mut pages))| {
                pages.sort_unstable();
                pages.dedup();
                let list: Vec<String> = pages.iter().map(u32::to_string).collect();
                Paragraph {
                    id: INDEX_ID | (number as u64 + 1),
                    revision: 0,
                    text: format!("{name}\t{}", list.join(", ")),
                    style: entry.clone(),
                    ..Paragraph::default()
                }
            })
            .collect();
        back_matter.push((INDEX_ID, "Index", items));
    }
    if !endnotes.is_empty() {
        // The Notes section: a heading that opens a page, then each endnote
        // as a paragraph, numbered as its reference mark is.
        let mut title = map_style(&named_style(book, "Chapter Title"));
        title.name = "Chapter Title".into();
        title.keep_with_next = true;
        title.space_after = title.space_after.max(title.leading);
        let notes_master = master_at.last().copied().unwrap_or(default_master);
        paragraphs.push(Paragraph {
            id: ENDNOTES_ID,
            revision: 0,
            text: "Notes".into(),
            style: title,
            ..Paragraph::default()
        });
        reused.push(None);
        recto_at.push(false);
        break_at.push(true);
        master_at.push(notes_master);
        heads.push(Arc::from("Notes"));
        let mut entry = map_style(&body);
        entry.first_indent = 0.0;
        entry.hyphenate = false;
        for (number, (id, text)) in endnotes.iter().enumerate() {
            paragraphs.push(Paragraph {
                id: ENDNOTES_ID | id,
                revision: 0,
                text: format!("{}. {text}", number + 1),
                style: entry.clone(),
                ..Paragraph::default()
            });
            reused.push(None);
            recto_at.push(false);
            break_at.push(false);
            master_at.push(notes_master);
            heads.push(Arc::from("Notes"));
        }
    }
    for (heading_id, heading, items) in back_matter {
        let mut title = map_style(&named_style(book, "Chapter Title"));
        title.name = "Chapter Title".into();
        title.keep_with_next = true;
        title.space_after = title.space_after.max(title.leading);
        let section_master = master_at.last().copied().unwrap_or(default_master);
        paragraphs.push(Paragraph {
            id: heading_id,
            revision: 0,
            text: heading.into(),
            style: title,
            ..Paragraph::default()
        });
        reused.push(None);
        recto_at.push(false);
        break_at.push(true);
        master_at.push(section_master);
        heads.push(Arc::from(heading));
        for item in items {
            paragraphs.push(item);
            reused.push(None);
            recto_at.push(false);
            break_at.push(false);
            master_at.push(section_master);
            heads.push(Arc::from(heading));
        }
    }
    if paragraphs.is_empty() {
        paragraphs.push(Paragraph {
            id: u64::MAX,
            revision: 0,
            text: String::new(),
            style: map_style(&body),
            ..Paragraph::default()
        });
        reused.push(None);
        recto_at.push(false);
        break_at.push(false);
        master_at.push(default_master);
        heads.push(Arc::from(""));
    }
    let hints = LayoutHints {
        recto_at,
        break_at,
        masters,
        master_at,
        heads,
        folio: master.is_some_and(|master| master.folio),
        hide_opener_folio: template.is_some_and(|template| !template.show_opener_folio),
        running_head: master.is_some_and(|master| master.running_head),
        facing: master.is_some_and(|master| master.facing),
    };
    debug_assert_eq!(reused.len(), paragraphs.len());
    (geometry, paragraphs, hints, reused)
}

/// The model's runs (character ranges) as byte ranges over `text`, keeping
/// only runs that change how text is set.
fn styled_runs(
    text: &str,
    runs: &[docwrite_model::Run],
    style_bold: bool,
    style_italic: bool,
) -> Vec<StyledRun> {
    if style_bold || style_italic {
        // A bold or italic style sets every run so; italic runs inside an
        // italic style turn roman, for emphasis within an epigraph.
        let mut spans = styled_runs(text, runs, false, false);
        if spans.is_empty() && !text.is_empty() {
            spans.push(StyledRun {
                start: 0,
                end: text.len(),
                ..StyledRun::default()
            });
        }
        let mut covered = 0;
        let mut out = Vec::new();
        for span in spans {
            if span.start > covered {
                out.push(StyledRun {
                    start: covered,
                    end: span.start,
                    ..StyledRun::default()
                });
            }
            covered = covered.max(span.end);
            out.push(span);
        }
        if covered < text.len() {
            out.push(StyledRun {
                start: covered,
                end: text.len(),
                ..StyledRun::default()
            });
        }
        for span in &mut out {
            span.bold |= style_bold;
            span.italic ^= style_italic;
        }
        return out;
    }
    if runs.iter().all(|run| {
        !run.marks.bold
            && !run.marks.italic
            && !run.marks.small_caps
            && run.marks.script == docwrite_model::Script::Normal
    }) {
        return Vec::new();
    }
    let mut bytes: Vec<usize> = text.char_indices().map(|(byte, _)| byte).collect();
    bytes.push(text.len());
    let at = |chars: usize| bytes.get(chars).copied().unwrap_or(text.len());
    runs.iter()
        .filter(|run| run.start < run.end)
        .map(|run| StyledRun {
            start: at(run.start),
            end: at(run.end),
            bold: run.marks.bold,
            italic: run.marks.italic,
            small_caps: run.marks.small_caps,
            script: match run.marks.script {
                docwrite_model::Script::Super => 1,
                docwrite_model::Script::Sub => -1,
                docwrite_model::Script::Normal => 0,
            },
        })
        .collect()
}

fn named_style(book: &Book, name: &str) -> docwrite_model::ParagraphStyle {
    book.paragraph_styles()
        .iter()
        .find(|style| style.name == name)
        .cloned()
        .unwrap_or_else(|| docwrite_model::ParagraphStyle {
            name: name.into(),
            first_indent_pt: 0.0,
            hyphenate: false,
            ..docwrite_model::ParagraphStyle::default()
        })
}

fn map_align(align: docwrite_model::Align) -> Alignment {
    match align {
        docwrite_model::Align::Left => Alignment::Left,
        docwrite_model::Align::Justify => Alignment::Justify,
        docwrite_model::Align::Center => Alignment::Center,
        docwrite_model::Align::Right => Alignment::Right,
    }
}

/// A block's style: its named style, then the block's own alignment, the
/// kind-derived names, and a table cell's measure.
fn compose_style(
    style: &mut ParagraphStyle,
    base: &ParagraphStyle,
    block: &Block,
    geometry: &Geometry,
) {
    style.clone_from(base);
    if let Some(align) = block.align() {
        style.align = map_align(align);
    }
    if matches!(block.kind(), BlockKind::Image { .. }) {
        style.name.clear();
        style.name.push_str("Image");
    }
    if matches!(block.kind(), BlockKind::Caption) {
        style.name.clear();
        style.name.push_str("Caption");
    }
    if let BlockKind::TableCell {
        table,
        row,
        column,
        columns,
        span,
        widths,
        ..
    } = block.kind()
    {
        let columns = (*columns).max(1);
        let total = widths.iter().map(|w| *w as f32).sum::<f32>().max(1.0);
        let fraction = |start: usize, end: usize| -> f32 {
            if widths.len() == columns {
                widths[start.min(columns)..end.min(columns)]
                    .iter()
                    .map(|w| *w as f32)
                    .sum::<f32>()
                    / total
            } else {
                end.saturating_sub(start) as f32 / columns as f32
            }
        };
        let width = geometry.content_width();
        style.table_row = Some((*table, *row));
        style.left_indent = fraction(0, *column) * width + 3.0;
        style.right_indent = fraction(column.saturating_add(*span), columns) * width + 3.0;
        style.first_indent = 0.0;
        style.align = Alignment::Left;
        style.space_before = 3.0;
        style.space_after = 3.0;
        style.widow_orphan = false;
        style.keep_with_next = false;
    }
}

fn map_style(style: &docwrite_model::ParagraphStyle) -> ParagraphStyle {
    ParagraphStyle {
        hyphenate: style.hyphenate,
        drop_cap_lines: style.drop_cap_lines,
        small_caps: style.small_caps,
        oldstyle_figures: style.oldstyle_figures,
        font_size: style.size_pt,
        leading: style.leading_pt,
        first_indent: style.first_indent_pt,
        name: style.name.clone(),
        widow_orphan: true,
        align: map_align(style.align),
        left_indent: style.left_indent_pt,
        right_indent: style.right_indent_pt,
        space_before: style.space_before_pt,
        space_after: style.space_after_pt,
        ..ParagraphStyle::default()
    }
}
