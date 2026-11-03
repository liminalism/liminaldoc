//! Native Pages imports retain cell identity through editing and persistence.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use docwrite_app::Editor;
use docwrite_model::{BlockKind, Position, Selection};

fn verify(path: &std::path::Path) {
    let before = std::fs::read(path).unwrap();
    let source = iwork::Document::open(path).unwrap();
    let mut editor = Editor::open_document(path).unwrap();
    assert!(editor.bundle().is_none());
    assert!(editor.is_dirty());
    for table in source.tables() {
        for cell in table.cells() {
            if table.merges.iter().any(|m| {
                cell.row >= m.row
                    && cell.row < m.row + m.rows
                    && cell.column >= m.column
                    && cell.column < m.column + m.columns
                    && (cell.row != m.row || cell.column != m.column)
            }) {
                continue;
            }
            let imported = editor.book().blocks().filter(|b| matches!(b.kind(), BlockKind::TableCell { table: id, row, column, .. } if *id == u64::from(table.identifier) && *row == cell.row && *column == cell.column)).map(|b| b.text()).collect::<Vec<_>>().join("\n");
            assert_eq!(
                imported,
                cell.value
                    .to_text()
                    .replace("\r\n", "\n")
                    .replace('\r', "\n")
            );
        }
    }
    let ids: Vec<_> = editor
        .book()
        .blocks()
        .filter(|b| matches!(b.kind(), BlockKind::TableCell { .. }))
        .map(|b| b.id())
        .collect();
    let first = ids[0];
    let kind = editor.book().block(first).unwrap().kind().clone();
    editor
        .book_mut()
        .set_selection(Selection::collapsed(Position::new(first, 0)))
        .unwrap();
    editor.book_mut().insert("Edited ").unwrap();
    editor.book_mut().undo().unwrap();
    editor.book_mut().redo().unwrap();
    assert_eq!(editor.book().block(first).unwrap().kind(), &kind);
    // Paragraph breaks stay in their source cell; backspace cannot join cells.
    editor.book_mut().insert("A\nB").unwrap();
    let continuation = editor.book().selection().focus.block;
    assert_eq!(editor.book().block(continuation).unwrap().kind(), &kind);
    editor
        .book_mut()
        .set_selection(Selection::collapsed(Position::new(ids[1], 0)))
        .unwrap();
    assert!(editor.book_mut().delete_backward().is_err());
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("import.legebook");
    editor.save_as_bundle(&bundle).unwrap();
    assert!(editor.export_docx_to(&dir.path().join("bad.docx")).is_err());
    assert!(
        editor
            .export_markdown_to(&dir.path().join("bad.md"))
            .is_err()
    );
    assert!(editor.export_text_to(&dir.path().join("bad.txt")).is_err());
    editor.export_pdf_to(&dir.path().join("table.pdf")).unwrap();
    let reopened = Editor::open_existing(&bundle).unwrap();
    assert_eq!(editor.book().plain_text(), reopened.book().plain_text());
    assert_eq!(
        editor.book().blocks().map(|b| b.kind()).collect::<Vec<_>>(),
        reopened
            .book()
            .blocks()
            .map(|b| b.kind())
            .collect::<Vec<_>>()
    );
    assert_eq!(before, std::fs::read(path).unwrap());
    let mut frame = liminalkit_raster::WindowBuffer::new(1100, 800);
    editor.paint(&mut frame);
    let document = editor.document().unwrap();
    let lines: Vec<_> = (1..=document.page_count())
        .flat_map(|p| document.page_painted_lines(p))
        .collect();
    // Two nonempty cells on the same row occupy different columns at the same baseline.
    let mut shared = false;
    for a in &lines {
        for b in &lines {
            if a.paragraph != b.paragraph
                && a.baseline == b.baseline
                && (a.indent - b.indent).abs() > 5.0
            {
                shared = true;
            }
        }
    }
    assert!(shared);
}

#[test]
fn imports_edits_and_reopens_table_cells() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("services.pages");
    let mut doc = iwork::Document::new(iwork::Kind::Pages).unwrap();
    doc.body_mut().unwrap().append("Service list\n").unwrap();
    doc.page_mut(1)
        .unwrap()
        .add(iwork::Table::with_rows(
            "Services",
            [
                ["Service", "Price"],
                ["Manicure\nเล็บมือ", "100"],
                ["Pedicure", "120"],
            ],
        ))
        .unwrap();
    doc.save(&path).unwrap();
    verify(&path);
}

#[test]
#[ignore = "local private Pages sample; set PAGES_SAMPLE"]
fn supplied_pages_document() {
    verify(std::path::Path::new(
        &std::env::var("PAGES_SAMPLE").unwrap(),
    ));
}

#[test]
fn opens_package_directories_and_refuses_graphics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.pages");
    let mut doc = iwork::Document::new(iwork::Kind::Pages).unwrap();
    doc.body_mut().unwrap().append("Body text").unwrap();
    doc.save(&path).unwrap();
    let package = dir.path().join("directory.pages");
    std::fs::create_dir(&package).unwrap();
    zip::ZipArchive::new(std::fs::File::open(&path).unwrap())
        .unwrap()
        .extract(&package)
        .unwrap();
    assert!(
        Editor::open_document(&package)
            .unwrap()
            .book()
            .plain_text()
            .contains("Body text")
    );
    doc.page_mut(1)
        .unwrap()
        .add(iwork::Shape::rectangle())
        .unwrap();
    doc.save(&path).unwrap();
    let error = Editor::open_document(&path).err().unwrap();
    assert!(error.contains("graphics"), "{error}");
}

#[test]
fn tall_rows_keep_every_cell_paragraph_reachable() {
    let mut book = docwrite_model::Book::new("Tall cells");
    let pieces = (0..90)
        .map(|n| format!("Cell line {n}"))
        .collect::<Vec<_>>();
    book.insert(&pieces.join("\n")).unwrap();
    for (n, id) in book.block_ids().into_iter().enumerate() {
        book.set_kind(
            id,
            BlockKind::TableCell {
                table: 1,
                row: 0,
                column: n / 45,
                columns: 2,
                span: 1,
                row_span: 1,
                widths: vec![1, 1],
            },
        )
        .unwrap();
    }
    let mut editor = Editor::with_book(book);
    let mut frame = liminalkit_raster::WindowBuffer::new(1100, 800);
    editor.paint(&mut frame);
    let doc = editor.document().unwrap();
    let text = (1..=doc.page_count())
        .flat_map(|p| doc.page_texts(p))
        .collect::<Vec<_>>()
        .join("\n");
    for piece in pieces {
        assert!(text.lines().any(|line| line == piece), "missing {piece}");
    }
}
