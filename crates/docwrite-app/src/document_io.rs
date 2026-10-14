//! Text-focused imports. The canonical editable format remains `.legebook`.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use docwrite_model::{Align, BlockKind, Book, RunMarks};
use docx_rust::DocxFile;
use docx_rust::document::{BodyContent, ParagraphContent, RunContent};
use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::Event;
use zip::ZipArchive;

const MAX_IMPORT_BYTES: u64 = 32 * 1024 * 1024;

/// One styled run of an imported paragraph.
struct Span {
    text: String,
    bold: bool,
    italic: bool,
}

struct Paragraph {
    spans: Vec<Span>,
    heading: bool,
}

impl Paragraph {
    fn plain(text: String, heading: bool) -> Self {
        Self {
            spans: vec![Span {
                text,
                bold: false,
                italic: false,
            }],
            heading,
        }
    }

    fn text(&self) -> String {
        self.spans
            .iter()
            .map(|span| span.text.as_str())
            .collect::<Vec<_>>()
            .join("")
    }
}

pub(crate) fn import_book(path: &Path) -> Result<Book, String> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let paragraphs = match extension.as_str() {
        "txt" => plain_paragraphs(&read_limited(File::open(path).map_err(io_error)?)?),
        "md" | "markdown" => {
            markdown_paragraphs(&read_limited(File::open(path).map_err(io_error)?)?)
        }
        "odt" => package_paragraphs(path, "content.xml")?,
        "docx" => docx_paragraphs(path)?,
        "doc" => {
            return Err("legacy .doc is unsupported; convert it to .docx or .odt first".into());
        }
        _ => return Err(format!("unsupported document format: {}", path.display())),
    };
    let title = path
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into());
    let mut book = Book::new(title.clone());
    let chapter = book.parts()[0].chapters()[0].id();
    book.rename_chapter(chapter, title).map_err(model_error)?;
    let plain = paragraphs
        .iter()
        .flat_map(|paragraph| paragraph.spans.iter())
        .all(|span| !span.bold && !span.italic);
    if plain {
        let text = paragraphs
            .iter()
            .map(|paragraph| paragraph.text())
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            book.insert(&text).map_err(model_error)?;
        }
    } else {
        for (index, paragraph) in paragraphs.iter().enumerate() {
            if index > 0 {
                book.insert("\n").map_err(model_error)?;
            }
            for span in &paragraph.spans {
                if span.text.is_empty() {
                    continue;
                }
                let marks = RunMarks {
                    bold: span.bold,
                    italic: span.italic,
                    ..RunMarks::default()
                };
                book.insert_with_marks(&span.text, marks)
                    .map_err(model_error)?;
            }
        }
    }
    let ids = book.block_ids();
    for (index, paragraph) in paragraphs.iter().enumerate() {
        if paragraph.heading
            && let Some(id) = ids.get(index)
        {
            book.set_kind(*id, BlockKind::Subhead)
                .map_err(model_error)?;
        }
    }
    Ok(book)
}

fn docx_paragraphs(path: &Path) -> Result<Vec<Paragraph>, String> {
    if std::fs::metadata(path).map_err(io_error)?.len() > MAX_IMPORT_BYTES {
        return Err("DOCX package exceeds the 32 MiB import limit".into());
    }
    let file = DocxFile::from_file(path).map_err(|err| err.to_string())?;
    let docx = file.parse().map_err(|err| err.to_string())?;
    let mut paragraphs = Vec::new();
    for content in &docx.document.body.content {
        if let BodyContent::Paragraph(paragraph) = content {
            let heading = paragraph
                .property
                .as_ref()
                .and_then(|property| property.style_id.as_ref())
                .is_some_and(|style| style.value.to_ascii_lowercase().starts_with("heading"));
            paragraphs.push(Paragraph {
                spans: docx_spans(paragraph),
                heading,
            });
        }
    }
    Ok(paragraphs)
}

