-- 告警中心（0005）。失败状态此前散在六处：jobs.last_error、documents.status、
-- documents.graph_status、sources.last_sync_status、source_sync_runs.status、日志。
-- 每来一类新的失败就往对应的表上加一列——推演层、执行层、OCR、湖仓连接都还没进来。
--
-- 真正伤人的不是失败，是**失败无声**：拖 100 份 PDF、12 份是扫描件，
-- 界面上 100 份全绿。

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
    subject_type TEXT,
    -- 聚合：同一 kind 下的所有对象。12 份扫描件是**一条**告警，不是 12 条
    subject_ids  UUID[] NOT NULL DEFAULT '{}',
    detail       JSONB NOT NULL DEFAULT '{}',
    first_seen   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 自愈优先于人工关闭：由产生方清空。做不到自愈的告警，
    -- 用户很快学会无视它，而这个习惯是不可逆的
    resolved_at  TIMESTAMPTZ
);

-- 聚合的实现关键：同类未解决的告警在库里只能有一条。
--
-- **COALESCE 不是可有可无的**：NULL 不参与唯一性判定，直接对 (kb_id, kind)
-- 建部分唯一索引的话，系统级告警（kb_id IS NULL）每次上报都会插一条新行——
-- 聚合对最需要它的那一类静默失效。0005 把这个坑记了下来，这里是那个坑的堵法
CREATE UNIQUE INDEX alerts_open_kind_idx
    ON alerts (COALESCE(kb_id, '00000000-0000-0000-0000-000000000000'::uuid), kind)
    WHERE resolved_at IS NULL;

-- 列表按未解决优先、最近在前
CREATE INDEX alerts_open_idx ON alerts (last_seen DESC) WHERE resolved_at IS NULL;
CREATE INDEX alerts_kb_idx ON alerts (kb_id);

-- 已读是各人的，解决是共享的。
--
-- 被否决的方案是共享已读（任何管理员点开就对所有人标记已读）。失效方式：
-- 三个管理员，A 早上顺手点开看了一眼没处理，这条从 B、C 的未读里**永远消失**——
-- 他们不知道发生过这件事，而 A 想着等会儿再说。所有人都以为别人在处理，
-- 且事后没有任何痕迹能发现漏了。
--
-- 根子是把两件事合并了：「已读」是我看没看过，「已解决」是事情完没完。
-- 代价只有这张两列小表，换来的是没有人能替别人把一件事读掉
CREATE TABLE alert_reads (
    alert_id UUID NOT NULL REFERENCES alerts(id) ON DELETE CASCADE,
    user_id  UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    read_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (alert_id, user_id)
);
