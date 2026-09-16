-- 扫描件和图片交给版面识别服务读（0040 第二刀）。
--
-- 读字的服务按工作区配，跟对话、嵌入分开：一份扫描的合同比一段文字敏感，部署会合理地把
-- 识别留在本地、对话交给托管模型。服务是 MinerU（`mineru-api`）：先认版面再逐区域认字，
-- 交回每个区域的页码和框，正是 `chunks.anchor` 的 `{"page", "bbox"}`。地址为空 = 关掉，
-- 需要它的文件照第一刀降级。密钥跟另外两把一样入库即封印。
ALTER TABLE llm_settings
    ADD COLUMN ocr_base_url TEXT,
    ADD COLUMN ocr_api_key  TEXT,
    -- 服务端的后端（pipeline、vlm-auto-engine……）；空 = 用服务自己的默认
    ADD COLUMN ocr_backend  TEXT;

-- 正在读这份文件的远端任务。
--
-- 识别一份几百页的扫描件要几分钟到几十分钟，不能占着一个任务槽等：提交之后把任务号记在
-- 这里，处理任务挂回队列过一会儿再来问。进程重启、任务重排都从这里接着问，不重新提交。
-- 形状：{"reader", "service", "task_id", "sha256", "submitted_at"}——`sha256` 对不上
-- （文件换了版本）或 `service` 对不上（换了服务）就作废重交。读完、失败都清空
ALTER TABLE documents
    ADD COLUMN reader_task JSONB;
