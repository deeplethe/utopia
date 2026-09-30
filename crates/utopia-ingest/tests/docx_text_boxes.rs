//! Word 文档里的文本框只读一遍。
//!
//! Word 2010 起把一个文本框写两遍，包在 `mc:AlternateContent` 里：`mc:Choice` 是新的画法
//! （wps 形状，字在 `wps:txbx/w:txbxContent`），`mc:Fallback` 是给旧版 Word 的 VML 画法
//! （字在 `v:textbox/w:txbxContent`）。两份是同一段字，读的人只看见一个框。

use std::io::{Cursor, Write};

fn read(body: &str) -> String {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (part, xml) in [
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string()),
        ("word/document.xml", format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" mc:Ignorable="w14"><w:body>{body}</w:body></w:document>"#)),
    ] {
        zip.start_file(part, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    utopia_ingest::parse("report.docx", &zip.finish().unwrap().into_inner())
        .unwrap()
        .text
}

fn drawn(text: &str) -> String {
    format!(
        r#"<w:drawing><wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" relativeHeight="251659264" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:extent cx="2743200" cy="914400"/><wp:docPr id="1" name="Text Box 1"/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr/><wps:txbx><w:txbxContent><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing>"#
    )
}

fn vml(text: &str) -> String {
    format!(
        r##"<w:pict><v:shape id="Text Box 1" o:spid="_x0000_s1026" type="#_x0000_t202" style="position:absolute"><v:textbox><w:txbxContent><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict>"##
    )
}

/// The way Word 2010 and later write a text box, and a variant with a second Choice.
fn text_boxes(text: &str) -> [String; 2] {
    [
        format!(
            r#"<w:r><mc:AlternateContent><mc:Choice Requires="wps">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent></w:r>"#,
            drawn(text),
            vml(text)
        ),
        format!(
            r#"<w:r><mc:AlternateContent><mc:Choice Requires="wps">{}</mc:Choice><mc:Choice Requires="v">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent></w:r>"#,
            drawn(text),
            vml(text),
            vml(text)
        ),
    ]
}

#[test]
fn a_text_box_is_read_once() {
    for text_box in text_boxes("Quarterly revenue rose 12%") {
        let text = read(&format!(
            r#"<w:p><w:r><w:t xml:space="preserve">Before the box. </w:t></w:r>{text_box}<w:r><w:t>After the box.</w:t></w:r></w:p><w:p><w:r><w:t>Next paragraph.</w:t></w:r></w:p>"#
        ));
        assert_eq!(
            text.matches("Quarterly revenue rose 12%").count(),
            1,
            "{text}"
        );
        assert!(text.contains("Before the box."), "{text}");
        assert!(text.contains("After the box.\nNext paragraph."), "{text}");
    }
}

#[test]
fn a_text_box_in_a_table_cell_is_read_once() {
    for text_box in text_boxes("Up 12%") {
        let text = read(&format!(
            r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Note</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>Revenue</w:t></w:r></w:p></w:tc><w:tc><w:p>{text_box}</w:p></w:tc></w:tr></w:tbl>"#
        ));
        assert!(text.contains("| Revenue | Up 12% |"), "{text}");
        assert_eq!(text.matches("Up 12%").count(), 1, "{text}");
    }
}
