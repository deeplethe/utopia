# 直接导入 RDF

OntoPilot 可以把已有 RDF 文档直接写入知识体系，不经过文档解析、分块、实体抽取或 LLM 请求。

## 支持格式

| 格式 | 常用扩展名 | `format` 值 |
| --- | --- | --- |
| Turtle | `.ttl`、`.owl` | `turtle` |
| RDF/XML | `.rdf`、`.xml`、`.owl` | `rdfxml` |
| N-Triples | `.nt` | `ntriples` |
| JSON-LD | `.jsonld`、`.json` | `jsonld` |

`auto` 会结合扩展名和内容识别格式。由于 `.owl` 常见 Turtle 和 RDF/XML 两种编码，系统会读取
内容后判断。

当前不接收 TriG 和 N-Quads，因为一个 OntoPilot 知识体系已经固定拥有 TBox 与 ABox 两张 named graph。

## 导入目标

| 目标 | 行为 |
| --- | --- |
| `auto` | 将 OWL/RDFS 模式分到 TBox，其余事实分到 ABox |
| `tbox` | 把全部三元组写入本体图 |
| `abox` | 把全部三元组写入实例图 |

自动分流会识别类、属性、本体声明、Restriction、定义域/值域、类与属性关系、OWL Collection、
Property Chain 和 SHACL Shape。已识别模式资源的标签和自定义 Annotation 会跟随资源进入 TBox；
Named Individual 和普通断言保留在 ABox。

自动分流是有意保持保守的启发式规则。使用 OWL Punning、未声明模式资源或领域元模型时，应选择
明确的 TBox 或 ABox 目标。

## 写入方式

- `merge` 只添加缺失三元组，不删除现有内容；
- `replace` 先清空所选目标图，再写入解析结果；
- `auto + replace` 会同时替换 TBox 和 ABox。

系统会先完成解析和限额检查，再执行清空。每张发生变化的图都会在历史中保存精确 N-Triples 差异，
因此合并和替换都可回滚；跨两张图的导入使用同一历史分组，并作为一个整体回滚。

Blank Node 使用知识体系、Base IRI、导入目标和源文件 SHA-256 做作用域隔离：无关导入不会因相同
局部标签意外合并，同一文件以相同选项重复导入则保持幂等。

## Web API

治理 API 要求已登录的人类用户具备 Editor 或 Owner 权限。知识体系外部 Token 始终只读，不能导入 RDF。

```http
POST /api/knowledge/{knowledge_system_id}/rdf/import
Content-Type: multipart/form-data
```

Multipart 字段：

| 字段 | 必填 | 可选值 / 默认值 |
| --- | --- | --- |
| `file` | 是 | RDF 文件 |
| `target` | 否 | `auto`（默认）、`tbox`、`abox` |
| `strategy` | 否 | `merge`（默认）、`replace` |
| `format` | 否 | `auto`（默认）、`turtle`、`rdfxml`、`ntriples`、`jsonld` |
| `base_iri` | 否 | 默认使用知识体系 Base IRI，用于解析相对 IRI |

响应包含解析与分流数量、每张图的净增删、本体最新视图、待处理冲突和 ABox 验证计数。

## 限额与存储

| 环境变量 | 默认值 |
| --- | --- |
| `RDF_IMPORT_MAX_BYTES` | `26214400`（25 MiB） |
| `RDF_IMPORT_MAX_TRIPLES` | `250000` |

OntoPilot 保存导入后的三元组、源文件名、SHA-256、导入选项和可逆图差异，不额外保存上传的 RDF
原文件。文档/分块级溯源适用于 LLM 抽取结果；直接导入提供文件级审计溯源。

本体工作台只展示其已理解的 OWL 子集。其他合法 RDF 三元组仍保存在 Oxigraph 中，可通过 RDF 导出
和授权 SPARQL 查询访问。
