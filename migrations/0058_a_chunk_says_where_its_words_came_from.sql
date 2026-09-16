-- 一块文字从哪来（0040）：原文写的、扫描页上认出来的、录音里转出来的、模型看图说的。
--
-- 分块之后的一切只吃 `chunks.text`，而一段转写、一段对柱状图的描述，与文件里的一句话进库的
-- 样子一模一样，事后没有任何一列能把它们分开。来源放在分块上：引用这一块的证据共享它的来源，
-- 抽取读的也是这一块。现有的块都是原文，这是真的，默认值就是它。
ALTER TABLE chunks
    ADD COLUMN origin TEXT NOT NULL DEFAULT 'stated'
        CHECK (origin IN ('stated', 'ocr', 'transcribed', 'described')),
    -- 读出这段文字的引擎或模型；原文为空。留着它，好在更好的模型出来时重读，
    -- 也好按模型数误读
    ADD COLUMN origin_model TEXT,
    -- 指回原文件字节的位置，形状由来源定。原文不需要（char_start / char_end 已经给出）；
    -- 扫描页记页码（引擎给了就带框）；录音记起止毫秒与说话人——转写必须分得出说话人，
    -- 分不出谁说的，承诺会记到错的人头上；图片描述记它在哪一页、哪一张图
    ADD COLUMN anchor JSONB;

ALTER TABLE chunks ADD CONSTRAINT chunks_anchor_matches_origin CHECK (
    (origin = 'stated' AND anchor IS NULL AND origin_model IS NULL)
    OR (origin = 'ocr' AND anchor ? 'page')
    OR (origin = 'transcribed' AND anchor ? 'start_ms' AND anchor ? 'end_ms' AND anchor ? 'speaker')
    OR (origin = 'described' AND anchor IS NOT NULL)
);

-- 这份文件的字要靠哪一种模型读，而那种模型还没配（0040）。
--
-- 扫描件、图片、录音在没配读取模型时不再解出乱码、也不再重试三次：文件留着，文档停在
-- failed，这一列记下缺的是哪一种。配上之后按这一列把它们重新排进处理队列；读成功了清空
ALTER TABLE documents
    ADD COLUMN reader_needed TEXT CHECK (reader_needed IN ('ocr', 'transcribe'));

CREATE INDEX documents_reader_needed_idx ON documents (reader_needed)
    WHERE reader_needed IS NOT NULL;
