-- 同一条不变量，补到 0070 之后才进库的表上（#901）。
--
-- 0070 把「出处与归属语义上的每一个引用，两端必须同属一个库」装上了当时的
-- 语义账本。但 catalog 守卫的 scratch 库只迁到 0070——之后进库的表对守卫
-- 结构性不可见，「新引用列不归档」这道关门在表粒度上又重演了一遍（与 #846
-- 修掉的列粒度盲区同形）。本次把整圈 post-0070 的库属表按责任分一遍类，
-- 该进同库家族的进家族：
--
--   implication_rules   (0073) 规则本体：subject_type_id/object_type_id →
--                       entity_types、conclude_property_id → relation_types
--                       全是单列外键；kb_id 也不在不可过户名单里——改 kb_id
--                       会把指着它的来源行一次全脱锚
--   phrase_readings     (0073) 读数缓存：entity_id → entities 单列外键；
--                       别库的实体答案会把别库语义喂进本库的蕴含
--   implied_fact_sources (0073) 隐含事实的出处：rule_id → implication_rules、
--                       statement_id → facts、entity_id → entities 三条单列
--                       外键；行自己没有 kb 列，归属按所属 fact 的库判——
--                       与 typed_fact_sources 同一个形状，走同一条路
--   errata_runs /
--   errata_actions      (0074) 勘误账：run/document/fact/statement/
--                       predicate/new_fact 六条引用全是单列外键。
--                       (statement_id, predicate_id) 这对撤销键物化每轮
--                       都要读——越库的行是一条永远不兑现的死引用
--   attribute_rule_versions (0076) 规则定义史：rule_id → attribute_rules
--                       单列外键；derived_facts.attribute_rule_version_id
--                       同。派生记下「凭哪一版定义推出」，那版定义不许在别库
--   name_vectors        (0080) 出生就带复合键与不可过户触发器，但一直没登记
--                       ——「机制装上了、责任面没认它」也是漂移，这里归档
--   attribute_rules.join_predicate_id (0071) 出生就有复合外键，同样缺体检
--                       分支——这次一并补齐
--
-- 机制沿用 0070 的同一套分法，不加新东西：
--   §1b 行自己带 kb_id 的边 → 复合外键 `(kb_id, ref) REFERENCES t (kb_id,
--       id)`，共 13 条新边（errata_actions.new_fact_id 的 ON DELETE SET NULL
--       照旧限定到引用列——复合键的 SET NULL 会把 kb_id 一起置空）。
--   §1c implied_fact_sources 没有 kb 列、归属由所属 fact 作证 → 与
--       typed_fact_sources 同款 owner-derived 触发器，三条引用一次判。
--   §2  五张新的 kb 自持表补 `keep_their_kb`（name_vectors 出生就有）。
--   §1a 被引表 implication_rules / attribute_rule_versions / errata_runs
--       各补一条 `UNIQUE (kb_id, id)`——同库家族里被引表的标配支撑。
--
-- §0 前置检查照 0070 的规矩：先数存量越界行（含绕过触发器恢复进来的
-- 悬空引用——LEFT JOIN 落空与越库同罪），有就整份中止并报出是哪条边、
-- 坏了几行。装上不变量却对已违反它的账本报喜，等于替坏数据背书；
-- 这段查询同时是运维审计，升级前在真实库上跑一遍就知道会不会半截停。
--
-- 函数体一律 `SET search_path = pg_catalog`、标识符一律 `public.*` 限定：
-- pg_restore 会把会话 search_path 置空再灌数据，裸名解析不到任何东西；
-- 首位被恶意 schema 占住的会话也不能把函数建错地方。
--
-- =====================================================================
-- §0 前置检查：新覆盖的 16 条边上，存量越界/悬空行 → 整份中止
-- =====================================================================
DO $$
DECLARE
    report text;
