-- ClickHouse 进注册表（README §Roadmap, "More sources: a ClickHouse driver"）。
-- 允许的名字与 `query_engine::ENGINES` 同一张表；挂载模型与引擎无关，
-- 加一个引擎仍然只放宽这一条 CHECK（同 0021 / 0025）。
-- 完整驱动分三刀落地，详见 `.roadmap-proposals/clickhouse-driver.md`：
--   1. 注册表 + URL scheme + 空壳（本迁移 + `clickhouse.rs` 的 cut 1）
--   2. 真实 HTTP 驱动（`system.columns` 读 schema，`FORMAT JSONEachRow` 读行）
--   3. PK/FK（#502 的 ClickHouse 部分）+ ON CLUSTER + 性能
ALTER TABLE data_sources DROP CONSTRAINT data_sources_engine_check;
ALTER TABLE data_sources ADD CONSTRAINT data_sources_engine_check
    CHECK (engine IN ('postgres', 'mysql', 'trino', 'databricks', 'snowflake', 'clickhouse'));
