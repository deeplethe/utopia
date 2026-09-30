//! PDF 文字层里长得一样的码位（部首、连字）读成它们显示的那个字。
//!
//! 照 macOS 生成的 PDF 的样子拼一份：Type0 字体、Identity-H，ToUnicode 把字形指到部首和
//! 连字的码位——Quartz 与 Chrome 用苹方生成的 PDF 就是这样指的。不带字体文件，取字只看
//! ToUnicode。

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream, StringFormat};

// Build a one-line PDF whose glyph i+1 maps to the i-th character of `shown` through ToUnicode.
fn pdf_showing(shown: &str) -> Vec<u8> {
    let chars: Vec<char> = shown.chars().collect();
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    cmap.push_str(&format!("{} beginbfchar\n", chars.len()));
    for (i, c) in chars.iter().enumerate() {
        let utf16: String = c
            .encode_utf16(&mut [0; 2])
            .iter()
            .map(|unit| format!("{unit:04X}"))
            .collect();
        cmap.push_str(&format!("<{:04X}> <{utf16}>\n", i + 1));
    }
    cmap.push_str("endbfchar\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");

    let mut document = Document::with_version("1.4");
    let pages_id = document.new_object_id();
    let to_unicode_id = document.add_object(Stream::new(dictionary! {}, cmap.into_bytes()));
    let descriptor_id = document.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => "Lookalikes-Regular",
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
        "Subtype" => "CIDFontType2",
        "BaseFont" => "Lookalikes-Regular",
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("Identity"),
            "Supplement" => 0,
        },
        "FontDescriptor" => descriptor_id,
        "DW" => 1000,
    });
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "Lookalikes-Regular",
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::Reference(cid_font_id)],
        "ToUnicode" => to_unicode_id,
    });
    let resources_id = document.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });
    let glyphs: Vec<u8> = (1..=chars.len() as u16)
        .flat_map(u16::to_be_bytes)
        .collect();
    let content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new("Td", vec![50.into(), 750.into()]),
            Operation::new(
                "Tj",
                vec![Object::String(glyphs, StringFormat::Hexadecimal)],
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

#[test]
fn a_mac_pdf_is_read_with_the_characters_it_shows() {
    // ⽂ ⾃ ⽇ ⾏ ⼀ ⽤ 是康熙部首，⻄ 是部首补充区的，ﬁ ﬂ 是连字
    let shown = "时间线按⽂件⾃带的⽇期排⾏，⼀律⽤⻄历。ﬁscal ﬂow";
    let parsed = utopia_ingest::parse("report.pdf", &pdf_showing(shown)).unwrap();
    assert!(
        parsed
            .text
            .contains("时间线按文件自带的日期排行，一律用西历。fiscal flow"),
        "{}",
        parsed.text
    );
    assert!(
        !parsed.text.chars().any(|c| matches!(
            c,
            '\u{2E80}'..='\u{2EFF}' | '\u{2F00}'..='\u{2FDF}' | '\u{FB00}'..='\u{FB06}'
        )),
        "{}",
        parsed.text
    );
}
