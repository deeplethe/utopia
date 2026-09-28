-- 来自语料的判据是一份待审提案（#507 cut 2 / 0064 cut 1）。
--
-- 「全烃大于 8 且解释结论为气测异常时，可判定为优秀井」这一句有出处——
-- 哪份文档、哪一段、哪一句——`attribute_rules` 一直没有装它。0021 把它当成
-- 「人写」的，列上没有源；语料里读出来的判据在结构上一格不少，差的就是
-- 「指回去」。指回去是 0015 在 `pending_facts` 那一档讲过的同一件事——
-- 句子被记下不是事实被断言，那条提案就等一个 nod。
--
-- 改动范围与机制沿 0081 同档：加列、加 CHECK、加索引、加复合外键。
-- 唯一的新机制是 `state`：`proposed` / `nodded` / `declined`——
-- 审阅队列与规则页面要分开「人关掉的」与「没人看的」（都读 `enabled =
-- false`，两件事）。`enabled` 一字不动；`state` 是它旁边那根并立的轴。
--
-- 出处与归属规则同库：复合外键 `(kb_id, source_chunk_id) REFERENCES chunks
-- (kb_id, id)` 与 `(kb_id, source_document_id) REFERENCES documents (kb_id,
-- id)`，0070 / 0091 已经给被引表装好 `(kb_id, id)` 唯一约束。这一节加入
-- 同库家族。
--
-- `keep_their_kb` 触发器 0070 已装，本迁移不重复。
--
-- 函数体一律 `SET search_path = pg_catalog`、标识符一律 `public.*` 限定
-- （与 0070 / 0091 同一规矩：pg_restore 把会话 search_path 置空再灌数据，
-- 裸名解析不到任何东西；首位被恶意 schema 占住的会话也不能把函数建错地方）。

-- =====================================================================
-- §1 加列：出处五件套 + 一根独立于 enabled 的 state 轴
-- =====================================================================
ALTER TABLE public.attribute_rules
    ADD COLUMN source_kind        TEXT NOT NULL DEFAULT 'hand'
        CHECK (source_kind IN ('hand', 'text')),
    ADD COLUMN source_chunk_id    UUID,
    ADD COLUMN source_document_id UUID,
    ADD COLUMN proposed_at        TIMESTAMPTZ,
    ADD COLUMN proposed_by        UUID REFERENCES public.users(id) ON DELETE SET NULL,
    ADD COLUMN state              TEXT NOT NULL DEFAULT 'nodded'
        CHECK (state IN ('proposed', 'nodded', 'declined'));

-- §2 source_kind 与 state 一致：手写 ⇒ nodded（写即 nod，无提案可等）；
--    语料读出 ⇒ 三选一。
ALTER TABLE public.attribute_rules
    ADD CONSTRAINT attribute_rules_source_kind_state CHECK (
        (source_kind = 'hand'  AND state = 'nodded')
     OR (source_kind = 'text' AND state IN ('proposed', 'nodded', 'declined'))
    );

-- §3 source_kind = 'text' 时，块与文档都得出处——没有就不叫 sourced。
ALTER TABLE public.attribute_rules
    ADD CONSTRAINT attribute_rules_text_source_required CHECK (
        source_kind <> 'text'
        OR (source_chunk_id IS NOT NULL AND source_document_id IS NOT NULL)
    );

-- =====================================================================
-- §4 复合外键：出处与所属规则同库（#901 家族扩展）
-- =====================================================================
-- 默认先装单列外键（`... REFERENCES chunks(id)` 等），§4 把它们换掉——
-- `chunks` / `documents` 的 `(kb_id, id)` 唯一约束 0070 已装，直接换成
-- 复合键即可。`ON DELETE SET NULL` 限定到引用列：复合键的 SET NULL 默认
-- 会把 kb_id 一起置空，与 0091 errata_actions 同款规矩。块与文档被删，
-- 规则行还在；state / source_kind 都不动（human decision 保留，origin
-- 记在 audit_events，cut 5 处理）。
ALTER TABLE public.attribute_rules
    DROP CONSTRAINT IF EXISTS attribute_rules_source_chunk_id_fkey,
    DROP CONSTRAINT IF EXISTS attribute_rules_source_document_id_fkey,
    ADD CONSTRAINT attribute_rules_source_chunk_same_kb
        FOREIGN KEY (kb_id, source_chunk_id) REFERENCES public.chunks (kb_id, id)
        ON DELETE SET NULL (source_chunk_id),
    ADD CONSTRAINT attribute_rules_source_document_same_kb
        FOREIGN KEY (kb_id, source_document_id) REFERENCES public.documents (kb_id, id)
        ON DELETE SET NULL (source_document_id);

-- =====================================================================
-- §5 索引：审阅队列按库按 kind 按 state 走；按 chunk 找出处
-- =====================================================================
-- 审阅页面、governance 的「sourced criteria 待审」按 (kb_id, source_kind,
-- state) 走：提案就在这三档里——手写不进这条 partial 索引。
CREATE INDEX attribute_rules_kb_source_kind_state_idx
    ON public.attribute_rules (kb_id, source_kind, state)
    WHERE source_kind = 'text';

-- 「这一块都引出了哪些判据」按 chunk 走：原文页想看「这一段被采纳成了哪
-- 几条规则」，从块反查最快。
CREATE INDEX attribute_rules_source_chunk_idx
    ON public.attribute_rules (source_chunk_id)
    WHERE source_chunk_id IS NOT NULL;