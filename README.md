# OntoPilot

**Turn your documents into a curated ontology.** OntoPilot parses documents, extracts a schema
(TBox) *and* instances (ABox) with cheap LLMs, and gives you an interactive, agent-assisted, and
fully traceable ontology — all local-first (FastAPI + embedded Oxigraph RDF store + SQLite; the only
external call is the LLM API via OpenRouter).

<!-- Add a screenshot here before release, e.g. the Ontology workbench:
     ![OntoPilot workbench](docs/screenshot-workbench.png) -->

**Highlights**

- **Extraction** — documents → chunks → TBox (classes, subclass, object/data properties) and ABox
  (individuals + assertions), retrieval-augmented or agentic (LLM tool-loop).
- **Agentic pipeline with learned memory** — entity resolution, duplicate/conflict resolution,
  domain–range reconciliation, datatype-validation triage, and isolated-class attachment. Each agent
  consults its past (human-editable, revocable) decisions and self-resolves recurring cases.
- **Provenance / 溯源** — every individual and every assertion traces back to its source document
  and text snippet.
- **Human-in-the-loop Review** — one filterable queue per concern (Conflicts / Entity resolution /
  Validation) with agent recommendations and one-click confirm.
- **Ontology workbench** — hierarchy tree + neighbourhood graph + flat tables, one shared selection,
  a cross-linked inspector, LaTeX-rendered axioms.
- **Also** — instances browser, change history with graph-scoped rollback, per-KS roles, and
  RDF/OWL export (Turtle / RDF-XML / N-Triples / JSON-LD).

Quick start: [backend](#后端安装与运行) then [frontend](#前端安装与运行) below (default login `admin` / `admin`
— change it via `backend/.env`). 中文文档见下。

---

从上传的文档中抽取本体（TBox），构建并可视化本地本体图。文档解析、分块、LLM 抽取、
RDF/OWL 存储、Web 前端一体化。所有用户共享同一套系统（无多租户）。

> **里程碑 1（核心链路）**：上传 → 哈希分层存储 + SQLite 元数据 → docling 解析 → 分块 →
> 新建知识体系 → 选中分块用 LLM 抽取 TBox → Oxigraph 落库 → 前端可视化本体图。
>
> **里程碑 2（已完成）**：冲突/矛盾检测队列（子类环、disjoint 违反、domain/range 冲突、
> equivalent+disjoint 矛盾、重复类）+ 用户解决冲突；Web 界面手动增删改本体图。

## 架构

```
前端  React + Vite + TypeScript + Tailwind + shadcn/ui   (frontend/)
  │   REST/JSON，开发期 Vite 代理 /api → 后端
后端  FastAPI (Python)                                    (backend/)
  ├─ storage/   内容寻址文件库：sha256 → 分片目录 aa/bb/<hash>
  ├─ db/        SQLite 元数据 (SQLModel)：Document / Chunk / KnowledgeSystem / ExtractionJob / AxiomProvenance
  ├─ parsing/   docling 结构化解析 + HybridChunker；轻量兜底(pypdf/python-docx/openpyxl/文本)
  ├─ llm/       OpenRouter 客户端（DeepSeek 等便宜模型）
  └─ ontology/  Oxigraph 嵌入式 RDF 库（每个知识体系 = 一个 named graph）+ RDFLib 操作/导出
```

技术选型说明见 `backend/app/ontology/`：本体用 **RDF/OWL**（TBox/ABox 本就是 OWL 概念），
存储用 **Oxigraph** 嵌入式图库（零服务、持久化、SPARQL），冲突检测（里程碑 2）走 pyshacl/owlrl 纯 Python 栈。

## 前置条件

- Python 3.12+（Windows 上注意用真实解释器路径，而非 Microsoft Store 存根）
- Node 20+ 与 pnpm
- OpenRouter API Key（放到 `backend/.env`）

## 后端：安装与运行

```bash
cd backend
python -m venv .venv
.venv\Scripts\activate            # Windows PowerShell: .venv\Scripts\Activate.ps1
pip install -r requirements.txt   # docling 较重（含 ML 栈），可稍等；缺席时自动走兜底解析器
```

在 `backend/.env` 写入（不要提交到版本库）：

```
OPENROUTER_API_KEY=sk-or-v1-xxxxxxxx
LLM_EXTRACT_MODEL=deepseek/deepseek-chat
LLM_TEMPERATURE=0.1
LLM_MAX_TOKENS=4000
# 语义近义类检测用的 embedding 模型（也走 OpenRouter，无需本地下载模型）
EMBEDDING_MODEL=baai/bge-m3
```

启动：

