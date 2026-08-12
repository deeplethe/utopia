# 对外 API 指南

[English](external-api.md) · [简体中文](external-api.zh-CN.md)

OntoPilot 为消费受治理知识体系的应用提供版本化只读 API。该接口与 Web 管理端使用的 Cookie 会话
治理 API 相互独立。

## 安全模型

- 每个凭证只属于一个知识体系；
- 知识体系 Owner 可以为不同调用方创建多个命名 Token；
- Token 带有明确的只读 Scope，可设置过期时间并可独立吊销；
- 鉴权使用 SHA-256 哈希；服务端另存加密密文，知识体系 Owner 可主动再次查看有效 Token；
- 吊销 Token 时会删除其密文；升级前创建的纯哈希 Token 无法恢复，需新建替代 Token；
- 对外路由不提供抽取、编辑、审阅、历史、成员或 Token 管理操作。

请在知识体系的 **API Access** 页面创建或吊销 Token，并仅通过 HTTP Header 发送：

```http
Authorization: Bearer opk_<public-id-prefix>_<secret>
```

不要把 Token 放入 URL，也不要提交到源码仓库。

## Scope

| Scope | 权限 |
| --- | --- |
| `ontology:read` | 读取本体 JSON 和导出 TBox RDF |
| `vocabulary:read` | 读取 SKOS 词表与概念、解析受控术语并导出词表 RDF |
| `instances:read` | 读取类统计、检索个体及其断言 |
| `query:read` | 在 TBox + ABox + SKOS 上执行受限 SPARQL `SELECT` 和 `ASK` |
| `provenance:read` | 在个体结果中包含源文档、分块标识和证据片段 |

`provenance:read` 是附加权限：Token 必须同时拥有 `instances:read` 才能读取个体，只有再授予
`provenance:read` 后才会返回来源证据。

## 基础地址

每个知识体系都有稳定、非数字形式的公开标识：

```text
https://<host>/api/v1/knowledge-systems/<public-id>
```

为保持兼容，该基础地址读取当前可变工作区。生产消费方应固定到不可变发布版本：

```text
https://<host>/api/v1/knowledge-systems/<public-id>/releases/<version>
https://<host>/api/v1/knowledge-systems/<public-id>/published
```

第一个地址永久绑定具体版本；第二个地址是最新已发布版本的别名，新版本发布后可能变化。两种地址都可以继续追加 `/ontology`、`/classes`、`/individuals`、`/vocabulary/...`、`/export` 或 `/query`，`/manifest` 返回发布清单。固定服务已停止或发布已删除时返回 `410 Gone`，部署过程中返回带 `Retry-After` 的 `503`。

**API Access** 页面会展示完整基础地址。

## 接口

| 方法 | 路径 | 所需 Scope | 用途 |
| --- | --- | --- | --- |
| `GET` | `/` | 任意有效 Scope | 公开元数据、图统计和已授权 Scope |
| `GET` | `/ontology` | `ontology:read` | TBox 结构化 JSON 视图 |
| `GET` | `/export?fmt=turtle` | `ontology:read` | 导出 Turtle、RDF/XML、N-Triples 或 JSON-LD |
| `GET` | `/vocabulary/schemes` | `vocabulary:read` | 获取 SKOS 词表及统计信息 |
| `GET` | `/vocabulary/concepts` | `vocabulary:read` | 检索并分页读取受控概念 |
| `GET` | `/vocabulary/resolve?q=<term>` | `vocabulary:read` | 通过首选词、别名或隐藏检索词解析术语 |
| `GET` | `/vocabulary/export?fmt=turtle` | `vocabulary:read` | 导出词表 Turtle、RDF/XML、N-Triples 或 JSON-LD |
| `GET` | `/classes` | `instances:read` | TBox 类及其 ABox 个体数量 |
| `GET` | `/individuals` | `instances:read` | 检索并分页读取个体 |
| `GET` | `/individual?iri=<iri>` | `instances:read` | 个体类型、属性与关系 |
| `POST` | `/query` | `query:read` | 在合并 TBox + ABox 数据集上执行只读 SPARQL |

`GET /individuals` 支持 `class_iri`、`q`、`limit`（最大 `200`）和 `offset` 参数。
`GET /vocabulary/concepts` 支持 `scheme_iri`、`q`、`status`、`limit`（最大 `1000`）和 `offset`。
`GET /vocabulary/resolve` 支持 `q`、可选的 `language` 和 `limit`（最大 `100`）。

## REST 示例

```bash
export ONTOPILOT_BASE="http://localhost:8080/api/v1/knowledge-systems/<public-id>"
export ONTOPILOT_TOKEN="opk_..."

curl -sS \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  "$ONTOPILOT_BASE/ontology"

curl -sS \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  "$ONTOPILOT_BASE/individuals?q=泵&limit=20"

curl -sS \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  "$ONTOPILOT_BASE/vocabulary/resolve?q=泵&language=zh-CN"
```

RDF 导出格式包括 `turtle`、`rdfxml`、`ntriples` 和 `jsonld`。

## SPARQL 查询

查询端点将当前知识体系的 TBox、ABox 与 SKOS 词表作为一张默认 RDF 图，并自动提供 `rdf`、`rdfs`、
`owl`、`xsd`、`skos`、`dcterms` 和 `onto` 前缀。

```bash
curl -sS "$ONTOPILOT_BASE/query" \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "SELECT ?entity ?label WHERE { ?entity rdfs:label ?label } ORDER BY ?label",
    "max_rows": 100
  }'
```

`SELECT` 响应采用 SPARQL Results JSON 的 Binding 结构，并增加 `truncated` 与 `max_rows` 字段。
`ASK` 返回 `{"head": {}, "boolean": true|false}`。

接口强制执行以下限制：

- 只接受 `SELECT` 和 `ASK`；
- 拒绝 `SERVICE`、`FROM`、`GRAPH` 和所有更新操作；
- 查询数据集仅包含 Token 所属知识体系的 TBox、ABox 与 SKOS 词表；
- `max_rows` 受 `EXTERNAL_QUERY_MAX_ROWS` 限制，默认上限为 `500`；
- 查询文本受 `EXTERNAL_QUERY_MAX_CHARS` 限制，默认上限为 `20000`。

这些限制用于阻止跨知识体系读取和意外修改。面向不可信公网流量时，还应在反向代理层配置 HTTPS、
请求限速、Body 大小限制和访问日志。

## 错误码

| 状态码 | 含义 |
| --- | --- |
| `400` | 请求或查询无效、不受支持 |
| `401` | Token 缺失、无效、过期、已吊销或不属于目标知识体系 |
| `403` | Token 有效，但缺少所需 Scope |
| `404` | 请求的个体不存在 |
| `413` | SPARQL 请求超过配置的长度限制 |

Token 吊销立即生效，不影响人工登录会话和同一知识体系的其他 Token。
