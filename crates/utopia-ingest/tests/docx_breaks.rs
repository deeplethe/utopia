use std::io::{Cursor, Write};

fn read(body: &str) -> String {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (part, xml) in [
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string()),
        ("word/document.xml", format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#)),
    ] {
        zip.start_file(part, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    utopia_ingest::parse("breaks.docx", &zip.finish().unwrap().into_inner())
        .unwrap()
        .text
}

fn table(run: &str) -> String {
    format!(
        r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Readings</w:t></w:r></w:p></w:tc></w:tr>
    <w:tr><w:tc><w:p><w:r><w:t>Sample</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r>{run}</w:r></w:p></w:tc></w:tr></w:tbl>"#
    )
}

#[test]
fn explicit_cell_breaks_separate_numbers_and_words() {
    for br in [
        "<w:br/>",
        "<w:cr/>",
        "<w:br></w:br>",
        "<w:cr></w:cr>",
        "<w:br/><w:br/>",
    ] {
        for (left, right) in [("10", "20"), ("hello", "world"), ("甲", "乙")] {
            let text = read(&table(&format!("<w:t>{left}</w:t>{br}<w:t>{right}</w:t>")));
            assert!(
                text.contains(&format!("| Sample | {left} {right} |")),
                "{br}: {text}"
            );
            assert_eq!(text.lines().filter(|line| line.starts_with('|')).count(), 3);
        }
    }
}

#[test]
fn paragraph_breaks_remain_newlines_and_formatting_runs_join() {
    for br in ["<w:br/>", "<w:cr/>", "<w:br></w:br>", "<w:cr></w:cr>"] {
        let text = read(&format!(
            "<w:p><w:r><w:t>Hel</w:t></w:r><w:r><w:t>lo</w:t>{br}<w:t>world</w:t></w:r></w:p>"
        ));
        assert_eq!(text.trim(), "Hello\nworld", "{br}");
    }
    let text = read(&table(
        "<w:t>Hel</w:t></w:r><w:r><w:t>lo</w:t><w:tab/><w:t>world</w:t>",
    ));
    assert!(text.contains("| Sample | Hello world |"), "{text}");
    let text = read("<w:p><w:r><w:t>first</w:t><w:tab/><w:t>line</w:t></w:r></w:p><w:p><w:r><w:t>second</w:t></w:r></w:p>");
    assert_eq!(text.trim(), "first line\nsecond");
}
