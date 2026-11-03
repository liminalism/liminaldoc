//! The semantic book: parts, chapters, sections, blocks and notes.
//!
//! Pages are not stored. [`Book::plain_text`] joins blocks with newlines only
//! so callers can see paragraph boundaries; a page boundary is wherever
//! typesetting later cuts this same sequence.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};

use ropey::Rope;

use crate::error::ModelError;
use crate::ids::{BlockId, ChapterId, IdGen, NoteId, PartId, SectionId};
use crate::publish::Align;
use crate::runs::{self, Run, RunMarks};

/// Where a block sits. Indices are valid only until the next structural edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BlockLoc {
    pub part: usize,
    pub chapter: usize,
    pub section: usize,
    pub block: usize,
}

/// Kind of block. Footnote and endnote *references* use [`Block::note`];
/// the body of the note is a [`Note`] on the book.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BlockKind {
    /// An editable table cell. Coordinates are zero based; continuation
    /// paragraphs in the same cell carry the same coordinates.
    TableCell {
        /// Source table identity, stable within this book.
        table: u64,
        /// Row position.
        row: usize,
        /// Column position.
        column: usize,
        /// Total columns in the table.
        columns: usize,
        /// Number of columns occupied by a merged cell.
        span: usize,
        /// Source vertical merge extent; currently reflowed at its top row.
        row_span: usize,
        /// Relative column widths, in thousandths of a point.
        widths: Vec<u32>,
    },
    /// A body paragraph.
    #[default]
    Body,
    /// The chapter title.
    ChapterTitle,
    /// A subhead.
    Subhead,
    /// A block quote.
    BlockQuote,
    /// An epigraph.
    Epigraph,
    /// An image. `asset` is a path inside the book bundle, when one is known.
    Image {
        /// Bundle-relative asset path.
        asset: Option<String>,
    },
    /// A caption.
    Caption,
    /// Verse.
    Verse,
    /// One bibliography entry.
    BibliographyEntry,
    /// A section or scene break. The text is usually empty.
    SceneBreak,
}

/// Footnote or endnote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NoteKind {
    /// Placed on the page that references it, and continued when it does not fit.
    Footnote,
    /// Placed on the page that references it, and continued when it does not fit.
    Endnote,
}

/// A note body. Pagination reads it through `from_book`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Note {
    id: NoteId,
    kind: NoteKind,
    #[serde(with = "rope_text")]
    text: Rope,
}

impl Note {
    /// Stable id.
    pub fn id(&self) -> NoteId {
        self.id
    }

    /// Footnote or endnote.
    pub fn kind(&self) -> NoteKind {
        self.kind
    }

    /// Note body as a string.
    pub fn text(&self) -> String {
        self.text.to_string()
    }

    pub(crate) fn from_text(id: NoteId, kind: NoteKind, text: &str) -> Self {
        Self {
            id,
            kind,
            text: Rope::from_str(text),
        }
    }

    pub(crate) fn set_text(&mut self, text: &str) {
        self.text = Rope::from_str(text);
    }
}

/// Process-wide source of block revisions: every change to any block takes
/// the next value, so one `(id, revision)` pair names one content for the
/// life of the process. Loaded blocks start at zero and move off it on
/// their first edit.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

/// One block: a rope of text, the runs over it, and an optional note slot.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Block {
    id: BlockId,
    kind: BlockKind,
    #[serde(with = "rope_text")]
    text: Rope,
    runs: Vec<Run>,
    note: Option<NoteId>,
    /// Direct alignment, as the alignment keys set it. `None` follows the
    /// paragraph style. Older bundles predate this field and load as `None`.
    #[serde(default)]
    align: Option<Align>,
    /// Changes whenever the text, runs, kind, note or alignment change.
    /// Not persisted and not part of equality: it is a cache key, not
    /// content. The typesetter keeps a paragraph whose block still has the
    /// revision it was built from instead of comparing the text, which a
    /// rope cannot do without measuring every leaf again.
    #[serde(skip)]
    revision: u64,
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.kind == other.kind
            && self.text == other.text
            && self.runs == other.runs
            && self.note == other.note
            && self.align == other.align
    }
}

impl Eq for Block {}