```bash
uvicorn app.main:app --host 127.0.0.1 --port 8000 --reload
```

- 健康检查：<http://127.0.0.1:8000/api/health>
- 交互式 API 文档：<http://127.0.0.1:8000/docs>
- 运行时数据都在 `backend/data/`（`blobs/` 原文件、`ontopilot.db` 元数据、`oxigraph/` 本体图），已 gitignore。

## 前端：安装与运行

```bash
cd frontend
pnpm install
pnpm dev        # http://localhost:5173
```

Vite 已配置把 `/api` 代理到 `http://127.0.0.1:8000`，所以开发期无需处理 CORS。

## 使用流程

1. **文档**页：按**虚拟文件夹**组织文档（面包屑导航、上传到当前文件夹、移动、新建文件夹）。上传 PDF/Word/Excel/TXT/Markdown/CSV，点「解析」得到文本分块，可「查看分块」。文件夹纯靠 `folder` 字段实现，物理仍按哈希内容寻址存储。
   - **删除文档**会弹「影响审阅」：列出「仅来自本文档、无其他文档支撑」的公理（按知识体系分组、人类可读）。这些公理**不允许保留**（否则会留下无来源的孤儿公理），删除时一并移除；需**逐条确认**（或「全部确认」）后删除按钮才启用。移除按溯源精确删三元组，并 GC 掉因此变孤立的类/属性；被其他文档共撑或手动添加的公理不受影响。
   - **修改文档** = 上传新版本（内容寻址下即新内容/新哈希）：新版本进来后重新解析，并走旧版本的删除影响审阅来处理它的本体贡献。
2. **知识体系**页：新建一个知识体系（= 一张本体图）。
3. 进入知识体系详情，点「**从文档抽取**」：选文档 → 勾选分块 → 选模型（默认 `deepseek/deepseek-chat`）→ 抽取。抽取是**后台任务**，页面顶部显示「抽取中 x/y 分块」进度条，前端每 ~1.5s 主动轮询；**中途刷新页面也不丢进度**（进度存在服务端，重载后自动接管）。
4. 抽取出的 TBox 会合并进本体图。「本体图」标签有两种视图（右上角切换）：
   - **浏览（Explorer，默认，适合大本体）**：左=可折叠类层级树 + 搜索；中=选中类的 1 跳邻域聚焦图（父类/子类/对象属性相连的类，点邻居可重新聚焦）；右=详情面板（说明、父子类、进出对象属性、数据属性、公理，带编辑/删除入口）。几百个类也能靠树折叠 + 聚焦图浏览，不会糊成一团。
   - **全图**：力导向全景图，适合小本体或整体一览。
   也可在「类与属性 / 公理」看明细表，或「导出 Turtle」（可直接用 Protégé 打开）。
5. **冲突**：每次抽取后自动检测冲突并入队；也可手动点「检测冲突」跑一次完整（含语义）检测。在「冲突」标签逐条解决（点建议的解决方案，如"删除某条子类关系"、"合并 A→B"）或忽略。解决根因时相关的派生冲突会自动一并清除。
6. **手动编辑**：在「类与属性」页新增/编辑/删除类与属性（含 domain/range），在「公理」页新增/删除子类、不相交、等价关系。编辑走快路径（仅结构检查，毫秒级），本体图、统计、冲突队列即时刷新。

## 实现要点

- **哈希分层存储**：文件按 SHA-256 内容寻址，存于 `blobs/aa/bb/<hash>`，同内容只存一份。
- **解析兜底**：docling 为默认后端；未装好或失败时自动降级到 pypdf/python-docx/openpyxl/纯文本，核心链路不被阻塞。
- **本体去重**：类/属性按规范化标签（大小写、分隔符无关）生成稳定 IRI，跨分块/多次抽取自动合并；Oxigraph 层三元组幂等。
- **溯源**：每条公理记录来源分块（`AxiomProvenance` 表），可查「哪个分块产生了哪条公理」。
- **LLM**：复用 OpenRouter `chat/completions`，默认 DeepSeek 便宜模型，严格 JSON 输出 + 容错解析。
- **后台抽取 + 轮询**：`/extract` 立即返回 job（`asyncio` 后台跑），进度写入 job 行；前端轮询 `GET /jobs/{id}`。
  比 SSE 稳——刷新/断线不丢状态。冲突检测在后台线程里做，不阻塞事件循环。

## 冲突检测与编辑（里程碑 2）

- **检测的冲突类型**：子类环（cycle）、不相交违反（disjoint_subclass / disjoint_common）、
  定义域/值域多值冲突（domain_multi / range_multi）、等价与不相交矛盾（equiv_disjoint）、
  疑似重复类（duplicate）。检测逻辑在 `backend/app/ontology/conflicts.py`。
