//! Native modern Pages import. Cells remain editable model blocks.
use docwrite_model::{BlockKind, Book};
use iwork::{
    Document,
    pb::{Message, decode_nested},
};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

pub(crate) fn import(path: &Path) -> Result<Book, String> {
    if path.is_file() {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let archive =
            zip::ZipArchive::new(file).map_err(|e| format!("invalid Pages package: {e}"))?;
        if archive.len() > 10_000
            || archive
                .decompressed_size()
                .is_none_or(|size| size > 64 * 1024 * 1024)
        {
            return Err("Pages package exceeds the 64 MiB import limit".into());
        }
    }
    let doc = Document::open(path).map_err(|e| format!("cannot read Pages document: {e}"))?;
    let structure = doc
        .structure()
        .ok_or("Pages document has no document structure")?;
    if structure.mode != iwork::pages::Mode::WordProcessing {
        return Err("Pages page-layout documents are not supported yet".into());
    }
    if !structure.footnotes.is_empty()
        || structure
            .header_footers
            .iter()
            .any(|h| !h.text.trim().is_empty())
    {
        return Err("Pages headers, footers and footnotes are not supported yet".into());
    }
    if doc
        .elements()
        .iter()
        .any(|e| !matches!(e.kind, iwork::drawable::Kind::Table))
    {
        return Err("Pages contains graphics or text boxes that cannot be imported yet".into());
    }
    let body = structure.body_storage.ok_or("Pages document has no body")?;
    let text = doc.storage_text(body).map_err(|e| e.to_string())?;
    let tables = doc.tables();
    let (_, object) = doc.object(body).ok_or("Pages body is missing")?;
    let storage = Message::decode(object.payload()).map_err(|e| e.to_string())?;
    let mut anchors = BTreeMap::new();
    if let Some(table) = storage.bytes(9).and_then(decode_nested) {
        for (at, attachment) in
            iwork::text::entry_indices(&table, iwork::text::Anchoring::Character)
        {
            if let Some(id) = attachment {
                let target = doc
                    .object(id)
                    .and_then(|(_, o)| Message::decode(o.payload()).ok())
                    .and_then(|m| m.bytes(1).and_then(decode_nested))
                    .and_then(|m| m.varint(1));
                if let Some(target) = target {
                    anchors.insert(at, target);
                }
            }
        }
    }
    let mut blocks = Vec::new();
    let mut seen = HashSet::new();
    let mut prose = String::new();
    let mut offset = 0;
    for ch in text.chars() {
        if ch == '\u{fffc}' {
            flush(&mut prose, &mut blocks);
            let target = anchors
                .get(&offset)
                .ok_or("Pages contains an unresolved embedded object")?;
            let table = tables
                .iter()
                .find(|t| u64::from(t.identifier) == *target)
                .ok_or("Pages contains an embedded object other than a supported table")?;
            append_table(table, &mut blocks)?;
            seen.insert(*target);
        } else if ch == '\n' || ch == '\r' || ch == '\u{4}' || ch == '\u{5}' {
            flush(&mut prose, &mut blocks);
        } else {
            prose.push(ch);
        }
        offset += ch.len_utf16() as u64;
    }
    flush(&mut prose, &mut blocks);
    // Floating tables do not have character anchors. Keep their contents,
    // after the anchored flow, rather than dropping them.
    for table in &tables {
        if !seen.contains(&u64::from(table.identifier)) {
            append_table(table, &mut blocks)?;
        }
    }
    let mut book = Book::new(path.file_stem().unwrap_or_default().to_string_lossy());
    let text = blocks
        .iter()
        .map(|(text, _)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    book.insert(&text).map_err(|e| e.to_string())?;
    for (id, (_, kind)) in book.block_ids().into_iter().zip(blocks) {
        book.set_kind(id, kind).map_err(|e| e.to_string())?;
    }
    Ok(book)
}
fn flush(prose: &mut String, blocks: &mut Vec<(String, BlockKind)>) {
    if !prose.trim().is_empty() {
        blocks.push((std::mem::take(prose), BlockKind::Body));
    } else {
        prose.clear();
    }
}
fn append_table(
    table: &iwork::TableInfo,
    blocks: &mut Vec<(String, BlockKind)>,
) -> Result<(), String> {
    if !table.problems.is_empty() {
        return Err(format!(
            "cannot read table {}: {}",
            table.name,
            table.problems.join("; ")
        ));
    }
    if table.rows.saturating_mul(table.columns) > 100_000 {
        return Err("Pages table exceeds 100,000 cells".into());
    }
    for row in 0..table.rows {
        for column in 0..table.columns {
            let merge = table.merges.iter().find(|m| {
                row >= m.row
                    && row < m.row.saturating_add(m.rows)
                    && column >= m.column
                    && column < m.column.saturating_add(m.columns)
            });
            if merge.is_some_and(|m| m.row != row || m.column != column) {
                continue;
            }
            let kind = BlockKind::TableCell {
                table: u64::from(table.identifier),
                row,
                column,
                columns: table.columns,
                span: merge.map_or(1, |m| m.columns),
                row_span: merge.map_or(1, |m| m.rows),
                widths: (0..table.columns)
                    .map(|c| {
                        (table
                            .column_extents
                            .get(c)
                            .and_then(|e| e.size)
                            .unwrap_or(table.default_column_width as f32)
                            .max(1.0)
                            * 1000.0) as u32
                    })
                    .collect(),
            };
            let text = table.value(row, column).to_text();
            for paragraph in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
                blocks.push((paragraph.to_string(), kind.clone()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    #[test]
    fn pointer_selects_price_instead_of_neighbouring_service() {
        let mut book = Book::new("Cells");
        book.insert("Menu\nPrice\nNails\n100").unwrap();
        let ids = book.block_ids();
        for (n, id) in ids.iter().enumerate() {
            book.set_kind(
                *id,
                BlockKind::TableCell {
                    table: 1,
                    row: n / 2,
                    column: n % 2,
                    columns: 2,
                    span: 1,
                    row_span: 1,
                    widths: vec![2, 1],
                },
            )
            .unwrap();
        }
        let mut editor = crate::Editor::with_book(book);
        let mut frame = liminalkit_raster::WindowBuffer::new(1100, 800);
        editor.paint(&mut frame);
        let slot = &editor.slots[0];
        let doc = editor.document().unwrap();
        let line = doc
            .page_painted_lines(slot.page)
            .into_iter()
            .find(|l| doc.paragraphs()[l.paragraph].id == ids[3].raw())
            .unwrap();
        let x =
            slot.left as f32 + (doc.page_content_inset(slot.page) + line.indent + 1.0) * slot.scale;
        let y = slot.top as f32 + (line.baseline - line.em * 0.3) * slot.scale;
        assert_eq!(editor.text_position_at(x, y, false).unwrap().block, ids[3]);
    }
}
