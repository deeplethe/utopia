-- 0004: 图谱层 —— 轻量本体 + 实体 + 双时态事实账本 + 证据链
-- 设计见 docs/DESIGN.md §3：事实 append-only；抽取错误设 invalidated_at，事实变化闭合 valid_to。

-- 类。**两个向量不是冗余**：类型消解发两种查询（见 `type_resolution.rs`），
-- 一种是模型给的短说法（`district. place`），一种是整段画像。此前两种比的是
-- 同一批 `label + description` 向量——查询分了两种形状，文档只有一种，于是短
-- 查询被同义反复的类接管（`Park\nA park.` 这样一行的类，赢在长度而不是语义：
-- 被检索出来的类原文长度中位数 44，而全体中位数 89）。
--
-- 「短的那一侧距离系统性地更小」这条规律本仓库栽过四次（跨实体不可比、两路之间
-- 不可比、同一路两个查询之间不可比、空描述的悬空类占便宜），前四次修的都是查询
-- 一侧,这一次是文档一侧：**两种查询就该有两份文档**，短对短、长对长。
--
-- 维度不定：跟 chunks.embedding 一样随工作区所选模型走，所以不建 HNSW，顺扫。
-- 本体行数以千计，顺扫完全够。
CREATE TABLE entity_types (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    label      TEXT NOT NULL,
    color      TEXT NOT NULL DEFAULT '#64748b',
    builtin    BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 图谱节点按类型的形状渲染（此前 organization/product = 方形是前端写死的）
    shape      TEXT NOT NULL DEFAULT 'circle' CHECK (shape IN ('circle', 'square')),
    -- 不是给人看的装饰，是喂给抽取 prompt 的语义指引（"Event: 有明确时间点的
    -- 事件，如发布、收购、会议"），直接影响抽取质量
    description TEXT NOT NULL DEFAULT '',
    -- IRI 是全局身份，key 是给模型读的短标签（见 0001 P2「IRI 与 key 的分工」）。
    -- 重导入按 IRI 匹配已有行——按 key 匹配会因为上游改了 rdfs:label 导致 key
    -- 变化而把同一个类当成新类建出来，实体全留在孤儿上
    iri        TEXT,
    -- 长文档：label + description
    embedding  vector,
    -- **存「当时嵌的是什么」而不是一个时间戳。** 时间戳只能回答「嵌过没有」，
    -- 回答不了「嵌的还是不是现在这段文字」——描述改了、模型换了，向量就是陈的，
    -- 而时间戳看不出来。存原文与模型名，填充任务一比对就知道该重嵌谁，也就不必
    -- 去每一个改描述的写入点挂钩子（漏一个就悄悄烂掉）
    embedded_text  text,
    embedded_model text,
    -- 短文档：只嵌 label
    label_embedding      vector,
    label_embedded_text  text,
    label_embedded_model text,
    UNIQUE (kb_id, key)
);
CREATE UNIQUE INDEX entity_types_iri_idx ON entity_types (kb_id, iri) WHERE iri IS NOT NULL;

CREATE TABLE relation_types (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    label      TEXT NOT NULL,
    -- 时间语义：state 状态型(区间) / event 事件型(时点) / eternal 永恒型(无时间)
    temporal   TEXT NOT NULL DEFAULT 'state' CHECK (temporal IN ('state', 'event', 'eternal')),
    -- 基数唯一（同一时刻单值），时态冲突检测（自动闭合 valid_to）的依据：
    -- functional = 主语侧唯一；inverse_functional = 宾语侧唯一（一个项目一个 leader）
    functional BOOLEAN NOT NULL DEFAULT FALSE,
    inverse_functional BOOLEAN NOT NULL DEFAULT FALSE,
    builtin    BOOLEAN NOT NULL DEFAULT FALSE,
    -- 属性系统：属性 = 值域为字面量的关系（RDF datatype property），一表两用。
    -- 属性值走 facts.object_value 通道，时态/证据/Review 全套复用
    kind       TEXT NOT NULL DEFAULT 'relation' CHECK (kind IN ('relation', 'attribute')),
    datatype   TEXT CHECK (datatype IN ('text', 'number', 'date', 'bool')),
    unit       TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    description TEXT NOT NULL DEFAULT '',
    iri        TEXT,
    embedding  vector,
    embedded_text  text,
    embedded_model text,
    -- 其余属性公理，跟 `functional` / `inverse_functional` 同族——那两个先落库
    -- 而已。**默认 false 而不是 NULL**：OWL 是开放世界，但一致性检查只能按写
    -- 下来的判，「没声明」与「声明为否」后果相同（都不构成报矛盾的依据），
    -- 不必用三态去区分一个不影响行为的差别。
    --
    -- **列名带 is_ 前缀不是风格洁癖**：`symmetric` 与 `asymmetric` 都是 Postgres
    -- 保留字（`BETWEEN SYMMETRIC`），裸用会在建表那一行就报语法错。加引号能绕
    -- 过去，但那要求之后每一处写这两列的 SQL 都记得加——漏一处就是运行时才炸
    is_transitive  BOOLEAN NOT NULL DEFAULT FALSE,
    is_symmetric   BOOLEAN NOT NULL DEFAULT FALSE,
    is_asymmetric  BOOLEAN NOT NULL DEFAULT FALSE,
    is_irreflexive BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE (kb_id, key)
);
CREATE UNIQUE INDEX relation_types_iri_idx ON relation_types (kb_id, iri) WHERE iri IS NOT NULL;