impl Block {
    /// The content revision: equal revisions on equal ids mean equal
    /// text, runs, kind, note and alignment.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn touch(&mut self) {
        self.revision = next_revision();
    }

    /// Stable id. Never reused.
    pub fn id(&self) -> BlockId {
        self.id
    }

    /// Structural kind.
    pub fn kind(&self) -> &BlockKind {
        &self.kind
    }

    /// Character length.
    pub fn len_chars(&self) -> usize {
        self.text.len_chars()
    }

    /// Block text.
    pub fn text(&self) -> String {
        self.text.to_string()
    }

    /// Whether the block's text is exactly `text`, without building a
    /// `String`: the typesetter asks this for every block on every
    /// keystroke to find the ones it can keep.
    pub fn text_eq(&self, text: &str) -> bool {
        // Walk the leaves by byte position and never ask the rope for its
        // length: `Rope == str`, the chunk iterator and even `len_bytes`
        // re-measure a leaf's text (characters, line breaks) before a byte
        // is compared, which on a thousand-page book was most of what a
        // keystroke cost. A positional chunk lookup only adds up the
        // measurements the tree already holds.
        let mut offset = 0;
        loop {
            let Some((chunk, chunk_start, _, _)) = self.text.get_chunk_at_byte(offset) else {
                return false;
            };
            let Some(piece) = chunk.get(offset - chunk_start..) else {
                return false;
            };
            if piece.is_empty() {
                // The rope has ended; so must the text.
                return offset == text.len();
            }
            if !text
                .get(offset..)
                .is_some_and(|rest| rest.starts_with(piece))
            {
                return false;
            }
            offset += piece.len();
        }
    }

    /// Styled spans covering [`Self::text`].
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// Footnote or endnote anchored to this block, if any.
    pub fn note(&self) -> Option<NoteId> {
        self.note
    }

    /// Direct alignment override, if the alignment keys set one. `None`
    /// follows the block's paragraph style.
    pub fn align(&self) -> Option<Align> {
        self.align
    }

    pub(crate) fn text_rope(&self) -> &Rope {
        &self.text
    }

    pub(crate) fn text_rope_mut(&mut self) -> &mut Rope {
        self.touch();
        &mut self.text
    }

    pub(crate) fn runs_mut(&mut self) -> &mut Vec<Run> {
        self.touch();
        &mut self.runs
    }

    pub(crate) fn set_kind(&mut self, kind: BlockKind) {
        self.kind = kind;
        self.touch();
    }

    pub(crate) fn set_note(&mut self, note: Option<NoteId>) {
        self.note = note;
        self.touch();
    }

    pub(crate) fn set_align(&mut self, align: Option<Align>) {
        self.align = align;
        self.touch();
    }

    pub(crate) fn new(id: BlockId, kind: BlockKind, text: &str, runs: Vec<Run>) -> Self {
        Self {
            id,
            kind,
            text: Rope::from_str(text),
            runs,
            note: None,
            align: None,
            revision: next_revision(),
        }
    }

    pub(crate) fn empty(id: BlockId, kind: BlockKind) -> Self {
        Self::new(id, kind, "", runs::covering(0, RunMarks::default()))
    }
}

/// A section: an optional heading and the blocks under it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Section {
    id: SectionId,
    heading: String,
    blocks: Vec<Block>,
}

impl Section {
    /// Stable id.
    pub fn id(&self) -> SectionId {
        self.id
    }

    /// Heading text. Empty when the section has no heading of its own.
    pub fn heading(&self) -> &str {
        &self.heading
    }

    /// Blocks in reading order.
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub(crate) fn blocks_mut(&mut self) -> &mut Vec<Block> {
        &mut self.blocks
    }
}

/// A chapter.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Chapter {
    id: ChapterId,
    title: String,
    /// Chapter template name. Changing that template changes every chapter that uses it.
    template: String,
    sections: Vec<Section>,
}

impl Chapter {
    /// Stable id.
    pub fn id(&self) -> ChapterId {
        self.id
    }

    /// Chapter title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Template this chapter was opened from.
    pub fn template(&self) -> &str {
        &self.template
    }

    /// Sections in reading order.
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub(crate) fn sections_mut(&mut self) -> &mut Vec<Section> {
        &mut self.sections
    }

    pub(crate) fn set_title(&mut self, title: String) {
        self.title = title;
    }

