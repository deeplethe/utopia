-- 改写出来的行找回它由哪些陈述算出（#967 的后续）。
--
-- 物化出来的行被改写（作废旧行、插修正行）时，要把「它由哪些陈述、哪条规则算出」带到
-- 修正行上：`from_statement_id`、`implied`、`typed_fact_sources`、`implied_fact_sources`。
-- 闭合与搬移从 #911 起带着，人改区间从 #967 起带着。在那之前改写出来的行没有这些：
-- 陈述不再被活着的类型化行代表，画面上它按原话、原来的区间回来；下一轮物化还照陈述
-- 再算一行，与修正行并排。这里把链接还给修正行，把后来算出来的重复行作废。
--
-- 修正行：活着的类型化行，有 supersedes，自己没有 from_statement_id、也不是 implied，
-- 顺着 supersedes 往上能走到一条带链接的行（从陈述物化的，或规则算的）——离它最近的
-- 那条就是它的来源行。人手写的行，链上没有这样的行，不动；带着链接改写出来的行不在此列。
--
-- 往上走的每一步都得是**改写**：新行与它作废的旧行在同一个事务里写下，新行的
-- recorded_at 就是旧行的 invalidated_at（两个都是 now()）。闭合、搬移、人改区间都是
-- 这样。一条后来的观察给裸行补上起点（`insert_fact` 的时间精化）也会链上 supersedes，
-- 但它是另一次观察、另一个事务写的：人点头确认的事实不是陈述算出来的，不给它挂上
-- 陈述——挂上了，陈述一撤，它会被当成算出来的行作废。
--
-- 它找回的陈述：来源行的 from_statement_id，加上链上各行还留着的来源（来源行一作废，
-- 下一轮物化会删掉它的 typed_fact_sources；implied_fact_sources 还在）。
--
-- 重复行：同库、同主语谓词宾语、活着、不是修正行，来源落在修正行找回的陈述里。一条陈述
-- 在一个谓词下只该有一行，所以它只能是链接丢了之后物化重算的。它的来源也归修正行；
-- 它自己**作废，不删**——账本只追加，记录轴倒回去仍看得见它活过。
--
-- 已知的边界：重复行活着时可能按旧起点关上过别的行，那一端要等这条时间线下次重算才回到
-- 修正后的起点，迁移跑不了引擎的重算。来源陈述后来撤回了的修正行，找回链接之后，下一轮
-- 物化会把它作废——与 #967 之后改写出来的行一样：人改的是时间，不是出处。
--
-- 普通语句（sqlx 把每个迁移包在一个事务里）；两个计数用 NOTICE 报出来，再跑一遍是 0 和 0。
DO $$
DECLARE
    repaired bigint;
    retired bigint;
