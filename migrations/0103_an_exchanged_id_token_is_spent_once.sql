-- 换会话用过的 ID token（0066）。
--
-- `POST /auth/oidc/exchange` 拿别的应用刚登录到的 ID token 换一个 Utopia 会话。
-- 同一个令牌在有效窗口里只能换一次：记下它的 SHA-256，再来一次就拒。
-- 不记 `jti`——不是每个身份提供方都发；整个令牌的哈希一定有。
-- 行只活到令牌本身不再被接受的那一刻，之后每次换会话时顺手扫掉。
CREATE TABLE oidc_exchanges (
    token_hash TEXT PRIMARY KEY,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX oidc_exchanges_expiry_idx ON oidc_exchanges (expires_at);
