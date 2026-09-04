-- MySQL 线协议进注册表（#303）。
-- 允许的名字与 `query_engine::ENGINES` 同一张表；挂载模型与引擎无关，
-- 加一个引擎仍然只放宽这一条 CHECK（同 0021）。
ALTER TABLE data_sources DROP CONSTRAINT data_sources_engine_check;
ALTER TABLE data_sources ADD CONSTRAINT data_sources_engine_check
    CHECK (engine IN ('postgres', 'mysql', 'trino', 'databricks', 'snowflake'));
