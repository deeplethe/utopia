-- 审计日志：谁在何时对什么做了什么（纯审计，不承载回滚等衍生功能）。
--
-- **kb_id 与 actor_id 是裸 UUID，没有外键。** 台账不该依赖它所记录的对象存活：
-- kb_id 若级联删除，删掉一个知识库就删掉了它的全部审计记录——包括刚刚写下的
-- 那条 kb.deleted，删除是最需要留痕的动作，却成了唯一不留痕的。actor_id 若
-- SET NULL，一个用户被停用后他做过的每一次确认、驳回、合并都变成匿名。
-- 合规审计要的恰恰是这两个场景。
--
-- 这也是哈希链的前提：链要求记录只增不删，级联删除会在链中间挖掉一段，
-- 让它断得「合法」而无从分辨。
CREATE TABLE audit_events (
    id          UUID PRIMARY KEY,
    kb_id       UUID,
    actor_id    UUID,
    action      TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id   UUID,
    -- 变更要点，按 action 语义自定
    detail      JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 从哪里做的。ISO 27001 A.8.15 要求日志覆盖 Where / How，即请求的来源；
    -- 没有它，一个账号被盗用后的所有活动看起来与本人操作毫无差别。
    -- 两列都可空：后台任务（攒批裁决、定时同步）没有客户端，本就该是空
    client_ip   TEXT,
    user_agent  TEXT,
    -- 操作者的身份快照。actor_id 没有外键，用户停用时行会留下，但 LEFT JOIN
    -- users 取不到名字，界面只剩一串 UUID。把当时的邮箱与显示名一并写进记录，
    -- 台账才真正自包含
    actor_label TEXT
);
CREATE INDEX audit_events_kb_time_idx ON audit_events (kb_id, created_at DESC);

-- 按 IP 排查（同一来源的登录失败、异常时段的活动）需要这条索引
CREATE INDEX audit_events_client_ip_idx ON audit_events (client_ip, created_at DESC)
    WHERE client_ip IS NOT NULL;

-- 台账只增不改。应用本就只 INSERT，这道触发器挡的是绕过应用的那条路：
-- 运维直连数据库、一次手滑的 UPDATE、或者有人回头想抹掉自己的痕迹。
--
-- 它挡不住 superuser——那个身份可以 DROP TRIGGER 或 ALTER TABLE ... DISABLE
-- TRIGGER 之后从容修改。所以这一层的作用是把门槛从「顺手就能改」抬到「必须先
-- 动 DDL」，而 DDL 本身会留在数据库日志里。要让蓄意篡改也无所遁形，得靠后续
-- 的哈希链：改动会让链在被改的那一条上断开。
--
-- 刻意不留应用层的绕过开关。审计不可变性一旦有开关，就等于没有。将来若要做
-- 保留期清理，那是特权运维动作，应显式 DROP TRIGGER、清理、再重建，全过程
-- 留在 DDL 记录里。
CREATE FUNCTION audit_events_immutable() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'audit_events is append-only (attempted %)', TG_OP
        USING HINT = 'Audit records cannot be modified or deleted.';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER audit_events_no_update_delete
    BEFORE UPDATE OR DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION audit_events_immutable();

-- TRUNCATE 不触发行级触发器，单独挡一道，否则一句 TRUNCATE 就绕过了上面全部。
CREATE TRIGGER audit_events_no_truncate
    BEFORE TRUNCATE ON audit_events
    FOR EACH STATEMENT EXECUTE FUNCTION audit_events_immutable();
