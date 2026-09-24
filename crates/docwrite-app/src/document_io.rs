//! Text-focused imports. The canonical editable format remains `.legebook`.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use docwrite_model::{BlockKind, Book};
use docx_rust::DocxFile;
use docx_rust::document::BodyContent;
use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::Event;
use zip::ZipArchive;

const MAX_IMPORT_BYTES: u64 = 32 * 1024 * 1024;

struct Paragraph {
    text: String,
    heading: bool,
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
    let text = paragraphs
        .iter()
        .map(|paragraph| paragraph.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if !text.is_empty() {
        book.insert(&text).map_err(model_error)?;
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
                text: paragraph.text(),
                heading,
            });
        }
    }
    Ok(paragraphs)
}

pub(crate) fn export_docx(book: &Book, path: &Path) -> Result<(), String> {
    use docx_rust::document::Paragraph as DocxParagraph;
    use docx_rust::formatting::{ParagraphProperty, ParagraphStyleId};

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
                    let mut paragraph = DocxParagraph::default().push_text(block.text());
                    if matches!(block.kind(), BlockKind::Subhead) {
                        paragraph.property = Some(ParagraphProperty {
                            style_id: Some(ParagraphStyleId::from("Heading2")),
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
        .map(|line| Paragraph {
            text: line.trim_end_matches('\r').to_string(),
            heading: false,
        })
        .collect()
}

fn markdown_paragraphs(text: &str) -> Vec<Paragraph> {
    let mut paragraphs = Vec::new();
    let mut prose = String::new();
    let flush = |paragraphs: &mut Vec<Paragraph>, prose: &mut String| {
        if !prose.is_empty() {
            paragraphs.push(Paragraph {
                text: std::mem::take(prose),
                heading: false,
            });
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
            paragraphs.push(Paragraph {
                text: heading.to_string(),
                heading: true,
            });
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
    let mut current: Option<Paragraph> = None;
    loop {
        match reader.read_event().map_err(|err| err.to_string())? {
            Event::Start(tag) => match local_name(tag.name().as_ref()) {
                b"p" | b"h" if current.is_none() => {
                    current = Some(Paragraph {
                        text: String::new(),
                        heading: local_name(tag.name().as_ref()) == b"h",
                    });
                }
                b"tab" | b"s" if current.is_some() => {
                    if let Some(paragraph) = &mut current {
                        paragraph.text.push(' ');
                    }
                }
                b"br" | b"line-break" if current.is_some() => {
                    if let Some(paragraph) = &mut current {
                        paragraph.text.push(' ');
                    }
                }
                _ => {}
            },
            Event::Empty(tag) => match local_name(tag.name().as_ref()) {
                b"tab" | b"s" if current.is_some() => {
                    if let Some(paragraph) = &mut current {
                        paragraph.text.push(' ');
                    }
                }
                b"br" | b"line-break" if current.is_some() => {
                    if let Some(paragraph) = &mut current {
                        paragraph.text.push(' ');
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some(paragraph) = &mut current {
                    let decoded = text.decode().map_err(|err| err.to_string())?;
                    paragraph
                        .text
                        .push_str(&unescape(&decoded).map_err(|err| err.to_string())?);
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(paragraph) = &mut current {
                    let entity =
                        format!("&{};", reference.decode().map_err(|err| err.to_string())?);
                    paragraph
                        .text
                        .push_str(&unescape(&entity).map_err(|err| err.to_string())?);
                }
            }
            Event::CData(text) => {
                if let Some(paragraph) = &mut current {
                    paragraph
                        .text
                        .push_str(&text.decode().map_err(|err| err.to_string())?);
                }
            }
            Event::End(tag) if matches!(local_name(tag.name().as_ref()), b"p" | b"h") => {
                if let Some(paragraph) = current.take() {
                    paragraphs.push(paragraph);
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
            lines.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
            ["One & two", "Next"]
        );
        let odt = r#"<office:document-content xmlns:office="office" xmlns:text="text"><text:h>Heading</text:h><text:p>Body<text:s/>text</text:p></office:document-content>"#;
        let lines = xml_paragraphs(odt).unwrap();
        assert!(lines[0].heading);
        assert_eq!(lines[1].text, "Body text");
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
