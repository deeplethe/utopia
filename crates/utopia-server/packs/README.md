# 预制本体包

来源文件按原样保存（gzip 压缩），由 `src/ontology_packs.rs` 用 `include_bytes!` 内嵌进二进制。

**为什么内嵌而不是运行时下载**：README 承诺整套系统可以运行在完全离线的内网环境。
运行时抓取会让这句话失效，也让构建不可复现（上游随时会发新版）。

**为什么压缩**：未压缩合计 1.7 MB，压缩后 316 KB。这是个公开仓库，clone 体积是真实成本。
`flate2` 本来就在依赖树里，解压是三行。

| 文件 | 来源 | 许可 | 抓取日 |
|---|---|---|---|
| `schema-org.ttl.gz` | https://schema.org/version/latest/schemaorg-current-https.ttl | CC BY-SA 3.0 | 2026-08-30 |
| `w3c-org.ttl.gz` | https://www.w3.org/ns/org.ttl | W3C Document License | 2026-08-30 |
| `prov-o.ttl.gz` | https://www.w3.org/ns/prov.ttl | W3C Document License | 2026-08-30 |
| `foaf.rdf.gz` | http://xmlns.com/foaf/spec/index.rdf | CC BY 1.0 | 2026-08-30 |
| `iof-core.rdf.gz` | https://spec.industrialontologies.org/ontology/core/Core/ | MIT | 2026-08-30 |

## 更新一个包

重新抓取、`gzip -9c` 覆盖、更新 `ontology_packs.rs` 里的计数与本文件的抓取日。
**不要改动原文内容**——投影只覆盖当下能消费的部分，原文保真是 [0001](../../../docs/decisions/0001-ontology-import-and-governance.md) 判据 1。

## 场景包（`scenarios/`，0073）

不是抓来的本体，是手写的：一个场景一个 JSON，几个类、十来条属性和关系、要回答的能力问题，
标签与定义两种语言都写。由 `src/scenario_packs.rs` 内嵌并在建库时走编辑器的路建进库。
改一个包：改 JSON，跑 `cargo test -p utopia-server scenario_packs`——它检查键不重复、
定义域和父类指向本包排在前面的类、datatype 合法、每条定义两种语言都有。
