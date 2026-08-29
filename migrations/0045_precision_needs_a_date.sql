-- 没有日期的事实不该自称"精确到日"。
--
-- `valid_precision` 是 NOT NULL DEFAULT 'day'，于是每一条根本没有 valid_from
-- 也没有 valid_to 的事实，都带着 'day' 落库。实测：国情咨文那个库 728 条、
-- ai-timeline 843 条——都是活行，都在说一句假话。任何按这个字段渲染
-- "精确到日" 的界面都会照着念。
--
-- 这不是显示瑕疵。这个产品卖的是"什么时候知道什么"，而账本在**无知的地方
-- 填了一个确定的值**。默认值本身就是这条 bug：它让"没量过"和"量到日"
-- 长得一模一样。
--
-- 约束按**两端**写，不是只看 valid_from：全库有 144 条只有 valid_to 没有
-- valid_from（"直到 2023 年"这类），它们的精度描述的是结束那一端，是真的。
ALTER TABLE facts ALTER COLUMN valid_precision DROP DEFAULT;
ALTER TABLE facts ALTER COLUMN valid_precision DROP NOT NULL;

UPDATE facts SET valid_precision = NULL
 WHERE valid_from IS NULL AND valid_to IS NULL;

ALTER TABLE facts ADD CONSTRAINT facts_precision_needs_a_date
  CHECK ((valid_from IS NULL AND valid_to IS NULL) = (valid_precision IS NULL));
