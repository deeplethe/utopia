-- 抽取失败的原因此前只进日志与 jobs.last_error，文档上什么都不留，界面无从显示。
-- graph_error 独立成列：documents.error 归解析管道所有（set_status 会清空它），互不干扰。
ALTER TABLE documents ADD COLUMN graph_error TEXT;

-- 抽取任务的所有权凭证。重抽时自增即"解雇"正在跑的那个任务：它每处理完一个
-- 分块回读一次，发现 epoch 变了就安静退出，把文档让给新任务。
-- 单靠 graph_status 判断不可靠——接手者会把状态写回 extracting，旧任务无从分辨。
ALTER TABLE documents ADD COLUMN extract_epoch INT NOT NULL DEFAULT 0;
