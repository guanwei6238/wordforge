-- 句型：出題難度的另一半。
--
-- ## 為什麼要改這張表，而不是開一張新的
--
-- 難度一直只有詞彙那一半有依據：閱讀實算生詞覆蓋率，翻譯實際檢查指派的字
-- 有沒有用上。句法那一半完全沒有——prompt 只寫「句子要自然、日常」。
-- 結果是初學者拿到分詞構句與假設語氣，每題都錯，而系統對此一無所知。
--
-- 句型需要的東西（教材定義、可匯入、可編輯、掌握度排程）跟文法點一模一樣，
-- 而 `grammar_point` 是靠 `(profile_id, point)` 記錄的，**不是外鍵**
-- （見 0011 的說明：刪教材不該抹掉學習歷史）。所以句型只要有識別碼，
-- FSRS 排程、文法頁的進度顯示、匯入與編輯全部現成可用。
-- 開一張平行的新表等於把同樣的東西再寫一遍。
--
-- ## kind：為什麼要分開
--
-- 這張表同時服務兩件事，而它們要的粒度相反：
--
--   point   批改時的**錯誤標籤**。要粗——切太細的話每個標籤各錯一次，
--           FSRS 排不動，「最常錯的文法點」也被稀釋成一堆各錯一次的東西。
--   pattern 教學用的**句型**。要細——「條件句第二類」跟「條件句第三類」
--           是兩課，混成一個「conditionals」就練不到重點。
--
-- 把上百條課綱句型混進錯誤標籤的清單，會同時撐爆批改 prompt 並稀釋掉
-- 現有的排程。所以 `grammar_point_rule` 只列 kind='point'，
-- 難度上限只讀 kind='pattern'。
--
-- ## level_ordinal：為什麼不能只靠既有的 level
--
-- `level` 是自由文字（'A2'、'N4'、'國中七年級'），字串沒有順序，
-- 程式不能拿它比大小。要判斷「這個句型超過他的程度」就一定要有一個
-- 排得出先後的數字。`level` 繼續當顯示名稱，程式只認 ordinal。
--
-- **同一個語言只能有一套刻度在生效。** 現在不會撞：既有的 40 條種子是
-- kind='point' 的 CEFR，課綱句型是 kind='pattern'，而上限只看後者。
-- 哪天有人又匯入一套 CEFR 分級的**句型**，兩套刻度就會在同一個 kind
-- 底下打架，那時要加的是 scheme 欄位加上「用哪一套當進度刻度」的設定。
-- 寫在這裡是因為那個症狀是「難度莫名其妙亂跳」，看起來不像資料問題。
--
-- ## detectors_json：這才是驗收成立的地方
--
-- prompt 說「不要用超過這一級的句型」只是請求。真正擋得住的是出題之後
-- 拿 regex 在本地實跑一次——這跟覆蓋率、跟「指派的字有沒有用上」
-- 是同一個原則：凡是本地驗得到的，就不要只相信模型。
--
-- 格式（見 wordforge_core::patterns::Detector）：
--   [{"type": "regex", "value": "\\bif\\b[^.?!]*\\bwould\\b",
--     "positive": ["If I had money, I would buy it."],
--     "negative": ["I know if he would come."]}]
--
-- `positive` 不是選配。一條打錯字的 regex 什麼都比對不到，於是超綱檢查
-- **永遠通過**，而畫面上完全正常——跟拿 `strings`（預設只掃 ASCII）
-- 去找中文字串一樣，方法自己壞了，結論卻很有信心。匯入時會實跑控制組，
-- 過不了的那條不寫進來。

ALTER TABLE grammar_def ADD COLUMN kind TEXT NOT NULL DEFAULT 'point';
ALTER TABLE grammar_def ADD COLUMN level_ordinal INTEGER;
ALTER TABLE grammar_def ADD COLUMN detectors_json TEXT NOT NULL DEFAULT '[]';

-- 熱路徑：出題前要撈「這個語言、這個上限之內／之外的句型」。
-- 每出一題都會走一次，而句型清單會隨匯入長大。
--
-- 欄位順序就是 `pattern_defs` 的 ORDER BY，一個都不能少：少了 sort_order
-- 或 point，EXPLAIN QUERY PLAN 就會多一行
-- `USE TEMP B-TREE FOR RIGHT PART OF ORDER BY`——在 40 筆上看不出來，
-- 但這條清單的長度是由匯入的資料決定的，而那是使用者說了算。
CREATE INDEX idx_grammar_def_kind
    ON grammar_def (lang, kind, level_ordinal, sort_order, point);
