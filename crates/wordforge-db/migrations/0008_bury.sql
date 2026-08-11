-- 埋葬：把一張卡藏到明天，但不動它的排程。
--
-- 跟暫停（suspended）的差別是「會不會自己回來」：
--
--   埋葬  今天不想看到這張（剛看過同一個字的另一種卡型、
--         或是這題卡住了想先跳過），明天自動回來。
--   暫停  這張根本不該出現（分級測驗判定已經會了），
--         要使用者主動恢復才回來。
--
-- 存到期時間而不是布林值，才不需要一支「每天清掉埋葬旗標」的排程工作。
-- 那種工作在桌面應用程式上特別不可靠：使用者可能三天沒開 App。
ALTER TABLE card ADD COLUMN buried_until TEXT;

-- 熱路徑索引要一起換掉：每次開 App 的第一個查詢都會用到 buried_until。
-- 不換的話那個查詢會退化成掃描整個牌組。
DROP INDEX IF EXISTS idx_card_due;
CREATE INDEX idx_card_due ON card (profile_id, suspended, buried_until, due);
