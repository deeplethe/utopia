-- 类的**第二个**向量：只嵌 label，不带描述。
--
-- 类型消解对每个实体发两种查询（见 `type_resolution.rs`）：一种是模型自己给的
-- 说法（`district. place`，短），一种是整段画像（长）。两种此前比的是同一批
-- `label + description` 向量——**查询分了两种形状，文档只有一种**。
--
-- 后果是短查询被同义反复的类接管。实测「杭州拱墅区」（specific_type = district）
-- 的短查询回来的是：
--
--   Map(0.308) / Park(0.345) / Country(0.347) / Museum(0.356)
--
-- 四个赢家的嵌入原文分别是 `Map\nA map.`、`Park\nA park.`、`Country\nA country.`、
-- `Museum\nA museum.`——全是一行同义反复。而正确答案 AdministrativeArea 的原文是
-- `AdministrativeArea\nA geographical region, typically under the jurisdiction of a
-- particular government.`，一百字，排不进前八。
--
-- 不是语义更近，是**长度对上了**。整体测得：被检索出来的类，嵌入原文长度中位数
-- 44，而全部 965 个类的中位数是 89——检索系统性地偏好短文本。
--
-- 「短的那一侧距离系统性地更小」这条规律，本仓库已经栽过四次（跨实体不可比、
-- 两路之间不可比、同一路两个查询之间不可比、空描述的悬空类占便宜）。前四次修的
-- 都是查询一侧，这一次是文档一侧：**两种查询就该有两份文档**，短对短、长对长。
ALTER TABLE entity_types ADD COLUMN label_embedding vector;

-- 与 `embedded_text` / `embedded_model` 同一套自愈判据（见 0038）：比对「当时嵌的
-- 原文与模型名跟现在对不对得上」，而不是看时间戳。改了 label、换了嵌入模型都会
-- 当场对不上，于是自动重嵌——不必在每个改 label 的写入点挂钩子，漏挂一个就悄悄烂掉。
ALTER TABLE entity_types
    ADD COLUMN label_embedded_text text,
    ADD COLUMN label_embedded_model text;
