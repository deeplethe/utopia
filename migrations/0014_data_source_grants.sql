-- 数据源授权：这个库可以给哪些工作区用。
--
-- **在此之前这一层不存在。** 注册是部署级动作（`require_admin`），而挂载的
-- 守卫是 `require_kb(kb_id, Role::Admin)`——请求者自己那个库的管理员。可挂载
-- 列表返回的又是 `datasources::list(pool)`：全部署每一个源，不过滤。
--
-- 于是任何一个知识库的管理员，都能列出部署里每一个已注册数据库，并把任意一个
-- 挂进自己库。挂上之后该库的每个 Viewer 都能通过 `query_data` 对它跑只读 SQL
-- （问数是读，viewer 亦可用）。多工作区部署下这是跨租户的。
--
-- **为什么是新表，而不是给 `data_sources` 加一列 `workspace_id`：**
-- 那一列意味着每个数据源只属于一个工作区。公司只有一个数仓、几个部门各自一个
-- 工作区时，这个数仓就只能给一个部门用——把一个本该多对多的关系退化成了一对多。
-- 授权本身就是多对多的：一个源可授权给多个工作区，一个工作区可拿到多个源。
--
-- **与 `kb_data_sources` 的分工**（那张表一个字段都不动）：
--
--   授权 = 系统管理员说「这个库可以给哪些工作区用」  ← 本表
--   挂载 = KB 管理员说「我这个库挂哪几个」          ← kb_data_sources
--
-- 两层都是 M2M，各有各的主人。挂载不再能凭空发生：只能在授权过的集合里挑。
CREATE TABLE data_source_grants (
    data_source_id UUID NOT NULL REFERENCES data_sources(id) ON DELETE CASCADE,
    workspace_id   UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    granted_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 授权的人。**裸外键**，与 entity_merges.merged_by 那几处一致：
    -- 用户是软删除的，生产代码里没有 DELETE FROM users，所以归因保得住
    granted_by     UUID REFERENCES users(id),
    PRIMARY KEY (data_source_id, workspace_id)
);

-- 「这个工作区能用哪些源」是热路径（每次开数据映射页都问一次）；
-- 主键已覆盖反向的「这个源给了谁」
CREATE INDEX data_source_grants_workspace_idx ON data_source_grants (workspace_id);

-- 存量授权：把已经挂着的补上，否则这条迁移一上线就把正在用的源全断掉。
--
-- **补的是既成事实，不是补一个宽松默认**：只授权给「确实已经挂载过它」的
-- 那些工作区。没挂过的一个都不给——那正是这条迁移要关掉的门。
INSERT INTO data_source_grants (data_source_id, workspace_id)
SELECT DISTINCT kds.data_source_id, kb.workspace_id
  FROM kb_data_sources kds
  JOIN knowledge_bases kb ON kb.id = kds.kb_id
ON CONFLICT DO NOTHING;
