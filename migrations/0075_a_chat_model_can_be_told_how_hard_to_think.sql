-- 对话模型的推理强度。推理模型默认边想边答：第一次真跑（bench README，2026-09-24）里
-- 抽取每次调用平均 2.2k 进、7.2k 出，出的 94% 是思考 token，而我们要的只是几百 token 的
-- 照原文写的 JSON。OpenAI 兼容口的 `reasoning_effort` 能把它关小（minimal 时思考归零，
-- 答案不变）；空 = 不带这个字段，端点按自己的默认来。按工作区存，和模型放一起：
-- 它是「这个模型怎么用」的一部分，不是某个任务的参数。
ALTER TABLE llm_settings ADD COLUMN chat_reasoning_effort TEXT
    CHECK (chat_reasoning_effort IN ('minimal', 'low', 'medium', 'high'));
