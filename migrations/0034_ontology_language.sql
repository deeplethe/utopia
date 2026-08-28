-- 内置本体用哪种语言播种，以及新库的默认值。
--
-- **这不是"系统语言"**（见 docs/decisions/0004）。界面语言在客户端，后端没有 locale。
-- 这一列管的是**语料的语言**：类的 description 逐字进抽取提示词，读者是正在读你文档
-- 的模型——描述与被判断的文本同语言，判断更稳。所以中国团队读英文技术文档时，
-- 界面要中文而这一列该是 'en'，一个开关按不下去这两件事。
--
-- 语言只在**播种那一刻**决定内置类的措辞。此后 label/description 是这个库的数据，
-- 可编辑；改这一列不会回头重写它们。它继续管的是新描述（自动扩本体、AI 建议）写成什么语言。
ALTER TABLE knowledge_bases   ADD COLUMN ontology_lang         TEXT NOT NULL DEFAULT 'en';
ALTER TABLE deployment_settings ADD COLUMN default_ontology_lang TEXT NOT NULL DEFAULT 'en';

-- 取值收在 CHECK 里而不是应用层：这一列会被用来挑一张编译期常量表，
-- 写进一个没有对应表的值只会静默回落到英文，不报错——那种错最难查
ALTER TABLE knowledge_bases
    ADD CONSTRAINT knowledge_bases_ontology_lang_chk CHECK (ontology_lang IN ('en', 'zh'));
ALTER TABLE deployment_settings
    ADD CONSTRAINT deployment_default_ontology_lang_chk
    CHECK (default_ontology_lang IN ('en', 'zh'));
