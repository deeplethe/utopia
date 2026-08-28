-- JWT 签名密钥不再要求部署者手填。首次启动时自动生成一条存这里，
-- 于是「照 README 跑起来」和「安全」不再是两件要分别做的事——
-- 默认值 dev-secret-change-me 上生产这类事故，靠提醒是防不住的。
--
-- UTOPIA_JWT_SECRET 仍然优先于本列：轮换密钥、或者要多个实例显式对齐时，
-- 填环境变量即可，那条路没有被关掉。
ALTER TABLE deployment_settings ADD COLUMN jwt_secret TEXT;
