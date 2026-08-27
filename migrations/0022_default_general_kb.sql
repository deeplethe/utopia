-- 公共空间此前只活在注释里（kbs.rs：“General 由系统初建保持 open”），从来没有代码
-- 真的建过它。结果每个部署的第一屏都是空的：Graph 停在 Loading，切换器里无库可选，
-- 而"你需要先建一个库"这件事界面从没说过。注册流程已补上，这里管已经跑起来的部署。
--
-- 只补给一个库都没有的工作区——已经建过库的部署自有其默认库，不该被塞进第二个。
WITH created AS (
    INSERT INTO knowledge_bases
        (id, workspace_id, name, kind, description, is_default, visibility)
    SELECT gen_random_uuid(), w.id, 'General', 'knowledge',
           'Shared space for the whole deployment. Everyone can read it.', TRUE, 'open'
    FROM workspaces w
    WHERE NOT EXISTS (SELECT 1 FROM knowledge_bases k WHERE k.workspace_id = w.id)
    RETURNING id, workspace_id
)
-- 工作区 owner 记为本库 admin，与注册路径一致。只作用于上面刚建的库，
-- 不去动既有部署已有的成员矩阵。
INSERT INTO kb_members (kb_id, user_id, role, added_by)
SELECT c.id, m.user_id, 'admin', m.user_id
FROM created c
JOIN memberships m ON m.workspace_id = c.workspace_id AND m.role = 'owner';