    pub(crate) fn set_template(&mut self, template: String) {
        self.template = template;
    }
}

/// A part. Front matter, body and back matter are parts.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Part {
    id: PartId,
    title: String,
    chapters: Vec<Chapter>,
}

impl Part {
    /// Stable id.
    pub fn id(&self) -> PartId {
        self.id
    }

    /// Part title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Chapters in reading order.
    pub fn chapters(&self) -> &[Chapter] {
        &self.chapters
    }

    pub(crate) fn chapters_mut(&mut self) -> &mut Vec<Chapter> {
        &mut self.chapters
    }
}

/// A caret position. `offset` is a character index into the block, and may
/// equal the block length (the caret sits after the last character).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Position {
    /// Block that contains the caret.
    pub block: BlockId,
    /// Character offset inside the block.
    pub offset: usize,
}

impl Position {
    /// A position in `block`. The book validates the offset when it is used.
    pub fn new(block: BlockId, offset: usize) -> Self {
        Self { block, offset }
    }
}

/// An anchor and a focus. The focus is the end that motions move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Selection {
    /// The end that stays put while the selection is extended.
    pub anchor: Position,
    /// The end that [`crate::Motion`] moves.
    pub focus: Position,
}

impl Selection {
    /// A caret with no selected characters.
    pub fn collapsed(position: Position) -> Self {
        Self {
            anchor: position,
            focus: position,
        }
    }

    /// True when anchor and focus are the same position.
    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.focus
    }
}

/// The manuscript.
#[derive(Clone, Debug)]
pub struct Book {
    title: String,
    pub(crate) ids: IdGen,
    parts: Vec<Part>,
    pub(crate) stylesheet: crate::publish::Stylesheet,
    notes: BTreeMap<NoteId, Note>,
    pub(crate) index: HashMap<BlockId, BlockLoc>,
    pub(crate) selection: Selection,
    /// Marks the next typed text takes, set by toggling a mark with nothing
    /// selected. Cleared when the caret moves.
    pub(crate) typing_marks: Option<RunMarks>,
    pub(crate) undo: Vec<crate::edit::Transaction>,
    pub(crate) redo: Vec<crate::edit::Transaction>,
}

impl Book {
    /// A plain document with one implicit, untitled flow and one empty body block.
    /// The caret is at the start of that block.
    pub fn new(title: impl Into<String>) -> Self {
        let mut ids = IdGen::new();
        let block_id = ids.block();
        let block = Block::empty(block_id, BlockKind::Body);
        let section = Section {
            id: ids.section(),
            heading: String::new(),
            blocks: vec![block],
        };
        let chapter = Chapter {
            id: ids.chapter(),
            title: String::new(),
            template: "Chapter".to_string(),
            sections: vec![section],
        };
        let part = Part {
            id: ids.part(),
            title: "Body".to_string(),
            chapters: vec![chapter],
        };
        let selection = Selection::collapsed(Position::new(block_id, 0));
        let mut book = Self {
            title: title.into(),
            ids,
            parts: vec![part],
            stylesheet: crate::publish::Stylesheet::standard(),
            notes: BTreeMap::new(),
            index: HashMap::new(),
            selection,
            typing_marks: None,
            undo: Vec::new(),
            redo: Vec::new(),
        };
        book.reindex();
        book
    }

    /// Book title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Parts in reading order.
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    pub(crate) fn parts_mut(&mut self) -> &mut [Part] {
        &mut self.parts
    }

    pub(crate) fn parts_vec_mut(&mut self) -> &mut Vec<Part> {
        &mut self.parts
    }

    pub(crate) fn chapter_mut(&mut self, id: ChapterId) -> Result<&mut Chapter, ModelError> {
        self.parts
            .iter_mut()
            .flat_map(|part| part.chapters.iter_mut())
            .find(|chapter| chapter.id == id)
            .ok_or(ModelError::UnknownChapter(id))
    }

    /// Part and in-part index of chapter `id`.
    pub(crate) fn chapter_position(&self, id: ChapterId) -> Result<(usize, usize), ModelError> {
        self.chapter_index(id)
    }

    /// Current selection.
    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// True when undo would restore an earlier manuscript.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// True when redo would reapply an undone edit.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Block ids in reading order.
    pub fn block_ids(&self) -> Vec<BlockId> {
        self.blocks().map(Block::id).collect()
    }

