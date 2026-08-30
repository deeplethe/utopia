-- 推理机 R0 补完 + R1 物化推导（见 docs/decisions/0002）。

-- ============ R0 的另一半：本体自己的自洽性 ============
--
-- `axiom_violations` 说的是「事实与定义抵触」，这张说的是「定义自己站不住」。
-- 分开不是分类癖：一个自相矛盾的本体会让事实层的结论**全部可疑**——若某个谓词
-- 同时声明了 symmetric 与 asymmetric，那么据它报出来的每一条反对称违规都建立在
-- 一个本来就不成立的前提上。所以界面上这一档排在前面。
--
-- 形状也不同：那张表的两列是 `facts` 的外键，而这里指的是类与谓词。
CREATE TABLE ontology_defects (
    id      UUID PRIMARY KEY,
    kb_id   UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- symmetric_and_asymmetric  一个谓词同时声明了两者（只对空属性成立）
    -- transitive_and_functional OWL 2 DL 明文禁止的组合
    -- subclass_cycle            A ⊂ B ⊂ A，建表的 CHECK 只挡得住自环
    -- disjoint_with_ancestor    类与自己的祖先互斥 → 永远不可能有实例
    -- inherits_disjoint         两个祖先互斥 → 同上
    kind    TEXT NOT NULL CHECK (kind IN (
                'symmetric_and_asymmetric', 'transitive_and_functional',
                'subclass_cycle', 'disjoint_with_ancestor', 'inherits_disjoint')),
    -- **两列都是裸 UUID，没有外键。** 前两类指 relation_types，后三类指
    -- entity_types——同一列指两张表，外键表达不了。而这是派生状态：本体一改
    -- 就整批重算，指向已删对象的行在下一轮自然消失，不必靠级联兜底
    subject UUID NOT NULL,
    other   UUID,
    -- 环的路径（按类排列）。其余为空
    path    UUID[] NOT NULL DEFAULT '{}',
    status  TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved')),
    -- fixed     去本体里改了（改声明、断开继承、撤掉 disjoint）
    -- accepted  人看过，认为不必改
    resolution  TEXT CHECK (resolution IN ('fixed', 'accepted')),
    decided_by  UUID REFERENCES users(id),
    decided_at  TIMESTAMPTZ,
    detected_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (kb_id, kind, subject, other)
);

CREATE INDEX ontology_defects_open_idx ON ontology_defects (kb_id, detected_at DESC)
    WHERE status = 'open';

-- ============ R1：规则与派生 ============