-- domain / range 是**关联表而不是列**：OWL 里一个属性有多个 rdfs:domain 是常态
-- （works_at 的主语可能是 person 也可能是 organization），单列表达不了，导入时
-- 只能挑一个丢一个（FOAF 里就有这样的属性）。也不留一个「主 domain」列——那是
-- 同一件事有两处记录，而两处记录迟早分叉（预览与落库各判一次 key 冲突、结果
-- 预览说了假话，这个坑已经踩过）。一处权威，读的人不必猜该信哪个。
--
-- range 只服务对象属性：数据属性的 range 是字面量类型，落在 relation_types.datatype 上。
CREATE TABLE relation_type_domains (
    relation_type_id UUID NOT NULL REFERENCES relation_types(id) ON DELETE CASCADE,
    entity_type_id   UUID NOT NULL REFERENCES entity_types(id)   ON DELETE CASCADE,
    PRIMARY KEY (relation_type_id, entity_type_id)
);

CREATE TABLE relation_type_ranges (
    relation_type_id UUID NOT NULL REFERENCES relation_types(id) ON DELETE CASCADE,
    entity_type_id   UUID NOT NULL REFERENCES entity_types(id)   ON DELETE CASCADE,
    PRIMARY KEY (relation_type_id, entity_type_id)
);

-- 反向查：本体页要按类列出它的属性，抽取要按类筛可用属性。
-- 主键覆盖了正向，反向得自己建
CREATE INDEX relation_type_domains_entity_idx ON relation_type_domains (entity_type_id);
CREATE INDEX relation_type_ranges_entity_idx  ON relation_type_ranges  (entity_type_id);

-- subClassOf。**一个类可以有多个父类**，这在真实词汇表里是常态：FOAF 的 Person
-- 同时是 foaf:Agent 与 geo:SpatialThing——两个方向，不是同一根链上的祖孙。只认
-- 一个父类的话，domain 落在另一支上的属性判定不过（latitude 的 domain 是
-- SpatialThing，而 person 只挂在 agent 下，那条事实抽出来了却被挡掉）。
--
-- **is_primary 不是冗余**：左栏按树展示，一个类只能画在一处，而「画在哪一支下」
-- 是 subClassOf 集合本身答不出的问题——它是多出来的一条信息，不是同一件事记两遍。
CREATE TABLE entity_type_parents (
    child_id   UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    parent_id  UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    -- 左栏画树时走这一支。不参与语义，只管展示
    is_primary BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (child_id, parent_id),
    -- 自环在这里就挡掉；更长的环由应用在写入前查（SQL 拦不住 A→B→A）
    CONSTRAINT entity_type_parents_no_self CHECK (child_id <> parent_id)
);

-- 每个类至多一个主父
CREATE UNIQUE INDEX entity_type_parents_primary_idx
    ON entity_type_parents (child_id) WHERE is_primary;

-- 反向查：左栏要按父类找子类，域判定要沿父链上溯
CREATE INDEX entity_type_parents_parent_idx ON entity_type_parents (parent_id);

