-- 0021：属性事实上的业务规则。人写规则，引擎按物化的节奏跑，结论是派生的。

-- ---------------------------------------------------------------------------
-- 一、`derived_facts` 拓宽到与 `facts` 同宽（0021 决策 1）
-- ---------------------------------------------------------------------------
--
-- 从前派生只能是「实体—实体」边：`object_id NOT NULL`。而业务规则的两种结论
-- 都不是边——派生归类是一个类，派生属性是一个字面值。
--
-- **拓宽机制，而不是在旁边另开一张表。** 下游那些路径（证明链、失效、物化
-- 对账、图上画成派生、asserted > derived 的优先级）认的是「这一行在
-- derived_facts 里」，不是「它的宾语是实体」，所以拓宽之后它们一行都不用改。
-- `facts` 早就是这个形状了，派生表比断言表还窄本身就是那条缝。
ALTER TABLE derived_facts ALTER COLUMN object_id DROP NOT NULL;
ALTER TABLE derived_facts ADD COLUMN object_value JSONB;
ALTER TABLE derived_facts ADD CONSTRAINT derived_object_or_value
    CHECK (object_id IS NOT NULL OR object_value IS NOT NULL);

-- ---------------------------------------------------------------------------
-- 二、规则本身
-- ---------------------------------------------------------------------------
--
-- 与 `rules`（公理编译出来的那张）分开：那张是 `(谓词, 公理种类)`，一个谓词
-- 一行，没有条件可放。业务规则是「某个类的实体，其自身属性满足一组条件，
-- 就得出一个结论」——主类、条件集、结论，三样都无处可去。
--
-- **人写，模型永不提议**（与 0002 对公理的判断同一条线：推理的判据是写下来的）。
CREATE TABLE attribute_rules (
    id              UUID PRIMARY KEY,
    kb_id           UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    -- 规则只看这个类（及其子类）的实体
    subject_type_id UUID NOT NULL REFERENCES entity_types(id) ON DELETE CASCADE,
    -- typing   → 派生归类，结论在 conclude_type_id
    -- attribute → 派生属性，结论在 conclude_predicate_id + conclude_value
    conclusion      TEXT NOT NULL CHECK (conclusion IN ('typing', 'attribute')),
    conclude_type_id      UUID REFERENCES entity_types(id) ON DELETE CASCADE,
    conclude_predicate_id UUID REFERENCES relation_types(id) ON DELETE CASCADE,
    conclude_value        JSONB,
    -- 关掉不等于删掉：关掉的规则下一轮不产出，已产出的按前提消失的老路失效
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 两种结论各填各的那几列，填串了直接挡住
    CONSTRAINT attribute_rule_conclusion_shape CHECK (
        (conclusion = 'typing'
             AND conclude_type_id IS NOT NULL
             AND conclude_predicate_id IS NULL
             AND conclude_value IS NULL)
        OR
        (conclusion = 'attribute'
             AND conclude_type_id IS NULL
             AND conclude_predicate_id IS NOT NULL
             AND conclude_value IS NOT NULL)
    ),
    UNIQUE (kb_id, name)
);
CREATE INDEX attribute_rules_kb_idx ON attribute_rules (kb_id) WHERE enabled;

-- 条件是**结构化的，不是一段文本**。存成表达式字符串就等于发明了一门小语言，
-- 而 0021 明确把通用规则语言排除在外：写下来的判据要能被读、被校验、被界面
-- 原样画出来。
--
-- operand 按 op 取不同形状（number / [lo,hi] / 字符串数组 / null），一列 JSONB
-- 装下：拆成四列会有三列永远是空的。
CREATE TABLE attribute_rule_conditions (
    id           UUID PRIMARY KEY,
    rule_id      UUID NOT NULL REFERENCES attribute_rules(id) ON DELETE CASCADE,
    seq          INT NOT NULL,
    -- 只能是 kind='attribute' 的谓词；跨实体的关系不参与（0021 决策 3）
    predicate_id UUID NOT NULL REFERENCES relation_types(id) ON DELETE CASCADE,
    op           TEXT NOT NULL CHECK (op IN ('gt', 'gte', 'lt', 'lte', 'between', 'in', 'present')),
    operand      JSONB,
    -- present 不带操作数，其余都必须带
    CONSTRAINT attribute_rule_condition_operand CHECK (
        (op = 'present' AND operand IS NULL) OR (op <> 'present' AND operand IS NOT NULL)
    ),
    UNIQUE (rule_id, seq)
);
CREATE INDEX attribute_rule_conditions_rule_idx ON attribute_rule_conditions (rule_id);

-- ---------------------------------------------------------------------------
-- 三、一行派生来自公理规则或业务规则，恰好一个
-- ---------------------------------------------------------------------------
ALTER TABLE derived_facts ALTER COLUMN rule_id DROP NOT NULL;
ALTER TABLE derived_facts
    ADD COLUMN attribute_rule_id UUID REFERENCES attribute_rules(id) ON DELETE CASCADE;
ALTER TABLE derived_facts ADD CONSTRAINT derived_one_rule
    CHECK ((rule_id IS NOT NULL) <> (attribute_rule_id IS NOT NULL));

-- ---------------------------------------------------------------------------
-- 四、同一性索引要认得字面值结论
-- ---------------------------------------------------------------------------
--
-- 旧索引按 `(kb, 主, 谓, 宾, 起, 止)` 认「还是那一条」，而字面值结论的
-- `object_id` 是 NULL。**Postgres 默认把 NULL 视作互不相同**，于是同一条结论
-- 每跑一轮都能再插一行，物化对账那一步永远认不出它已经在库里——重复行会随
-- 轮次线性增长，而且每一轮都报成 inserted。
--
-- 重建成 NULLS NOT DISTINCT 并把 object_value 一起纳入：两端各有一个可空列，
-- 靠它们区分的正是「归类」与「属性值」两种结论。
DROP INDEX derived_facts_identity_idx;
CREATE UNIQUE INDEX derived_facts_identity_idx
    ON derived_facts (kb_id, subject_id, predicate_id, object_id, object_value,
                      valid_from, valid_to)
    NULLS NOT DISTINCT
    WHERE invalidated_at IS NULL;