-- 规则**只从本体公理编译**，没有用户自定义 DSL——那是另一个产品（0002）。
--
-- 为什么要一张表而不是把规则种类塞进派生行：`facts.derived_by_rule` 需要一个
-- 指得着的东西，而「这条是靠哪条规则来的」在解释（R2）与「撤掉这条公理，
-- 哪些派生要跟着走」两处都要按规则聚合。
--
-- 它是派生状态：每次推导前按本体重编译一遍。所以身份取 `(kb, 谓词, 种类)`
-- 而不是自增——重编译要能认出「还是那条规则」，否则每跑一次
-- `derived_facts.rule_id` 就指向一个新 id，历史全断。
--
-- **公理撤了的规则不删。** `derived_facts` 上已失效的行仍指着它，解释
-- 「当时是靠哪条规则推的」需要它还在。规则一个库也就几条，留着不占地方。
CREATE TABLE rules (
    id           UUID PRIMARY KEY,
    kb_id        UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    predicate_id UUID NOT NULL REFERENCES relation_types(id) ON DELETE CASCADE,
    -- transitive | symmetric。`inverseOf` 与 `subPropertyOf` 投影侧还没落库，
    -- 所以也就编不出来——少一条规则不是缺陷，是「没声明就不推」的同一条
    kind         TEXT NOT NULL CHECK (kind IN ('transitive', 'symmetric')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (kb_id, predicate_id, kind)
);

-- 推出来的事实。**自己一张表，不进 `facts`。**
--
-- 试过塞进 `facts` 加一位 `derived_by_rule` 标记，那一版的问题是**失败方向反了**：
-- 仓库里有四十多处读 `facts` 的查询，其中只有一处认识那个标记，于是新写一条
-- 查询默认就是把派生当断言看，得记得加过滤。写这个功能的人（我）当场就漏了
-- 两处——低置信审核队列会把派生事实端给人 Confirm/Reject（确认一条推导没有
-- 意义，而拒绝它下一轮会原样推回来，因为前提还在），时态对账会拿一条推导去
-- 闭合一条断言（引擎拿自己的结论改人的数据，0001 判据 2 正好禁止这个）。
--
-- 分开之后忘了 UNION 的后果是**看不见**派生，而不是**混进去**。
--
-- 另外两条：
--
-- 一、**列本来就不一样**。派生没有 `supersedes`（它没有「纠正」语义）、没有
--    `fact_evidence`（它的证据是前提，在 `fact_derivations` 里）、`confidence`
--    的含义也不同（算出来的，不是模型自报的）。塞一张表里这些列对它全是借用的。
--
-- 二、**数量级差一档**。0002 在真实语料上量过 185 → 828，派生可能是断言的四倍多。
--    让每一条 `facts` 查询都去过滤掉大半行，是白付的代价。
CREATE TABLE derived_facts (
    id           UUID PRIMARY KEY,
    kb_id        UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    subject_id   UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    -- **非空**，与 `facts.predicate_id` 不同：规则是挂在谓词上的，没有谓词
    -- 就没有规则，也就推不出这一行
    predicate_id UUID NOT NULL REFERENCES relation_types(id) ON DELETE CASCADE,
    -- 同样非空：公理谈的是实体之间的关系，字面值宾语的属性事实不参与推导
    object_id    UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    rule_id      UUID NOT NULL REFERENCES rules(id),
    -- 有效期取前提的交集（0002 开放问题里给的语义）。精度与 `facts` 同一套
    -- 不变量：有日期才有精度
    valid_from   TIMESTAMPTZ,
    valid_to     TIMESTAMPTZ,
    valid_from_precision TEXT,
    valid_to_precision   TEXT,
    -- 前提里最小的那个。一条链只和它最弱的一环一样可信
    confidence   REAL NOT NULL DEFAULT 1.0,
    derived_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 前提没了就置这个，**不删行**：与拒绝一条事实完全同构，记录轴上留下
    -- 「我们曾据此推出，后来前提没了」，实体历史页面直接就能展示（0002 第 3 节）
    invalidated_at TIMESTAMPTZ,
    CONSTRAINT derived_from_precision_matches_date
      CHECK ((valid_from IS NULL) = (valid_from_precision IS NULL)),
    CONSTRAINT derived_to_precision_matches_date
      CHECK ((valid_to IS NULL) = (valid_to_precision IS NULL))
);

-- 图要按 (主, 宾) 取边；对账要按三元组 + 区间认出「还是那一条」
CREATE INDEX derived_facts_live_idx
    ON derived_facts (kb_id, subject_id, object_id) WHERE invalidated_at IS NULL;
CREATE UNIQUE INDEX derived_facts_identity_idx
    ON derived_facts (kb_id, subject_id, predicate_id, object_id, valid_from, valid_to)
    WHERE invalidated_at IS NULL;

-- 证明树的一层：这条派生用了哪几条前提。
--
-- **不存整棵树，只存直接前提。** 顺着这张表递归展开就是完整的证明（R2 要的
-- 东西）。存整棵树是同一份信息记 N 遍，而 N 是路径数。
--
-- 前提一律是断言（`facts`）：推导的输入里排除了派生，否则同一次调用的输出会
-- 变成下一次的输入，重跑结果依赖上一轮的残留。
--
-- `seq` 保证顺序：`A→B→C→D` 的证明读起来要是这个顺序，人才看得懂链是怎么走的。
CREATE TABLE fact_derivations (
    derived_fact_id UUID NOT NULL REFERENCES derived_facts(id) ON DELETE CASCADE,
    premise_fact_id UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    seq             INT  NOT NULL,
    PRIMARY KEY (derived_fact_id, seq)
);

-- 「这条前提被撤了，哪些派生要跟着失效」——反向查，主键覆盖不了
CREATE INDEX fact_derivations_premise_idx ON fact_derivations (premise_fact_id);

-- 推理开关。**默认关**：R1 会往图里加东西，而 0001 判据 2 说「本体是引导不是
-- 执法」——声明可能是错的，所以不该在用户没表态时就按它改图。
--
-- 放在 KB 上而不是部署上：一个库的本体带公理、另一个库全是自由文本抽出来的，
-- 该不该推是按库不同的。
ALTER TABLE knowledge_bases
    ADD COLUMN materialize_inferences BOOLEAN NOT NULL DEFAULT FALSE;

-- 多久重推一次。**必须定时,不能只靠手点**：事实是持续变的（每篇文档抽取都在
-- 加边），而派生只在跑的那一刻算。不定时的话，下一篇文档进来之后图上的派生就
-- 是**缺的**——不是错的（前提还在），是新链没推出来，而这种缺失界面上看不出来。
--
-- 跟来源同步同一个形状：一个间隔 + 一个上次时间，调度器每分钟扫一遍到期的。
-- 60 分钟是拍的：推导是纯计算不花钱，但在增量维护（0002 R3）做出来之前每次都是
-- 全库重算，所以也不该太密。
ALTER TABLE knowledge_bases
    ADD COLUMN inference_interval_minutes INT NOT NULL DEFAULT 60
        CHECK (inference_interval_minutes BETWEEN 5 AND 10080);

-- 上次推完的时间。**到点对比就是拿这一次的结果跟库里现有的比**——
-- `materialize` 本来就在做这件事（算出来的对上现有的，多的插、少的作废），
-- 所以「对比」不是新机制，是把那次对比的结果记下来给人看
ALTER TABLE knowledge_bases ADD COLUMN last_inference_at TIMESTAMPTZ;
