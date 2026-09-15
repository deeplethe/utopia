-- 录音交给会标说话人的转写模型读（0040 第三刀）。
--
-- 走 OpenAI 的 `/audio/transcriptions` 形状、要 `diarized_json`：每一句带说话人和起止时刻。
-- 按工作区配、跟对话分开，理由与读扫描件的服务相同：一段董事会录音比一段文字敏感。
-- 交回的分段没有说话人就当没配——不读、告警（决定 5）。地址或模型为空 = 关掉。
ALTER TABLE llm_settings
    ADD COLUMN transcribe_base_url TEXT,
    ADD COLUMN transcribe_api_key  TEXT,
    ADD COLUMN transcribe_model    TEXT;
