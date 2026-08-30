-- 管理员可以停用一个账号。**软删除,不是 DELETE。**
--
-- 硬删删不掉：审计事件、合并日志、改类账本、口径确认——它们的 `actor_id`
-- 指着这个人,而那些记录是审计材料,人走了仍然要能回答「当时是谁做的」。
-- 真去 DELETE 只有两个结果：要么被外键挡住（那些列是裸外键），要么级联把
-- 审计一起删掉。两个都不是我们要的。
--
-- 所以停用 = 打一个时间戳。**归因保住,访问断掉**。
ALTER TABLE users ADD COLUMN deactivated_at TIMESTAMPTZ;

-- 谁停的。同样是裸外键——停用者自己也可能被停用,而那条记录还得在
ALTER TABLE users ADD COLUMN deactivated_by UUID REFERENCES users(id);

-- **唯一的 email 约束要放停用的账号一马。**
--
-- 从前 email 是全表唯一。停用之后那个地址还占着位置,于是同一个人回来、
-- 或者同一个邮箱转给别人,都建不了新账号——而「停用」不该等于「这个邮箱
-- 永久报废」。改成部分唯一索引：只约束在职的。
--
-- 代价写在前面：停用过的账号里可以有重复 email，`find_user_by_email`
-- 因此必须带 `deactivated_at IS NULL`——它本来就要带（不然停用的人还能登录），
-- 这里只是让那一句同时也是正确性的保证，而不只是权限的。
ALTER TABLE users DROP CONSTRAINT IF EXISTS users_email_key;
CREATE UNIQUE INDEX users_email_active_idx ON users (email) WHERE deactivated_at IS NULL;