/// A paragraph's runs as styled spans: bold and italic survive the import.
/// Breaks and tabs are dropped, as `paragraph.text()` drops them.
fn docx_spans(paragraph: &docx_rust::document::Paragraph) -> Vec<Span> {
    use docx_rust::document::Run;
    use docx_rust::document::Text;

    let run_spans = |run: &Run| -> Vec<Span> {
        let bold = run
            .property
            .as_ref()
            .and_then(|property| property.bold.as_ref())
            .is_some_and(|bold| bold.value != Some(false));
        let italic = run
            .property
            .as_ref()
            .and_then(|property| property.italics.as_ref())
            .is_some_and(|italics| italics.value != Some(false));
        run.content
            .iter()
            .filter_map(|content| match content {
                RunContent::Text(Text { text, .. }) => Some(Span {
                    text: text.to_string(),
                    bold,
                    italic,
                }),
                _ => None,
            })
            .collect()
    };
    let mut spans = Vec::new();
    for content in &paragraph.content {
        match content {
            ParagraphContent::Run(run) => spans.extend(run_spans(run)),
            ParagraphContent::Link(link) => {
                if let Some(run) = link.content.as_ref() {
                    spans.extend(run_spans(run));
                }
            }
            _ => {}
        }
    }
    if spans.is_empty() {
        spans.push(Span {
            text: String::new(),
            bold: false,
            italic: false,
        });
    }
    spans
}

pub(crate) fn export_docx(book: &Book, path: &Path) -> Result<(), String> {
    use docx_rust::document::{Paragraph as DocxParagraph, Run as DocxRun};
    use docx_rust::formatting::{
        Bold, CharacterProperty, Italics, Justification, JustificationVal, ParagraphProperty,
        ParagraphStyleId,
    };

    let justification = |align: Align| {
        Justification::from(match align {
            Align::Left => JustificationVal::Left,
            Align::Justify => JustificationVal::Both,
            Align::Center => JustificationVal::Center,
            Align::Right => JustificationVal::Right,
        })
    };
    let mut docx = docx_rust::Docx::default();
    for part in book.parts() {
        for chapter in part.chapters() {
            if !chapter.title().is_empty() {
                let mut heading = DocxParagraph::default().push_text(chapter.title().to_string());
                heading.property = Some(ParagraphProperty {
                    style_id: Some(ParagraphStyleId::from("Heading1")),
                    ..Default::default()
                });
                docx.document.push(heading);
            }
            for section in chapter.sections() {
                for block in section.blocks() {
                    let mut paragraph = DocxParagraph::default();
                    let chars: Vec<char> = block.text().chars().collect();
                    if block.runs().is_empty() {
                        paragraph = paragraph.push_text(block.text());
                    } else {
                        for run in block.runs() {
                            let piece: String =
                                chars.get(run.start..run.end.min(chars.len())).map_or(
                                    String::new(),
                                    |slice| slice.iter().collect(),
                                );
                            let mut property = CharacterProperty::default();
                            if run.marks.bold {
                                property.bold = Some(Bold::from(true));
                            }
                            if run.marks.italic {
                                property.italics = Some(Italics::from(true));
                            }
                            let styled = DocxRun::default().property(property).push_text(piece);
                            paragraph = paragraph.push(styled);
                        }
                    }
                    let subhead = matches!(block.kind(), BlockKind::Subhead);
                    if subhead || block.align().is_some() {
                        paragraph.property = Some(ParagraphProperty {
                            style_id: subhead.then(|| ParagraphStyleId::from("Heading2")),
                            justification: block.align().map(justification),
                            ..Default::default()
                        });
                    }
                    docx.document.push(paragraph);
                }
            }
        }
    }
    let bytes = docx
        .write(std::io::Cursor::new(Vec::new()))
        .map_err(|err| err.to_string())?
        .into_inner();
    crate::commands::write_atomically(path, &bytes)
}

