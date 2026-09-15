-- 审核台的每一档都交给 agent（0043）：从前只有重复对（0025），冲突、低置信、证据过期的
-- 事实都等人。记忆点头（pending_facts，0015）照旧留给人——那是说话的人自己点头。
--
-- `agent_decisions` 一张表装所有档：`target_kind` 说是哪一档的哪一行，`action` 是那一档
-- 自己的出路。`summary` 是做决定那一刻这一项长什么样，写给人看——冲突与事实没有「两个
-- 名字」可以拼，而行后来会被改写、换 id，到时候再去拼就对不上了。`detail` 放这一步的
-- 参数（闭合在哪天、改成哪天、看的是哪段原文）和撤回要用的东西（改写出来的行、原来的
-- 置信度、补上的那条证据）：agent 动过手的每一步都得撤得回，撤回靠的就是这一列。
ALTER TABLE agent_decisions DROP CONSTRAINT agent_decisions_target_kind_check;
ALTER TABLE agent_decisions ADD CONSTRAINT agent_decisions_target_kind_check
    CHECK (target_kind IN ('review', 'fact', 'conflict'));

ALTER TABLE agent_decisions DROP CONSTRAINT agent_decisions_action_check;
ALTER TABLE agent_decisions ADD CONSTRAINT agent_decisions_action_check
    CHECK (action IN ('merge', 'keep', 'unsure',
                      'confirm', 'reject',
                      'close_old', 'retime_new', 'keep_both', 'reject_new'));

ALTER TABLE agent_decisions
    ADD COLUMN summary TEXT,
    ADD COLUMN detail  JSONB NOT NULL DEFAULT '{}';
