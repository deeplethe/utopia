-- 身份文档每次抽取都按 document_id 清理名字向量，来源开启身份名时也按文档找证据。
-- 原主键 (fact_id, chunk_id) 服务的是从事实溯源，不能定位一篇文档的证据；没有这个
-- 反向入口，一批逐篇推送会反复扫描整张 fact_evidence（#1408）。
CREATE INDEX fact_evidence_document_idx ON fact_evidence (document_id);
