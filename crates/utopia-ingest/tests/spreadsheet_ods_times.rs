//! ODS 的时刻格读成它显示的钟点，和 xlsx 里同一格写法一样。
//!
//! LibreOffice 把时刻存成 ISO 时长：`office:value-type="time"
//! office:time-value="PT09H30M00S"`，格子里显示的是 `09:30:00`。

use std::io::{Cursor, Write};

fn ods(rows: &[(&str, &str)]) -> Vec<u8> {
    let cells: String = rows
        .iter()
        .map(|(label, time)| {
            format!(
                r#"<table:table-row><table:table-cell office:value-type="string"><text:p>{label}</text:p></table:table-cell><table:table-cell office:value-type="time" office:time-value="{time}"><text:p>{time}</text:p></table:table-cell></table:table-row>"#
            )
        })
        .collect();
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.3"><office:body><office:spreadsheet><table:table table:name="Shifts"><table:table-row><table:table-cell office:value-type="string"><text:p>Item</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>When</text:p></table:table-cell></table:table-row>{cells}</table:table></office:spreadsheet></office:body></office:document-content>"#
    );
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.oasis.opendocument.spreadsheet")
        .unwrap();
    zip.start_file(
        "META-INF/manifest.xml",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#)
        .unwrap();
    zip.start_file("content.xml", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(content.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn an_ods_time_cell_reads_as_clock_time() {
    let text = utopia_ingest::parse(
        "shifts.ods",
        &ods(&[
            ("Daily call", "PT09H30M00S"),
            ("Cutoff", "PT17H45M30S"),
            ("Close", "PT14H29M59.999999998S"),
            ("Downtime", "PT36H15M00S"),
        ]),
    )
    .unwrap()
    .text;
    assert!(text.contains("| Daily call | 09:30 |"), "{text}");
    assert!(text.contains("| Cutoff | 17:45:30 |"), "{text}");
    assert!(text.contains("| Close | 14:30 |"), "{text}");
    assert!(text.contains("| Downtime | 36:15:00 |"), "{text}");
}
