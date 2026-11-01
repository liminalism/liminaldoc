//! Preflight names the problems it was asked to find.

#![allow(clippy::unwrap_used)]

use docwrite_export::{AssetFacts, preflight};
use docwrite_model::Book;

#[test]
fn preflight_reports_fonts_images_and_broken_links() {
    let mut book = Book::new("Essay");
    book.insert_with_marks(
        "See",
        docwrite_model::RunMarks {
            link: Some("missing-figure".into()),
            ..docwrite_model::RunMarks::default()
        },
    )
    .unwrap();
    let facts = AssetFacts {
        typefaces: &["Body".into(), "Absent Serif".into()],
        fonts_on_disk: &["Body".into()],
        embeddable: &[],
        images: &[("plate", 72)],
        linked_targets_that_exist: &[],
    };
    let report = preflight(&book, &facts);
    assert!(report.missing_fonts == ["Absent Serif"]);
    assert!(report.unembeddable_fonts.iter().any(|font| font == "Body"));
    assert!(
        report
            .low_resolution_images
            .iter()
            .any(|image| image == "plate")
    );
    assert!(
        report
            .broken_references
            .iter()
            .any(|link| link == "missing-figure")
    );
    assert_eq!(report.font_embedding_warnings, ["Body cannot be embedded"]);
    println!("preflight missing fonts: {:?}", report.missing_fonts);
    println!(
        "preflight unembeddable fonts: {:?}",
        report.unembeddable_fonts
    );
    println!(
        "preflight low-resolution images: {:?}",
        report.low_resolution_images
    );
    println!(
        "preflight broken references: {:?}",
        report.broken_references
    );
    println!(
        "preflight font-embedding warnings: {:?}",
        report.font_embedding_warnings
    );
}

#[test]
fn preflight_does_not_assume_a_default_face_and_ignores_unused_fonts() {
    let book = Book::new("Essay");
    let facts = AssetFacts {
        typefaces: &["Source Serif".into()],
        fonts_on_disk: &["Source Serif".into(), "Unrelated Font".into()],
        embeddable: &["Source Serif".into()],
        images: &[],
        linked_targets_that_exist: &[],
    };
    assert!(preflight(&book, &facts).is_clean());
    let none = AssetFacts {
        typefaces: &[],
        ..facts
    };
    assert_eq!(
        preflight(&book, &none).missing_fonts,
        ["no typeface selected"]
    );
}
