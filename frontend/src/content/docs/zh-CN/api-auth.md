# API 认证与地址

在知识体系的 **API 访问** 页面创建命名 Token。对外 API 是只读接口，与 Web 管理端使用的 Cookie 会话完全分离。

## Authorization Header

```http
Authorization: Bearer opk_<public-id-prefix>_<secret>
```

不要把 Token 放入 URL，也不要提交到源码仓库。不同调用方应使用不同 Token，以便分别设置 Scope、过期时间和吊销策略。

## Scope

| Scope | 权限 |
| --- | --- |
| `ontology:read` | 本体 JSON 与 TBox RDF 导出 |
| `vocabulary:read` | SKOS 词表、概念、术语解析与导出 |
| `instances:read` | 类统计、实例和断言 |
| `query:read` | 受限只读 SPARQL `SELECT` / `ASK` |
| `provenance:read` | 在实例结果中附加文档、chunk 与证据片段 |

`provenance:read` 是附加权限：必须同时拥有 `instances:read` 才能获取实例，再由该 Scope 决定是否返回来源。

## 基础地址

工作区地址读取当前可变状态，适合内部工具：

```text
https://<host>/api/v1/knowledge-systems/<public-id>
```

固定发布地址永久绑定一个不可变版本，推荐生产使用：

```text
https://<host>/api/v1/knowledge-systems/<public-id>/releases/<version>
```

最新发布别名会随下一次发布移动：

```text
https://<host>/api/v1/knowledge-systems/<public-id>/published
```

要求结果可复现时必须固定 `/releases/<version>`。固定服务停止或发布删除时返回 `410 Gone`，部署过程中返回带 `Retry-After` 的 `503`。

## OpenAPI

- Swagger：[`/api/docs`](/api/docs)
- Schema：[`/api/openapi.json`](/api/openapi.json)
