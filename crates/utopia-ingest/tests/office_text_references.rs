use std::io::{Cursor, Write};

fn package(part: &str, xml: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(part, zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(xml.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

const ENCODED: &str =
    "R&amp;D: 2 &lt; 3 &gt; 1; &quot;quote&quot; &apos;word&apos;; &#20013;&#x6587;; &amp;lt;";
const DECODED: &str = "R&D: 2 < 3 > 1; \"quote\" 'word'; 中文; &lt;";

#[test]
fn a_docx_keeps_references_in_paragraphs_and_table_cells() {
    let xml = format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
        <w:p><w:r><w:t>{ENCODED}</w:t></w:r></w:p>
        <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Team</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Budget</w:t></w:r></w:p></w:tc></w:tr>
        <w:tr><w:tc><w:p><w:r><w:t>R&amp;D</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>100</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
        </w:body></w:document>"#
    );
    let parsed = utopia_ingest::parse("report.docx", &package("word/document.xml", &xml)).unwrap();
    assert!(parsed.text.starts_with(DECODED), "{}", parsed.text);
    assert!(parsed.text.contains("| R&D | 100 |"), "{}", parsed.text);
}

#[test]
fn a_pptx_keeps_references_in_slide_text() {
    let xml = format!(
        r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{ENCODED}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
    );
    let parsed =
        utopia_ingest::parse("report.pptx", &package("ppt/slides/slide1.xml", &xml)).unwrap();
    assert!(parsed.text.contains(DECODED), "{}", parsed.text);
}

#[test]
fn office_cdata_is_literal_and_text_outside_runs_is_ignored() {
    for (filename, part, xml) in [
        ("report.docx", "word/document.xml", "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>outside&amp;<w:p><w:r><w:t>Before <![CDATA[R&D &amp;]]> after</w:t></w:r></w:p></w:body></w:document>"),
        ("report.pptx", "ppt/slides/slide1.xml", "<a:p xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">outside&amp;<a:r><a:t>Before <![CDATA[R&D &amp;]]> after</a:t></a:r></a:p>"),
    ] {
        let parsed = utopia_ingest::parse(filename, &package(part, xml)).unwrap();
        assert!(parsed.text.contains("Before R&D &amp; after"), "{}", parsed.text);
        assert!(!parsed.text.contains("outside"), "{}", parsed.text);
    }
}