-- 类互斥。**存成一张表而不是数组列**：要查的问题是「A 与 B 互斥吗」，那是一次
-- 点查；数组列查起来要么全表扫要么建 GIN，而这里的语义就是一条边。
CREATE TABLE entity_type_disjoint (
    kb_id UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    a_id  UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    b_id  UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    -- 两个方向各存一行（导入侧已经把公理的对称性展开了）。主键因此天然去重，
    -- 而查询不必关心调用方从哪一头问
    PRIMARY KEY (kb_id, a_id, b_id),
    -- 自己跟自己互斥是无意义的声明，挡在门口比留着让检查去猜好
    CHECK (a_id <> b_id)
);

-- 「跟这个类互斥的有哪些」是唯一的查法（一致性检查拿实体的类去问）
CREATE INDEX entity_type_disjoint_a_idx ON entity_type_disjoint (kb_id, a_id);

CREATE TABLE entities (
    id             UUID PRIMARY KEY,
    kb_id          UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- **可以为空**，见 docs/decisions/0009：「还没判出来」不是一个类。
    -- 从前它是一行叫 concept 的哨兵，而哨兵有名字，有名字就会撞——SKOS 的
    -- skos:Concept 派生出的 key 正是 concept，导入逻辑「占位者没有 IRI 就认领它」
    -- 会让它接管哨兵，所有未分类的实体一夜之间变成正经的 skos:Concept。
    -- 不是撞名被跳过，是语义被静默改写。NULL 没有名字，撞不着；也忘不掉——
    -- 漏过滤一个哨兵不会有任何提示，漏处理一个 NULL 会当场报出来
    type_id        UUID REFERENCES entity_types(id) ON DELETE RESTRICT,
    canonical_name TEXT NOT NULL,
    aliases        TEXT[] NOT NULL DEFAULT '{}',
    attrs          JSONB NOT NULL DEFAULT '{}',
    -- 被合并后指向存活实体（合并可回滚，P2 后续）
    merged_into    UUID REFERENCES entities(id),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 实体画像：证据分块向量的增量质心（上下文相似度判定用；
    -- 复用摄入阶段已算好的 chunk embedding）
    profile_embedding vector,
    profile_n      INTEGER NOT NULL DEFAULT 0,
    -- 同名并存时的展示消歧后缀（如 张三 · Platform Engineering）
    disambiguator  TEXT,
    -- 模型提议的类型：**本体装不下时它说的那个词**。不记的话，想加 model 类时
    -- 找不出那些实体——它们混在未分类里，只能整库重抽
    proposed_type  TEXT,
    -- 模型对这个实体自己的说法：它认为这最具体是个什么。
    --
    -- **跟 proposed_type 不能合用一列**：那一列的含义是「模型要的东西本体里没有」，
    -- 本体增长回路靠它的稀有性设门槛，每个实体都填的话回路会给每一个实体提议建新类。
    --
    -- 为什么需要它：清单里总有个「差不多」的。本体有 product，模型觉得够用就选了，
    -- 心里那个「向量数据库软件」就此丢失。而类型消解最需要的正是这个名字：
    -- 短名字对短标签，比拿一段中文散文去匹配 schema.org 的 "A software application." 近得多
    specific_type  text,
    -- **这个类型是怎么来的。** 保护要事前——`entity_retypes` 记得住「谁改的」，
    -- 但那是事后的账；实体身上得有一位说明有没有人拍过板，否则每一条读路径都
    -- 只能看见 `type_id`。漏在类型消解那条路上：人工定成 `organization` 的实体，
    -- 只要 `organization` 有子类，下一轮消解照样把它拿去重判。
    --
    -- 0009 之后还多了一种情形：「没有类型」现在可能是**人的决定**——他看过这个
    -- 实体，认为本体里没有合适的类。而 `type_id IS NULL` 分不出「还没判」和
    -- 「人判了，就是没有」，于是下一次抽取会给它安一个类型。
    --
    --   extracted  抽取判出来的（`resolve_type_drift` 的升格）
    --   inferred   引擎裁决的（类型消解、本体长出新类后的认领）
    --   human      人拍板的（实体面板直改、审核队列里点批准）
    --
    -- `inferred` 与 `extracted` 分开而不是合成一个「非人」：它们的可信度不同，
    -- 将来若要「引擎可以改引擎的，但不能改抽取的」，判据现成
    type_source    TEXT NOT NULL DEFAULT 'extracted'
                   CHECK (type_source IN ('extracted', 'human', 'inferred'))
);
-- **不唯一**：名字不是身份（0001 P0 的两个张伟），同名实体允许并存，
-- 这里只做候选召回。
--
-- 0009 之后 type_id 还可能是 NULL，而 Postgres 里 NULL <> NULL——两个都没类型的
-- 同名实体更不会被拦住。这是要的行为：未分类时我们对它们是不是同一个东西
-- 知道得更少，更没有理由合并
CREATE INDEX entities_kb_type_name_idx
    ON entities (kb_id, type_id, lower(canonical_name)) WHERE merged_into IS NULL;
