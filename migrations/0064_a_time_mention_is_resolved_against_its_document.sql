-- 一条时间提及按它的文档来读（0045 第一、二刀的账本侧）。
--
-- 0061 把时间词照抄进 `time_mentions`，说好读法的列后一刀再加。这就是那一刀。0045 的
-- 前五条决定，账本各落成什么：
--
--   决定 1  时间提及是开放图谱的事实：字、块、字符偏移（0061 已有）。一条陈述的
--           `when` 与 `ended` 各指一条提及——`role` 记它来自陈述的哪个槽，唯一键加上
--           它：起于「2019 年」、止于「2019 年」是同一处字的两条提及。
--   决定 2  模型读，代码算。模型给的只是解释——`shape`（点、区间、截至、时长、没给
--           日期的结束）、`reference`（照写的绝对值；或锚点 + 偏移；或没有）、
--           `granularity`（字说到梯子的哪一级）——三列照存，取值是抽取合同的词汇，库不认。
--           日期算术在代码里做，结果落 `resolved_*`：两端各自的时刻与精度，梯子与 `facts`
--           同一张（0024：year … second），结束端多一个 'unknown'（结束了，不知哪天）；
--           两条 CHECK 照抄 `facts_from_precision_matches_date` /
--           `facts_to_precision_matches_date`（0033），存的值截到自己的精度。
--           `grade`：A 原文写明的绝对日期，B 按文档自己给的锚点算出来的，C 没算出来。
--           `resolved_at` 是最近一次算的时刻。
--   决定 3  文档把它的时间语境从一块带到下一块：`documents.time_context` 存它（文档自己
--           的日期、它定义的期间与历法、叙述设下的锚点），`time_context_at` 是最近一次
--           写下它的时刻。服务端边抽取边填。
--           文档的日期只来自正文或给它日期的来源系统。上传、同步、抽取的时刻是记录时间，
--           不进语境：`doc_time_source = 'upload_time'` 读作**没有日期**（#714）。文件的
--           修改时刻也不是文档的日期——它说的是文件系统上次写这个文件是什么时候，与文档
--           说的是哪一天无关，'file_mtime' 同样读作没有日期。两种都把 `doc_time` 清空、
--           来源改成 'none'；列的默认值也改成 'none'：从前默认 'file_mtime'，一行没给
--           日期插进来就顶着一个它没有的来源。这一列没有 CHECK，代码里的取值是
--           content / source / none（`Document::dated_at` 只认前两个）。
--   决定 4  算不出来的等着：grade C 的提及留着字和解释，`resolved_*` 全空，不给任何陈述
--           日期；锚点后来到了再算一次，`resolved_at` 跟着走。
--   决定 5  三个时间三列：valid 是算出来的（开放行的 `valid_*` 由 `graph::set_open_validity`
--           按解算写，不由模型写）、attested 是文档自己的日期（没有就空——服务端后续那
--           一刀）、recorded 是账本何时知道。
--
-- 纯增量，不回填解释。已有的提及全记成 role = 'when'：0061 记 when 与 ended 时没分槽，
-- 分不回去了；没有已发布的版本，不留遗留层。
ALTER TABLE time_mentions
    -- 来自陈述的哪个槽
    ADD COLUMN role TEXT NOT NULL DEFAULT 'when' CHECK (role IN ('when', 'ended')),
    -- 模型的解释：照存
    ADD COLUMN shape       TEXT,
    ADD COLUMN reference   JSONB,
    ADD COLUMN granularity TEXT,
    -- 代码的解算
    ADD COLUMN grade                   CHAR(1) CHECK (grade IN ('A', 'B', 'C')),
    ADD COLUMN resolved_from           TIMESTAMPTZ,
    ADD COLUMN resolved_from_precision TEXT,
    ADD COLUMN resolved_to             TIMESTAMPTZ,
    ADD COLUMN resolved_to_precision   TEXT,
    ADD COLUMN resolved_at             TIMESTAMPTZ,
    -- 三值逻辑的坑同 0033：每个分支先写 IS NOT NULL
    ADD CONSTRAINT time_mentions_from_precision_matches_date CHECK (
        (resolved_from IS NULL AND resolved_from_precision IS NULL)
        OR (resolved_from IS NOT NULL
            AND resolved_from_precision IS NOT NULL
            AND resolved_from_precision IN ('year', 'month', 'day', 'hour', 'minute', 'second')
            AND resolved_from = date_trunc(resolved_from_precision, resolved_from, 'UTC'))),
    ADD CONSTRAINT time_mentions_to_precision_matches_date CHECK (
        (resolved_to IS NOT NULL
            AND resolved_to_precision IS NOT NULL
            AND resolved_to_precision IN ('year', 'month', 'day', 'hour', 'minute', 'second')
            AND resolved_to = date_trunc(resolved_to_precision, resolved_to, 'UTC'))
        OR (resolved_to IS NULL
            AND (resolved_to_precision IS NULL OR resolved_to_precision = 'unknown')));

-- 同一处字在起与止各一条
ALTER TABLE time_mentions
    DROP CONSTRAINT time_mentions_fact_id_chunk_id_char_start_key,
    ADD CONSTRAINT time_mentions_fact_id_chunk_id_char_start_role_key
        UNIQUE (fact_id, chunk_id, char_start, role);

ALTER TABLE documents
    ADD COLUMN time_context    JSONB,
    ADD COLUMN time_context_at TIMESTAMPTZ,
    ALTER COLUMN doc_time_source SET DEFAULT 'none';

-- 上传时刻与文件修改时刻都不是文档的日期
UPDATE documents SET doc_time = NULL, doc_time_source = 'none'
 WHERE doc_time_source IN ('upload_time', 'file_mtime');
