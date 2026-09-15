-- 抽取质量审计（诊断用，启发式）：psql -v kb=<uuid>
\set QUIET on
\pset footer off
\pset border 0
CREATE TEMP VIEW ent AS
  SELECT e.id, e.canonical_name AS name, t.key AS type
    FROM entities e LEFT JOIN entity_types t ON t.id = e.type_id
   WHERE e.kb_id = :'kb' AND e.merged_into IS NULL;
CREATE TEMP VIEW fx AS
  SELECT f.id, f.subject_id, f.object_id, s.canonical_name AS subj, o.canonical_name AS obj,
         f.object_value #>> '{value}' AS val, f.object_value #>> '{}' AS raw_val,
         coalesce(rt.key, (SELECT fe.proposed_predicate FROM fact_evidence fe WHERE fe.fact_id = f.id LIMIT 1)) AS pred,
         rt.id IS NOT NULL AS bound, rt.key = 'known_as' AS is_name,
         f.valid_from, f.valid_to, f.confidence,
         (SELECT fe.quote FROM fact_evidence fe WHERE fe.fact_id = f.id AND fe.quote IS NOT NULL LIMIT 1) AS quote
    FROM facts f
    JOIN entities s ON s.id = f.subject_id
    LEFT JOIN entities o ON o.id = f.object_id
    LEFT JOIN relation_types rt ON rt.id = f.predicate_id
   WHERE f.kb_id = :'kb' AND f.invalidated_at IS NULL;

\echo '== 总量'
SELECT (SELECT count(*) FROM ent) AS entities,
       (SELECT count(*) FROM fx WHERE NOT coalesce(is_name, false)) AS facts,
       (SELECT count(*) FROM fx WHERE NOT coalesce(is_name, false) AND NOT bound) AS unbound_facts,
       (SELECT count(DISTINCT pred) FROM fx WHERE NOT coalesce(is_name, false)) AS distinct_predicates;

\echo '== A 名字像描述：小写开头 / 含数字+小写词 / 超过 7 个词 / 带 this·that·such·dated·as amended'
SELECT name, type FROM ent
 WHERE name ~ '^[a-z]' OR name ~ '[0-9].* [a-z]{3,}' OR array_length(regexp_split_to_array(name, '\s+'), 1) > 7
    OR name ~* '^(this|that|such|said|the said)\s' OR name ~* '\s(dated|as amended)\b'
 ORDER BY name LIMIT 25;
SELECT count(*) AS a_count FROM ent
 WHERE name ~ '^[a-z]' OR name ~ '[0-9].* [a-z]{3,}' OR array_length(regexp_split_to_array(name, '\s+'), 1) > 7
    OR name ~* '^(this|that|such|said|the said)\s' OR name ~* '\s(dated|as amended)\b';

\echo '== B 别名比本名长且包含本名（可能是下属/描述，不是别名）'
SELECT e.name, f.val AS alias FROM fx f JOIN ent e ON e.id = f.subject_id
 WHERE f.is_name AND length(f.val) > length(e.name) AND position(lower(e.name) IN lower(f.val)) > 0
 LIMIT 15;

\echo '== C 大小写/空白归一后同名的多个实体'
SELECT lower(regexp_replace(name, '\s+', ' ', 'g')) AS norm, count(*), string_agg(DISTINCT coalesce(type,'-'), ',') AS types
  FROM ent GROUP BY 1 HAVING count(*) > 1 ORDER BY 2 DESC LIMIT 15;

\echo '== D 一个名字是另一个名字 + 尾巴（dated/of/the/逗号…）且同类型'
SELECT a.name AS shorter, b.name AS longer FROM ent a JOIN ent b ON a.id <> b.id AND coalesce(a.type,'') = coalesce(b.type,'')
 WHERE length(b.name) > length(a.name) + 3 AND lower(b.name) LIKE lower(a.name) || ' %'
 LIMIT 20;

\echo '== E 值事实：值（去掉空白与逗号）不在引文里'
SELECT subj, pred, left(val, 40) AS val, left(quote, 90) AS quote FROM fx
 WHERE NOT coalesce(is_name, false) AND val IS NOT NULL AND quote IS NOT NULL
   AND position(lower(regexp_replace(val, '[\s,]', '', 'g')) IN lower(regexp_replace(quote, '[\s,]', '', 'g'))) = 0
   AND val !~ '^\d{4}-\d{2}(-\d{2})?$'
 LIMIT 15;
SELECT count(*) AS e_count FROM fx
 WHERE NOT coalesce(is_name, false) AND val IS NOT NULL AND quote IS NOT NULL
   AND position(lower(regexp_replace(val, '[\s,]', '', 'g')) IN lower(regexp_replace(quote, '[\s,]', '', 'g'))) = 0
   AND val !~ '^\d{4}-\d{2}(-\d{2})?$';

\echo '== F 关系事实：宾语名字的首词不在引文里（可能挂错了宾语）'
SELECT subj, pred, obj, left(quote, 90) AS quote FROM fx
 WHERE object_id IS NOT NULL AND quote IS NOT NULL
   AND position(lower(split_part(obj, ' ', 1)) IN lower(quote)) = 0
 LIMIT 15;
SELECT count(*) AS f_count, (SELECT count(*) FROM fx WHERE object_id IS NOT NULL) AS relation_facts FROM fx
 WHERE object_id IS NOT NULL AND quote IS NOT NULL
   AND position(lower(split_part(obj, ' ', 1)) IN lower(quote)) = 0;

\echo '== G 关系事实：主语名字的首词也不在引文里'
SELECT count(*) AS g_count FROM fx
 WHERE NOT coalesce(is_name, false) AND quote IS NOT NULL
   AND position(lower(split_part(subj, ' ', 1)) IN lower(quote)) = 0;

\echo '== H 没有起点、但引文里写着年份的状态事实'
SELECT subj, pred, coalesce(obj, left(val, 30)) AS what, left(quote, 90) AS quote FROM fx
 WHERE NOT coalesce(is_name, false) AND valid_from IS NULL AND quote ~ '(19|20)\d{2}'
 LIMIT 10;
SELECT count(*) AS h_count FROM fx WHERE NOT coalesce(is_name, false) AND valid_from IS NULL AND quote ~ '(19|20)\d{2}';

\echo '== I 最常见的未绑定谓词（说法泛滥/同义词）'
SELECT pred, count(*) FROM fx WHERE NOT coalesce(is_name, false) AND NOT bound GROUP BY 1 ORDER BY 2 DESC LIMIT 15;

\echo '== J 孤立实体（除名字外没有任何事实）'
SELECT count(*) AS orphan_entities FROM ent e
 WHERE NOT EXISTS (SELECT 1 FROM fx f WHERE (f.subject_id = e.id OR f.object_id = e.id) AND NOT coalesce(f.is_name, false));
SELECT name, type FROM ent e
 WHERE NOT EXISTS (SELECT 1 FROM fx f WHERE (f.subject_id = e.id OR f.object_id = e.id) AND NOT coalesce(f.is_name, false))
 LIMIT 12;