CREATE INDEX entities_kb_idx ON entities (kb_id);
-- 跨类型同名召回（类型漂移处理）：上面那个索引带 type_id 前缀，跨类型查询用不上
CREATE INDEX entities_kb_name_idx
    ON entities (kb_id, lower(canonical_name)) WHERE merged_into IS NULL;

-- 事实账本：SPO + 双时间轴（append-only，永不 DELETE）
CREATE TABLE facts (
    id              UUID PRIMARY KEY,
    kb_id           UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    subject_id      UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    -- **可以为空**：「说不出是什么关系」不该是一种关系（与 0009 对 concept 做的
    -- 事同构）。从前它是一行叫 related_to 的 builtin 关系，在本体页上跟真关系
    -- 并排列着——而它编码的是「抽取器抽到了一条边，但本体里没有对应的关系」，
    -- 那是控制流，不是词汇。
    --
    -- 删掉它一个字的信息都不丢：原意一直存在证据的 `proposed_predicate` 里，
    -- 被那行假词汇盖着而已。删掉之后信息反而更多——从前统统显示「有关联」，
    -- 之后各自显示原文说的 acquired / runs_on / sued（见 fact_surface_predicate）
    predicate_id    UUID REFERENCES relation_types(id) ON DELETE CASCADE,
    object_id       UUID REFERENCES entities(id) ON DELETE CASCADE,
    object_value    JSONB,
    valid_from      TIMESTAMPTZ,
    valid_to        TIMESTAMPTZ,
    -- **两端各记各的精度，且没日期就没精度。**
    --
    -- 从前是一个 `valid_precision NOT NULL DEFAULT 'day'`，于是根本没有日期的
    -- 事实也带着 'day' 落库——账本在无知的地方填了一个确定的值，任何按这个字段
    -- 渲染「精确到日」的界面都会照着念。默认值本身就是那条 bug：它让「没量过」
    -- 和「量到日」长得一模一样。
    --
    -- 一个列描述两个端点也不行：只有 valid_to 的事实（「直到 2023 年」这类），
    -- 那个列名写着 from、描述的却是 to。
    valid_from_precision TEXT,
    recorded_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    invalidated_at  TIMESTAMPTZ,
    confidence      REAL NOT NULL DEFAULT 1.0,
    derived_by_rule UUID,
    supersedes      UUID REFERENCES facts(id),
    -- 结束端。多一个 'unknown'，因为账本原本说不出「结束了，但不知道哪天」：
    -- `valid_to IS NULL` 一直承载两个意思，于是 "former CEO of Weta Digital"
    -- 这句——**结束是原文明说的，日期是原文没给的**——只能写成 null，然后被
    -- 系统读成「至今仍是」，图会理直气壮地断言一件原文说已经结束的事。
    --
    --   仍在持续          valid_to IS NULL   valid_to_precision IS NULL
    --   结束了，不知哪天   valid_to IS NULL   valid_to_precision = 'unknown'
    --   2023 年结束       valid_to = …       valid_to_precision = 'year'
    --
    -- **不采用「把文档日期填进 valid_to 当上界」那种做法**（教科书里的
    -- indeterminate instant）：那会在列里放一个看起来确定的时间戳，每个读者都得
    -- 先查精度才敢用它，而这个产品卖的正是不骗人
    valid_to_precision text,
    CHECK (object_id IS NOT NULL OR object_value IS NOT NULL),
    -- 起始端没有 unknown——「开始了但不知道何时」与「不知道有没有开始」在这个
    -- 账本里不可区分，硬加一个状态只会让读者猜
    CONSTRAINT facts_from_precision_matches_date
      CHECK ((valid_from IS NULL) = (valid_from_precision IS NULL)),
    -- 那句 `IS NOT NULL` 不是冗余的。少了它，`valid_to` 有日期而精度为 NULL 的
    -- 行会**被放行**：`NULL IN ('year',…)` 求值是 NULL，`TRUE AND NULL` 是 NULL，
    -- `NULL OR FALSE` 还是 NULL，而 CHECK 约束遇到 NULL 判通过。三值逻辑在这里
    -- 是静默的
    CONSTRAINT facts_to_precision_matches_date
      CHECK (
        (valid_to IS NOT NULL AND valid_to_precision IS NOT NULL
           AND valid_to_precision IN ('year', 'month', 'day'))
        OR (valid_to IS NULL AND (valid_to_precision IS NULL OR valid_to_precision = 'unknown'))
      )
);
-- 热路径部分索引：作废行不进索引（账本边界与清理，DESIGN.md §3.1）
CREATE INDEX facts_live_subject_idx ON facts (kb_id, subject_id) WHERE invalidated_at IS NULL;
CREATE INDEX facts_live_object_idx  ON facts (kb_id, object_id)  WHERE invalidated_at IS NULL;
CREATE INDEX facts_live_time_idx    ON facts (kb_id, valid_from, valid_to) WHERE invalidated_at IS NULL;
-- 时态冲突检测的不变量点查：主语侧与宾语侧各一（开放期 + 未作废）
CREATE INDEX facts_open_pair_idx ON facts (kb_id, subject_id, predicate_id)
    WHERE valid_to IS NULL AND invalidated_at IS NULL;
