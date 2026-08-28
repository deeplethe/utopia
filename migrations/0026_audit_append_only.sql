-- 台账只增不改。应用本就只 INSERT，这道触发器挡的是绕过应用的那条路：
-- 运维直连数据库、一次手滑的 UPDATE、或者有人回头想抹掉自己的痕迹。
--
-- 它挡不住 superuser——那个身份可以 DROP TRIGGER 或 ALTER TABLE ... DISABLE
-- TRIGGER 之后从容修改。所以这一层的作用是把门槛从"顺手就能改"抬到"必须先
-- 动 DDL"，而 DDL 本身会留在数据库日志里。要让蓄意篡改也无所遁形，得靠后续
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