    /// Every block in reading order.
    pub fn blocks(&self) -> impl Iterator<Item = &Block> {
        self.parts
            .iter()
            .flat_map(|part| part.chapters.iter())
            .flat_map(|chapter| chapter.sections.iter())
            .flat_map(|section| section.blocks.iter())
    }

    /// The block, if the id is still in the book.
    pub fn block(&self, id: BlockId) -> Result<&Block, ModelError> {
        self.block_ref(id)
    }

    /// Character length of a block.
    pub fn block_len(&self, id: BlockId) -> Result<usize, ModelError> {
        Ok(self.block_ref(id)?.len_chars())
    }

    /// Note body.
    pub fn note(&self, id: NoteId) -> Result<&Note, ModelError> {
        self.notes.get(&id).ok_or(ModelError::UnknownNote(id))
    }

    /// Blocks joined by newlines. This is the paragraph sequence, not pages.
    ///
    /// ```
    /// use docwrite_model::Book;
    /// let mut book = Book::new("Essay");
    /// assert!(book.insert("Hello.").is_ok());
    /// assert_eq!(book.plain_text(), "Hello.");
    /// ```
    pub fn plain_text(&self) -> String {
        self.blocks()
            .map(Block::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether the book has a structure worth mapping: several chapters, a
    /// named section, or a title or subhead the author wrote. A fresh
    /// single-chapter manuscript has none.
    pub fn has_structure(&self) -> bool {
        let mut chapters = self.parts.iter().flat_map(|part| part.chapters.iter());
        let count = chapters.clone().count();
        count > 1
            || chapters.any(|chapter| {
                !chapter.title.is_empty()
                    || chapter.sections.iter().any(|section| {
                        !section.heading.is_empty()
                            || section.blocks.iter().any(|block| {
                                matches!(block.kind(), BlockKind::ChapterTitle | BlockKind::Subhead)
                            })
                    })
            })
    }

    /// Give chapter `id` a new title. The title heads the chapter's opener
    /// page, its running heads, the Book Map and every export. One undo step.
    pub fn rename_chapter(
        &mut self,
        id: ChapterId,
        title: impl Into<String>,
    ) -> Result<(), ModelError> {
        let new = title.into();
        let old = self.chapter_mut(id)?.title().to_string();
        if old == new {
            return Ok(());
        }
        self.record(crate::edit::Change::RenameChapter { id, old, new })
    }

    /// Start a named chapter at `position`, moving the following text into it.
    /// At the flow's first position this names the existing flow. Otherwise
    /// the boundary and any paragraph split are one undoable transaction.
    pub fn start_chapter_at(
        &mut self,
        position: Position,
        title: impl Into<String>,
    ) -> Result<ChapterId, ModelError> {
        self.ensure_position(position)?;
        let loc = self.loc(position.block)?;
        let before = self.parts[loc.part].chapters[loc.chapter].clone();
        let title = title.into();
        if loc.section == 0 && loc.block == 0 && position.offset == 0 {
            self.rename_chapter(before.id, title)?;
            return Ok(before.id);
        }
        let mut leading = before.clone();
        let sections = if loc.block == 0 && position.offset == 0 {
            leading.sections.split_off(loc.section)
        } else {
            let rest = leading.sections.split_off(loc.section + 1);
            let mut blocks = leading.sections[loc.section].blocks.split_off(loc.block);
            if position.offset > 0 {
                let mut head = blocks.remove(0);
                let text = head.text();
                let byte = crate::nav::byte_index(&text, position.offset);
                let len = head.len_chars();
                let mut tail = Block::new(
                    self.ids.block(),
                    head.kind.clone(),
                    &text[byte..],
                    runs::slice_runs(&head.runs, position.offset, len),
                );
                tail.align = head.align;
                head.text.remove(position.offset..len);
                head.runs = runs::slice_runs(&head.runs, 0, position.offset);
                head.touch();
                leading.sections[loc.section].blocks.push(head);
                blocks.insert(0, tail);
            }
            let section = Section {
                id: self.ids.section(),
                heading: String::new(),
                blocks,
            };
            std::iter::once(section).chain(rest).collect()
        };
        let id = self.ids.chapter();
        let trailing = Chapter {
            id,
            title,
            template: before.template.clone(),
            sections,
        };
        let first = trailing
            .sections
            .iter()
            .flat_map(|section| &section.blocks)
            .next()
            .ok_or(ModelError::Inconsistent("chapter has no text block"))?
            .id;
        self.record_at(
            crate::edit::Change::SplitChapter {
                part: loc.part,
                index: loc.chapter,
                before,
                leading,
                trailing,
            },
            Selection::collapsed(Position::new(first, 0)),
        )?;
        Ok(id)
    }

    /// Add a part titled `title` after the last. Returns its id.
    ///
    /// Structural builders are undoable transactions: ids are allocated once,
    /// so undo then redo restores the same ids.
    pub fn add_part(&mut self, title: impl Into<String>) -> PartId {
        let id = self.ids.part();
        let part = Part {
            id,
            title: title.into(),
            chapters: Vec::new(),
        };
        let index = self.parts.len();
        let _ = self.record(crate::edit::Change::InsertPart { index, part });
        id
    }

    /// Append a chapter with one empty section and one empty body block.
    ///
    /// See [`Self::add_part`] for the undo behavior.
    pub fn add_chapter(
        &mut self,
        part: PartId,
        title: impl Into<String>,
    ) -> Result<ChapterId, ModelError> {
        let part_index = self.part_index(part)?;
        let chapter_id = self.ids.chapter();
        let block_id = self.ids.block();
        let section = Section {
            id: self.ids.section(),
            heading: String::new(),
            blocks: vec![Block::empty(block_id, BlockKind::Body)],
        };
        let chapter = Chapter {
            id: chapter_id,
            title: title.into(),
            template: "Chapter".to_string(),
            sections: vec![section],
        };
        let index = self.parts[part_index].chapters.len();
        self.record(crate::edit::Change::InsertChapter {
            part: part_index,
            index,
            chapter,
        })?;
        Ok(chapter_id)
    }

    /// Append a section with one empty body block.
    ///
    /// See [`Self::add_part`] for the undo behavior.
    pub fn add_section(
        &mut self,
        chapter: ChapterId,
        heading: impl Into<String>,
    ) -> Result<SectionId, ModelError> {
        let (part_index, chapter_index) = self.chapter_index(chapter)?;
        let section_id = self.ids.section();
        let block_id = self.ids.block();
        let section = Section {
            id: section_id,
            heading: heading.into(),
            blocks: vec![Block::empty(block_id, BlockKind::Body)],
        };
        let index = self.parts[part_index].chapters[chapter_index]
            .sections
            .len();
        self.record(crate::edit::Change::InsertSection {
            part: part_index,
            chapter: chapter_index,
            index,
            section,
        })?;
        Ok(section_id)
    }

    /// The invariant check an edit ends with. The full walk below visits
    /// every block of the book and rebuilds its index, which on a
    /// thousand-page manuscript is about a millisecond spent on every
    /// keystroke verifying the model against itself; debug builds and the
    /// test suite keep it, a release build checks only what the edit could
    /// have left dangling: the selection.
    pub(crate) fn check_after_edit(&self) -> Result<(), ModelError> {
        if cfg!(debug_assertions) {
            return self.check_consistency();
        }
        self.ensure_position(self.selection.anchor)?;
        self.ensure_position(self.selection.focus)
    }

    /// Ids, runs, the selection and note links agree with the tree.
    pub fn check_consistency(&self) -> Result<(), ModelError> {
        let mut seen = HashMap::<BlockId, BlockLoc>::new();
        for (part_index, part) in self.parts.iter().enumerate() {
            for (chapter_index, chapter) in part.chapters.iter().enumerate() {
                for (section_index, section) in chapter.sections.iter().enumerate() {
                    for (block_index, block) in section.blocks.iter().enumerate() {
                        let loc = BlockLoc {
                            part: part_index,
                            chapter: chapter_index,
                            section: section_index,
                            block: block_index,
                        };
                        if seen.insert(block.id, loc).is_some() {
                            return Err(ModelError::Inconsistent("duplicate block id"));
                        }
                        if !runs::runs_cover(&block.runs, block.len_chars()) {
                            return Err(ModelError::Inconsistent("runs do not cover text"));
                        }
                        if let Some(note) = block.note
                            && !self.notes.contains_key(&note)
                        {
                            return Err(ModelError::Inconsistent("block note is missing"));
                        }
                    }
                }
            }
        }
        if seen.len() != self.index.len() {
            return Err(ModelError::Inconsistent("index length"));
        }
        for (id, loc) in &seen {
            match self.index.get(id) {
                Some(indexed) if indexed == loc => {}
                _ => return Err(ModelError::Inconsistent("index location")),
            }
        }
        self.ensure_position(self.selection.anchor)?;
        self.ensure_position(self.selection.focus)?;
        Ok(())
    }

    pub(crate) fn block_ref(&self, id: BlockId) -> Result<&Block, ModelError> {
        let loc = self.loc(id)?;
        self.parts
            .get(loc.part)
            .and_then(|part| part.chapters.get(loc.chapter))
            .and_then(|chapter| chapter.sections.get(loc.section))
            .and_then(|section| section.blocks.get(loc.block))
            .ok_or(ModelError::UnknownBlock(id))
    }

    pub(crate) fn block_mut(&mut self, id: BlockId) -> Result<&mut Block, ModelError> {
        let loc = self.loc(id)?;
        self.parts
            .get_mut(loc.part)
            .and_then(|part| part.chapters.get_mut(loc.chapter))
            .and_then(|chapter| chapter.sections.get_mut(loc.section))
            .and_then(|section| section.blocks.get_mut(loc.block))
            .ok_or(ModelError::UnknownBlock(id))
    }

    pub(crate) fn section_mut(&mut self, loc: BlockLoc) -> Result<&mut Section, ModelError> {
        self.parts
            .get_mut(loc.part)
            .and_then(|part| part.chapters.get_mut(loc.chapter))
            .and_then(|chapter| chapter.sections.get_mut(loc.section))
            .ok_or(ModelError::EditConflict)
    }

    pub(crate) fn loc(&self, id: BlockId) -> Result<BlockLoc, ModelError> {
        self.index
            .get(&id)
            .copied()
            .ok_or(ModelError::UnknownBlock(id))
    }

    pub(crate) fn reindex(&mut self) {
        self.index.clear();
        for (part_index, part) in self.parts.iter().enumerate() {
            for (chapter_index, chapter) in part.chapters.iter().enumerate() {
                for (section_index, section) in chapter.sections.iter().enumerate() {
                    for (block_index, block) in section.blocks.iter().enumerate() {
                        self.index.insert(
                            block.id,
                            BlockLoc {
                                part: part_index,
                                chapter: chapter_index,
                                section: section_index,
                                block: block_index,
                            },
                        );
                    }
                }
            }
        }
    }

    pub(crate) fn ensure_position(&self, position: Position) -> Result<(), ModelError> {
        let len = self.block_ref(position.block)?.len_chars();
        if position.offset > len {
            Err(ModelError::OffsetOutOfRange {
                block: position.block,
                offset: position.offset,
                len,
            })
        } else {
            Ok(())
        }
    }

    pub(crate) fn cmp_pos(
        &self,
        left: Position,
        right: Position,
    ) -> Result<std::cmp::Ordering, ModelError> {
        if left.block == right.block {
            return Ok(left.offset.cmp(&right.offset));
        }
        let left_loc = self.loc(left.block)?;
        let right_loc = self.loc(right.block)?;
        Ok((
            left_loc.part,
            left_loc.chapter,
            left_loc.section,
            left_loc.block,
            left.offset,
        )
            .cmp(&(
                right_loc.part,
                right_loc.chapter,
                right_loc.section,
                right_loc.block,
                right.offset,
            )))
    }

    pub(crate) fn neighbor(
        &self,
        id: BlockId,
        direction: crate::edit::Direction,
    ) -> Result<Option<BlockId>, ModelError> {
        let ids = self.block_ids();
        let index = ids
            .iter()
            .position(|block| *block == id)
            .ok_or(ModelError::UnknownBlock(id))?;
        let next = match direction {
            crate::edit::Direction::Forward => index.checked_add(1).and_then(|next| ids.get(next)),
            crate::edit::Direction::Backward => index.checked_sub(1).and_then(|prev| ids.get(prev)),
        };
        Ok(next.copied())
    }

    pub(crate) fn notes_mut(&mut self) -> &mut BTreeMap<NoteId, Note> {
        &mut self.notes
    }

    pub(crate) fn notes(&self) -> &BTreeMap<NoteId, Note> {
        &self.notes
    }

    fn part_index(&self, id: PartId) -> Result<usize, ModelError> {
        self.parts
            .iter()
            .position(|part| part.id == id)
            .ok_or(ModelError::UnknownPart(id))
    }

    fn chapter_index(&self, id: ChapterId) -> Result<(usize, usize), ModelError> {
        for (part_index, part) in self.parts.iter().enumerate() {
            if let Some(chapter_index) = part.chapters.iter().position(|chapter| chapter.id == id) {
                return Ok((part_index, chapter_index));
            }
        }
        Err(ModelError::UnknownChapter(id))
    }
}

/// Ropes are stored in a bundle as plain strings.
mod rope_text {
    use ropey::Rope;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(rope: &Rope, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&rope.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Rope, D::Error> {
        Ok(Rope::from_str(&String::deserialize(deserializer)?))
    }
}

/// Everything in a book except its chapters' contents: the bundle's
/// `book.json`. Chapters are stored one per file and listed here in order.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct BookManifest {
    pub format: u32,
    pub title: String,
    pub ids: IdGen,
    pub stylesheet: crate::publish::Stylesheet,
    pub notes: Vec<Note>,
    pub selection: Selection,
    pub parts: Vec<PartEntry>,
}

/// A part in the manifest: its chapters by id, in reading order.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct PartEntry {
    pub id: PartId,
    pub title: String,
    pub chapters: Vec<ChapterId>,
}

impl Book {
    /// The manifest and the chapters it lists, for writing a bundle.
    pub(crate) fn to_manifest(&self) -> (BookManifest, Vec<&Chapter>) {
        let manifest = BookManifest {
            format: 2,
            title: self.title.clone(),
            ids: self.ids.clone(),
            stylesheet: self.stylesheet.clone(),
            notes: self.notes.values().cloned().collect(),
            selection: self.selection,
            parts: self
                .parts
                .iter()
                .map(|part| PartEntry {
                    id: part.id,
                    title: part.title.clone(),
                    chapters: part.chapters.iter().map(|chapter| chapter.id).collect(),
                })
                .collect(),
        };
        let chapters = self
            .parts
            .iter()
            .flat_map(|part| part.chapters.iter())
            .collect();
        (manifest, chapters)
    }

