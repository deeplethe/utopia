//! utopia-store: sqlx 仓储、迁移、任务队列。
//! 全部使用运行时查询（非编译期宏），构建无需数据库。

pub mod access;
pub mod accounts;
pub mod alerts;
pub mod audit;
pub mod business_rules;
pub mod conversations;
pub mod datasources;
pub mod db;
pub mod documents;
pub mod export;
pub mod extraction_drops;
pub mod graph;
pub mod jobs;
pub mod kbs;
pub mod mappings;
pub mod members;
pub mod memory;
pub mod model_limits;
pub mod ontology;
pub mod palette;
pub mod pending;
pub mod reasoning;
pub mod record_axis;
pub mod resolution;
pub mod review;
pub mod review_summary;
pub mod sealing;
pub mod settings;
pub mod sources;
pub mod temporal;
pub mod test_db;
pub mod tokens;
pub mod workspaces;
