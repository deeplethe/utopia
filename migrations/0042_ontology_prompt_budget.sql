-- 本体铺进抽取提示词的字符预算。超了就改成按分块检索候选。
--
-- 放部署设置而不是环境变量：这一档要能不重启就改。定死它需要一条曲线——
-- 每个本体规模下，全量内联与按块检索各测一次，看它们在哪里交叉。重启一次
-- 服务测一档的话，那条曲线不会有人去跑第二遍。
--
-- 缺省 24000 字符（约 6000 token）是**拍的**，正等着那条曲线来定。
ALTER TABLE deployment_settings
    ADD COLUMN ontology_prompt_budget integer NOT NULL DEFAULT 24000;
