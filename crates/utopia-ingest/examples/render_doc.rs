//! 把一个文件按摄入层的读法打印成文本：`cargo run -p utopia-ingest --example render_doc -- <文件>`。
//! 看一张表、一份 docx 被读成什么样，比等一轮抽取快得多（#744 的教训）。
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("要一个文件路径");
    let bytes = std::fs::read(&path)?;
    let name = std::path::Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let parsed = utopia_ingest::parse(name, &bytes)?;
    print!("{}", parsed.text);
    Ok(())
}
