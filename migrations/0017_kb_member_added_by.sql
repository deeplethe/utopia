-- 加入信息：记录把成员加进库的人（历史行留 NULL，展示退化为只有时间）。
-- IF NOT EXISTS：允许开发期手动预应用与启动迁移共存。
ALTER TABLE kb_members
    ADD COLUMN IF NOT EXISTS added_by uuid REFERENCES users(id) ON DELETE SET NULL;
