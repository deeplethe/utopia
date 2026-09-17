-- 一个类别词绑到一个类（0044 决定 3–4 的第一片：类型化图谱是按签名绑定从开放图谱
-- 算出来的视图；这一片的签名 = 实体的类别词）。
--
-- 开放抽取只记文档自己的类别词（`entities.specific_type`：company、person、
-- stockholder proposal、「指标」），不选类（`type_id` 空着）。类是本体的事——包、
-- 工作台建出来的。一个库里 distinct 的类别词就几十上百个，每个只判一次：判了记在
-- `type_bindings` 里，本体一改只重判过期的。绑上的类写到该类别词下每个实体的
-- `type_id`，来源记 `aligned`——它不是抽取判的、不是引擎按相似度猜的，也不是人拍的，
-- 是按签名绑定算出来的视图；人定过类的实体不动。身份消解的按类圈范围随之恢复。
-- 没有类对得上的类别词按老流程提成「建议加类」（`proposed_type` → 本体页采纳）。
--
-- 过期怎么判：绑到的类在判定之后改过（`entity_types.updated_at`），或者判成 none /
-- undecided 之后库里长出了新类（`entity_types.created_at`）。类被删了，绑定随之级联。

-- 类改过没有，从前谁也答不上来——只有 created_at。绑定要靠它判过期，每条 UPDATE
-- entity_types 都得摸一下
ALTER TABLE entity_types ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

ALTER TABLE entities DROP CONSTRAINT entities_type_source_check;
ALTER TABLE entities ADD CONSTRAINT entities_type_source_check
    CHECK (type_source IN ('extracted', 'human', 'inferred', 'aligned'));

CREATE TABLE type_bindings (
    id          UUID PRIMARY KEY,
    kb_id       UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- 归一过的：空白折成一个空格、小写、去两端空白。"Company" 与 "company" 是一个词
    kind_word   TEXT NOT NULL,
    -- 文档里见过的写法
    words       TEXT[] NOT NULL DEFAULT '{}',
    -- status 为 none / undecided 时为空；类删了绑定跟着走
    type_id     UUID REFERENCES entity_types(id) ON DELETE CASCADE,
    --   bound      绑上了
    --   none       没有类对得上（已按老流程提成建议加类）
    --   undecided  两票不一致，留给审核
    status      TEXT NOT NULL CHECK (status IN ('bound', 'none', 'undecided')),
    -- 得出这个判定的那几票
    votes       JSONB,
    decided_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 人的判定不被代理覆盖
    decided_by  TEXT NOT NULL DEFAULT 'agent' CHECK (decided_by IN ('agent', 'person')),
    UNIQUE (kb_id, kind_word),
    -- 绑上了就得有类，没绑就不能有
    CONSTRAINT type_bindings_shape CHECK ((status = 'bound') = (type_id IS NOT NULL))
);
