-- 教材詞表從「整本書」改成「逐塊」。
--
-- 原本的 material_vocab 只記「這本書用到哪些字」，出題挑段落時只能
-- 拿字面去 LIKE。那個做法不做詞形還原：課本寫 went、學習者在練 go，
-- 字面比對找不到，反而會誤中 going、good、ago。
--
-- 逐塊記錄之後，「哪些塊含有 lemma go」是一個有索引的查詢，
-- 而且詞表本來就是用 base_form 建的，went / goes / gone 自動涵蓋。
-- 這對每一種有字典的語言都成立，不需要任何模型。

DROP TABLE IF EXISTS material_vocab;

CREATE TABLE material_chunk_vocab (
    chunk_id  INTEGER NOT NULL REFERENCES material_chunk (id) ON DELETE CASCADE,
    lemma_id  INTEGER NOT NULL REFERENCES lemma (id) ON DELETE CASCADE,
    count     INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (chunk_id, lemma_id)
);

-- chunk_id 由主鍵的前綴涵蓋；lemma_id 要自己建索引，
-- 否則刪一個詞條時 CASCADE 會全表掃描（見 0004 的教訓）。
CREATE INDEX idx_chunk_vocab_lemma ON material_chunk_vocab (lemma_id);
