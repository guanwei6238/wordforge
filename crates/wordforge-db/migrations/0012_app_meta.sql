-- 跨 profile 的小狀態：「這個資料庫已經做過什麼」。
--
-- 目前只有一個用途：記下 `grammar_def` 已經補到哪一版種子。
--
-- 為什麼需要它：`seed_defs` 原本只在「一筆都沒有」時才寫入，所以
-- 種子清單改了之後，早就用過的資料庫永遠看不到新增的文法點——
-- 只有全新安裝的人拿得到。
--
-- 而「每次啟動都補齊缺的」也不行：使用者刪掉一個用不到的點之後，
-- 下次開 App 它又回來了，而且怎麼刪都刪不掉。記下版號才能只補一次。
--
-- 不放在 profile.settings_json：那是每個 profile 各一份，而
-- grammar_def 是照語言存的、跨 profile 共用。放進去的話兩個 profile
-- 學同一個語言時會各補一次。
CREATE TABLE app_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
