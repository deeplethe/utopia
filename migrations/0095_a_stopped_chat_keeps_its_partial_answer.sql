-- 主动停止保留已发布正文；普通消息与旧历史默认完整（0063 / #961）。
ALTER TABLE conversation_messages
    ADD COLUMN stopped BOOLEAN NOT NULL DEFAULT FALSE;
