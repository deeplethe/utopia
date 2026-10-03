-- 推理强度多认一个值：none。
--
-- 0075 认的是 minimal / low / medium / high，照的是 minimal 能把思考归零的端点。DeepSeek 不是：
-- minimal 关不掉，只有 none 关得掉（bench README，2026-09-30 的第九次真跑是在测量库上去掉这条
-- CHECK 直接写的）。这一列从这里起也是对齐、提规则、本体代理用的强度——它们从前固定用 low，
-- 在 DeepSeek 上每次调用想七八千 token，占了每篇文档 token 的一半——所以要关得掉。
ALTER TABLE llm_settings DROP CONSTRAINT IF EXISTS llm_settings_chat_reasoning_effort_check;
ALTER TABLE llm_settings ADD CONSTRAINT llm_settings_chat_reasoning_effort_check
    CHECK (chat_reasoning_effort IN ('none', 'minimal', 'low', 'medium', 'high'));
