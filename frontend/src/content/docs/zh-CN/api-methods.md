# 接口与调用示例

下列路径可以追加到工作区、`published` 别名或固定版本地址；`/manifest` 仅适用于发布地址。

| 方法 | 路径 | Scope | 用途 |
| --- | --- | --- | --- |
| GET | `/` | 任意有效 Scope | 元数据、图统计、发布信息和已授权 Scope |
| GET | `/ontology` | `ontology:read` | TBox 结构化 JSON |
| GET | `/export?fmt=turtle` | `ontology:read` | 导出 TBox RDF |
| GET | `/vocabulary/schemes` | `vocabulary:read` | SKOS 词表和统计 |
| GET | `/vocabulary/concepts` | `vocabulary:read` | 搜索、筛选并分页读取概念 |
| GET | `/vocabulary/resolve?q=<term>` | `vocabulary:read` | 按首选词、别名或隐藏词解析术语 |
| GET | `/vocabulary/export?fmt=turtle` | `vocabulary:read` | 导出 SKOS RDF |
| GET | `/classes` | `instances:read` | 类及其实例数量 |
| GET | `/individuals` | `instances:read` | 搜索并分页读取实例 |
| GET | `/individual?iri=<iri>` | `instances:read` | 实例类型、属性、关系与可选溯源 |
| POST | `/query` | `query:read` | 只读 SPARQL `SELECT` / `ASK` |
| GET | `/manifest` | `ontology:read` | 不可变发布清单，仅发布地址 |

## REST 示例

```bash
export ONTOPILOT_BASE="http://localhost:8080/api/v1/knowledge-systems/<public-id>/releases/v1"
export ONTOPILOT_TOKEN="opk_..."

curl -sS \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  "$ONTOPILOT_BASE/ontology"
```

## 实例与术语查询

```bash
curl -sS \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  "$ONTOPILOT_BASE/individuals?q=泵&limit=20"

curl -sS \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  "$ONTOPILOT_BASE/vocabulary/resolve?q=泵&language=zh-CN"
```

## SPARQL

```bash
curl -sS "$ONTOPILOT_BASE/query" \
  -H "Authorization: Bearer $ONTOPILOT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "SELECT ?entity ?label WHERE { ?entity rdfs:label ?label } ORDER BY ?label",
    "max_rows": 100
  }'
```

查询只接受 `SELECT` 和 `ASK`；`SERVICE`、`FROM`、`GRAPH` 与更新操作会被拒绝。默认最大 500 行，查询文本默认上限 20,000 字符，响应会通过 `truncated` 说明是否截断。

## 常见错误

| 状态码 | 含义 |
| --- | --- |
| 400 | 参数、格式或 SPARQL 无效 |
| 401 | Token 缺失、无效、过期、已吊销或不属于目标知识体系 |
| 403 | Token 有效但缺少 Scope |
| 404 | 资源或发布不存在 |
| 410 | 发布服务停止或版本已删除 |
| 413 | 查询文本超过限制 |
