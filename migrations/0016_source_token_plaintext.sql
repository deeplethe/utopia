-- 自部署威胁模型下"只看一次"是自找麻烦：改存明文，随时可查（Editor 权限专用端点）。
-- DB 失守时文档本体早已泄露，密钥哈希化没有额外收益；Rotate 保留应对泄露。
ALTER TABLE sources DROP COLUMN ingest_token_hash;
ALTER TABLE sources ADD COLUMN ingest_token TEXT;
