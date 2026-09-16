-- 一条陈述留着文档自己的字（0044 第一刀，#729）。
--
-- 开放陈述是文档说了什么、用它自己的话：主语、照抄的关系短语、一个宾语实体或一个字面值、
-- 按文档的角色词记的属性、它提到的时间词、指回分块的出处。今天一条本体没接住的关系是能
-- 表达的（0010：`predicate_id` 为空、说法留在 `fact_evidence.proposed_predicate`），但它
-- 去重去得很差（去重键对 NULL 视而不见，每次观察都插一行）、属性必须挂在本体的某个
-- 属性上（0037）、一个被描述而没有名字的东西成不了实体（`canonical_name` 与 `known_as`
-- 都是必填）、时间只以算出来的日期存在，而且没有任何一列标出这一层是哪一层。
--
-- 全部纯增量，不回填；没有已发布的版本。
--
--   facts.layer            'typed' 是今天的那条路；'open' 是照文档的字写的陈述。
--   facts.phrase           关系短语，照写。开放行必有它、且没有 predicate_id
--                          （`facts_open_statement_shape`）：短语就是它的名字，本体的
--                          关系是后一刀（对齐、绑定）才给它的。`proposed_predicate`
--                          仍照写在证据上，于是所有已经容得下空谓词的读路径
--                          （图、实体面板、路径、导出、RDF、工具）不改一字就按短语显示它。
--   fact_evidence.quote_*  引文在 `chunks.text` 里的字符偏移；NULL = 没定位到。
--                          偏移由服务端搜文本算出，从不取模型报的数。
--   entities.description   一个被描述、没有名字的东西。它不写 `known_as`，于是永远不
--                          成为召回的桥（0041）。
--   statement_qualifiers   开放陈述上的属性，按文档的角色词（"amount"、"buyer"）记，
--                          不按本体属性；值或实体二选一，与 `fact_qualifiers` 同形。
--   time_mentions          陈述提到的时间词，**照抄，永远不是算出来的日期**（0045）。
--                          解释列（形状、锚点、偏移、粒度）等后一刀。开放陈述不写任何
--                          `valid_*`：它在世界轴上的位置还没有人给。
--
-- 两个时钟：`attested_from`（记录轴）总是有；`attested_at` 只在 `doc_time_source IN
-- ('content', 'source')` 时来自 `doc_time`，从不取上传时间（#714）。

-- 缺省开：抽取只写开放图谱（0044 决定 2），升级后所有库都走这条路——没有已发布的版本，
-- 不留遗留层。关掉 = 走带本体的那条老路，它按 0044 决定 3 作为已批准本体下的可选第二路
-- 保留到对齐追平为止
ALTER TABLE knowledge_bases
    ADD COLUMN open_extraction BOOLEAN NOT NULL DEFAULT TRUE;

ALTER TABLE facts
    ADD COLUMN layer TEXT NOT NULL DEFAULT 'typed' CHECK (layer IN ('typed', 'open')),
    ADD COLUMN phrase TEXT,
    ADD CONSTRAINT facts_open_statement_shape
        CHECK (layer <> 'open' OR (phrase IS NOT NULL AND predicate_id IS NULL));

-- 开放陈述按 (库, 主语, 短语) 去重，只在活着的行里找
CREATE INDEX facts_open_by_subject ON facts (kb_id, subject_id, phrase)
    WHERE layer = 'open' AND invalidated_at IS NULL;

-- 引文在 chunks.text 里的字符偏移（不是字节）；NULL = 没定位到
ALTER TABLE fact_evidence
    ADD COLUMN quote_start INT,
    ADD COLUMN quote_end   INT;

-- 一个被描述、没有名字的东西
ALTER TABLE entities ADD COLUMN description TEXT;

CREATE TABLE statement_qualifiers (
    fact_id   UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    -- 文档自己的角色词
    role      TEXT NOT NULL,
    value     JSONB,
    entity_id UUID REFERENCES entities(id) ON DELETE CASCADE,
    PRIMARY KEY (fact_id, role),
    CHECK ((value IS NOT NULL) <> (entity_id IS NOT NULL))
);

CREATE TABLE time_mentions (
    id          UUID PRIMARY KEY,
    kb_id       UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    fact_id     UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    chunk_id    UUID NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    -- 照抄的字，永远不是算出来的日期
    text        TEXT NOT NULL,
    -- 在 chunks.text 里的字符偏移
    char_start  INT  NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (fact_id, chunk_id, char_start)
);

-- 读陈述时把它的时间词一起带出来，按事实取
CREATE INDEX time_mentions_fact_idx ON time_mentions (fact_id);
