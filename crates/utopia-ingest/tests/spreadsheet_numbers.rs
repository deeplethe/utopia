//! 电子表格里的数按 Excel 认的精度读：15 位有效数字。
//!
//! 公式的缓存值是按双精度的 17 位写进文件的：`=0.1+0.2` 存成 `<v>0.30000000000000004</v>`，
//! Excel 里看到的却是 0.3。这里照 Excel 写出来的样子拼一份 xlsx，值都是缓存值的写法，
//! 数字格式是常规（没有 `s`）。

use std::io::{Cursor, Write};

fn read(values: &[(&str, &str)]) -> String {
    let rows: String = values
        .iter()
        .enumerate()
        .map(|(i, (label, value))| {
            let r = i + 2;
            format!(
                r#"<row r="{r}"><c r="A{r}" t="inlineStr"><is><t>{label}</t></is></c><c r="B{r}"><v>{value}</v></c></row>"#
            )
        })
        .collect();
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (part, xml) in [
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string()),
        ("xl/workbook.xml", r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="计算" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string()),
        ("xl/_rels/workbook.xml.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string()),
        ("xl/worksheets/sheet1.xml", format!(r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>项目</t></is></c><c r="B1" t="inlineStr"><is><t>金额</t></is></c></row>{rows}</sheetData></worksheet>"#)),
    ] {
        zip.start_file(part, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    utopia_ingest::parse("计算.xlsx", &zip.finish().unwrap().into_inner())
        .unwrap()
        .text
}

#[test]
fn a_computed_number_reads_with_the_digits_excel_keeps() {
    let text = read(&[
        ("合计", "0.30000000000000004"),
        ("增长率", "0.21000000000000019"),
        ("差额", "-0.30000000000000004"),
    ]);
    for row in ["| 合计 | 0.3 |", "| 增长率 | 0.21 |", "| 差额 | -0.3 |"] {
        assert!(text.contains(row), "{row}: {text}");
    }
}

#[test]
fn a_number_that_was_already_clean_reads_as_before() {
    let text = read(&[
        ("含税金额", "1395.0527999999999"),
        ("单价", "19.899999999999999"),
        ("数量", "1200"),
        ("编号", "123456789012345000"),
        ("比例", "0.125"),
    ]);
    for row in [
        "| 含税金额 | 1395.0528 |",
        "| 单价 | 19.9 |",
        "| 数量 | 1200 |",
        "| 编号 | 123456789012345000 |",
        "| 比例 | 0.125 |",
    ] {
        assert!(text.contains(row), "{row}: {text}");
    }
}
