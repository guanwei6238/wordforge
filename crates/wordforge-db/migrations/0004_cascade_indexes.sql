-- 為 ON DELETE CASCADE 的外鍵補上索引。
--
-- 這是實際踩到才發現的：重新匯入一份已經匯過的字典時，write_entry 會先
-- `DELETE FROM sense`，而 example.sense_id 的 CASCADE 必須找出對應的例句。
-- 沒有索引的話，每刪一個詞條就要把 example 整張表掃過一次——
-- 71 萬筆 example × 148 萬個詞條，程式跑了七分鐘讀掉 604 GB 卻一筆都沒寫進去。
--
-- 第一次匯入不會出事，因為那時子表是空的。只有重匯才會暴露。
-- SQLite 官方文件也明確建議替 CASCADE 的子表欄位建索引。
CREATE INDEX IF NOT EXISTS idx_example_sense ON example (sense_id);

-- 同理：刪掉一個 lemma 時，這兩張表也要找得到對應的列
CREATE INDEX IF NOT EXISTS idx_surface_form_lemma ON surface_form (lemma_id);
CREATE INDEX IF NOT EXISTS idx_card_lemma ON card (lemma_id);

-- 移除一個字典來源時要清掉它寫過的內容
CREATE INDEX IF NOT EXISTS idx_sense_source ON sense (source_id);
CREATE INDEX IF NOT EXISTS idx_example_source ON example (source_id);
CREATE INDEX IF NOT EXISTS idx_pron_source ON pronunciation (source_id);

-- 這幾個是靠測試自動掃出來的。教材與練習相關的表目前資料量還小，
-- 但同樣的坑不值得踩第二次。
CREATE INDEX IF NOT EXISTS idx_material_profile ON material (profile_id);
CREATE INDEX IF NOT EXISTS idx_material_vocab_lemma ON material_vocab (lemma_id);
CREATE INDEX IF NOT EXISTS idx_exercise_material ON exercise (material_id);
CREATE INDEX IF NOT EXISTS idx_conversation_profile ON conversation (profile_id);
