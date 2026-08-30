-- 外层兜底的上限恰好等于它自己的缺省，于是一格都调不上去。
--
-- `worker_concurrency` 的设计写在 0001 的注释里：**这是外层兜底而不是节流**，
-- 真正的节流交给按模型的信号量，兜底"要明显大于各模型限额之和，否则被限流的
-- 任务会占满槽位把别的饿死"。
--
-- 但约束是 `BETWEEN 1 AND 32`，而缺省也是 32——**约束堵死了它自己的设计**。
-- 来历是分两步走的：约束写于缺省还是 4 的时候（上限 32 是八倍余量），
-- 后来缺省从 4 提到 32，没人回头看那条约束。Rust 侧的校验倒是改成了 `1..=256`，
-- 于是两边对不上：从设置页填 33 到 256 之间任何一个值，Rust 放行、数据库拒绝，
-- 用户看到的是一条 CHECK 约束报错而不是"超出范围"。
--
-- 上限跟 Rust 对齐到 256。
--
-- **缺省提到 64**，因为那句"否则会把别的饿死"是实测发生过的：一次 219 篇文档的
-- 抽取里，32 个 `extract_document` 占满槽位，`embed_ontology` 排在 queued 进不来。
-- 32 对模型限额 10 只有 3.2 倍，抽取一来就把队列糊死。
--
-- 为什么是 64 而不是更大：卡在信号量上的任务几乎零成本（不持有连接——抽取路径
-- 上没有跨 await 的事务），但**它在够到信号量之前**每块还要做两件带数据库的事——
-- `extract_epoch` 查一次、大本体时还要对全部关系与类做一次向量检索。这两件不受
-- 模型信号量约束。所以并发数直接决定同时砸向数据库的检索数，而池子是 32。
-- 64 是 2 倍超发，还在"变慢"而不是"超时"那一侧；再往上就该先把许可前移到
-- 每块工作的最前面，让槽位真的接近零成本，那是另一个改动。
--
-- 只改没被人动过的那些（值还等于旧缺省 32），谁手工调过就尊重他的选择——
-- 与当初 4 → 32 那次同一个口径。

ALTER TABLE deployment_settings
    DROP CONSTRAINT IF EXISTS deployment_settings_worker_concurrency_check;

ALTER TABLE deployment_settings
    ADD CONSTRAINT deployment_settings_worker_concurrency_check
    CHECK (worker_concurrency BETWEEN 1 AND 256);

ALTER TABLE deployment_settings
    ALTER COLUMN worker_concurrency SET DEFAULT 64;

UPDATE deployment_settings SET worker_concurrency = 64 WHERE worker_concurrency = 32;
