-- 台账此前只记「谁做了什么」，不记「从哪里做的」。ISO 27001 A.8.15 要求日志
-- 覆盖 Where / How，即请求的来源；没有它，一个账号被盗用后的所有活动看起来
-- 与本人操作毫无差别。
--
-- 两列都可空：后台任务（攒批裁决、定时同步）没有客户端，本就该是空。
ALTER TABLE audit_events ADD COLUMN client_ip TEXT;
ALTER TABLE audit_events ADD COLUMN user_agent TEXT;

-- 操作者的身份快照。actor_id 在 0025 之后不再有外键，用户删除时行会留下，
-- 但 LEFT JOIN users 取不到名字，界面只剩一串 UUID。把当时的邮箱与显示名
-- 一并写进记录，台账才真正自包含。
ALTER TABLE audit_events ADD COLUMN actor_label TEXT;

-- 按 IP 排查（同一来源的登录失败、异常时段的活动）需要这条索引。
CREATE INDEX audit_events_client_ip_idx ON audit_events (client_ip, created_at DESC)
    WHERE client_ip IS NOT NULL;
