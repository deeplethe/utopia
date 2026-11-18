-- 角色词记在说它的那一块上（0070 决定 1，#1267）。
--
-- `statement_qualifiers` 从前按 (fact, role) 只留一行、先写者留着：同一个 external_id 重推、
-- 值变了，新值被 `DO NOTHING` 丢掉。现在每条角色词带上说它的 `chunk_id`，主键变成
-- (fact, role, chunk)，新推的块写新行，旧行不动。
--
-- 回填分三步，先陈述、后由陈述推出的行：
--   1. 开放陈述：每个 (fact, role) 的现值抄到它的每一块证据上，被取代的块也抄。
--   2. 类型化行与隐含行：从来源陈述（`typed_fact_sources`、`implied_fact_sources`）第 1 步
--      写下的逐块行里，挑出与这一行自己的值和实体都相同的，抄下它们落在这一行证据上的块。
--      于是另一条来源陈述的块不会替没说过这个词的陈述说话，说了别的值的来源也不会。
--   3. 剩下的（来源已经不在、或没有一条来源给过同样的值）照第 1 步抄到这一行的每一块证据上，
--      今天读得到的角色词一个不丢。
-- 每个 (fact, role) 只留下原来那一个值，所以迁移之后读出来的和之前一样。几条来源陈述对同一个
-- 角色词给了不同的值时，类型化行仍只读它原来的值；别的值由 0070 决定 3 带过来。
--
-- 没有证据的事实：`documents::purge` 删掉块和证据、留下已失效的事实和它的角色词。这些行
-- 没有块可挂，删掉并报数。**活着的**事实有角色词却没有证据，是账本坏了，整份中止。
--
-- `chunk_id` 是一条新的出处边（0048）：触发器同时查块与事实同库，导出体检扫它。

-- §0 前置检查：活着的事实有角色词却没有证据 → 中止
DO $$
DECLARE
    orphans bigint;
BEGIN
    SELECT COUNT(DISTINCT q.fact_id) INTO orphans
      FROM public.statement_qualifiers q
      JOIN public.facts f ON f.id = q.fact_id
     WHERE f.invalidated_at IS NULL
       AND NOT EXISTS (SELECT 1 FROM public.fact_evidence e WHERE e.fact_id = q.fact_id);
    IF orphans > 0 THEN
        RAISE EXCEPTION 'live facts with role words and no evidence (x%) — repair the ledger before role words can move onto chunks', orphans;
    END IF;
END;
$$;

DROP TRIGGER statement_qualifiers_same_kb ON public.statement_qualifiers;

ALTER TABLE public.statement_qualifiers
    ADD COLUMN chunk_id uuid,
    DROP CONSTRAINT statement_qualifiers_pkey;

INSERT INTO public.statement_qualifiers (fact_id, role, value, entity_id, chunk_id)
SELECT q.fact_id, q.role, q.value, q.entity_id, e.chunk_id
  FROM public.statement_qualifiers q
  JOIN public.facts f ON f.id = q.fact_id AND f.layer = 'open'
  JOIN public.fact_evidence e ON e.fact_id = q.fact_id
 WHERE q.chunk_id IS NULL;

-- 两条来源陈述在同一块上说了同一个值时只留一行
INSERT INTO public.statement_qualifiers (fact_id, role, value, entity_id, chunk_id)
SELECT DISTINCT q.fact_id, q.role, q.value, q.entity_id, s.chunk_id
  FROM public.statement_qualifiers q
  JOIN (SELECT fact_id, statement_id FROM public.typed_fact_sources
        UNION
        SELECT fact_id, statement_id FROM public.implied_fact_sources
         WHERE statement_id IS NOT NULL) src ON src.fact_id = q.fact_id
  JOIN public.statement_qualifiers s
    ON s.fact_id = src.statement_id AND s.role = q.role AND s.chunk_id IS NOT NULL
   AND s.value IS NOT DISTINCT FROM q.value AND s.entity_id IS NOT DISTINCT FROM q.entity_id
  JOIN public.fact_evidence e ON e.fact_id = q.fact_id AND e.chunk_id = s.chunk_id
 WHERE q.chunk_id IS NULL;

INSERT INTO public.statement_qualifiers (fact_id, role, value, entity_id, chunk_id)
SELECT q.fact_id, q.role, q.value, q.entity_id, e.chunk_id
  FROM public.statement_qualifiers q
  JOIN public.fact_evidence e ON e.fact_id = q.fact_id
 WHERE q.chunk_id IS NULL
   AND NOT EXISTS (SELECT 1 FROM public.statement_qualifiers n
                    WHERE n.fact_id = q.fact_id AND n.role = q.role AND n.chunk_id IS NOT NULL);

DO $$
DECLARE
    dropped bigint;
BEGIN
    DELETE FROM public.statement_qualifiers q
     WHERE q.chunk_id IS NULL
       AND NOT EXISTS (SELECT 1 FROM public.fact_evidence e WHERE e.fact_id = q.fact_id);
    GET DIAGNOSTICS dropped = ROW_COUNT;
    RAISE NOTICE 'dropped % role words on facts with no evidence', dropped;
END;
$$;

DELETE FROM public.statement_qualifiers WHERE chunk_id IS NULL;

ALTER TABLE public.statement_qualifiers
    ALTER COLUMN chunk_id SET NOT NULL,
    ADD CONSTRAINT statement_qualifiers_chunk_id_fkey
        FOREIGN KEY (chunk_id) REFERENCES public.chunks(id) ON DELETE CASCADE,
    ADD PRIMARY KEY (fact_id, role, chunk_id);

-- 删块时级联要按块找
CREATE INDEX statement_qualifiers_chunk_fk_idx ON public.statement_qualifiers (chunk_id);

-- 实体值与块都必须与所属事实同库
CREATE OR REPLACE FUNCTION public.squalifier_stays_inside_its_facts_kb() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE
    fact_kb uuid;
    ref_kb  uuid;
BEGIN
    SELECT kb_id INTO fact_kb FROM public.facts WHERE id = NEW.fact_id;
    IF fact_kb IS NULL THEN
        RETURN NEW;
    END IF;
    SELECT kb_id INTO ref_kb FROM public.chunks WHERE id = NEW.chunk_id;
    IF ref_kb IS NOT NULL AND ref_kb <> fact_kb THEN
        RAISE EXCEPTION 'statement qualifier chunk % cannot live outside the fact''s knowledge base',
            NEW.chunk_id;
    END IF;
    IF NEW.entity_id IS NOT NULL THEN
        SELECT kb_id INTO ref_kb FROM public.entities WHERE id = NEW.entity_id;
        IF ref_kb IS NOT NULL AND ref_kb <> fact_kb THEN
            RAISE EXCEPTION 'statement qualifier entity % cannot live outside the fact''s knowledge base',
                NEW.entity_id;
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER statement_qualifiers_same_kb
    BEFORE INSERT OR UPDATE OF fact_id, entity_id, chunk_id ON public.statement_qualifiers
    FOR EACH ROW EXECUTE FUNCTION public.squalifier_stays_inside_its_facts_kb();
