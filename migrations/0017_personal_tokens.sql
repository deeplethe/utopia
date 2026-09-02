-- 个人访问令牌：给 MCP 客户端一把长命的钥匙（见 docs/decisions/0014）。
--
-- **它以这个人的身份行事，但不必是这个人的全部。**
--
--     有效权限 = 这个人的角色 ∩ 这枚令牌的 scope
--
-- 交集不是并集：viewer 的令牌勾上 write 也还是只读。scope 是上限，不是授权。
--
-- 为什么不发机器令牌（0014 里留痕的那条岔路）：机器身份要引入第三套授权模型，
-- 而且 `audit_events.actor_id` 会多出一类「不是任何人做的」记录——账本存在的
-- 理由正是「谁在什么时候认下了什么」。
CREATE TABLE personal_tokens (
    id           UUID PRIMARY KEY,
    -- **有外键且级联**，与 `audit_events.actor_id` 的裸 UUID 相反：
    -- 台账要活得比用户久，钥匙不该。人没了，他的钥匙就该一起没
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 人自己起的名字，"我的笔记本"。撤销时要认得出撤的是哪一把
    name         TEXT NOT NULL,

    -- **哈希存，与 `sources.ingest_token` 的明文相反。**
    --
    -- 那一条的理由是「DB 失守时文档本体早已泄露，哈希化没有额外收益」，
    -- 而它成立是因为 ingest_token 只能**往里推文档**。这一把不一样：它经
    -- `query_data` 能读出 Utopia 之外的生产库。数仓在另一台机器上、装着另一批
    -- 数据，不该跟着 Utopia 的库一起丢。**爆炸半径不同，所以存法不同。**
    token_hash   TEXT NOT NULL UNIQUE,
    -- 给人认的前缀（`utp_ab12…`）。列表里要能一眼对上配置文件里那一串，
    -- 而不必把整条明文留下来
    token_prefix TEXT NOT NULL,

    -- read = 只读工具；write = 额外放开 remember。**默认只读**：
    -- 要让 agent 写进账本，得显式勾
    scope        TEXT NOT NULL DEFAULT 'read' CHECK (scope IN ('read', 'write')),
    -- 限定到哪几个库。NULL = 这个人能进的全部。
    -- 裸 UUID 数组不是懒：库删了这一项只是失效，不该把令牌整个删掉
    kb_ids       UUID[],

    -- NULL = 不过期。界面默认给 90 天——不过期是能选的，但不是缺省
    expires_at   TIMESTAMPTZ,
    -- 「这把还在用吗」。撤之前要答得出这个问题，否则没人敢撤
    last_used_at TIMESTAMPTZ,
    -- **撤销不删行**：撤过这件事本身要留痕。删了行，「这把钥匙存在过」
    -- 就查不到了，而那正是事后追查要问的第一件事
    revoked_at   TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 校验是热路径：**每次工具调用都要查一遍**，不是握手时查一次。
-- 那是 0014_data_source_grants 的教训——列表过滤不是守卫，MCP 的对应形态是
-- 「信任一整条连接的生命周期」，而 revoked_at 在中途被写上时必须立刻生效
CREATE UNIQUE INDEX personal_tokens_hash_idx ON personal_tokens (token_hash);
-- 「我发过哪几把」：账户页按人列，最近发的在前
CREATE INDEX personal_tokens_user_idx ON personal_tokens (user_id, created_at DESC);