BEGIN
    SELECT string_agg(edge || ' x' || n, '; ' ORDER BY edge) INTO report
      FROM (
        SELECT edge, COUNT(*) AS n FROM (
            -- 隐含事实的出处：行自己没有 kb 列，归属按所属 fact 的库判
            SELECT 'implied.rule' AS edge, f.kb_id AS owner_kb, r.kb_id AS ref_kb
              FROM public.implied_fact_sources i
              JOIN public.facts f ON f.id = i.fact_id
              LEFT JOIN public.implication_rules r ON r.id = i.rule_id
            UNION ALL
            SELECT 'implied.statement', f.kb_id, s.kb_id
              FROM public.implied_fact_sources i
              JOIN public.facts f ON f.id = i.fact_id
              LEFT JOIN public.facts s ON s.id = i.statement_id
             WHERE i.statement_id IS NOT NULL
            UNION ALL
            SELECT 'implied.entity', f.kb_id, e.kb_id
              FROM public.implied_fact_sources i
              JOIN public.facts f ON f.id = i.fact_id
              LEFT JOIN public.entities e ON e.id = i.entity_id
             WHERE i.entity_id IS NOT NULL
            UNION ALL
            -- 规则本体与读数缓存：行自己的 kb_id 作证
            SELECT 'irule.subject_type', r.kb_id, t.kb_id
              FROM public.implication_rules r
              LEFT JOIN public.entity_types t ON t.id = r.subject_type_id
             WHERE r.subject_type_id IS NOT NULL
            UNION ALL
            SELECT 'irule.object_type', r.kb_id, t.kb_id
              FROM public.implication_rules r
              LEFT JOIN public.entity_types t ON t.id = r.object_type_id
             WHERE r.object_type_id IS NOT NULL
            UNION ALL
            SELECT 'irule.conclude_property', r.kb_id, p.kb_id
              FROM public.implication_rules r
              LEFT JOIN public.relation_types p ON p.id = r.conclude_property_id
            UNION ALL
            SELECT 'reading.entity', pr.kb_id, e.kb_id
              FROM public.phrase_readings pr
              LEFT JOIN public.entities e ON e.id = pr.entity_id
             WHERE pr.entity_id IS NOT NULL
            UNION ALL
            -- 规则定义史与凭它推出的派生
            SELECT 'arversion.rule', v.kb_id, r.kb_id
              FROM public.attribute_rule_versions v
              LEFT JOIN public.attribute_rules r ON r.id = v.rule_id
            UNION ALL
            SELECT 'derived.attribute_rule_version', d.kb_id, v.kb_id
              FROM public.derived_facts d
              LEFT JOIN public.attribute_rule_versions v
                ON v.id = d.attribute_rule_version_id
             WHERE d.attribute_rule_version_id IS NOT NULL
            UNION ALL
            -- 勘误账
            SELECT 'erratarun.document', r.kb_id, d.kb_id
              FROM public.errata_runs r
              LEFT JOIN public.documents d ON d.id = r.document_id
            UNION ALL
            SELECT 'errata.run', a.kb_id, r.kb_id
              FROM public.errata_actions a
              LEFT JOIN public.errata_runs r ON r.id = a.run_id
            UNION ALL
            SELECT 'errata.document', a.kb_id, d.kb_id
              FROM public.errata_actions a
              LEFT JOIN public.documents d ON d.id = a.document_id
            UNION ALL
            SELECT 'errata.fact', a.kb_id, f.kb_id
              FROM public.errata_actions a
              LEFT JOIN public.facts f ON f.id = a.fact_id
             WHERE a.fact_id IS NOT NULL
            UNION ALL
            SELECT 'errata.statement', a.kb_id, f.kb_id
              FROM public.errata_actions a
              LEFT JOIN public.facts f ON f.id = a.statement_id
             WHERE a.statement_id IS NOT NULL
            UNION ALL
            SELECT 'errata.predicate', a.kb_id, p.kb_id
              FROM public.errata_actions a
              LEFT JOIN public.relation_types p ON p.id = a.predicate_id
             WHERE a.predicate_id IS NOT NULL
            UNION ALL
            SELECT 'errata.new_fact', a.kb_id, f.kb_id
              FROM public.errata_actions a
              LEFT JOIN public.facts f ON f.id = a.new_fact_id
             WHERE a.new_fact_id IS NOT NULL
        ) refs
        WHERE ref_kb IS DISTINCT FROM owner_kb
        GROUP BY edge
    ) bad;
    IF report IS NOT NULL THEN
        RAISE EXCEPTION 'cross-KB references already present (%) — repair the ledger before this invariant can be installed', report
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
END;
$$;

