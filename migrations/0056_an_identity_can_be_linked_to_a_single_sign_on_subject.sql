-- 单点登录：一个身份提供方的 subject 显式绑定到一个账号（0056）。
--
-- 绑定必须由管理员显式建立，绝不按 email 这类可变声明自动关联——
-- 那等于让身份提供方的一个字段直接决定「这是哪个账号」。
CREATE TABLE oidc_identities (
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (issuer, subject),
    UNIQUE (issuer, user_id)
);

-- 一次登录尝试的临时状态：授权码换令牌之前，state/nonce/PKCE verifier 都得先落着。
-- 短命、一次性——回调一到就删（成功）或者过期扫走（半途弃单）。
CREATE TABLE oidc_flows (
    state TEXT PRIMARY KEY,
    nonce TEXT NOT NULL,
    verifier TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX oidc_flows_expiry_idx ON oidc_flows (expires_at);
