-- 一条关系短语按签名绑到一个属性（0044 决定 3 的第二片：签名 = 短语 × 主语的类 ×
-- 宾语的类，宾语是字面值时记「值」）。
--
-- 开放陈述的关系是文档自己的短语（`facts.phrase`：acquired、「细化解读」、Revenue），
-- 不选属性。属性是本体的事。一个库里 distinct 的签名比陈述少得多——同一个短语在同一对
-- 类之间出现多少次，只判一次：判了记在 `phrase_bindings` 里，本体一改只重判过期的
-- （绑到的属性判定后改过 `relation_types.updated_at`；判成 none / undecided 之后长出了
-- 新属性 `relation_types.created_at`）。绑上的签名下的陈述算成类型化事实，那是下一片
-- （物化视图，`facts.from_statement_id`）；这一片只记判定。
--
-- 两端的类来自类别词绑定（`entities.type_id`）；一端没有类的签名照样判——文档的例句
-- 和引文就是证据——但类为空的签名和类为某个类的签名是两条不同的签名。
-- `direction`：forward 是陈述的主语就是属性的主语，reverse 是反过来（「owns」绑到
-- subsidiary_of）。人的判定不被代理覆盖。

-- 属性改过没有，从前也只有 created_at；绑定按它判过期，每条 UPDATE relation_types 都摸一下
ALTER TABLE relation_types ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE TABLE phrase_bindings (
    id                  UUID PRIMARY KEY,
    kb_id               UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- 归一过的短语：空白折成一个空格、小写、去两端空白（同 type_bindings.kind_word）
    phrase              TEXT NOT NULL,
    -- 两端的类；空表示那一端的类别词还没绑到类（或宾语是字面值）。类删了绑定跟着走
    subject_type_id     UUID REFERENCES entity_types(id) ON DELETE CASCADE,
    object_type_id      UUID REFERENCES entity_types(id) ON DELETE CASCADE,
    -- 宾语是字面值（金额、百分比、称号）而不是一样东西
    object_is_value     BOOLEAN NOT NULL DEFAULT false,
    -- status 为 none / undecided 时为空；属性删了绑定跟着走
    relation_type_id    UUID REFERENCES relation_types(id) ON DELETE CASCADE,
    -- forward：陈述的主语是属性的主语；reverse：陈述的宾语是
    direction           TEXT CHECK (direction IN ('forward', 'reverse')),
    --   bound      绑上了
    --   none       没有属性对得上：陈述留在开放图谱，签名计入工作台的建议
    --   undecided  两票不一致，留给审核（#725 对齐队列）
    status              TEXT NOT NULL CHECK (status IN ('bound', 'none', 'undecided')),
    -- 得出这个判定的那几票
    votes               JSONB,
    -- 判定时这个签名下有几条陈述，三条例句——工作台按它排建议
    statement_count     INTEGER NOT NULL DEFAULT 0,
    examples            TEXT[] NOT NULL DEFAULT '{}',
    decided_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 人的判定不被代理覆盖
    decided_by          TEXT NOT NULL DEFAULT 'agent' CHECK (decided_by IN ('agent', 'person')),
    -- 一个库里一条签名一行；类为空也是一种签名（NULLS NOT DISTINCT）
    UNIQUE NULLS NOT DISTINCT (kb_id, phrase, subject_type_id, object_type_id, object_is_value),
    -- 绑上了就得有属性和方向，没绑就都不能有
    CONSTRAINT phrase_bindings_shape
        CHECK ((status = 'bound') = (relation_type_id IS NOT NULL)
               AND (status = 'bound') = (direction IS NOT NULL)),
    -- 宾语是字面值的签名没有宾语类
    CONSTRAINT phrase_bindings_value_has_no_class
        CHECK (NOT object_is_value OR object_type_id IS NULL)
);

CREATE INDEX phrase_bindings_kb_status_idx ON phrase_bindings (kb_id, status);
