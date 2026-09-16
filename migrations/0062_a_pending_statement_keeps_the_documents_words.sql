-- 等人点头的一条陈述也留着文档自己的字（0044 第一刀的记忆侧，接 0061）。
--
-- 记忆文档（对话里的「记住……」，0015/0018）不直接写事实：抽出来的陈述先在
-- `pending_facts` 里等人点头。那张表只认带本体的形状——`predicate_id`、
-- `proposed_predicate`、算出来的 `valid_*`。抽取只写开放图谱之后（0061），一条
-- 待确认的行必须能装下一条开放陈述，人点头时它才能按开放陈述落进 `facts`：
-- 照写的短语、按文档角色词记的属性、照抄的时间词、引文在块里的位置。
--
-- 全部可空、纯增量：老的带本体形状的行一列不动。`phrase` 非空就是开放陈述，其余
-- 四列只在它非空时有意义。时间词照抄、从不是日期（0045）；偏移由服务端搜文本算出，
-- 从不取模型报的数。
ALTER TABLE pending_facts
    -- 文档自己的关系短语（开放陈述）；老的带本体形状为 NULL
    ADD COLUMN phrase      TEXT,
    -- [{"role": "amount", "value": "$2 million"} | {"role": "to", "entity_id": "<uuid>"}]
    ADD COLUMN qualifiers  JSONB,
    -- [{"text": "March 4, 2011", "char_start": 143}]  照抄的字，永远不是日期
    ADD COLUMN time_words  JSONB,
    -- 引文在 chunks.text 里的字符偏移（不是字节），服务端算出；NULL = 没定位到
    ADD COLUMN quote_start INT,
    ADD COLUMN quote_end   INT;

-- 拒绝过的开放陈述按 (主语, 短语, 宾语) 挡，不按 (主语, 谓词, 宾语)：
-- 同一对实体之间换一个短语是另一句话，不该被上一句的拒绝连累
ALTER TABLE rejected_facts ADD COLUMN phrase TEXT;