CREATE INDEX facts_open_obj_pair_idx ON facts (kb_id, object_id, predicate_id)
    WHERE valid_to IS NULL AND invalidated_at IS NULL;
-- 认知轴。上面那几个索引全在世界轴上、且都只认活行，服务的是"某时刻什么为真"；
-- 这两个服务的是另一个问题——**什么时候写进来的、什么时候被推翻的**
--（"上个季度我们的认知有哪些变化"，见 chat 的 changes 工具）。
-- 作废那条是部分索引且条件取反：活行的 invalidated_at 全是 NULL，
-- 把它们收进来只会让索引跟表一样大，而被推翻的事实天然是少数
CREATE INDEX facts_recorded_idx ON facts (kb_id, recorded_at DESC);
CREATE INDEX facts_invalidated_idx ON facts (kb_id, invalidated_at DESC)
    WHERE invalidated_at IS NOT NULL;

-- 证据链：事实 ↔ 原文分块（溯源一等公民）
CREATE TABLE fact_evidence (
    fact_id  UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    chunk_id UUID NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    quote    TEXT,
    -- 证据出处版本：出自哪份文档的第几版（版本对账与"证据过期"展示的判定依据）
    document_id UUID REFERENCES documents(id) ON DELETE CASCADE,
    doc_version INT,
    -- 模型原话。**落在证据上而不是事实上**：事实按 (kb, 主, 谓, 宾) 去重，
    -- 甲块说 "runs on"、乙块说 "optimized for" 会并成同一行，放事实上就是
    -- 先写者胜、其余静默丢弃。证据是每分块一行，粒度对，而且它已经带着 quote
    -- （该块的原文佐证），原话是同一种东西：每次观察的原始形态
    proposed_predicate TEXT,
    PRIMARY KEY (fact_id, chunk_id)
);


-- 时态冲突（S3）：自动闭合拿不准的进审，人裁 close / keep / reject_new
CREATE TABLE fact_conflicts (
    id          UUID PRIMARY KEY,
    kb_id       UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    old_fact_id UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    new_fact_id UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    -- no_time | simultaneous | low_confidence
    reason      TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved')),
    -- closed | kept_both | rejected_new
    resolution  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    UNIQUE (old_fact_id, new_fact_id)
);
CREATE INDEX fact_conflicts_open_idx ON fact_conflicts (kb_id) WHERE status = 'open';

-- 没有谓词的事实**显示什么**。
--
-- 做成函数而不是在每条读查询里塞一段子查询：读事实的路径有六条以上（图的边、
-- 实体面板、变更历史、低置信审核、文档产出、消解画像），六份同样的 SQL 迟早
-- 分叉，而分叉在这里的后果是同一条边在不同页面上叫不同的名字。
--
-- **确定性**：出现次数相同时按字典序，所以同一条事实每次显示同一个词。
CREATE FUNCTION fact_surface_predicate(fact uuid) RETURNS text
LANGUAGE sql STABLE AS $$
    SELECT e.proposed_predicate
      FROM fact_evidence e
     WHERE e.fact_id = fact AND e.proposed_predicate IS NOT NULL
     GROUP BY e.proposed_predicate
     ORDER BY count(*) DESC, e.proposed_predicate
     LIMIT 1
$$;
