-- 公理违规：一致性检查（0002 R0）查出来的矛盾。
--
-- **不复用 `fact_conflicts`**，尽管两者都是「两条事实打架，等人裁决」。理由是
-- 裁决动作根本不同：
--
--   fact_conflicts    时态冲突，问的是「哪条对」
--                     → closed / kept_both / rejected_new，三个答案都在改事实
--   axiom_violations  公理违规，问的是「错在数据还是错在定义」
--                     → 撤事实，或者去改本体那条公理
--
-- 后者那条出路是本质的。用户导一份 FOAF 进来，里面某个属性声明成反对称，而他
-- 自己的语料里那关系其实双向——这时该改的是本体，不是二十条事实。硬塞进一张表，
-- `resolution` 那一列就要同时表达两套语义，而读它的代码得先看 `reason` 才知道
-- 该怎么解释 `resolution`。
--
-- 形状也对不上：`fact_conflicts` 假设冲突总是「新的顶掉旧的」（old/new 两列），
-- 而自反违规只有**一条**事实（它自己跟自己矛盾），环是**一串**。

CREATE TABLE axiom_violations (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    -- self_loop  自反：一条事实的主宾相同，而谓词声明了 Irreflexive
    -- asymmetry  反对称：A→B 与 B→A 并存
    -- cycle      传递环：A→B→C→A，谓词同时是 Transitive 与 Asymmetric
    -- functional 基数：该唯一的主语侧（或宾语侧）出现了两个值
    kind       TEXT NOT NULL
               CHECK (kind IN ('self_loop', 'asymmetry', 'cycle', 'functional')),
    -- 涉及的两条事实。**自反那类两列相同**——一条事实跟自己矛盾，不需要第二条；
    -- 环取首尾，中间的在 path 里
    left_fact  UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    right_fact UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    -- 环的完整路径，按事实排列；其余三类为空。
    -- 留着是因为「A→B→C→A」比「A 与 C 矛盾」有用得多——人要顺着看一遍才知道
    -- 该撤哪一条。
    --
    -- 裸 UUID 数组而不是关联表：它是一条**证据**（当时那个环长这样），不是一组
    -- 需要被查询的关系。没有「哪些环经过这条事实」这种查法
    path       UUID[] NOT NULL DEFAULT '{}',
    status     TEXT NOT NULL DEFAULT 'open'
               CHECK (status IN ('open', 'resolved')),
    -- fact_retracted 判数据错，撤了事实
    -- axiom_relaxed  判定义错，去本体里改了那条公理
    -- accepted       两边都对，人认可这种并存（下一轮不再报）
    resolution TEXT CHECK (resolution IN ('fact_retracted', 'axiom_relaxed', 'accepted')),
    decided_by UUID REFERENCES users(id),
    decided_at TIMESTAMPTZ,
    detected_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- 同一处矛盾重跑不重复入库。检查是确定性的（环按事实 id 排序去重），
    -- 所以同一个环每次算出来的首尾都是同一对
    UNIQUE (kb_id, kind, left_fact, right_fact)
);

-- Review 页只捞待表态的
CREATE INDEX axiom_violations_open_idx ON axiom_violations (kb_id, detected_at DESC)
    WHERE status = 'open';