-- =====================================================================
-- §1a 复合键的支撑唯一约束：每个被引表一个 (kb_id, id)
-- =====================================================================
-- implied_fact_sources.rule_id 由触发器查 implication_rules 的 kb，复合外键
-- 用不上这条唯一约束；补上它是同库家族的标配——被引表一家一个 (kb_id, id)，
-- 以后的复合边才有键可指（与 0070 给八张被引表补键同一道理）。
ALTER TABLE public.implication_rules
    ADD CONSTRAINT implication_rules_kb_id_key UNIQUE (kb_id, id);
ALTER TABLE public.attribute_rule_versions
    ADD CONSTRAINT attribute_rule_versions_kb_id_key UNIQUE (kb_id, id);
ALTER TABLE public.errata_runs
    ADD CONSTRAINT errata_runs_kb_id_key UNIQUE (kb_id, id);

-- =====================================================================
-- §1b 声明式边：源行自己带 kb_id 的引用 → 复合外键（13 条）
-- =====================================================================
ALTER TABLE public.implication_rules
    DROP CONSTRAINT implication_rules_subject_type_id_fkey,
    DROP CONSTRAINT implication_rules_object_type_id_fkey,
    DROP CONSTRAINT implication_rules_conclude_property_id_fkey,
    ADD CONSTRAINT implication_rules_subject_type_same_kb
        FOREIGN KEY (kb_id, subject_type_id) REFERENCES public.entity_types (kb_id, id)
        ON DELETE CASCADE,
    ADD CONSTRAINT implication_rules_object_type_same_kb
        FOREIGN KEY (kb_id, object_type_id) REFERENCES public.entity_types (kb_id, id)
        ON DELETE CASCADE,
    ADD CONSTRAINT implication_rules_conclude_property_same_kb
        FOREIGN KEY (kb_id, conclude_property_id) REFERENCES public.relation_types (kb_id, id)
        ON DELETE CASCADE;

ALTER TABLE public.phrase_readings
    DROP CONSTRAINT phrase_readings_entity_id_fkey,
    ADD CONSTRAINT phrase_readings_entity_same_kb
        FOREIGN KEY (kb_id, entity_id) REFERENCES public.entities (kb_id, id)
        ON DELETE CASCADE;

ALTER TABLE public.attribute_rule_versions
    DROP CONSTRAINT attribute_rule_versions_rule_id_fkey,
    ADD CONSTRAINT attribute_rule_versions_rule_same_kb
        FOREIGN KEY (kb_id, rule_id) REFERENCES public.attribute_rules (kb_id, id)
        ON DELETE CASCADE;

ALTER TABLE public.derived_facts
    DROP CONSTRAINT derived_facts_attribute_rule_version_id_fkey,
    ADD CONSTRAINT derived_facts_attribute_rule_version_same_kb
        FOREIGN KEY (kb_id, attribute_rule_version_id)
        REFERENCES public.attribute_rule_versions (kb_id, id)
        ON DELETE CASCADE;

ALTER TABLE public.errata_runs
    DROP CONSTRAINT errata_runs_document_id_fkey,
    ADD CONSTRAINT errata_runs_document_same_kb
        FOREIGN KEY (kb_id, document_id) REFERENCES public.documents (kb_id, id)
        ON DELETE CASCADE;

ALTER TABLE public.errata_actions
    DROP CONSTRAINT errata_actions_run_id_fkey,
    DROP CONSTRAINT errata_actions_document_id_fkey,
    DROP CONSTRAINT errata_actions_fact_id_fkey,
    DROP CONSTRAINT errata_actions_statement_id_fkey,
    DROP CONSTRAINT errata_actions_predicate_id_fkey,
    DROP CONSTRAINT errata_actions_new_fact_id_fkey,
    ADD CONSTRAINT errata_actions_run_same_kb
        FOREIGN KEY (kb_id, run_id) REFERENCES public.errata_runs (kb_id, id)
        ON DELETE CASCADE,
    ADD CONSTRAINT errata_actions_document_same_kb
        FOREIGN KEY (kb_id, document_id) REFERENCES public.documents (kb_id, id)
        ON DELETE CASCADE,
    ADD CONSTRAINT errata_actions_fact_same_kb
        FOREIGN KEY (kb_id, fact_id) REFERENCES public.facts (kb_id, id)
        ON DELETE CASCADE,
    ADD CONSTRAINT errata_actions_statement_same_kb
        FOREIGN KEY (kb_id, statement_id) REFERENCES public.facts (kb_id, id)
        ON DELETE CASCADE,
    ADD CONSTRAINT errata_actions_predicate_same_kb
        FOREIGN KEY (kb_id, predicate_id) REFERENCES public.relation_types (kb_id, id)
        ON DELETE CASCADE,
    -- SET NULL 限定到引用列：不限定会把 kb_id 一起置空（0070 同一条规矩，
    -- PG15+ 的列清单语法）
    ADD CONSTRAINT errata_actions_new_fact_same_kb
        FOREIGN KEY (kb_id, new_fact_id) REFERENCES public.facts (kb_id, id)
        ON DELETE SET NULL (new_fact_id);

