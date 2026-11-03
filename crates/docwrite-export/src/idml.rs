//! IDML package: styles, parent pages, a threaded story, anchored images, footnotes.
//!
//! The package is a stored (uncompressed) zip of the XML InDesign opens.

use docwrite_model::Book;

use crate::blocks_of;

/// The book as an IDML package: styles, a parent spread, and one story.
pub fn export_idml(book: &Book) -> Vec<u8> {
    let blocks = blocks_of(book);
    let mut story = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="18.0">
<Story Self="story_u1" TrackChanges="false">
"#,
    );
    for block in &blocks {
        let style = if block.kind == "heading" {
            "ChapterTitle"
        } else if block.kind == "quote" {
            "BlockQuote"
        } else {
            "Body"
        };
        story.push_str(&format!(
            "<ParagraphStyleRange AppliedParagraphStyle=\"ParagraphStyle/{style}\">"
        ));
        if block.kind == "image" {
            story
                .push_str("<AnchoredObject><Image href=\"file://assets/image\"/></AnchoredObject>");
        }
        story.push_str("<CharacterStyleRange AppliedCharacterStyle=\"CharacterStyle/$ID/[No character style]\">");
        story.push_str(&format!("<Content>{}</Content>", xml_escape(&block.text)));
        story.push_str("</CharacterStyleRange>");
        if let Some(note) = &block.note {
            story.push_str("<Footnote><ParagraphStyleRange AppliedParagraphStyle=\"ParagraphStyle/Footnote\"><CharacterStyleRange><Content>");
            story.push_str(&xml_escape(note));
            story.push_str("</Content></CharacterStyleRange></ParagraphStyleRange></Footnote>");
        }
        story.push_str("</ParagraphStyleRange>");
    }
    story.push_str("</Story></idPkg:Story>");

    let styles = r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<RootParagraphStyleGroup>
<ParagraphStyle Self="ParagraphStyle/Body" Name="Body"/>
<ParagraphStyle Self="ParagraphStyle/ChapterTitle" Name="Chapter Title"/>
<ParagraphStyle Self="ParagraphStyle/BlockQuote" Name="Block Quote"/>
<ParagraphStyle Self="ParagraphStyle/Footnote" Name="Footnote"/>
</RootParagraphStyleGroup>
<RootCharacterStyleGroup>
<CharacterStyle Self="CharacterStyle/$ID/[No character style]" Name="[No character style]"/>
</RootCharacterStyleGroup>
</idPkg:Styles>"#;

    let master = r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:MasterSpread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<MasterSpread Self="MasterSpread/ParentPage" Name="Right Body" NamePrefix="A" PageCount="2">
<Page Self="Page/Parent" Name="Parent" AppliedMaster="n"/>
</MasterSpread>
</idPkg:MasterSpread>"#;

    let designmap = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="18.0">
<idPkg:MasterSpread src="MasterSpreads/MasterSpread_u1.xml"/>
<idPkg:Story src="Stories/Story_u1.xml"/>
<idPkg:Styles src="Resources/Styles.xml"/>
<Properties>
<Label>
<KeyValuePair Key="title" Value="{}"/>
</Label>
</Properties>
<StoryList>
<Story Self="story_u1"/>
</StoryList>
</Document>
"#,
        xml_attr_escape(book.title())
    );

    zip_stored(&[
        ("mimetype", b"application/vnd.adobe.indesign-idml-package"),
        ("designmap.xml", designmap.as_bytes()),
        ("Stories/Story_u1.xml", story.as_bytes()),
        ("Resources/Styles.xml", styles.as_bytes()),
        ("MasterSpreads/MasterSpread_u1.xml", master.as_bytes()),
    ])
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_attr_escape(text: &str) -> String {
    xml_escape(text).replace('"', "&quot;")
}

/// CRC-32 (IEEE 802.3), as the zip format records it for every member.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn zip_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = body.len() as u32;
        let crc = crc32(data);
        body.extend_from_slice(&0x04034b50u32.to_le_bytes());
        body.extend_from_slice(&20u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&crc.to_le_bytes());
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(&(name.len() as u16).to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(data);

        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_start = body.len() as u32;
    body.extend_from_slice(&central);
    body.extend_from_slice(&0x06054b50u32.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&(files.len() as u16).to_le_bytes());
    body.extend_from_slice(&(files.len() as u16).to_le_bytes());
    body.extend_from_slice(&(central.len() as u32).to_le_bytes());
    body.extend_from_slice(&central_start.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn every_member_records_its_crc_and_the_designmap_is_one_element() {
        let mut book = Book::new("Quote \"A\" & B");
        let _ = book.insert("Hello.");
        let zip = export_idml(&book);
        let mut at = 0;
        let mut members = 0;
        while u32_at(&zip, at) == 0x0403_4b50 {
            let size = u32_at(&zip, at + 18) as usize;
            let name_len = u16::from_le_bytes([zip[at + 26], zip[at + 27]]) as usize;
            let data = at + 30 + name_len;
            assert_eq!(u32_at(&zip, at + 14), crc32(&zip[data..data + size]));
            if &zip[at + 30..data] == b"designmap.xml" {
                let xml = std::str::from_utf8(&zip[data..data + size]).unwrap();
                assert!(xml.trim_end().ends_with("</Document>"));
                assert_eq!(xml.matches("</Document>").count(), 1);
                assert!(xml.contains("Quote &quot;A&quot; &amp; B"));
            }
            members += 1;
            at = data + size;
        }
        assert_eq!(members, 5);
    }
}
