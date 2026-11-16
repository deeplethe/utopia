-- Google Drive 来源（0074）：由一个人授权连上，按他选的文件夹同步。
--
-- 一次连接尝试的临时状态：授权码换令牌之前，state 与 PKCE verifier 得先落着，
-- 还要记下是谁、替哪个来源发起的——回调时这个浏览器里的会话必须还是他。
-- 短命、一次性：回调一到就删，半途弃单的过期扫走。与 oidc_flows 同一个形状。
CREATE TABLE IF NOT EXISTS google_drive_flows (
    state TEXT PRIMARY KEY,
    verifier TEXT NOT NULL,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source_id UUID NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS google_drive_flows_expiry_idx ON google_drive_flows (expires_at);

-- 每个文件上一次摄入时 Drive 给的变更标记：二进制文件是 md5Checksum，Google 文档
-- 没有 md5，用 modifiedTime。**不能靠内容哈希判「没变」**：Docs/Sheets/Slides 每次
-- 导出成 Office 格式，压缩包里的时间戳都不同，哈希每轮都变，每轮都会重新抽取。
-- 标记没变、文档还在，就连下载都省了。
CREATE TABLE IF NOT EXISTS google_drive_files (
    source_id UUID NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL,
    marker TEXT NOT NULL,
    PRIMARY KEY (source_id, file_id)
);
