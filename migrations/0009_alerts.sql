-- 告警中心（0005）。失败状态此前散在六处：jobs.last_error、documents.status、
-- documents.graph_status、sources.last_sync_status、source_sync_runs.status、日志。
-- 每来一类新的失败就往对应的表上加一列——推演层、执行层、OCR、湖仓连接都还没进来。
--
-- 真正伤人的不是失败，是**失败无声**：拖 100 份 PDF、12 份是扫描件，
-- 界面上 100 份全绿。

-- **一次故障一条，写完就不再变。**
--
-- 这张表刻意没有状态机：没有"已解决"，没有自愈，没有把多次故障并成一行。
-- 曾经有过，代价是每种新告警都得自己实现一遍"怎么算修好了"——
-- source.sync_failed 有天然的成功信号，llm.unreachable 没有，就得为它单独造
-- 一个后台探针；第三种告警要造第三套，而漏写清除编译期看不出来。
--
-- 更根本的是**那不是告警中心该回答的问题**：现在还坏不坏，来源页面上写着，
-- 文档状态里写着。告警的职责是让人去看一眼，不是当实时看板。
CREATE TABLE alerts (
    id           UUID PRIMARY KEY,
    -- NULL = 系统级（端点不可达、连接池打满），仅 is_admin 可见
    kb_id        UUID REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    severity     TEXT NOT NULL CHECK (severity IN ('info', 'warning', 'error')),
    -- 'source.sync_failed' / 'llm.unreachable' / ...
    kind         TEXT NOT NULL,
    -- 存在行上而不是按 kind 硬编码：同一类告警在不同场景下该找的人不同。
    -- 配置类（端点、配额）找 admin；内容类（解析、抽取、同步）要给到 editor——
    -- 传那 12 份扫描件的人比管理员更需要知道"你传的东西没进去"
    min_role     TEXT NOT NULL CHECK (min_role IN ('viewer', 'editor', 'admin', 'owner')),
    -- 出问题的那个对象：document / source / system。系统级的两列都为空
    subject_type TEXT,
    subject_id   UUID,
    -- 给人看的那份：名字、报错原文
    detail       JSONB NOT NULL DEFAULT '{}',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 列表按时间倒序，这是唯一的排序方式
CREATE INDEX alerts_recent_idx ON alerts (created_at DESC);
CREATE INDEX alerts_kb_idx ON alerts (kb_id);

-- 已读是各人的。
--
-- 被否决的方案是共享已读（任何管理员点开就对所有人标记已读）。失效方式：
-- 三个管理员，A 早上顺手点开看了一眼没处理，这条从 B、C 的未读里**永远消失**——
-- 他们不知道发生过这件事，而 A 想着等会儿再说。所有人都以为别人在处理，
-- 且事后没有任何痕迹能发现漏了。
--
-- 告警行写完不再变，所以已读也是一次性的：读过就是读过。
CREATE TABLE alert_reads (
    alert_id UUID NOT NULL REFERENCES alerts(id) ON DELETE CASCADE,
    user_id  UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    read_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (alert_id, user_id)
);