    /// Rebuild a book from a manifest and its chapters. Undo history starts
    /// empty; a selection that no longer fits is moved to the first block.
    pub(crate) fn from_manifest(
        manifest: BookManifest,
        mut chapters: HashMap<ChapterId, Chapter>,
    ) -> Result<Self, ModelError> {
        let mut parts = Vec::with_capacity(manifest.parts.len());
        for entry in manifest.parts {
            let mut part = Part {
                id: entry.id,
                title: entry.title,
                chapters: Vec::with_capacity(entry.chapters.len()),
            };
            for id in entry.chapters {
                let chapter = chapters
                    .remove(&id)
                    .ok_or(ModelError::Inconsistent("manifest lists a missing chapter"))?;
                part.chapters.push(chapter);
            }
            parts.push(part);
        }
        let mut book = Self {
            title: manifest.title,
            ids: manifest.ids,
            parts,
            stylesheet: manifest.stylesheet,
            notes: manifest
                .notes
                .into_iter()
                .map(|note| (note.id, note))
                .collect(),
            index: HashMap::new(),
            selection: manifest.selection,
            typing_marks: None,
            undo: Vec::new(),
            redo: Vec::new(),
        };
        // A book from before a standard style existed gains it, by name.
        for style in crate::publish::standard_styles() {
            if !book
                .stylesheet
                .paragraphs
                .iter()
                .any(|existing| existing.name == style.name)
            {
                book.stylesheet.paragraphs.push(style);
            }
        }
        book.reindex();
        book.check_consistency()?;
        if book.set_selection(manifest.selection).is_err() {
            let first = *book
                .block_ids()
                .first()
                .ok_or(ModelError::Inconsistent("book has no blocks"))?;
            book.selection = Selection::collapsed(Position::new(first, 0));
        }
        Ok(book)
    }
}
