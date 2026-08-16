-- 「這個字我在哪句話裡用過」。
--
-- 複習單字時只看得到字典的釋義與例句，而那些例句是別人寫的。真正記得住的
-- 是**自己做過的那一句**：翻譯題裡寫過的句子、閱讀文章裡讀到的那一行。
-- 這張表把單字接回那些句子，複習頁與字典頁共用同一份資料。
--
-- ## 為什麼句子存在這裡，而不是查的時候去 payload 裡撈
--
-- 句子本文重複存了一份（原本在 `exercise.payload_json` 裡），這是刻意的：
--
--   * 閱讀文章的「第幾句」要靠切句得到，而切句規則會改。存位置的話，
--     規則一改，所有既有連結都指到別的句子上，而且完全看不出來。
--   * 查詢是「這個字有哪些句子」，走 payload 得把每一份練習都解析一次。
--
-- 練習被刪掉時這些句子一起 CASCADE：刪掉紀錄就是全部刪掉，
-- 不留一句沒有出處的殘影。
CREATE TABLE word_sentence (
    id           INTEGER PRIMARY KEY,
    profile_id   INTEGER NOT NULL REFERENCES profile (id) ON DELETE CASCADE,
    -- 哪個字。存 lemma 而不是字串：查 `ran` 要看得到練 `run` 時寫的句子。
    lemma_id     INTEGER NOT NULL REFERENCES lemma (id) ON DELETE CASCADE,
    exercise_id  INTEGER NOT NULL REFERENCES exercise (id) ON DELETE CASCADE,
    -- 目標語言那一句（學英文就是英文句）
    text         TEXT NOT NULL,
    -- 母語翻譯。閱讀文章對不齊時可能是整段，也可能沒有。
    translation  TEXT,
    -- translation / reading / cloze——UI 要說得出「這是你翻譯過的句子」
    -- 還是「這是你讀過的文章裡的一句」
    origin       TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    -- 同一份練習裡同一個字重複出現時只留一句
    UNIQUE (profile_id, lemma_id, exercise_id, text)
);

-- 「這個字有哪些句子」是唯一的熱查詢，而且要新的在前
CREATE INDEX idx_word_sentence_lemma ON word_sentence (profile_id, lemma_id, created_at DESC);

-- 外鍵一定要有索引，否則刪練習時 CASCADE 會全表掃描
CREATE INDEX idx_word_sentence_exercise ON word_sentence (exercise_id);

-- `lemma_id` 也是 CASCADE 的外鍵。上面那個索引以 profile_id 開頭，
-- 對「刪掉一個詞條」這個方向用不上——重匯字典時每刪一列都會全表掃描。
-- （`repo.rs` 那條掃 PRAGMA 的測試就是為了抓這種漏網的。）
CREATE INDEX idx_word_sentence_lemma_fk ON word_sentence (lemma_id);
