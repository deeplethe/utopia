-- 本体：抽取未匹配统计、导入、提案与人认可过的细化配对。

-- 抽取过程中命中白名单之外的类型/关系：不是垃圾，是本体扩展的信号
CREATE TABLE ontology_misses (
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- attribute_type 与 relation_type 分开：词表外的谓词带着字面值时（
    -- `founding_date: "2015"`），缺的是一个属性而不是一个关系，记错了会让
    -- 本体提案去建一条关系
    kind       TEXT NOT NULL
               CHECK (kind IN ('entity_type', 'relation_type', 'attribute_type')),
    key        TEXT NOT NULL,
    example    TEXT,
    count      INT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 人说过不要。**标记而不是删除**：dismiss 从前是 DELETE，下一次抽取遇到
    -- 同一个词原样插回来——用户的「不要」活不过一轮抽取。自动扩本体开着时，
    -- 那等于系统覆盖人的明确决定
    dismissed_at TIMESTAMPTZ,
    PRIMARY KEY (kb_id, kind, key)
);

-- 本体导入的第一层：原文保真。
--
-- 投影只覆盖今天能消费的那部分（类、标签、rdfs:comment、subClassOf、
-- 对象/数据属性、functional、domain/range）。**读不懂的不是错误，是"暂未投影"**——
-- 原文按内容寻址存进 blob，将来推理机上线或我们补上新消费者时重跑，
-- 用户什么都不用做。这样"我们表达不了"从能力缺口降级成"投影暂未覆盖"。
CREATE TABLE ontology_imports (
    id            UUID PRIMARY KEY,
    kb_id         UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- blob 的内容指纹；同一份文件重复导入不重复占空间
    sha256        TEXT NOT NULL,
    filename      TEXT NOT NULL,
    -- turtle | rdfxml
    format        TEXT NOT NULL,
    byte_size     BIGINT NOT NULL,
    -- 投影版本：将来投影逻辑变了，据此知道哪些导入该重跑
    projection_version INT NOT NULL DEFAULT 1,
    -- 这次投影做了什么（新建/更新/暂未投影的计数与明细），预览与事后审计共用
    summary       JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- 谁导的。删号后留 NULL，与审计台账同规矩
    imported_by   UUID REFERENCES users(id) ON DELETE SET NULL,
    imported_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ontology_imports_kb_idx ON ontology_imports (kb_id, imported_at DESC);


-- 本体提案落库。
--
-- 从前它只活在浏览器内存里（`Ontology.tsx` 的 `useState<OntologyProposals>`）：
-- 刷新一次、切走一次、崩一次，整批提议就没了，想再看只能重跑一次模型。
--
-- **丢的不是原材料。** 未匹配的说法一直存在 `ontology_misses` 里。丢的是**聚类
-- 结果**——哪些说法被归到同一条提议底下、以及"采纳后将重新归类 N 条"那个估算。
-- 而那正是唯一能查证过并的东西：0003 记着模型建议把 `optimized_for` 并进
-- `runs_on`（"为 RTX 优化"不等于"跑在 RTX 上"），**它是靠 tooltip 里看得见归并了
-- 哪些说法才被抓出来的**，并且成了"不该全自动归并"的直接证据。能查证的东西不该
-- 只活在一个页面的生命周期里。
CREATE TABLE ontology_proposals (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- 提案分档，与接口返回的四个小节同名：
    -- entity_types | relation_types | attribute_types | map_to
    section    TEXT NOT NULL,
    key        TEXT NOT NULL,
    -- 那一条提案的原样：label、description、reason、forms、datatype、temporal…
    --
    -- **存 JSONB 而不是拆成列**：四个小节的形状本来就不同（关系有 temporal 与
    -- forms，属性有 datatype，map_to 有目标），拆开要么四张表要么一张稀疏宽表。
    -- 前端消费的也正是这个 JSON，存原样等于不改契约。要查归并了哪些说法仍然
    -- 查得动（`payload->'forms'`）
    payload    JSONB NOT NULL,
    -- open = 还等着人看；adopted / rejected = 已经有人表过态
    status     TEXT NOT NULL DEFAULT 'open'
               CHECK (status IN ('open', 'adopted', 'rejected')),
    decided_by UUID REFERENCES users(id),
    decided_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 同一个库里同一档下的同一个 key 只有一条。重跑 Suggest 是刷新它，不是再堆一条
    UNIQUE (kb_id, section, key)
);

-- "还有多少条等着看"是这张表最常被问的问题（0003 的另一个缺口：关掉自动扩展
-- 开关之后没有"自上次以来有 N 个新说法"的提醒，信号在面板里但没人主动看）
CREATE INDEX ontology_proposals_open_idx
    ON ontology_proposals (kb_id, created_at DESC)
    WHERE status = 'open';

-- 人认可过的"粗类 → 细类"配对。认可一次，之后同一对不再进人工。
--
-- 为什么需要它：待人工那一档由"选中的类在不在粗类子树里"触发，而实测那条
-- 判据测的往往不是风险，是**种子类跟导入词汇表的分类树连没连上**。
-- schema.org 的 Place 另起了 place 这个 key，内置 location 一个子类都没有，
-- 于是每一次 location → city 都算跨轴——24 个实体里报了 14 条，条条正确。
--
-- 跨轴是 (粗类, 目标类) 这一对的属性，不是实体的属性。人看过一次
-- "location 下面的东西可以是 city"，第二个城市就不该再问一遍。
CREATE TABLE type_refinement_pairs (
    kb_id       uuid NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    from_type_id uuid NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    to_type_id  uuid NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    approved_by uuid REFERENCES users(id),
    approved_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (kb_id, from_type_id, to_type_id)
);
