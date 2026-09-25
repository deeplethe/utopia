-- 一版文档记下它自己的日期（#900）。
--
-- 同一身份再推一份新内容是原地更新：`documents.doc_time` 换成新的，旧块作废、旧证据
-- 停在旧版上。时间线给没起点的行排序用的是证据文件自带的日期——按文档当前的日期算，
-- 停在旧版上的行就和新行「同时」开始，函数型属性的前一段永远关不上（对账记成
-- simultaneous 冲突）。版本表记下每一版推来时的日期，证据按自己那一版取日期。
--
-- 回填：只有当前这一版的日期是知道的（就是文档现在的日期）；更早的版本留空，取日期时
-- 退回文档的日期，与从前一样。
ALTER TABLE document_versions ADD COLUMN doc_time TIMESTAMPTZ;

UPDATE document_versions v
   SET doc_time = d.doc_time
  FROM documents d
 WHERE d.id = v.document_id
   AND v.version = (SELECT max(x.version) FROM document_versions x WHERE x.document_id = v.document_id);
