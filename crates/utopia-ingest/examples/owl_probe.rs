//! 拿真实公开本体探一遍投影结果。用法：cargo run -p utopia-ingest --example owl_probe -- <文件>
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("需要文件路径");
    let bytes = std::fs::read(&path)?;
    let fmt = utopia_ingest::ontology_rdf::RdfFormat::detect(&path, &bytes);
    let p = utopia_ingest::ontology_rdf::project(&bytes, fmt)?;
    println!(
        "格式 {fmt:?} · 三元组 {} · 类 {} · 属性 {}",
        p.triples,
        p.classes.len(),
        p.properties.len()
    );
    println!("\n-- 类（前 6）--");
    for c in p.classes.iter().take(6) {
        println!(
            "  {} | {} | 父 {} | 描述 {}",
            c.key,
            c.label,
            c.parents.len(),
            if c.description.is_empty() {
                "（无）".into()
            } else {
                format!("{}…", c.description.chars().take(50).collect::<String>())
            }
        );
    }
    println!("\n-- 属性（前 6）--");
    for r in p.properties.iter().take(6) {
        println!(
            "  {} | {} | {}{}{} | domain {} range {}",
            r.key,
            r.label,
            if r.is_datatype { "字面值" } else { "对象" },
            if r.functional { " ·functional" } else { "" },
            if r.inverse_functional {
                " ·inv_functional"
            } else {
                ""
            },
            r.domains.len(),
            r.ranges.len()
        );
    }
    println!("\n-- 暂未投影（前 8）--");
    let mut u: Vec<_> = p.unprojected.iter().collect();
    u.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (k, n) in u.into_iter().take(8) {
        println!("  {n:4} × {k}");
    }
    Ok(())
}
