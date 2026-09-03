-- 问数引擎扩到 HTTP 协议族：trino（Iceberg / Delta / Hive 都是它的 catalog）、
-- databricks（SQL Statement API）、snowflake（SQL API v2）。
-- 挂载模型与注册表引擎无关（0006 的判断仍成立），这里只放宽 engine 的取值；
-- 允许的名字与 `query_engine::ENGINES` 同一张表。
ALTER TABLE data_sources DROP CONSTRAINT data_sources_engine_check;
ALTER TABLE data_sources ADD CONSTRAINT data_sources_engine_check
    CHECK (engine IN ('postgres', 'trino', 'databricks', 'snowflake'));
