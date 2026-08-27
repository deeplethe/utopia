-- 0003: 单租户部署模型
-- 一次部署 = 一个组织；首个注册用户为系统管理员，后续用户加入同一组织。
-- organizations 表与 org_id 列保留为内部管道（永远单行），为将来可能的托管版留门。

ALTER TABLE users ADD COLUMN is_admin BOOLEAN NOT NULL DEFAULT FALSE;

-- 存量数据：最早注册的用户视为系统管理员
UPDATE users SET is_admin = TRUE
WHERE id = (SELECT id FROM users ORDER BY created_at LIMIT 1);
