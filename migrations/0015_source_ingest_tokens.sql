-- api 来源的专属推送密钥：只存 sha256 十六进制哈希，明文仅在创建/轮换时返回一次。
-- KB 级 /ingest 改为落入 Uploads（普通上传语义）；带身份的推送走 api 来源 + 本密钥。
ALTER TABLE sources ADD COLUMN ingest_token_hash TEXT;
