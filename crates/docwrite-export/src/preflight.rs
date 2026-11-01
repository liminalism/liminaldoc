//! Preflight before export: selected typefaces that are missing or cannot
//! be embedded,
//! low-resolution images, and broken references.

use docwrite_model::Book;

/// What preflight found before export.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreflightReport {
    /// Fonts the book names that are not on this machine.
    pub missing_fonts: Vec<String>,
    /// Fonts whose licence forbids embedding.
    pub unembeddable_fonts: Vec<String>,
    /// Images below print resolution.
    pub low_resolution_images: Vec<String>,
    /// Links and citations whose target is missing.
    pub broken_references: Vec<String>,
    /// Other font embedding problems.
    pub font_embedding_warnings: Vec<String>,
}

impl PreflightReport {
    /// Nothing to fix.
    pub fn is_clean(&self) -> bool {
        self.missing_fonts.is_empty()
            && self.unembeddable_fonts.is_empty()
            && self.low_resolution_images.is_empty()
            && self.broken_references.is_empty()
    }
}

/// What preflight is told about the machine and the book's assets.
#[derive(Debug)]
pub struct AssetFacts<'a> {
    /// Typefaces the export will set the book in (the selected family).
    pub typefaces: &'a [String],
    /// Font names available on this machine.
    pub fonts_on_disk: &'a [String],
    /// Font names that may be embedded.
    pub embeddable: &'a [String],
    /// Image name and pixels per inch.
    pub images: &'a [(&'a str, u32)],
    /// Link and citation targets that exist.
    pub linked_targets_that_exist: &'a [String],
}

/// Check `book` for problems an export would carry.
pub fn preflight(book: &Book, facts: &AssetFacts<'_>) -> PreflightReport {
    let mut report = PreflightReport::default();
    // The manuscript is set in the typefaces the export will actually use,
    // not in a fixed default. Each is checked once, for presence and for
    // permission to embed. Other fonts on the machine are not the book's concern.
    if facts.typefaces.is_empty() {
        report.missing_fonts.push("no typeface selected".into());
    }
    for face in facts.typefaces {
        if report.missing_fonts.contains(face) || report.unembeddable_fonts.contains(face) {
            continue;
        }
        if !facts.fonts_on_disk.iter().any(|font| font == face) {
            report.missing_fonts.push(face.clone());
        } else if !facts.embeddable.iter().any(|ok| ok == face) {
            report.unembeddable_fonts.push(face.clone());
            report
                .font_embedding_warnings
                .push(format!("{face} cannot be embedded"));
        }
    }
    for (name, dpi) in facts.images {
        if *dpi < 300 {
            report.low_resolution_images.push((*name).to_string());
        }
    }
    for block in crate::blocks_of(book) {
        if let Some(link) = block.link
            && !facts
                .linked_targets_that_exist
                .iter()
                .any(|target| target == &link)
        {
            report.broken_references.push(link);
        }
    }
    for source in book_sources(book) {
        if source.is_empty() {
            report.broken_references.push("empty source".into());
        }
    }
    report
}

fn book_sources(book: &Book) -> Vec<String> {
    // Sources are reached through citation index terms.
    book.index_terms()
        .iter()
        .filter(|term| term.term.starts_with("cite:"))
        .map(|term| term.term.clone())
        .collect()
}
