-- 单点登录：一个身份提供方的 subject 显式绑定到一个账号（0056）。
--
-- 绑定由账号本人建立：先用密码登录，再走一遍 SSO，回调核对是同一个人。
-- 绝不按 email 这类可变声明自动关联——那等于让身份提供方的一个字段直接决定
-- 「这是哪个账号」；也不让管理员替别人绑，那是一条冒充任何人登录的路。
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
    -- 绑定流程：发起它的已登录账号。回调时这个浏览器里的会话必须还是他。空 = 登录流程
    link_user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX oidc_flows_expiry_idx ON oidc_flows (expires_at);
