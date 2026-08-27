-- 类型描述：不是给人看的装饰，是喂给抽取 prompt 的语义指引
-- （"Event: 有明确时间点的事件，如发布、收购、会议"），直接影响抽取质量。
ALTER TABLE entity_types ADD COLUMN description TEXT NOT NULL DEFAULT '';
ALTER TABLE relation_types ADD COLUMN description TEXT NOT NULL DEFAULT '';
