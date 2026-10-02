//! 幻灯片上的换行把两边的字隔开。
//!
//! 在 PowerPoint 里按 Shift+Enter 换的行不是新段落，是段落里的一个 `<a:br>`，前后各是一个
//! run；PowerPoint 写的是 `<a:br><a:rPr …/></a:br>`，别的程序也写 `<a:br/>`。标题
//! 「Q1」换行「2024」、指标「Revenue」换行「$1.2B」都是这个形状。

use std::io::{Cursor, Write};

fn deck(paragraphs: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (part, xml) in [
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#.to_string()),
        ("ppt/presentation.xml", r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId2"/></p:sldIdLst></p:presentation>"#.to_string()),
        ("ppt/_rels/presentation.xml.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#.to_string()),
        ("ppt/slides/slide1.xml", format!(r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:bodyPr/><a:lstStyle/>{paragraphs}</p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#)),
    ] {
        zip.start_file(part, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn read(paragraphs: &str) -> String {
    utopia_ingest::parse("deck.pptx", &deck(paragraphs))
        .unwrap()
        .text
}

fn broken(before: &str, br: &str, after: &str) -> String {
    format!(
        r#"<a:p><a:r><a:rPr lang="en-US"/><a:t>{before}</a:t></a:r>{br}<a:r><a:rPr lang="en-US"/><a:t>{after}</a:t></a:r></a:p>"#
    )
}

#[test]
fn a_line_break_on_a_slide_keeps_the_words_on_either_side_apart() {
    for br in [
        r#"<a:br><a:rPr lang="en-US" dirty="0"/></a:br>"#,
        "<a:br/>",
        "<a:br></a:br>",
    ] {
        let text = read(&format!(
            "{}{}",
            broken("Q1", br, "2024"),
            broken("Revenue", br, "$1.2B")
        ));
        assert!(text.contains("Q1\n2024\n"), "{br}: {text}");
        assert!(text.contains("Revenue\n$1.2B\n"), "{br}: {text}");
        assert!(!text.contains("Q12024"), "{br}: {text}");
    }
}

#[test]
fn runs_without_a_break_still_join() {
    let text = read(
        r#"<a:p><a:r><a:t>Hel</a:t></a:r><a:r><a:t>lo</a:t></a:r></a:p><a:p><a:r><a:t>world</a:t></a:r></a:p>"#,
    );
    assert!(text.contains("## Slide 1\nHello\nworld\n"), "{text}");
}