-- =====================================================================
-- §1c 触发器边：库归属在所属 fact 手上、引用行没有 kb_id 的 3 条
-- =====================================================================
-- 隐含事实的出处：规则、触发它的陈述、触发它的实体必须与所属 fact 同库——
-- 别库的规则改了状态会隔空作废本库的事实，别库的来源行也可能把本该作废的
-- 事实钉活（materialize 的 sweep 按 rule.kb_id join 查，越库行两头都够不着）。
-- 父行不存在交给外键报错；这里只管「都存在，却不在同一个库」。
CREATE FUNCTION public.implied_source_stays_inside_its_kb() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE
    fact_kb uuid;
    ref_kb  uuid;
BEGIN
    SELECT kb_id INTO fact_kb FROM public.facts WHERE id = NEW.fact_id;
    IF fact_kb IS NULL THEN
        RETURN NEW;
    END IF;
    SELECT kb_id INTO ref_kb FROM public.implication_rules WHERE id = NEW.rule_id;
    IF ref_kb IS NOT NULL AND ref_kb <> fact_kb THEN
        RAISE EXCEPTION 'implied fact source % cannot name rule % in another knowledge base',
            NEW.fact_id, NEW.rule_id;
    END IF;
    IF NEW.statement_id IS NOT NULL THEN
        SELECT kb_id INTO ref_kb FROM public.facts WHERE id = NEW.statement_id;
        IF ref_kb IS NOT NULL AND ref_kb <> fact_kb THEN
            RAISE EXCEPTION 'implied fact source % cannot name statement % in another knowledge base',
                NEW.fact_id, NEW.statement_id;
        END IF;
    END IF;
    IF NEW.entity_id IS NOT NULL THEN
        SELECT kb_id INTO ref_kb FROM public.entities WHERE id = NEW.entity_id;
        IF ref_kb IS NOT NULL AND ref_kb <> fact_kb THEN
            RAISE EXCEPTION 'implied fact source % cannot name entity % in another knowledge base',
                NEW.fact_id, NEW.entity_id;
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER implied_fact_sources_same_kb
    BEFORE INSERT OR UPDATE OF fact_id, rule_id, statement_id, entity_id
    ON public.implied_fact_sources
    FOR EACH ROW EXECUTE FUNCTION public.implied_source_stays_inside_its_kb();

-- =====================================================================
-- §2 库归属不可过户：0070 的同一个共享函数，补到五张新 kb 表上
-- =====================================================================
-- 被引行的 kb_id 一动，所有指着它的行一次全脱锚；复合外键只护住被指的行，
-- 不可过户是全量保留的。name_vectors 出生就带着这条，这里不重复装。
CREATE TRIGGER implication_rules_keep_their_kb
    BEFORE UPDATE OF kb_id ON public.implication_rules
    FOR EACH ROW EXECUTE FUNCTION public.kb_ownership_is_not_reassigned();

CREATE TRIGGER phrase_readings_keep_their_kb
    BEFORE UPDATE OF kb_id ON public.phrase_readings
    FOR EACH ROW EXECUTE FUNCTION public.kb_ownership_is_not_reassigned();

CREATE TRIGGER errata_runs_keep_their_kb
    BEFORE UPDATE OF kb_id ON public.errata_runs
    FOR EACH ROW EXECUTE FUNCTION public.kb_ownership_is_not_reassigned();

CREATE TRIGGER errata_actions_keep_their_kb
    BEFORE UPDATE OF kb_id ON public.errata_actions
    FOR EACH ROW EXECUTE FUNCTION public.kb_ownership_is_not_reassigned();

CREATE TRIGGER attribute_rule_versions_keep_their_kb
    BEFORE UPDATE OF kb_id ON public.attribute_rule_versions
    FOR EACH ROW EXECUTE FUNCTION public.kb_ownership_is_not_reassigned();