- **domain/range 是 TBox 概念**（模式级公理，非 ABox）。多个 `rdfs:domain`/`rdfs:range` 在 RDFS 语义下是
  「交集」（须同时满足），常非本意——所以标为 `warning` 级并给出解法：**改为 `owl:unionOf` 并集**、
  **收敛到公共父类**（若存在）、或**只保留一个**。并集在视图里显示成「A ∪ B」，Turtle 里是标准 `owl:unionOf`。
- **语义重复检测（embedding + LLM 裁判）**：先用 **OpenRouter 的 embedding**（默认多语言 `baai/bge-m3`，
  中英都强、无需本地下载模型）把所有类两两算余弦、生成候选；再用便宜 LLM（DeepSeek）逐对判「是否同一概念」。
  这样避免了纯阈值把"相关/兄弟类"误判成"重复"（如 `Degree/Grade`、`本科生/研究生`），只保留真同义（如 `Person/Human`、`泵/水泵`）。
  可用 `ENABLE_SEMANTIC_CONFLICTS` / `VERIFY_DUPLICATES_WITH_LLM` 开关。
- **冲突队列**：`Conflict` 表按 `signature` 去重——已解决/忽略的不会反复弹出；一旦底层问题消失（如被编辑修掉），
  对应的未决冲突自动标记为已解决。每个冲突自带若干「解决方案」（本质是一条编辑操作），一键应用。
- **编辑操作**（`backend/app/ontology/editor.py`）：add/update/delete class、add/update/delete property、
  add/delete axiom、merge_classes（合并重复类，自动重指向所有引用）。冲突解决与手动编辑共用这套操作。
  手动编辑只跑结构检查（毫秒级），语义检测放在抽取和「检测冲突」按钮上，保证编辑手感。
- **IRI 稳定性**：类/属性 IRI 一经创建即固定，改名只改 `rdfs:label`；再次抽取时按图中已有标签合并，避免重复。

## 抽取：检索驱动 / 智能体

抽取会**参考知识体系已有的本体**并与之对齐，所以同一文档解析到不同知识体系会得到**不同**的、契合各自本体的结果。三种模式（`EXTRACTION_MODE` = `rag` / `agentic` / `auto`，默认 auto）：

- **A · 检索增强单次（rag）**：每个分块先**向量检索**出与它相关的现有本体片（`search_ontology`，`app/ontology/retrieval.py`），把这一小片聚焦上下文喂给一次抽取。可扩展到大本体（只喂相关片，不 dump 全部）、精准、快。
- **B · 智能体（agentic）**：ReAct 式工具循环——LLM 自主调 `search_ontology` / `get_neighborhood`（图检索父/子类/属性）多轮探索，再产出本体 delta。最强，但多轮往返更慢；失败自动回退 A。
- **auto**：本体类数 ≥ `AGENTIC_MIN_CLASSES`（默认 12）用 agentic，否则用 rag。

检索复用 OpenRouter embedding（实体向量按知识体系内存缓存、增量更新），无需额外向量库。

**并发抽取**：一个抽取任务的多个分块**并发**跑（`EXTRACTION_CONCURRENCY` 上限，默认 5）。LLM/agent 调用重叠，图写入由锁串行化（无写写竞争，事件循环不被阻塞、进度轮询照常）。同名概念靠"标签生成 IRI"自动合并，跨块残留的近义交给冲突队列。实测 8 分块 152s→63s（~2.4x，受 OpenRouter 吞吐限制）。

## 文档与本体的联动（溯源）

文档和本体之间靠 `AxiomProvenance`（公理→分块→文档）连接，这是回答「删/改文档时本体怎么变」的关键。

- **本体是合并产物**：一条公理可能被多个文档共撑，或被用户手动添加，所以不能"删文档就抹掉贡献"。
- **删除**：物理层删文件+分块+溯源；本体层**移除全部「仅来自本文档」的公理**（无其他来源支撑的不允许保留成孤儿），需逐条确认后删除（`app/ontology/provenance.py`）。共撑/手动的公理保留。
- **修改 = 上传新版本**：内容寻址下改过的文件即新哈希；重新解析会**同时清掉旧分块的溯源**（避免悬空），再走旧版本删除审阅。
- **IRI 稳定**保证跨文档/多次抽取合并到同一实体，溯源才能准确归并。

## 后续可做（Roadmap）

- 本体图上直接拖拽建边编辑；抽取的多分块并发化。
- 可选接入 owlready2 + HermiT 做严格 OWL-DL 一致性深检查（本体是标准 OWL，可无缝导出）。
