//! Import a Pages document, save a native copy and render its first page.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use docwrite_app::Editor;
use docwrite_model::BlockKind;
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(
        args.len(),
        3,
        "pages_import SOURCE.pages OUT.legebook OUT.png"
    );
    let source = std::fs::read(&args[0]).unwrap();
    let mut editor = Editor::open_document(std::path::Path::new(&args[0])).unwrap();
    let cells = editor
        .book()
        .blocks()
        .filter(|b| matches!(b.kind(), BlockKind::TableCell { .. }))
        .count();
    editor
        .save_as_bundle(std::path::Path::new(&args[1]))
        .unwrap();
    let reopened = Editor::open_existing(&args[1]).unwrap();
    assert_eq!(editor.book().plain_text(), reopened.book().plain_text());
    assert_eq!(source, std::fs::read(&args[0]).unwrap());
    let mut frame = liminalkit_raster::WindowBuffer::new(1100, 800);
    editor.paint(&mut frame);
    liminalkit_raster::png::write(&args[2], &frame).unwrap();
    println!(
        "Imported {cells} cell paragraphs, {} pages; native save/reopen and unchanged source verified",
        editor.pages_painted()
    );
}
