-- 十萬張卡的實測讓兩個熱路徑查詢跑到 200~340 ms，使用者會感覺到卡。
--
-- ## 問題一：0008 的 buried_until 讓 due 的索引失效
--
-- 判斷式寫成 `(buried_until IS NULL OR buried_until <= ?)`。那個 OR 讓 SQLite
-- 沒辦法把它當成範圍條件，於是索引在 buried_until 這一欄就斷掉，
-- 後面的 due 完全用不到——每次開 App 都要掃過整個牌組。
--
-- 改成用空字串當「沒有被埋葬」。空字串排在任何 RFC 3339 時間戳之前，
-- 所以 `buried_until <= now` 對它永遠成立，而且是純範圍條件，索引接得下去。
--
-- 直接 DROP 再 ADD：埋葬狀態最多只活一天，重建不會損失有意義的東西。
-- 索引要先拿掉再動欄位：SQLite 會在 DROP COLUMN 時檢查每一個索引，
-- 而 idx_card_due 正好蓋在這一欄上。
DROP INDEX IF EXISTS idx_card_due;

ALTER TABLE card DROP COLUMN buried_until;
ALTER TABLE card ADD COLUMN buried_until TEXT NOT NULL DEFAULT '';

-- buried_until **不能**放在 due 前面。它是範圍條件，一旦進了索引，
-- SQLite 就沒辦法再用索引替 due 排序，只能 USE TEMP B-TREE FOR ORDER BY
-- ——把所有到期的卡撈出來排一遍，LIMIT 也救不了。
--
-- 被埋葬的卡是極少數，當成殘留條件逐列檢查便宜得多。
CREATE INDEX idx_card_due ON card (profile_id, suspended, due);

-- ## 問題二：known_lemma_ids 沒有任何索引可用
--
-- 「他會哪些字」是覆蓋率驗收與已知詞抽樣的基礎，每次出題都要跑。
-- 但它篩的是 kind + state + stability，跟上面那個索引完全對不上，
-- 只能全表掃描。
--
-- 把 lemma_id 也放進索引，讓它變成覆蓋索引：查詢完全不必回表。
CREATE INDEX idx_card_known ON card (profile_id, kind, state, stability, lemma_id);

-- ## 問題三：queue_status 要數「還沒學的新卡」與「被收起來的」
--
-- 兩個都是 state / suspended 上的計數，同樣沒有合適的索引。
CREATE INDEX idx_card_state ON card (profile_id, suspended, state);
