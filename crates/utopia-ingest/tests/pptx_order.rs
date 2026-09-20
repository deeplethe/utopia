use std::io::{Cursor, Write};

type Parts = Vec<(String, String)>;

fn package(parts: &Parts) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, xml) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn slide(text: &str) -> String {
    format!(
        r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
    )
}

fn deck() -> Parts {
    vec![
        ("[Content_Types].xml".into(),r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/slides/slide17.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>"#.into()),
        ("_rels/.rels".into(),r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="office" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#.into()),
        ("ppt/presentation.xml".into(),r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="257" r:id="rIdB"/><p:sldId id="256" r:id="rIdA"/></p:sldIdLst></p:presentation>"#.into()),
        ("ppt/_rels/presentation.xml.rels".into(),r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdA" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/><Relationship Id="rIdB" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide17.xml"/><Relationship Id="notes" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="notesSlides/notesSlide1.xml"/></Relationships>"#.into()),
        ("ppt/slides/slide2.xml".into(),slide("FIRST-CREATED")),
        ("ppt/slides/slide17.xml".into(),slide("SECOND-CREATED &amp; &#x4E2D; <![CDATA[<raw>]]>")),
        ("ppt/slides/slide99.xml".into(),slide("UNLISTED")),
        ("ppt/notesSlides/notesSlide1.xml".into(),slide("NOTES")),
        ("ppt/slideMasters/slideMaster1.xml".into(),slide("MASTER")),
    ]
}

fn part<'a>(parts: &'a mut Parts, name: &str) -> &'a mut String {
    &mut parts.iter_mut().find(|(n, _)| n == name).unwrap().1
}

fn parsed(parts: &Parts) -> String {
    utopia_ingest::parse("slides.pptx", &package(parts))
        .unwrap()
        .text
}

#[test]
fn logical_order_and_page_numbers_do_not_depend_on_zip_or_part_order() {
    let mut parts = deck();
    let expected = parsed(&parts);
    assert!(
        expected.contains("## Slide 1\nSECOND-CREATED & 中 <raw>"),
        "{expected}"
    );
    assert!(expected.contains("## Slide 2\nFIRST-CREATED"), "{expected}");
    assert!(!expected.contains("UNLISTED"));
    assert!(!expected.contains("NOTES"));
    assert!(!expected.contains("MASTER"));
    for _ in 0..parts.len() {
        parts.rotate_left(1);
        assert_eq!(parsed(&parts), expected);
    }
    parts.reverse();
    assert_eq!(parsed(&parts), expected);
    let xml = part(&mut parts, "ppt/presentation.xml");
    *xml = xml
        .replace("rIdB", "temp")
        .replace("rIdA", "rIdB")
        .replace("temp", "rIdA");
    let normal = parsed(&parts);
    assert!(normal.contains("## Slide 1\nFIRST-CREATED"));
    assert!(normal.contains("## Slide 2\nSECOND-CREATED"));
}

#[test]
fn relationships_resolve_internal_paths_and_namespaces() {
    let expected = parsed(&deck());
    let mut parts = deck();
    // The root relationship can locate a differently named main part.
    for (name, xml) in &mut parts {
        *name = name
            .replace("ppt/presentation.xml", "deck/main.xml")
            .replace(
                "ppt/_rels/presentation.xml.rels",
                "deck/_rels/main.xml.rels",
            );
        *xml = xml.replace("ppt/presentation.xml", "deck/main.xml");
    }
    let xml = part(&mut parts, "deck/_rels/main.xml.rels");
    *xml = xml
        .replace("slides/slide2.xml", "../ppt/slides/./slide2.xml")
        .replace("slides/slide17.xml", "/ppt/slides/slide17.xml");
    let xml = part(&mut parts, "deck/main.xml");
    *xml = xml
        .replace("xmlns:p=", "xmlns:q=")
        .replace("<p:", "<q:")
        .replace("</p:", "</q:")
        .replace("xmlns:r=", "xmlns:rel=")
        .replace("r:id", "rel:id");
    assert_eq!(parsed(&parts), expected);
    // Relationship values are XML-unescaped, then percent-decoded as package URIs.
    let mut parts = deck();
    let xml = part(&mut parts, "ppt/_rels/presentation.xml.rels");
    *xml = xml
        .replace("slides/slide17.xml", "slides/B%20&amp;%20C.xml")
        .replace("rIdB", "rId&amp;B");
    let xml = part(&mut parts, "ppt/presentation.xml");
    *xml = xml.replace("rIdB", "rId&amp;B");
    parts
        .iter_mut()
        .find(|(n, _)| n == "ppt/slides/slide17.xml")
        .unwrap()
        .0 = "ppt/slides/B & C.xml".into();
    assert_eq!(parsed(&parts), expected);
    let mut strict = deck();
    for (_, xml) in &mut strict {
        *xml = xml
            .replace(
                "http://schemas.openxmlformats.org/presentationml/2006/main",
                "http://purl.oclc.org/ooxml/presentationml/main",
            )
            .replace(
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                "http://purl.oclc.org/ooxml/officeDocument/relationships",
            );
    }
    assert_eq!(parsed(&strict), expected);
}

#[test]
fn present_but_broken_manifests_fail_instead_of_guessing_or_skipping_pages() {
    for (old, new) in [
        ("slides/slide17.xml", "slides/missing.xml"),
        ("slides/slide17.xml", "../../../outside.xml"),
        ("slides/slide17.xml", "%2e%2e/%2e%2e/outside.xml"),
        ("slides/slide17.xml", "https://example.test/slide.xml"),
        (
            "Target=\"slides/slide17.xml\"",
            "Target=\"https://example.test/slide.xml\" TargetMode=\"External\"",
        ),
        ("Id=\"rIdB\"", "Id=\"rIdA\""),
        ("</Relationships>", "</Broken>"),
        ("</Relationships>", ""),
    ] {
        let mut parts = deck();
        let xml = part(&mut parts, "ppt/_rels/presentation.xml.rels");
        *xml = xml.replace(old, new);
        assert!(
            utopia_ingest::parse("broken.pptx", &package(&parts)).is_err(),
            "{old} -> {new}"
        );
    }
    for (name, old, new) in [
        ("ppt/presentation.xml", "rIdB", "missing"),
        (
            "ppt/_rels/presentation.xml.rels",
            "/relationships/slide\"",
            "/relationships/notesSlide\"",
        ),
        (
            "_rels/.rels",
            "Target=\"ppt/presentation.xml\"",
            "Target=\"https://example.test/deck.xml\" TargetMode=\"External\"",
        ),
        ("_rels/.rels", "</Relationships>", ""),
    ] {
        let mut parts = deck();
        let xml = part(&mut parts, name);
        *xml = xml.replace(old, new);
        assert!(
            utopia_ingest::parse("broken.pptx", &package(&parts)).is_err(),
            "{name}: {old}"
        );
    }
    for name in [
        "ppt/slides/slide17.xml",
        "ppt/_rels/presentation.xml.rels",
        "ppt/presentation.xml",
    ] {
        let mut parts = deck();
        parts.retain(|(n, _)| n != name);
        assert!(
            utopia_ingest::parse("broken.pptx", &package(&parts)).is_err(),
            "missing {name}"
        );
    }
    for xml in [
        "<bad/>",
        "<p:presentation",
        r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldIdLst><p:sldId id="256"/></p:sldIdLst></p:presentation>"#,
    ] {
        let mut parts = deck();
        *part(&mut parts, "ppt/presentation.xml") = xml.into();
        assert!(
            utopia_ingest::parse("broken.pptx", &package(&parts)).is_err(),
            "{xml}"
        );
    }
}

#[test]
fn empty_presentations_and_manifest_free_legacy_packages_keep_their_behavior() {
    let mut parts = deck();
    *part(&mut parts,"ppt/presentation.xml")=r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldIdLst/></p:presentation>"#.into();
    assert!(utopia_ingest::parse("empty.pptx", &package(&parts))
        .unwrap_err()
        .to_string()
        .contains("No text could be extracted"));
    let legacy = vec![
        ("ppt/slides/slide17.xml".into(), slide("Later")),
        ("ppt/slides/slide2.xml".into(), slide("Earlier")),
    ];
    let text = parsed(&legacy);
    assert!(text.contains("## Slide 2\nEarlier"));
    assert!(text.contains("## Slide 17\nLater"));
    assert!(text.find("Earlier") < text.find("Later"));
    let mut no_root = deck();
    no_root.retain(|(n, _)| n != "_rels/.rels");
    assert_eq!(parsed(&no_root), parsed(&deck()));
}
