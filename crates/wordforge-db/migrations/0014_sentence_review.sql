-- 翻譯句子的複習排程。
--
-- 翻譯題原本是一次性的：做完、批改、結束。錯的那幾句沒有任何機制
-- 讓它們再回來——而那幾句正是他還不會的東西。整份「再做一次」也不對，
-- 已經對的三題重寫一遍只是浪費時間與額度。
--
-- 所以改成**一句一句排**，規則刻意保持得比 FSRS 簡單：
--
--   答錯 → 明天再出現
--   答對 → 從此不再出現（那一句練起來了）
--
-- 為什麼不用 FSRS：卡片排程算的是「這個字的記憶強度」，而一句翻譯
-- 練的是「這個句型 / 這個字在這個情境的用法」。同一句反覆練到會了
-- 就沒有再練的價值了——它不像單字需要長期維持。
--
-- 一句每天只出現一次也是刻意的：`due` 是日期粒度，今天做過的
-- 下一次最快是明天。當天反覆重寫同一句刷到全對，看起來是 100 分，
-- 實際上只是背下了剛剛看到的參考答案。
CREATE TABLE sentence_review (
    id           INTEGER PRIMARY KEY,
    profile_id   INTEGER NOT NULL REFERENCES profile (id) ON DELETE CASCADE,
    -- 句子屬於哪一份練習的第幾題。句子本文不存在這裡：
    -- 它在 exercise.payload_json 裡，存兩份只會有兩份互相漂移的真相。
    -- 練習被刪掉時這裡也跟著 CASCADE，符合「刪紀錄就是全部刪掉」。
    exercise_id  INTEGER NOT NULL REFERENCES exercise (id) ON DELETE CASCADE,
    item_index   INTEGER NOT NULL,
    -- 下次該出現的時間。今天做過的一律排到明天。
    due          TEXT NOT NULL,
    -- 最後一次練這句是什麼時候。用來擋「同一天又練一次」。
    last_review  TEXT NOT NULL,
    -- 錯過幾次。UI 可以用它說「這句你錯過三次了」，也是之後要不要
    -- 拉長間隔的依據。
    misses       INTEGER NOT NULL DEFAULT 0,
    UNIQUE (profile_id, exercise_id, item_index)
);

-- 「今天有哪幾句要練」是這張表的唯一熱查詢
CREATE INDEX idx_sentence_review_due ON sentence_review (profile_id, due);

-- 外鍵一定要有索引，否則刪練習時 CASCADE 會全表掃描
CREATE INDEX idx_sentence_review_exercise ON sentence_review (exercise_id);
