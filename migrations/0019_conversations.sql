-- 0019: Chat 会话持久化——对话、消息、行动轨迹与引用随消息落库。
-- 上下文由服务端从这里拼装（前端只传 conversation_id + 新消息）；
-- steps/sources 与实时 SSE 事件同构，历史回放与流式渲染共用一套组件。

CREATE TABLE conversations (
    id         UUID PRIMARY KEY,
    kb_id      UUID NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title      TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX conversations_kb_user_idx ON conversations (kb_id, user_id, updated_at DESC);

CREATE TABLE conversation_messages (
    id              UUID PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content         TEXT NOT NULL,
    -- 行动轨迹（工具调用步骤）与引用清单（历史回放；与 SSE step/sources 同构）
    steps           JSONB NOT NULL DEFAULT '[]',
    sources         JSONB NOT NULL DEFAULT '[]',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX conversation_messages_conv_idx
    ON conversation_messages (conversation_id, created_at);