BEGIN
    -- 修正行与它最近的来源行（depth = 从修正行往上走几步到它）；每一步都是同一个事务里的改写
    CREATE TEMP TABLE repair_rows ON COMMIT DROP AS
    WITH RECURSIVE up (fact_id, prev, depth) AS (
        SELECT f.id, p.id, 1
          FROM facts f
          JOIN facts p ON p.id = f.supersedes AND p.invalidated_at = f.recorded_at
         WHERE f.layer = 'typed' AND f.invalidated_at IS NULL
           AND f.from_statement_id IS NULL AND NOT f.implied
        UNION ALL
        SELECT up.fact_id, pp.id, up.depth + 1
          FROM up
          JOIN facts p ON p.id = up.prev
          JOIN facts pp ON pp.id = p.supersedes AND pp.invalidated_at = p.recorded_at
         WHERE p.from_statement_id IS NULL AND NOT p.implied
    )
    SELECT DISTINCT ON (up.fact_id)
           up.fact_id, up.depth, p.from_statement_id, p.implied
      FROM up
      JOIN facts p ON p.id = up.prev
     WHERE p.from_statement_id IS NOT NULL OR p.implied
     ORDER BY up.fact_id, up.depth;

    -- 链上的每一行：修正行自己，一直到来源行
    CREATE TEMP TABLE repair_chain ON COMMIT DROP AS
    WITH RECURSIVE down (fact_id, member, depth) AS (
        SELECT r.fact_id, r.fact_id, 0 FROM repair_rows r
        UNION ALL
        SELECT down.fact_id, f.supersedes, down.depth + 1
          FROM down
          JOIN facts f ON f.id = down.member
          JOIN repair_rows r ON r.fact_id = down.fact_id
         WHERE f.supersedes IS NOT NULL AND down.depth < r.depth
    )
    SELECT fact_id, member FROM down;

    -- 找回的陈述与规则来源
    CREATE TEMP TABLE repair_statements ON COMMIT DROP AS
    SELECT fact_id, from_statement_id AS statement_id
      FROM repair_rows
     WHERE from_statement_id IS NOT NULL
    UNION
    SELECT c.fact_id, s.statement_id
      FROM repair_chain c
      JOIN typed_fact_sources s ON s.fact_id = c.member;

    CREATE TEMP TABLE repair_implied ON COMMIT DROP AS
    SELECT DISTINCT c.fact_id, i.rule_id, i.statement_id, i.entity_id
      FROM repair_chain c
      JOIN implied_fact_sources i ON i.fact_id = c.member;

    -- 物化之后照同一陈述（或同一规则的同一来源）重算出来的重复行
    CREATE TEMP TABLE repair_duplicates ON COMMIT DROP AS
    SELECT DISTINCT r.fact_id, d.id AS duplicate
      FROM repair_rows r
      JOIN facts f ON f.id = r.fact_id
      JOIN facts d ON d.kb_id = f.kb_id AND d.id <> f.id
                  AND d.layer = 'typed' AND d.invalidated_at IS NULL
                  AND d.subject_id = f.subject_id
                  AND d.predicate_id IS NOT DISTINCT FROM f.predicate_id
                  AND d.object_id IS NOT DISTINCT FROM f.object_id
                  AND d.object_value IS NOT DISTINCT FROM f.object_value
     WHERE NOT EXISTS (SELECT 1 FROM repair_rows x WHERE x.fact_id = d.id)
       AND (EXISTS (SELECT 1 FROM repair_statements s
                     WHERE s.fact_id = r.fact_id
                       AND (s.statement_id = d.from_statement_id
                            OR EXISTS (SELECT 1 FROM typed_fact_sources t
                                        WHERE t.fact_id = d.id AND t.statement_id = s.statement_id)))
            OR EXISTS (SELECT 1 FROM repair_implied i
                         JOIN implied_fact_sources x ON x.fact_id = d.id AND x.rule_id = i.rule_id
                        WHERE i.fact_id = r.fact_id
                          AND x.statement_id IS NOT DISTINCT FROM i.statement_id
                          AND x.entity_id IS NOT DISTINCT FROM i.entity_id));

    UPDATE facts f
       SET from_statement_id = r.from_statement_id, implied = r.implied
      FROM repair_rows r
     WHERE f.id = r.fact_id;
    GET DIAGNOSTICS repaired = ROW_COUNT;

    INSERT INTO typed_fact_sources (fact_id, statement_id)
    SELECT fact_id, statement_id FROM repair_statements
    UNION
    SELECT d.fact_id, t.statement_id
      FROM repair_duplicates d
      JOIN typed_fact_sources t ON t.fact_id = d.duplicate
    ON CONFLICT DO NOTHING;

    INSERT INTO implied_fact_sources (fact_id, rule_id, statement_id, entity_id)
    SELECT fact_id, rule_id, statement_id, entity_id FROM repair_implied
    UNION
    SELECT d.fact_id, x.rule_id, x.statement_id, x.entity_id
      FROM repair_duplicates d
      JOIN implied_fact_sources x ON x.fact_id = d.duplicate
    ON CONFLICT DO NOTHING;

    UPDATE facts
       SET invalidated_at = now()
     WHERE id IN (SELECT duplicate FROM repair_duplicates) AND invalidated_at IS NULL;
    GET DIAGNOSTICS retired = ROW_COUNT;

    RAISE NOTICE '0096：% 条改写出来的行找回了陈述，% 条重复行作废', repaired, retired;
END $$;
