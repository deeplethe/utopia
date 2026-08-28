-- 模型提议的类型/谓词：本体装不下时，它说的那个词。
--
-- 事实那边已经有了（surface_predicate），实体这边还没有：类型不在本体里就降级成
-- concept，模型说的 "model" 只剩一个「model ×43」的总数，实体行上一字未留。
-- 后果是想加 model 类时找不出那 43 个实体——它们混在 322 个 concept 里，只能整库重抽。
ALTER TABLE entities ADD COLUMN IF NOT EXISTS proposed_type TEXT;

-- 顺手改名。"表层"是造出来的词，得先解释才懂；"提议的"一眼就对，而且更准确——
-- 那不是猜测：模型读了原文得出 model，通常是对的，只是本体表达不了。
-- 真正带兜底性质的是我们存进去的 concept。
-- 幂等：改号之前这两条已在部分库里跑过（0025/0026 与另一条改动撞号，见 0030 的
-- 说明），renumber 后必须能在那些库上安全重放。
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.columns
               WHERE table_name = 'fact_evidence' AND column_name = 'surface_predicate') THEN
        ALTER TABLE fact_evidence RENAME COLUMN surface_predicate TO proposed_predicate;
    END IF;
END $$;
