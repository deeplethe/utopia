-- 0013: 实体类型形状升格为本体属性（此前 organization/product = 方形是前端写死的）。
-- 图谱节点按类型的 shape 渲染：circle（四层圆）| square（四层方）。
ALTER TABLE entity_types ADD COLUMN shape TEXT NOT NULL DEFAULT 'circle'
    CHECK (shape IN ('circle', 'square'));
-- 保持现有观感：内置的组织/产品沿用方形
UPDATE entity_types SET shape = 'square' WHERE key IN ('organization', 'product');