fn read_limited(reader: impl Read) -> Result<String, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_IMPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_IMPORT_BYTES {
        return Err("document text exceeds the 32 MiB import limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "document text is not UTF-8".into())
}

fn plain_paragraphs(text: &str) -> Vec<Paragraph> {
    text.lines()
        .map(|line| Paragraph::plain(line.trim_end_matches('\r').to_string(), false))
        .collect()
}

fn markdown_paragraphs(text: &str) -> Vec<Paragraph> {
    let mut paragraphs = Vec::new();
    let mut prose = String::new();
    let flush = |paragraphs: &mut Vec<Paragraph>, prose: &mut String| {
        if !prose.is_empty() {
            paragraphs.push(Paragraph::plain(std::mem::take(prose), false));
        }
    };
    for line in text.lines().map(|line| line.trim_end_matches('\r')) {
        if line.trim().is_empty() {
            flush(&mut paragraphs, &mut prose);
            continue;
        }
        if let Some(heading) = line.trim_start_matches('#').strip_prefix(' ')
            && line.starts_with('#')
        {
            flush(&mut paragraphs, &mut prose);
            paragraphs.push(Paragraph::plain(heading.to_string(), true));
            continue;
        }
        if !prose.is_empty() {
            prose.push(' ');
        }
        prose.push_str(line.strip_prefix("> ").unwrap_or(line));
    }
    flush(&mut paragraphs, &mut prose);
    paragraphs
}

fn package_paragraphs(path: &Path, member: &str) -> Result<Vec<Paragraph>, String> {
    let file = File::open(path).map_err(io_error)?;
    let mut archive = ZipArchive::new(file).map_err(|err| err.to_string())?;
    let xml = archive
        .by_name(member)
        .map_err(|_| format!("{} is missing {member}", path.display()))?;
    let xml = read_limited(xml)?;
    xml_paragraphs(&xml)
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn xml_paragraphs(xml: &str) -> Result<Vec<Paragraph>, String> {
    let mut reader = Reader::from_str(xml);
    let mut paragraphs = Vec::new();
    let mut current: Option<(String, bool)> = None;
    let push_space = |current: &mut Option<(String, bool)>| {
        if let Some((text, _)) = current {
            text.push(' ');
        }
    };
    loop {
        match reader.read_event().map_err(|err| err.to_string())? {
            Event::Start(tag) => match local_name(tag.name().as_ref()) {
                b"p" | b"h" if current.is_none() => {
                    current = Some((
                        String::new(),
                        local_name(tag.name().as_ref()) == b"h",
                    ));
                }
                b"tab" | b"s" if current.is_some() => push_space(&mut current),
                b"br" | b"line-break" if current.is_some() => push_space(&mut current),
                _ => {}
            },
            Event::Empty(tag) => match local_name(tag.name().as_ref()) {
                b"tab" | b"s" if current.is_some() => push_space(&mut current),
                b"br" | b"line-break" if current.is_some() => push_space(&mut current),
                _ => {}
            },
            Event::Text(text) => {
                if let Some((paragraph, _)) = &mut current {
                    let decoded = text.decode().map_err(|err| err.to_string())?;
                    paragraph.push_str(&unescape(&decoded).map_err(|err| err.to_string())?);
                }
            }
            Event::GeneralRef(reference) => {
                if let Some((paragraph, _)) = &mut current {
                    let entity =
                        format!("&{};", reference.decode().map_err(|err| err.to_string())?);
                    paragraph.push_str(&unescape(&entity).map_err(|err| err.to_string())?);
                }
            }
            Event::CData(text) => {
                if let Some((paragraph, _)) = &mut current {
                    paragraph.push_str(&text.decode().map_err(|err| err.to_string())?);
                }
            }
            Event::End(tag) if matches!(local_name(tag.name().as_ref()), b"p" | b"h") => {
                if let Some((text, heading)) = current.take() {
                    paragraphs.push(Paragraph::plain(text, heading));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(paragraphs)
}

fn io_error(err: std::io::Error) -> String {
    err.to_string()
}

fn model_error(err: docwrite_model::ModelError) -> String {
    err.to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    #[test]
    fn package_text_preserves_paragraphs_and_entities() {
        let docx = r#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>One &amp; two</w:t></w:r></w:p><w:p><w:r><w:t>Next</w:t></w:r></w:p></w:body></w:document>"#;
        let lines = xml_paragraphs(docx).unwrap();
        assert_eq!(
            lines.iter().map(|p| p.text()).collect::<Vec<_>>(),
            ["One & two", "Next"]
        );
        let odt = r#"<office:document-content xmlns:office="office" xmlns:text="text"><text:h>Heading</text:h><text:p>Body<text:s/>text</text:p></office:document-content>"#;
        let lines = xml_paragraphs(odt).unwrap();
        assert!(lines[0].heading);
        assert_eq!(lines[1].text(), "Body text");
    }

    #[test]
    fn imports_docx_and_odt_packages_as_unsaved_prose() {
        let dir = std::env::temp_dir().join(format!("docwrite-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let docx_path = dir.join("sample.docx");
        let mut source = Book::new("Sample");
        source.insert("First & second.\nAnother line.").unwrap();
        export_docx(&source, &docx_path).unwrap();
        let odt_path = dir.join("sample.odt");
        let file = File::create(&odt_path).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.start_file("content.xml", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(br#"<office:document-content xmlns:office="office" xmlns:text="text"><text:h>Heading</text:h><text:p>Body text.</text:p></office:document-content>"#).unwrap();
        zip.finish().unwrap();
        for (extension, path) in [("docx", docx_path), ("odt", odt_path)] {
            let editor = crate::Editor::open_document(&path).unwrap();
            assert!(editor.bundle().is_none());
            assert!(editor.is_dirty());
            let text = editor.book().plain_text();
            assert!(text.contains(if extension == "docx" {
                "First & second."
            } else {
                "Heading"
            }));
            if extension == "docx" {
                assert!(matches!(
                    editor.book().blocks().next().unwrap().kind(),
                    BlockKind::Subhead
                ));
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn docx_round_trip_keeps_bold_and_italic() {
        use docwrite_model::{Mark, Position, Selection};

        let dir = std::env::temp_dir().join(format!("docwrite-styled-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut source = Book::new("Styled");
        source.insert("One bold and one italic word.").unwrap();
        let block = source.block_ids()[0];
        source
            .set_selection(Selection {
                anchor: Position::new(block, 4),
                focus: Position::new(block, 8),
            })
            .unwrap();
        source.toggle_mark(Mark::Bold).unwrap();
        source
            .set_selection(Selection {
                anchor: Position::new(block, 17),
                focus: Position::new(block, 23),
            })
            .unwrap();
        source.toggle_mark(Mark::Italic).unwrap();
        let path = dir.join("styled.docx");
        export_docx(&source, &path).unwrap();

        let editor = crate::Editor::open_document(&path).unwrap();
        let reopened = editor.book();
        // The chapter title imports first, as a subhead; the body follows it.
        assert_eq!(reopened.block_ids().len(), 2);
        let body = reopened.block(reopened.block_ids()[1]).unwrap();
        assert_eq!(body.text(), "One bold and one italic word.");
        let marks_at = |offset| {
            body.runs()
                .iter()
                .find(|run| run.start <= offset && offset < run.end)
                .map(|run| (run.marks.bold, run.marks.italic))
        };
        assert_eq!(marks_at(0), Some((false, false)));
        assert_eq!(marks_at(5), Some((true, false)), "bold survives");
        assert_eq!(marks_at(18), Some((false, true)), "italic survives");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn markdown_headings_become_subheads() {
        let dir = std::env::temp_dir().join(format!("docwrite-md-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("article.md");
        std::fs::write(&path, "# Opening\n\nA paragraph\nwrapped across lines.\n").unwrap();
        let book = import_book(&path).unwrap();
        assert_eq!(
            book.plain_text(),
            "Opening\nA paragraph wrapped across lines."
        );
        assert!(matches!(
            book.block(book.block_ids()[0]).unwrap().kind(),
            BlockKind::Subhead
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
