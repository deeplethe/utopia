use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream, StringFormat};

// Build a Type0 PDF with a named CMap and no ToUnicode map, using only test text.
fn gbk_pdf(text: &str) -> Vec<u8> {
    let mut document = Document::with_version("1.4");
    let pages_id = document.new_object_id();
    let descriptor_id = document.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => "STSong-Light",
        "Flags" => 4,
        "FontBBox" => vec![0.into(), (-200).into(), 1000.into(), 900.into()],
        "ItalicAngle" => 0,
        "Ascent" => 880,
        "Descent" => -120,
        "CapHeight" => 880,
        "StemV" => 80,
    });
    let cid_font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType0",
        "BaseFont" => "STSong-Light",
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("GB1"),
            "Supplement" => 2,
        },
        "FontDescriptor" => descriptor_id,
        "DW" => 1000,
    });
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "STSong-Light",
        "Encoding" => "GBK-EUC-H",
        "DescendantFonts" => vec![Object::Reference(cid_font_id)],
    });
    let resources_id = document.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });
    let (encoded, _, had_errors) = encoding_rs::GBK.encode(text);
    assert!(!had_errors);
    let content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 16.into()]),
            Operation::new("Td", vec![50.into(), 750.into()]),
            Operation::new(
                "Tj",
                vec![Object::String(encoded.into_owned(), StringFormat::Literal)],
            ),
            Operation::new("ET", vec![]),
        ],
    };
    let content_id = document.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
    });
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    bytes
}

/// 回退用的 `pdftotext` 是不是 Poppler。同名的程序有两个实现——Git for Windows 带的是
/// Xpdf 4.00，它对这份文件退出码 0、输出为空——只有 Poppler 连着 `poppler-data` 的 CJK
/// CMap 表读得了 `GBK-EUC-H`。
fn fallback_is_poppler() -> bool {
    let Ok(out) = std::process::Command::new("pdftotext").arg("-v").output() else {
        return false;
    };
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    said.to_lowercase().contains("poppler")
}

#[test]
fn gbk_euc_h_pdf_extracts_chinese_text() {
    if !fallback_is_poppler() {
        // CI 装了 Poppler 并设了这个变量：那里不许跳过，否则绿色是假的
        assert!(
            std::env::var("UTOPIA_TEST_REQUIRE_PDFTOTEXT").is_err(),
            "this run requires Poppler: install poppler-utils and poppler-data"
        );
        eprintln!("skipped: pdftotext on PATH is not Poppler");
        return;
    }
    let expected = "中文测试";
    let bytes = gbk_pdf(expected);
    let parsed = utopia_ingest::parse("sample.pdf", &bytes).unwrap();
    assert!(parsed.text.contains(expected), "{}", parsed.text);
}

#[test]
fn empty_gbk_euc_h_pdf_still_requests_ocr() {
    let bytes = gbk_pdf("");
    let error = utopia_ingest::parse("empty.pdf", &bytes).unwrap_err();
    assert!(error.downcast_ref::<utopia_ingest::NeedsReader>().is_some());
}
