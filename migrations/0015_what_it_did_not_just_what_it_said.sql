-- 助手那一轮**做了什么**，而不只是最后说了什么。
--
-- 起因是一个看着像提示词问题的 bug：接着上一轮说「翻译」，助手把整轮工具
-- 重新跑了一遍，而且因为同名实体有八个，第二遍落到了另一批上——要的是同一段话
-- 换个语言，拿到的是另一段内容。
--
-- 真正的原因不在提示词。`conversations::recent_context` 回的是 `(role, content)`：
-- 一轮之内模型看得见自己调过什么（那些消息还在 `msgs` 里），**跨轮之后全丢了**，
-- 只剩它自己写的那段散文。于是下一轮它并不知道自己查过——重查是它能做的
-- 最合理的判断。用提示词去压一个正确的推断，压得赢一部分：实测四次里三次。
--
-- 所以别丢。这一列存那一轮完整的消息尾巴——带 `tool_calls` 的助手消息，
-- 以及配套的 tool 结果消息——回放时原样发回去。
ALTER TABLE conversation_messages
    -- 形如 `[{"role":"assistant","tool_calls":[...]}, {"role":"tool","tool_call_id":...}, ...]`。
    --
    -- **存已经截断过的那一份**：工具结果在发给模型之前就按
    -- `TOOL_CHUNK_CHARS` 截过，这里存的是同一份，不是原始返回。存原始的等于
    -- 让回放比当初那一轮还占地方。
    --
    -- 只对 assistant 行有意义；user 行恒为空数组
    ADD COLUMN tool_exchange JSONB NOT NULL DEFAULT '[]'::jsonb;

-- **只回放最近一轮。** 这一列是为了让模型知道「我刚做过什么」，
-- 而不是把二十轮的工具输出都搬回上下文——那正是当初只存正文的理由。
-- 所以没有索引：查的时候本来就带着 conversation_id 和时间序。
COMMENT ON COLUMN conversation_messages.tool_exchange IS
    'The assistant turn''s tool calls and their results, replayed for the most recent turn so the model knows what it already did.';
