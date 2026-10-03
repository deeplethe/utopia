-- 一条证据记下它那段原文说话的那一刻（0064 决定 3；0022 修订 2026-10-03 留下的口子）。
--
-- 陈述的见证是它所在那一节里文本说话的那一刻，一条陈述几份文档都说过时取最早的。这个
-- 「最早」从前只记在事实上：哪份证据给的、别的证据各是哪天，都没留下。于是删掉一篇文档、
-- 撤销一次删除时没法照章节重算，`documents::reattest_tx` 只能退回文档的日期，一份带日期的
-- 文档都不剩时干脆留着原值——删掉的文档的日期还在给这条陈述排时间线。
--
-- 现在每条证据自己记着：时间解析给陈述作证时顺手写下这一节的日期和那条日期的名字。
-- 证据所在的文档变了，事实的见证就是还在的证据里最早的那个，一条带日期的都不剩就是空。
-- 只有开放陈述的证据有这两列的值；类型化的行跟着它的来源陈述走，抄过去的证据不带。
ALTER TABLE fact_evidence ADD COLUMN attested_at TIMESTAMPTZ;
ALTER TABLE fact_evidence ADD COLUMN attested_by TEXT;

-- 已有的证据：事实的见证带着日期的名字时，名字出自哪篇文档的时间语境，那篇里的证据就是
-- 给出这个见证的那一条。别的证据各是哪天这里算不出来（要读标题），留空——留空的按文档
-- 自己的日期读，和这一列之前一样；那篇文档下次解析时间时补上。
UPDATE fact_evidence fe
   SET attested_at = f.attested_from, attested_by = f.attested_by
  FROM facts f, documents d
 WHERE f.id = fe.fact_id AND d.id = fe.document_id
   AND f.layer = 'open' AND f.attested_from IS NOT NULL AND f.attested_by IS NOT NULL
   AND jsonb_typeof(d.time_context -> 'entries') = 'array'
   AND EXISTS (
        SELECT 1 FROM jsonb_array_elements(d.time_context -> 'entries') e
         WHERE e ->> 'kind' = 'now'
           AND f.attested_by = CASE WHEN COALESCE(e ->> 'name', '') = '' THEN e ->> 'words'
                                    ELSE (e ->> 'name') || ' ' || (e ->> 'words') END);
