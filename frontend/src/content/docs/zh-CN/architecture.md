# 系统架构

React 工作台负责交互，FastAPI 负责治理和服务边界；关系元数据、RDF 图和不可变制品各自进入适合的存储。

```mermaid
flowchart TB
    H[本体工程师 / 领域审核人] --> UI[React 治理工作台]
    C[下游业务应用] --> EXT[带 Scope 的只读 API]
    UI --> API[FastAPI 治理 API]
    EXT --> API
    API --> PG[(PostgreSQL)]
    API --> OXI[(工作区 Oxigraph)]
    API --> SOXI[(服务 Oxigraph)]
    API --> ART[文档与发布制品]
    API --> LLM[OpenAI 兼容 LLM]
    API --> EMB[Embedding 端点]
```

## 写入路径

浏览器会话调用治理 API；后台任务访问模型端点；通过判定的语句写入工作区 Oxigraph，同时在 PostgreSQL 写入任务、证据、审阅项和审计事件。

```mermaid
sequenceDiagram
    participant UI as React
    participant API as FastAPI
    participant PG as PostgreSQL
    participant RDF as Oxigraph
    UI->>API: 治理操作
    API->>PG: 检查用户与知识体系角色
    API->>RDF: 应用 RDF 变更
    API->>PG: 写入溯源与审计事件
    API-->>UI: 返回更新后的视图
```

## 读取路径

内部 UI 按 Owner、Editor、Viewer 角色读取工作区。机器 Token 只能调用独立的只读路由。固定版本路由只访问服务 Oxigraph 中的发布投影，不会穿透到可变工作区。

## 关键边界

- 浏览器 Cookie 会话与机器 Token 分离；
- 工作区图与发布服务图分离；
- TBox、SKOS、ABox 使用独立命名图；
- 模型只获得选中 chunk 与有界上下文；
- 图变更与发布行为必须留下审计记录。
