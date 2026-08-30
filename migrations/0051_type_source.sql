-- 人定过的类型，引擎不许改。
--
-- `entity_retypes` 记得住「谁改的」（0048 加的 `actor_id`），但那是**事后**的账。
-- 保护要**事前**：实体身上得有一位说明这个类型是怎么来的，否则每一条读路径都
-- 只能看见 `type_id`，看不见它背后有没有人拍过板。
--
-- 今天的漏在类型消解那条路上。`entities_for_type_resolution` 的取材条件是：
--
--     AND (e.type_id IS NULL OR e.proposed_type IS NOT NULL
--          OR e.specific_type IS NOT NULL
--          OR EXISTS (SELECT 1 FROM entity_type_parents p WHERE p.parent_id = t.id))
--
-- 最后那行意味着：**人工定成 `organization` 的实体，只要 `organization` 有子类，
-- 下一轮消解照样把它拿去重判。** 抽取那条路有守卫（`resolve_type_drift` 只在
-- `type_key.is_none()` 时升格），消解这条没有。
--
-- 0009 之后还多了一种情形：「没有类型」现在可能是**人的决定**——他看过这个实体，
-- 认为本体里没有合适的类。而 `type_id IS NULL` 分不出「还没判」和「人判了，
-- 就是没有」，于是下一次抽取会给它安一个类型。这一条 0001 写不出来，因为 0009
-- 比它晚三天。
--
-- 三个取值的分工：
--
--   extracted  抽取判出来的（`resolve_type_drift` 的升格）
--   inferred   引擎裁决的（类型消解、本体长出新类后的认领）
--   human      人拍板的（实体面板直改、审核队列里点批准）
--
-- `inferred` 与 `extracted` 分开而不是合成一个「非人」：它们的可信度不同，
-- 将来若要「引擎可以改引擎的，但不能改抽取的」，判据现成。

ALTER TABLE entities ADD COLUMN type_source TEXT NOT NULL DEFAULT 'extracted'
  CHECK (type_source IN ('extracted', 'human', 'inferred'));

-- 回填**不靠猜**：审计台账里的 entity.retyped 带 actor_id 与 target_id，
-- 那是人在实体面板上改类型时留下的唯一痕迹（那条路至今不写 entity_retypes）。
--
-- 更早的引擎改类无从追溯——`entity_retypes.actor_id` 是 0048 才有的，之前的行
-- 分不出人机。所以默认值取 `extracted`：**承认不知道，而不是断言没人碰过**。
UPDATE entities e SET type_source = 'human'
 WHERE EXISTS (
     SELECT 1 FROM audit_events a
      WHERE a.action = 'entity.retyped' AND a.target_id = e.id
 );
