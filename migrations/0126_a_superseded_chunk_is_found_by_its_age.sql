-- 每日的清理（utopia_store::retention）按截止时间读一个库里被取代的分块。
-- chunks_live_idx 只覆盖现行的分块，chunks_kb_idx 则要走遍库里每一个分块。
-- 这个索引是部分索引：现行的分块不会进来。
CREATE INDEX IF NOT EXISTS chunks_superseded_idx ON chunks (kb_id, superseded_at)
    WHERE superseded_at IS NOT NULL;
