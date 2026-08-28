-- 并发限制按模型算，不按部署算。
--
-- 真正的约束是模型供应商的速率限制，那是按模型（连同 base_url）来的：本地
-- Ollama 可能只扛 2 个并发，托管 API 能吃 50——一个全局数字管两者本来就不对。
--
-- 限流放在 LLM 调用处而不是任务调度处：不调模型的任务（文件夹同步）不该受它
-- 约束，调不同模型的任务（抽取用 chat、摄入用 embedding）之间也不该互相挤。
CREATE TABLE IF NOT EXISTS model_concurrency (
    base_url        TEXT NOT NULL,
    model           TEXT NOT NULL,
    max_concurrent  INT  NOT NULL CHECK (max_concurrent BETWEEN 1 AND 256),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (base_url, model)
);

-- 没配过的模型走这个缺省
-- 幂等：改号前这条已在部分库跑过（0026 撞号），必须能安全重放
ALTER TABLE deployment_settings
    ADD COLUMN IF NOT EXISTS default_model_concurrency INT NOT NULL DEFAULT 10;

-- worker 并发降级成外层兜底：真正的节流交给按模型的信号量，这里只防任务
-- 无限堆积。要明显大于各模型限额之和，否则被限流的任务会占满槽位把别的饿死。
-- 幂等：改号前这条已在部分库跑过（0026 撞号），必须能安全重放
ALTER TABLE deployment_settings
    ALTER COLUMN worker_concurrency SET DEFAULT 32;
UPDATE deployment_settings SET worker_concurrency = 32 WHERE worker_concurrency = 4;
