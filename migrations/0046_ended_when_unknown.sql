-- 账本说不出「结束了，但不知道哪天」。
--
-- `valid_to IS NULL` 一直承载两个意思。抽取提示词的规则 3 把它们并排写在一起：
--
--     If a relation is still ongoing, valid_to is null.
--     If the text states no date, use null — never invent dates.
--
-- 于是 "Prem Akkaraju, former CEO of Weta Digital" 这句——**结束是原文明说的，
-- 日期是原文没给的**——只能写成 null，然后被系统读成「至今仍是」。图会理直气壮
-- 地断言一件原文说已经结束的事。
--
-- 实测（ai-timeline，348 块）：`valid_from` 覆盖 58.5%，`valid_to` 只有 8.6%。
-- 而语料里结束措辞不少：`former` 30 块、`left`/`departed` 13、`until` 8、
-- `stepped down`/`resigned` 7、`no longer`/`ceased` 5。**一条都落不进账本。**
--
-- 这跟 0045 修的是同一类缺陷：**时间模型没有表达无知的位置**。那次是「不知道
-- 精度」被写成「精确到日」，这次是「不知道何时结束」被写成「仍在继续」。
--
-- 顺带修掉一个一直存在的窄处：**一个精度列描述两个端点**。全库 556 条事实两端
-- 都有日期，它们的粒度只能共用一个值；另有 145 条只有 valid_to，那个列名写着
-- from、描述的却是 to。拆成按端记，两个问题一起没了。
--
-- 三种状态从此分得开：
--
--   仍在持续          valid_to IS NULL   valid_to_precision IS NULL
--   结束了，不知哪天   valid_to IS NULL   valid_to_precision = 'unknown'
--   2023 年结束       valid_to = …       valid_to_precision = 'year'
--
-- **不采用「把文档日期填进 valid_to 当上界」那种做法**（教科书里的 indeterminate
-- instant）。那会在列里放一个看起来确定的时间戳，每个读者都得先查精度才敢用它，
-- 而这个产品卖的正是不骗人。上界信息本来就能从证据的文档日期反推，需要时再说。

ALTER TABLE facts RENAME COLUMN valid_precision TO valid_from_precision;
ALTER TABLE facts ADD COLUMN valid_to_precision text;

-- **旧约束先撤，再动数据。** 0045 那条按「两端都没日期才没有精度」判，而下面第一个
-- UPDATE 恰恰要把「只有 valid_to」的行的起始精度置空——在旧约束眼里那是违规。
--
-- 这个顺序错误在空表上看不出来（UPDATE 影响 0 行），我第一版就是在一个全新的
-- 一次性库上验的，全绿；打到有数据的 dev 库上当场炸，145 行。
-- **迁移的验证库得有代表性的数据，不然验的只是语法。**
ALTER TABLE facts DROP CONSTRAINT IF EXISTS facts_precision_needs_a_date;

-- 只有 valid_to 的那 145 条：那个精度描述的一直是结束端，搬过去而不是复制
UPDATE facts
   SET valid_to_precision = valid_from_precision,
       valid_from_precision = NULL
 WHERE valid_from IS NULL AND valid_to IS NOT NULL;

-- 两端都有的 556 条：历史上只存过一个粒度，无从区分，只能同值复制。
-- 这是**回填的近似**，不是测量——从此以后两端各记各的
UPDATE facts
   SET valid_to_precision = valid_from_precision
 WHERE valid_from IS NOT NULL AND valid_to IS NOT NULL;

-- 起始端：有日期才有精度，没日期就没有。没有 unknown——「开始了但不知道何时」
-- 与「不知道有没有开始」在这个账本里不可区分，硬加一个状态只会让读者猜
ALTER TABLE facts ADD CONSTRAINT facts_from_precision_matches_date
  CHECK ((valid_from IS NULL) = (valid_from_precision IS NULL));

-- 结束端：多一个 unknown，且它**只在没有日期时**成立——有日期还说不知道，
-- 是自相矛盾，而约束正是用来挡住这种矛盾进库的。
--
-- 那句 `IS NOT NULL` 不是冗余的。少了它，`valid_to` 有日期而精度为 NULL 的行
-- 会**被放行**：`NULL IN ('year',…)` 求值是 NULL，`TRUE AND NULL` 是 NULL，
-- `NULL OR FALSE` 还是 NULL，而 CHECK 约束遇到 NULL 判通过。三值逻辑在这里
-- 是静默的——写完自测才发现，六个用例里就这一个漏网。
ALTER TABLE facts ADD CONSTRAINT facts_to_precision_matches_date
  CHECK (
    (valid_to IS NOT NULL AND valid_to_precision IS NOT NULL
       AND valid_to_precision IN ('year', 'month', 'day'))
    OR (valid_to IS NULL AND (valid_to_precision IS NULL OR valid_to_precision = 'unknown'))
  );
