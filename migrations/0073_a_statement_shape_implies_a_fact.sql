-- 一种形状的陈述蕴含另一条属性的事实（0044 决定 3 的第五片：蕴含规则与缓存的读数）。
--
-- 原型量过的召回差距在这里（0044 §2）：「a 1952 British film」蕴含 country of origin，
-- 「located in the Piedmont region of Virginia」蕴含 country——读的人不用原文说就能得出，
-- 开放陈述却不会把它写成陈述。补法不是再抽一遍，是**规则**：某个签名（短语 × 两端的类）
-- 或某个类别词下的东西，蕴含某条属性的事实，宾语要么就是陈述的宾语，要么由一个「读数」
-- 从宾语的字里读出来（民族形容词指的国家、地名所属的国家、短语给出的年份）。
-- 对齐器提规则，工作台批，代码执行；读数按 distinct 的字算一次、缓存，物化只查缓存。
--
--   facts.implied           规则算出来的类型化行。不是陈述直接说的，导出与界面要能分辨
--   implication_rules       规则本体：触发（phrase 签名 / kind_word 类别词）、结论属性、读数、
--                           与绑定同一套 status / decided_by / basis（0053）：人的不被代理盖
--   phrase_readings         读数缓存：(读数种类, 字) → 库里的一样东西或一个字面值；两者都空
--                           = 读不出来，也缓存住，别每轮再问
--   implied_fact_sources    一行隐含事实的来源：哪条规则、由哪条陈述或哪个实体触发。来源全空
--                           行就作废，与 typed_fact_sources 同一条规矩（0068）
ALTER TABLE facts ADD COLUMN implied BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE implication_rules (
    id                   UUID PRIMARY KEY,
    kb_id                UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- phrase：签名下的每条陈述触发；kind_word：带这个类别词的每个实体触发
    trigger              TEXT NOT NULL CHECK (trigger IN ('phrase', 'kind_word')),
    -- 归一过的短语或类别词（同 phrase_bindings.phrase / type_bindings.kind_word）
    phrase               TEXT NOT NULL,
    subject_type_id      UUID REFERENCES entity_types(id) ON DELETE CASCADE,
    object_type_id       UUID REFERENCES entity_types(id) ON DELETE CASCADE,
    object_is_value      BOOLEAN NOT NULL DEFAULT false,
    conclude_property_id UUID NOT NULL REFERENCES relation_types(id) ON DELETE CASCADE,
    -- 空 = 宾语就是陈述的宾语；否则是读数的种类（见 utopia_extract::implication::READINGS）
    reading              TEXT,
    status               TEXT NOT NULL CHECK (status IN ('proposed', 'approved', 'rejected')),
    votes                JSONB,
    decided_by           TEXT NOT NULL DEFAULT 'agent' CHECK (decided_by IN ('agent', 'person')),
    basis                TEXT,
    statement_count      INTEGER NOT NULL DEFAULT 0,
    examples             TEXT[] NOT NULL DEFAULT '{}',
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE NULLS NOT DISTINCT (kb_id, trigger, phrase, subject_type_id, object_type_id, object_is_value,
                               conclude_property_id, reading),
    CONSTRAINT implication_rules_kind_word_shape
        CHECK (trigger <> 'kind_word' OR (subject_type_id IS NULL AND object_type_id IS NULL AND NOT object_is_value))
);
CREATE INDEX implication_rules_kb_status_idx ON implication_rules (kb_id, status);

CREATE TABLE phrase_readings (
    kb_id       UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    reading     TEXT NOT NULL,
    phrase      TEXT NOT NULL,
    entity_id   UUID REFERENCES entities(id) ON DELETE CASCADE,
    value       JSONB,
    answered_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (kb_id, reading, phrase),
    CHECK (entity_id IS NULL OR value IS NULL)
);

CREATE TABLE implied_fact_sources (
    fact_id      UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    rule_id      UUID NOT NULL REFERENCES implication_rules(id) ON DELETE CASCADE,
    statement_id UUID REFERENCES facts(id) ON DELETE CASCADE,
    entity_id    UUID REFERENCES entities(id) ON DELETE CASCADE,
    PRIMARY KEY (fact_id, rule_id),
    CHECK ((statement_id IS NOT NULL) <> (entity_id IS NOT NULL))
);
CREATE INDEX implied_fact_sources_rule_idx ON implied_fact_sources (rule_id);
CREATE INDEX implied_fact_sources_statement_idx ON implied_fact_sources (statement_id) WHERE statement_id IS NOT NULL;
