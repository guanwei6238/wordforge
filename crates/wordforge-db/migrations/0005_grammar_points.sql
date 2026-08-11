-- 文法點的掌握狀態。
--
-- 原本是每次出題就掃最近 20 筆批改結果、在 Rust 端數 grammar_point 出現幾次。
-- 那個做法有三個問題：
--   1. 只知道「錯過幾次」，不知道「已經練到什麼程度」
--   2. 沒有複習間隔——練熟的文法點還是會一直被挑出來
--   3. 餵給模型的是「最常錯的前幾個」，題數一多就是在浪費 token
--
-- 改成獨立的表，並套用跟單字卡同一套 FSRS：錯了間隔縮短、對了拉長。
-- 出題時只取「今天到期」的幾個，token 用量固定而且練的都是真的該練的。
CREATE TABLE grammar_point (
    id             INTEGER PRIMARY KEY,
    profile_id     INTEGER NOT NULL REFERENCES profile (id) ON DELETE CASCADE,
    -- 一致的英文術語：tense、articles、subject-verb agreement…
    point          TEXT NOT NULL,

    -- FSRS 狀態，欄位與 card 一致
    state          TEXT NOT NULL DEFAULT 'new',
    step           INTEGER NOT NULL DEFAULT 0,
    stability      REAL,
    difficulty     REAL,
    due            TEXT NOT NULL,
    last_review    TEXT,
    scheduled_days INTEGER NOT NULL DEFAULT 0,

    -- 累計統計，用來顯示「這個文法點你對了幾次錯了幾次」
    error_count    INTEGER NOT NULL DEFAULT 0,
    correct_count  INTEGER NOT NULL DEFAULT 0,
    first_seen     TEXT NOT NULL,

    UNIQUE (profile_id, point)
);

-- 出題時的熱路徑：這位學習者現在有哪些文法點到期了
CREATE INDEX idx_grammar_due ON grammar_point (profile_id, due);
