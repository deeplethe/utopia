-- 0061 · cut 3：本体用两个数来量（决定 5）。其中一个是"人改过或拒掉的提案占比"——
-- 采纳时人改过标签、定义、域值域，此前只在审计里留痕，算不出比例。
ALTER TABLE ontology_proposals ADD COLUMN edited BOOLEAN NOT NULL DEFAULT false;
