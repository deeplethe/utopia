-- 台账不该依赖它所记录的对象存活。
--
-- kb_id 原为 ON DELETE CASCADE：删掉一个知识库，它的全部审计记录随之消失——
-- 包括刚刚写下的那条 kb.deleted。删除是最需要留痕的动作，却成了唯一不留痕的。
-- actor_id 原为 ON DELETE SET NULL：删掉一个用户，他做过的每一次确认、驳回、
-- 合并都变成匿名，再也追不回是谁做的。合规审计要的恰恰是这两个场景。
--
-- 去掉两个外键，只保留 UUID 值：对象没了，记录还在，且仍指得出那个已不存在的
-- 对象。这也是后续追加哈希链的前提——链要求记录只增不删，级联删除会在链中间
-- 挖掉一段，让它断得"合法"而无从分辨。
--
-- 应用从不依赖这两处级联（只 INSERT，删 KB 走 DELETE FROM knowledge_bases，
-- 级联是数据库自己做的），(kb_id, created_at DESC) 索引保持不变，查询不受影响。
ALTER TABLE audit_events DROP CONSTRAINT audit_events_kb_id_fkey;
ALTER TABLE audit_events DROP CONSTRAINT audit_events_actor_id_fkey;
