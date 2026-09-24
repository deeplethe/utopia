-- 导出前的出处体检（export::provenance_integrity 的唯一查询）。
--
-- 覆盖面 = 0070 保护的全部结构引用边：catalog 守卫
-- （migration_0070_runs_under_any_search_path.rs）从 pg_catalog 数出每一条
-- 受保护的列级引用，再回来核对这份文件里的扫描分支——一边漏登记都是 CI 红。
-- 所以每条边不是「导出会解析才查」，而是「schema 保护了就必须在发出第一个
-- 字节前查过」。
--
-- 机读约定（守卫按它比对，不许只改一边）：
--   -- @edge src_table.src_col -> tgt_table.tgt_col
--       紧随其后的 SELECT 分支是这条结构边的体检。结构身份按四元组核对，
--       不是按报错 label——两条边可以共用一个 label（class.disjoint 的
--       a_id/b_id），分支数必须等于 catalog 数出的边数。
--   -- @filter label
--       紧随其后的分支是「同库但不在导出集」的过滤完整性检查
--       （merged_into 非空的实体）：它护的是导出过滤口径，不是一条
--       schema 引用边，与 @edge 分开登记。
--
-- 每个扫描分支恰好带一个标记；标记写错方向、分支漏标记、或
-- catalog 边没有对应分支，守卫都会报出来。新增一条边时同时在
-- malformed_rows_fail_every_exported_edge_closed 里种一行坏数据。
-- 分支注释里不要写单引号、select 关键字、或 union all 合并字样——
-- 守卫按字面量与分支分隔符解析，注释里的同名字样会被误当成结构。
SELECT edge, kind, COUNT(*) AS rows FROM (
    -- @edge fact_evidence.chunk_id -> chunks.id
    -- 证据的段落：quote_origins 按它 JOIN chunks 取 origin——别库/悬空
    -- 的段落会让引文来源静默消失
    SELECT 'evidence.chunk'::text AS edge, 'cross_kb'::text AS kind,
           c.kb_id IS DISTINCT FROM f.kb_id AS bad
      FROM fact_evidence e
      JOIN facts f ON f.id = e.fact_id
      LEFT JOIN chunks c ON c.id = e.chunk_id
     WHERE f.kb_id = $1
    UNION ALL
    -- @edge fact_evidence.document_id -> documents.id
    -- 证据的文档指针：铸成 prov:wasDerivedFrom 的文档 IRI
    SELECT 'evidence.document', 'cross_kb', d.kb_id IS DISTINCT FROM f.kb_id
      FROM fact_evidence e
      JOIN facts f ON f.id = e.fact_id
      LEFT JOIN documents d ON d.id = e.document_id
     WHERE f.kb_id = $1 AND e.document_id IS NOT NULL
    UNION ALL
    -- @edge chunks.document_id -> documents.id
    -- 段落自己的文档归属：复合外键护写入，存量坏行在这里拦
    SELECT 'chunk.document', 'cross_kb', d.kb_id IS DISTINCT FROM c.kb_id
      FROM chunks c
      LEFT JOIN documents d ON d.id = c.document_id
     WHERE c.kb_id = $1
    UNION ALL
    -- @edge fact_derivations.premise_fact_id -> facts.id
    -- 派生前提：铸成 prov:used 的事实/派生 IRI
    SELECT 'derivation.premise_fact', 'cross_kb', p.kb_id IS DISTINCT FROM d.kb_id
      FROM fact_derivations fd
      JOIN derived_facts d ON d.id = fd.derived_fact_id
      LEFT JOIN facts p ON p.id = fd.premise_fact_id
     WHERE d.kb_id = $1 AND fd.premise_fact_id IS NOT NULL
    UNION ALL
    -- @edge fact_derivations.premise_derived_id -> derived_facts.id
    SELECT 'derivation.premise_derived', 'cross_kb', p.kb_id IS DISTINCT FROM d.kb_id
      FROM fact_derivations fd
      JOIN derived_facts d ON d.id = fd.derived_fact_id
      LEFT JOIN derived_facts p ON p.id = fd.premise_derived_id
     WHERE d.kb_id = $1 AND fd.premise_derived_id IS NOT NULL
    UNION ALL
    -- @edge fact_qualifiers.qualifier_type_id -> relation_types.id
    -- 边上的属性：类型进词汇表按 id 查（查不着静默丢），实体值铸 IRI
    SELECT 'qualifier.type', 'cross_kb', r.kb_id IS DISTINCT FROM f.kb_id
      FROM fact_qualifiers q
      JOIN facts f ON f.id = q.fact_id
      LEFT JOIN relation_types r ON r.id = q.qualifier_type_id
     WHERE f.kb_id = $1
    UNION ALL
    -- @edge fact_qualifiers.entity_id -> entities.id
    SELECT 'qualifier.entity', 'cross_kb', e.kb_id IS DISTINCT FROM f.kb_id
      FROM fact_qualifiers q
      JOIN facts f ON f.id = q.fact_id
      LEFT JOIN entities e ON e.id = q.entity_id
     WHERE f.kb_id = $1 AND q.entity_id IS NOT NULL
    UNION ALL
    -- @filter qualifier.entity(merged)
    SELECT 'qualifier.entity(merged)', 'unexported', TRUE
      FROM fact_qualifiers q
      JOIN facts f ON f.id = q.fact_id
      JOIN entities e ON e.id = q.entity_id AND e.merged_into IS NOT NULL
     WHERE f.kb_id = $1
    UNION ALL
    -- @edge facts.subject_id -> entities.id
    -- 事实本体：主语铸 entity IRI，谓词进词汇表，supersedes 铸 fact IRI
    SELECT 'fact.subject', 'cross_kb', s.kb_id IS DISTINCT FROM f.kb_id
      FROM facts f LEFT JOIN entities s ON s.id = f.subject_id
     WHERE f.kb_id = $1
    UNION ALL
    -- @filter fact.subject(merged)
    SELECT 'fact.subject(merged)', 'unexported', TRUE
      FROM facts f JOIN entities s ON s.id = f.subject_id AND s.merged_into IS NOT NULL
     WHERE f.kb_id = $1
    UNION ALL
    -- @edge facts.object_id -> entities.id
    SELECT 'fact.object', 'cross_kb', o.kb_id IS DISTINCT FROM f.kb_id
      FROM facts f LEFT JOIN entities o ON o.id = f.object_id
     WHERE f.kb_id = $1 AND f.object_id IS NOT NULL
    UNION ALL
    -- @filter fact.object(merged)
    SELECT 'fact.object(merged)', 'unexported', TRUE
      FROM facts f JOIN entities o ON o.id = f.object_id AND o.merged_into IS NOT NULL
     WHERE f.kb_id = $1
    UNION ALL
    -- @edge facts.predicate_id -> relation_types.id
    SELECT 'fact.predicate', 'cross_kb', r.kb_id IS DISTINCT FROM f.kb_id
      FROM facts f LEFT JOIN relation_types r ON r.id = f.predicate_id
     WHERE f.kb_id = $1 AND f.predicate_id IS NOT NULL
    UNION ALL
    -- @edge facts.supersedes -> facts.id
    SELECT 'fact.supersedes', 'cross_kb', s.kb_id IS DISTINCT FROM f.kb_id
      FROM facts f LEFT JOIN facts s ON s.id = f.supersedes
     WHERE f.kb_id = $1 AND f.supersedes IS NOT NULL
    UNION ALL
    -- @edge facts.from_statement_id -> facts.id
    -- 陈述来源是同表自指：与 supersedes 同一条判定，别库陈述不许当被引本体
    SELECT 'fact.from_statement', 'cross_kb', s.kb_id IS DISTINCT FROM f.kb_id
      FROM facts f LEFT JOIN facts s ON s.id = f.from_statement_id
     WHERE f.kb_id = $1 AND f.from_statement_id IS NOT NULL
    UNION ALL
    -- @edge derived_facts.subject_id -> entities.id
    -- 派生本体：规则 id 铸成 wasGeneratedBy 的 Activity IRI
    SELECT 'derived.subject', 'cross_kb', s.kb_id IS DISTINCT FROM d.kb_id
      FROM derived_facts d LEFT JOIN entities s ON s.id = d.subject_id
     WHERE d.kb_id = $1
    UNION ALL
    -- @filter derived.subject(merged)
    SELECT 'derived.subject(merged)', 'unexported', TRUE
      FROM derived_facts d JOIN entities s ON s.id = d.subject_id AND s.merged_into IS NOT NULL
     WHERE d.kb_id = $1
    UNION ALL
    -- @edge derived_facts.object_id -> entities.id
    SELECT 'derived.object', 'cross_kb', o.kb_id IS DISTINCT FROM d.kb_id
      FROM derived_facts d LEFT JOIN entities o ON o.id = d.object_id
     WHERE d.kb_id = $1 AND d.object_id IS NOT NULL
    UNION ALL
    -- @filter derived.object(merged)
    SELECT 'derived.object(merged)', 'unexported', TRUE
      FROM derived_facts d JOIN entities o ON o.id = d.object_id AND o.merged_into IS NOT NULL
     WHERE d.kb_id = $1
    UNION ALL
    -- @edge derived_facts.predicate_id -> relation_types.id
    SELECT 'derived.predicate', 'cross_kb', r.kb_id IS DISTINCT FROM d.kb_id
      FROM derived_facts d LEFT JOIN relation_types r ON r.id = d.predicate_id
     WHERE d.kb_id = $1
    UNION ALL
    -- @edge derived_facts.rule_id -> rules.id
    SELECT 'derived.rule', 'cross_kb', r.kb_id IS DISTINCT FROM d.kb_id
      FROM derived_facts d LEFT JOIN rules r ON r.id = d.rule_id
     WHERE d.kb_id = $1 AND d.rule_id IS NOT NULL
    UNION ALL
    -- @edge derived_facts.attribute_rule_id -> attribute_rules.id
    SELECT 'derived.attribute_rule', 'cross_kb', r.kb_id IS DISTINCT FROM d.kb_id
      FROM derived_facts d LEFT JOIN attribute_rules r ON r.id = d.attribute_rule_id
     WHERE d.kb_id = $1 AND d.attribute_rule_id IS NOT NULL
    UNION ALL
    -- @edge entities.type_id -> entity_types.id
    -- 实体的类进词汇表按 id 查
    SELECT 'entity.type', 'cross_kb', t.kb_id IS DISTINCT FROM e.kb_id
      FROM entities e LEFT JOIN entity_types t ON t.id = e.type_id
     WHERE e.kb_id = $1 AND e.type_id IS NOT NULL
    UNION ALL
    -- @edge entity_type_parents.parent_id -> entity_types.id
    -- 类层级与互斥都进词汇表按 id 查
    SELECT 'class.parent', 'cross_kb', p.kb_id IS DISTINCT FROM c.kb_id
      FROM entity_type_parents x
      JOIN entity_types c ON c.id = x.child_id
      LEFT JOIN entity_types p ON p.id = x.parent_id
     WHERE c.kb_id = $1
    UNION ALL
    -- @edge entity_type_disjoint.a_id -> entity_types.id
    SELECT 'class.disjoint', 'cross_kb', a.kb_id IS DISTINCT FROM dd.kb_id
      FROM entity_type_disjoint dd
      LEFT JOIN entity_types a ON a.id = dd.a_id
     WHERE dd.kb_id = $1
    UNION ALL
    -- @edge entity_type_disjoint.b_id -> entity_types.id
    -- a_id 与 b_id 是两条结构边、共用一个报错 label
    SELECT 'class.disjoint', 'cross_kb', b.kb_id IS DISTINCT FROM dd.kb_id
      FROM entity_type_disjoint dd
      LEFT JOIN entity_types b ON b.id = dd.b_id
     WHERE dd.kb_id = $1
    UNION ALL
    -- @edge relation_type_domains.entity_type_id -> entity_types.id
    -- domain/range 进词汇表按 id 查；inverse/sub_property 铸关系 IRI
    SELECT 'relation.domain', 'cross_kb', t.kb_id IS DISTINCT FROM r.kb_id
      FROM relation_type_domains x
      JOIN relation_types r ON r.id = x.relation_type_id
      LEFT JOIN entity_types t ON t.id = x.entity_type_id
     WHERE r.kb_id = $1
    UNION ALL
    -- @edge relation_type_ranges.entity_type_id -> entity_types.id
    SELECT 'relation.range', 'cross_kb', t.kb_id IS DISTINCT FROM r.kb_id
      FROM relation_type_ranges x
      JOIN relation_types r ON r.id = x.relation_type_id
      LEFT JOIN entity_types t ON t.id = x.entity_type_id
     WHERE r.kb_id = $1
    UNION ALL
    -- @edge relation_type_qualifiers.qualifier_type_id -> relation_types.id
    -- 关系声明的边属性：归属按所属 relation 的库判
    SELECT 'relation.qualifier', 'cross_kb', q.kb_id IS DISTINCT FROM r.kb_id
      FROM relation_type_qualifiers x
      JOIN relation_types r ON r.id = x.relation_type_id
      LEFT JOIN relation_types q ON q.id = x.qualifier_type_id
     WHERE r.kb_id = $1
    UNION ALL
    -- @edge relation_types.inverse_of -> relation_types.id
    SELECT 'relation.inverse', 'cross_kb', t.kb_id IS DISTINCT FROM r.kb_id
      FROM relation_types r LEFT JOIN relation_types t ON t.id = r.inverse_of
     WHERE r.kb_id = $1 AND r.inverse_of IS NOT NULL
    UNION ALL
    -- @edge relation_types.sub_property_of -> relation_types.id
    SELECT 'relation.sub_property', 'cross_kb', t.kb_id IS DISTINCT FROM r.kb_id
      FROM relation_types r LEFT JOIN relation_types t ON t.id = r.sub_property_of
     WHERE r.kb_id = $1 AND r.sub_property_of IS NOT NULL
    UNION ALL
    -- @edge rules.predicate_id -> relation_types.id
    -- 公理编在哪个谓词上是规则本体的语义
    SELECT 'rule.predicate', 'cross_kb', p.kb_id IS DISTINCT FROM u.kb_id
      FROM rules u
      LEFT JOIN relation_types p ON p.id = u.predicate_id
     WHERE u.kb_id = $1
    UNION ALL
    -- @edge attribute_rules.subject_type_id -> entity_types.id
    SELECT 'arule.subject_type', 'cross_kb', t.kb_id IS DISTINCT FROM a.kb_id
      FROM attribute_rules a
      LEFT JOIN entity_types t ON t.id = a.subject_type_id
     WHERE a.kb_id = $1
    UNION ALL
    -- @edge attribute_rules.conclude_type_id -> entity_types.id
    SELECT 'arule.conclude_type', 'cross_kb', t.kb_id IS DISTINCT FROM a.kb_id
      FROM attribute_rules a
      LEFT JOIN entity_types t ON t.id = a.conclude_type_id
     WHERE a.kb_id = $1 AND a.conclude_type_id IS NOT NULL
    UNION ALL
    -- @edge attribute_rules.conclude_predicate_id -> relation_types.id
    SELECT 'arule.conclude_predicate', 'cross_kb', p.kb_id IS DISTINCT FROM a.kb_id
      FROM attribute_rules a
      LEFT JOIN relation_types p ON p.id = a.conclude_predicate_id
     WHERE a.kb_id = $1 AND a.conclude_predicate_id IS NOT NULL
    UNION ALL
    -- @edge attribute_rule_conditions.predicate_id -> relation_types.id
    -- 条件行自己没有 kb 列：归属按所属规则的库判
    SELECT 'condition.predicate', 'cross_kb', p.kb_id IS DISTINCT FROM a.kb_id
      FROM attribute_rule_conditions c
      JOIN attribute_rules a ON a.id = c.rule_id
      LEFT JOIN relation_types p ON p.id = c.predicate_id
     WHERE a.kb_id = $1
    UNION ALL
    -- @edge typed_fact_sources.statement_id -> facts.id
    -- 来源边行自己没有 kb 列：归属按所属 fact 的库判
    SELECT 'factsource.statement', 'cross_kb', s.kb_id IS DISTINCT FROM f.kb_id
      FROM typed_fact_sources ts
      JOIN facts f ON f.id = ts.fact_id
      LEFT JOIN facts s ON s.id = ts.statement_id
     WHERE f.kb_id = $1
    UNION ALL
    -- @edge statement_qualifiers.entity_id -> entities.id
    -- 开放陈述的属性行自己没有 kb 列：归属按所属 fact 的库判
    SELECT 'squalifier.entity', 'cross_kb', e.kb_id IS DISTINCT FROM f.kb_id
      FROM statement_qualifiers q
      JOIN facts f ON f.id = q.fact_id
      LEFT JOIN entities e ON e.id = q.entity_id
     WHERE f.kb_id = $1 AND q.entity_id IS NOT NULL
    UNION ALL
    -- @edge time_mentions.fact_id -> facts.id
    -- 时间提及以行自己的 kb 归属：指的事实与段落都必须同库
    SELECT 'timemention.fact', 'cross_kb', f.kb_id IS DISTINCT FROM t.kb_id
      FROM time_mentions t
      LEFT JOIN facts f ON f.id = t.fact_id
     WHERE t.kb_id = $1
    UNION ALL
    -- @edge time_mentions.chunk_id -> chunks.id
    SELECT 'timemention.chunk', 'cross_kb', c.kb_id IS DISTINCT FROM t.kb_id
      FROM time_mentions t
      LEFT JOIN chunks c ON c.id = t.chunk_id
     WHERE t.kb_id = $1
    UNION ALL
    -- @edge type_bindings.type_id -> entity_types.id
    SELECT 'binding.type', 'cross_kb', t.kb_id IS DISTINCT FROM b.kb_id
      FROM type_bindings b
      LEFT JOIN entity_types t ON t.id = b.type_id
     WHERE b.kb_id = $1 AND b.type_id IS NOT NULL
    UNION ALL
    -- @edge phrase_bindings.subject_type_id -> entity_types.id
    SELECT 'pbinding.subject_type', 'cross_kb', t.kb_id IS DISTINCT FROM b.kb_id
      FROM phrase_bindings b
      LEFT JOIN entity_types t ON t.id = b.subject_type_id
     WHERE b.kb_id = $1 AND b.subject_type_id IS NOT NULL
    UNION ALL
    -- @edge phrase_bindings.object_type_id -> entity_types.id
    SELECT 'pbinding.object_type', 'cross_kb', t.kb_id IS DISTINCT FROM b.kb_id
      FROM phrase_bindings b
      LEFT JOIN entity_types t ON t.id = b.object_type_id
     WHERE b.kb_id = $1 AND b.object_type_id IS NOT NULL
    UNION ALL
    -- @edge phrase_bindings.relation_type_id -> relation_types.id
    SELECT 'pbinding.relation', 'cross_kb', r.kb_id IS DISTINCT FROM b.kb_id
      FROM phrase_bindings b
      LEFT JOIN relation_types r ON r.id = b.relation_type_id
     WHERE b.kb_id = $1 AND b.relation_type_id IS NOT NULL
) refs WHERE bad GROUP BY edge, kind
