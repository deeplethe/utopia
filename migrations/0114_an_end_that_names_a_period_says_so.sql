-- 终点说的是它之前那个时段时，行上记一笔（#994，0053 的修订）。
--
-- 「works for Meridian from 2024-03-01」再加「left in 2024」，#975 之后存成 2024-03-01 →
-- 2025-01-01（年精度）：世界轴上这是对的，2024 年全年在职，2025 年不在。可「until 2025」
-- 关上同一行，存的也是 2025-01-01（年精度）。只看值分不出原文说的是 2024 还是 2025，读者
-- 只能按存的时刻印，印出来是下一个时段。
--
-- `end_after_period`：终点是原文所说时段之后的那一刻。存的时刻不变，世界轴照旧读它，读者
-- 据此印原文说的时段。写入路径在 #975 挪终点的两处设它（收尾、精化）；一条开着的行被
-- 「until 2025」与「left in 2024」两句关上，哪一句先到都设；改写、搬主宾、勘误改出来的行
-- 带着它；时间线画的终点、不知哪天的终点从不带。约束要求有起点：终点只在起点落在那个时段
-- 里时才被挪到时段的尽头，没有起点的行第一刀不标。第二刀让时间解析自己写这种终点时，按
-- 名字换掉这条约束，去掉起点那一条（Rust 一侧同一句话在 `graph::period_mark_fits`）。
--
-- 约束先删再加，整份文件可以重放。
ALTER TABLE facts ADD COLUMN IF NOT EXISTS end_after_period BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE facts DROP CONSTRAINT IF EXISTS facts_end_after_period_needs_a_stated_end;
ALTER TABLE facts ADD CONSTRAINT facts_end_after_period_needs_a_stated_end
  CHECK (NOT end_after_period OR (valid_from IS NOT NULL AND valid_to IS NOT NULL AND NOT end_derived));
