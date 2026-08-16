-- 「這一句我錯過幾次」。
--
-- 次數本來就記在 `sentence_review.misses` 裡，但那張表是**排程**：
-- 句子寫對之後整列會被刪掉（見 `sentences::pass`），紀錄跟著消失。
-- 而使用者要看的正好相反——「這句我錯過三次才寫對」是複習時最有用的
-- 一個訊號，練起來之後更值得留著。
--
-- 所以次數改成累計在句子這一側。兩張表分工：
--
--   sentence_review  還沒練起來的句子，什麼時候再出現（會被刪）
--   word_sentence    做過的句子本身，錯過幾次（留著）
ALTER TABLE word_sentence ADD COLUMN misses INTEGER NOT NULL DEFAULT 0;

-- 這一句是那份練習的第幾題。
--
-- 沒有它就對不回排程與批改結果——翻譯題的作答、對錯、文法標籤
-- 全都是照題號存的。閱讀與克漏字的句子不是「一題」，所以是 NULL。
ALTER TABLE word_sentence ADD COLUMN item_index INTEGER;
