-- 複習句子的作答紀錄，跟練習紀錄分開存。
--
-- ## 為什麼不繼續用 `attempt`
--
-- 句子複習原本走的是 `engine::regrade`，而那條路每批改一次就往
-- `attempt` 寫一筆——那張表是「這份練習做過幾次」。畫面改成一次一句
-- 之後，複習三句就在原本那份練習底下長出三筆「第 N 次」，清單上的
-- 「做過 12 次」其實是複習了 12 句，而不是重做了 12 次那份練習。
--
-- 兩件事本來就不一樣：
--
--   練習紀錄   一份題目做過幾次、每次幾分、可以整份再做一次
--   複習紀錄   今天複習了哪幾句、寫對沒有
--
-- 混在同一張表裡，兩邊的意思都會被對方稀釋。
--
-- ## 為什麼不存題目本文
--
-- 跟 `sentence_review` 同一個理由：本文在 `exercise.payload_json` 裡，
-- 拿 `exercise_id` + `item_index` 取得出來。存兩份只會有兩份互相漂移的
-- 真相——而且題目本來就不會改。
--
-- 代價是練習被刪掉時這裡的紀錄也跟著 CASCADE 消失。那是對的：
-- 題目都沒了，「你當時翻得對不對」沒有東西可以對照。

CREATE TABLE sentence_attempt (
    id           INTEGER PRIMARY KEY,
    profile_id   INTEGER NOT NULL REFERENCES profile (id) ON DELETE CASCADE,
    exercise_id  INTEGER NOT NULL REFERENCES exercise (id) ON DELETE CASCADE,
    -- 那份練習的第幾題（從 0 起算），跟 `sentence_review` 同一個座標
    item_index   INTEGER NOT NULL,
    -- 這次寫了什麼
    answer       TEXT NOT NULL,
    correct      INTEGER NOT NULL,
    -- 批改當下的參考答案。**存下來而不是每次重問模型**：同一句話
    -- 兩次批改給的說法不一定一樣，紀錄要能還原「當時看到的是什麼」。
    -- 口語那一欄只在它跟正式說法不一樣時才有值。
    reference        TEXT,
    reference_formal TEXT,
    comment      TEXT,
    created_at   TEXT NOT NULL
);

-- 外鍵一定要有索引，否則刪練習時 CASCADE 會全表掃描。
--
-- profile_id 這條同時是清單查詢的索引，而且**三欄都要**：清單是
-- `WHERE profile_id = ? ORDER BY created_at DESC, id DESC`，
-- 少了最後那個 id 就只蓋得到排序的前半，SQLite 會補一個
-- `USE TEMP B-TREE FOR LAST TERM OF ORDER BY`。
--
-- id 是拿來打破平手的，而平手是常態不是例外：一輪三句是同一次批改
-- 寫進來的，`created_at` 完全一樣。少了它翻頁的順序會浮動，
-- 同一句可能在第 1 頁看到一次、第 2 頁又看到一次。
--
-- 方向也要跟查詢一致（都是 DESC）。這一條是測試抓到的，寫的當下
-- 兩欄看起來完全合理。
CREATE INDEX idx_sentence_attempt_profile
    ON sentence_attempt (profile_id, created_at DESC, id DESC);
CREATE INDEX idx_sentence_attempt_exercise ON sentence_attempt (exercise_id);
