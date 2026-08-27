-- 0010: 来源定时同步支持 cron 表达式（标准 5 段，与 sync_interval_minutes 互斥；
-- UI 提供可视化选择器构建，Advanced 模式才暴露原生表达式）
ALTER TABLE sources ADD COLUMN sync_cron TEXT;
