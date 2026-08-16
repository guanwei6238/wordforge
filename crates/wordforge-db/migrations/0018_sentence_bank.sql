-- 句子存一份，「這個字有哪些句子」改成查出來的。
--
-- ## 原本的問題
--
-- `word_sentence` 是「一個字一列」，同一句話連到兩個字就存兩份本文。
-- 更要緊的是**連哪些字是在出題當下決定的**：只連那份練習指派的目標字。
-- 所以句子裡順帶用到的字一句都拿不到——`final` 出現在
-- 「before the final exam」裡，但那題練的是 `ahead`，查 `final` 什麼都沒有。
--
-- 而且那個決定是寫死的：今天才學的字，回頭也看不到三個月前做過、
-- 明明用到它的句子。
--
-- ## 改成兩張表
--
--   sentence         做過的句子本身，一句一列
--   sentence_lemma   這句出現了哪些詞條（倒排索引）
--
-- 索引**不限牌組**：句子裡每個查得到字典的詞都建一列。這樣「之後才學的字
-- 回頭看得到舊句子」才成立——那正是這次改動的重點，只挑牌組裡的字
-- 等於把同一個寫死換個地方。
--
-- ## 為什麼不是查詢時即時掃字串
--
--   * `LIKE '%run%'` 對不上 `ran`，而詞形變化正是這個功能的重點
--     （查 `ran` 要看得到練 `run` 時寫的句子）
--   * 全表掃描會隨句子數成長
--   * FTS5 的預設 tokenizer 不分中日文，這個專案不能假設目標語言是英文
--
-- 走 `base_form` 建索引則三個問題都沒有：詞形、片語、語言都交給字典決定。

CREATE TABLE sentence (
    id           INTEGER PRIMARY KEY,
    profile_id   INTEGER NOT NULL REFERENCES profile (id) ON DELETE CASCADE,
    exercise_id  INTEGER NOT NULL REFERENCES exercise (id) ON DELETE CASCADE,
    -- 這一句是那份練習的第幾題。翻譯題才有——閱讀與克漏字的句子不是
    -- 「一題」，對不回排程與批改結果。
    item_index   INTEGER,
    -- 目標語言那一句（學英文就是英文句）
    text         TEXT NOT NULL,
    -- 母語翻譯。閱讀文章對不齊時可能是整段，也可能沒有。
    translation  TEXT,
    -- translation / reading / cloze——UI 要說得出「這是你翻譯過的句子」
    -- 還是「這是你讀過的文章裡的一句」
    origin       TEXT NOT NULL,
    -- 這一句錯過幾次。累計在這裡而不是讀 `sentence_review`：那張表是排程，
    -- 句子寫對之後整列會被刪掉，而「錯過三次才寫對」正是練起來之後
    -- 最值得留著的訊號。
    misses       INTEGER NOT NULL DEFAULT 0,
    -- 這一句踩過哪些文法點（識別碼）。名稱在 `grammar_def` 裡查。
    grammar_points_json TEXT NOT NULL DEFAULT '[]',
    created_at   TEXT NOT NULL,
    -- 同一份練習裡同一句只留一份。原本的唯一鍵含 `lemma_id`，
    -- 那正是本文被重複存的原因。
    UNIQUE (profile_id, exercise_id, text)
);

-- 外鍵一定要有索引，否則刪練習時 CASCADE 會全表掃描
CREATE INDEX idx_sentence_exercise ON sentence (exercise_id);

-- 批改要照「第幾題」回頭更新 misses 與文法點
CREATE INDEX idx_sentence_item ON sentence (profile_id, exercise_id, item_index);

CREATE TABLE sentence_lemma (
    sentence_id  INTEGER NOT NULL REFERENCES sentence (id) ON DELETE CASCADE,
    lemma_id     INTEGER NOT NULL REFERENCES lemma (id) ON DELETE CASCADE,
    PRIMARY KEY (lemma_id, sentence_id)
) WITHOUT ROWID;

-- 「這個字有哪些句子」走上面的主鍵（lemma_id 開頭）。
-- 反方向是 CASCADE 用的：刪一句要把它的索引清掉。
CREATE INDEX idx_sentence_lemma_sentence ON sentence_lemma (sentence_id);

-- 舊資料搬過來。只搬得到「當初被連上的那些句子」，其餘由 backfill 補齊；
-- 這裡要保住的是 `misses` 與 `grammar_points_json`——那是使用者練出來的
-- 紀錄，重跑補不回來。
--
-- 同一句的多筆取任一即可：`mark_missed` 與 `add_grammar_points` 的 WHERE
-- 是 (exercise_id, item_index)，同一句的每一筆一向被一起更新，值必然相同。
INSERT INTO sentence (
    profile_id, exercise_id, item_index, text, translation, origin,
    misses, grammar_points_json, created_at
)
SELECT profile_id, exercise_id, MAX(item_index), text, MAX(translation), MAX(origin),
       MAX(misses), MAX(grammar_points_json), MIN(created_at)
FROM word_sentence
GROUP BY profile_id, exercise_id, text;

INSERT OR IGNORE INTO sentence_lemma (sentence_id, lemma_id)
SELECT s.id, w.lemma_id
FROM word_sentence w
  JOIN sentence s ON s.profile_id = w.profile_id
                 AND s.exercise_id = w.exercise_id
                 AND s.text = w.text;

DROP TABLE word_sentence;
