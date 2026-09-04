-- 经 MCP 记进来的那句话，是**哪一个 agent** 说的。
--
-- 人已经在 `proposed_by` 里（0015），但一个库上可以连着好几个 agent，
-- 而它们共用同一个人的身份。审核卡上三条「张三说的」分不出是哪个客户端记的，
-- 而「谁说的」正是人裁决时唯一能依据的东西——一条来自代码助手的记忆和一条
-- 来自会议纪要 agent 的记忆，可信度不是一回事。
--
-- 令牌撤销是**痕迹**不是删除（0014），行还在，所以这一列平时不会变空；
-- ON DELETE SET NULL 是为了人被删除时连带删掉令牌的那条路——待确认的事实
-- 不该跟着消失，它已经等在那里了。
ALTER TABLE pending_facts
    ADD COLUMN proposed_token UUID REFERENCES personal_tokens(id) ON DELETE SET NULL;
