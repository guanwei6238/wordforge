-- Wordforge 初始 schema
--
-- 設計原則：
--  1. 字典資料（lemma / sense / example / pronunciation）與學習資料（card / review_log）
--     完全分離。換一份字典不該弄丟你的複習歷程。
--  2. 所有時間欄位一律存 RFC 3339 UTC 字串，避免跨時區與夏令時間的坑。
--  3. 每筆外來資料都掛 dict_source，才能在 UI 上正確標示授權與出處。

-- ---------------------------------------------------------------- 字典層

-- 匯入來源。授權欄位不是裝飾用的：UI 需要據此顯示出處，
-- 匯出教材時也要據此判斷哪些內容可以帶走。
CREATE TABLE dict_source (
    id           INTEGER PRIMARY KEY,
    slug         TEXT NOT NULL UNIQUE,          -- 'wiktionary-en', 'cc-cedict'
    name         TEXT NOT NULL,
    license      TEXT,                          -- 'CC BY-SA 4.0'
    attribution  TEXT,                          -- 顯示在 UI 的出處字串
    homepage     TEXT,
    version      TEXT,                          -- dump 日期或版本
    imported_at  TEXT NOT NULL
);

-- 詞條。一個 lemma = 字典會收錄的那個形式 + 詞性。
CREATE TABLE lemma (
    id          INTEGER PRIMARY KEY,
    lang        TEXT NOT NULL,                  -- BCP 47，如 'en'
    text        TEXT NOT NULL,                  -- 原始拼寫，保留大小寫與重音
    normalized  TEXT NOT NULL,                  -- 比對用的正規化鍵值
    -- 'noun' / 'verb' / ...；用空字串而非 NULL 表示未分類，
    -- 因為 SQLite 的 UNIQUE 不會把兩個 NULL 視為相同，那會讓去重失效
    pos         TEXT NOT NULL DEFAULT '',
    freq_rank   INTEGER,                        -- 詞頻排名，越小越常用；90% 法則靠它排序
    cefr        TEXT,                           -- 'A1'..'C2'，若來源有提供
    UNIQUE (lang, text, pos)
);
CREATE INDEX idx_lemma_normalized ON lemma (lang, normalized);
CREATE INDEX idx_lemma_freq       ON lemma (lang, freq_rank);

-- 表面形 → lemma。詞形還原查這張表，因此讀取遠多於寫入。
CREATE TABLE surface_form (
    id          INTEGER PRIMARY KEY,
    lang        TEXT NOT NULL,
    form        TEXT NOT NULL,
    normalized  TEXT NOT NULL,
    lemma_id    INTEGER NOT NULL REFERENCES lemma (id) ON DELETE CASCADE,
    tag         TEXT NOT NULL DEFAULT '',       -- 'plural' / 'past' / 'comparative'
    UNIQUE (lang, normalized, lemma_id, tag)
);
CREATE INDEX idx_surface_lookup ON surface_form (lang, normalized);

-- 釋義
CREATE TABLE sense (
    id           INTEGER PRIMARY KEY,
    lemma_id     INTEGER NOT NULL REFERENCES lemma (id) ON DELETE CASCADE,
    source_id    INTEGER REFERENCES dict_source (id) ON DELETE SET NULL,
    gloss        TEXT NOT NULL,                 -- 目標語定義
    gloss_lang   TEXT NOT NULL,
    translation  TEXT,                          -- 母語翻譯
    register     TEXT,                          -- 'formal' / 'slang'
    domain       TEXT,                          -- 'medicine' / 'law'
    sort_order   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_sense_lemma ON sense (lemma_id, sort_order);

-- 例句。可掛在 sense 上，也可只掛 lemma。
CREATE TABLE example (
    id           INTEGER PRIMARY KEY,
    lemma_id     INTEGER NOT NULL REFERENCES lemma (id) ON DELETE CASCADE,
    sense_id     INTEGER REFERENCES sense (id) ON DELETE CASCADE,
    source_id    INTEGER REFERENCES dict_source (id) ON DELETE SET NULL,
    text         TEXT NOT NULL,
    translation  TEXT
);
CREATE INDEX idx_example_lemma ON example (lemma_id);

-- 發音。audio_path 存相對於 app 資料目錄的路徑，資料庫本身不塞二進位。
CREATE TABLE pronunciation (
    id             INTEGER PRIMARY KEY,
    lemma_id       INTEGER NOT NULL REFERENCES lemma (id) ON DELETE CASCADE,
    source_id      INTEGER REFERENCES dict_source (id) ON DELETE SET NULL,
    accent         TEXT,                        -- 'uk' / 'us' / 'au'
    ipa            TEXT,
    audio_path     TEXT,
    audio_license  TEXT,
    is_synthetic   INTEGER NOT NULL DEFAULT 0   -- 1 = 本機 TTS 合成，非真人錄音
);
CREATE INDEX idx_pron_lemma ON pronunciation (lemma_id);

-- ---------------------------------------------------------------- 學習者層

CREATE TABLE profile (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    native_lang   TEXT NOT NULL,
    target_lang   TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    -- FSRS 權重、每日新卡上限、目標留存率等，以 JSON 存放方便演進
    settings_json TEXT NOT NULL DEFAULT '{}'
);

-- 學習卡。同一個字依 kind 拆成多張，分別追蹤不同能力的記憶強度。
CREATE TABLE card (
    id              INTEGER PRIMARY KEY,
    profile_id      INTEGER NOT NULL REFERENCES profile (id) ON DELETE CASCADE,
    lemma_id        INTEGER NOT NULL REFERENCES lemma (id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,              -- recognition / recall / listening / spelling
    state           TEXT NOT NULL,              -- new / learning / review / relearning
    step            INTEGER NOT NULL DEFAULT 0,
    stability       REAL,
    difficulty      REAL,
    due             TEXT NOT NULL,
    last_review     TEXT,
    reps            INTEGER NOT NULL DEFAULT 0,
    lapses          INTEGER NOT NULL DEFAULT 0,
    scheduled_days  INTEGER NOT NULL DEFAULT 0,
    suspended       INTEGER NOT NULL DEFAULT 0,
    UNIQUE (profile_id, lemma_id, kind)
);
-- 每次開啟 App 的第一個查詢都是「今天要複習什麼」，這個索引是熱路徑
CREATE INDEX idx_card_due ON card (profile_id, suspended, due);

-- 完整複習歷程。保留所有 FSRS 輸入欄位，日後才能用個人資料重新訓練權重。
CREATE TABLE review_log (
    id              INTEGER PRIMARY KEY,
    card_id         INTEGER NOT NULL REFERENCES card (id) ON DELETE CASCADE,
    rating          INTEGER NOT NULL,           -- 1..4
    state           TEXT NOT NULL,              -- 複習「之前」的狀態
    stability       REAL NOT NULL,              -- 複習「之後」的記憶狀態
    difficulty      REAL NOT NULL,
    elapsed_days    INTEGER NOT NULL,
    scheduled_days  INTEGER NOT NULL,
    reviewed_at     TEXT NOT NULL,
    duration_ms     INTEGER
);
CREATE INDEX idx_review_card ON review_log (card_id, reviewed_at);
CREATE INDEX idx_review_time ON review_log (reviewed_at);

-- ---------------------------------------------------------------- 教材層

-- 使用者匯入的教材（課本、文章、電子書）。
-- license_note 由使用者填寫，提醒自己這份教材能不能分享出去。
CREATE TABLE material (
    id            INTEGER PRIMARY KEY,
    profile_id    INTEGER NOT NULL REFERENCES profile (id) ON DELETE CASCADE,
    title         TEXT NOT NULL,
    kind          TEXT NOT NULL,                -- textbook / article / epub / pdf / subtitle
    lang          TEXT NOT NULL,
    source_path   TEXT,
    license_note  TEXT,
    created_at    TEXT NOT NULL
);

-- 教材切塊，供 RAG 檢索用。embedding 存 f32 陣列的原始位元組。
CREATE TABLE material_chunk (
    id           INTEGER PRIMARY KEY,
    material_id  INTEGER NOT NULL REFERENCES material (id) ON DELETE CASCADE,
    ord          INTEGER NOT NULL,
    text         TEXT NOT NULL,
    token_count  INTEGER,
    embedding    BLOB,
    UNIQUE (material_id, ord)
);

-- 教材詞表：這本書出現了哪些字、各幾次。出題時用來「只考課本的字」。
CREATE TABLE material_vocab (
    material_id  INTEGER NOT NULL REFERENCES material (id) ON DELETE CASCADE,
    lemma_id     INTEGER NOT NULL REFERENCES lemma (id) ON DELETE CASCADE,
    count        INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (material_id, lemma_id)
);

-- ---------------------------------------------------------------- 練習層

-- LLM 產生的練習。payload_json 依 kind 有不同結構，
-- 刻意不拆表：題型會一直長出新的，用 JSON 才不會每加一種就要 migration。
CREATE TABLE exercise (
    id                  INTEGER PRIMARY KEY,
    profile_id          INTEGER NOT NULL REFERENCES profile (id) ON DELETE CASCADE,
    kind                TEXT NOT NULL,          -- reading / cloze / grammar / translation / writing
    material_id         INTEGER REFERENCES material (id) ON DELETE SET NULL,
    payload_json        TEXT NOT NULL,
    target_lemmas_json  TEXT NOT NULL DEFAULT '[]',
    coverage            REAL,                   -- 產生當下的已知詞覆蓋率，用於驗收 90% 法則
    model               TEXT,                   -- 產生用的模型，方便日後比較品質
    created_at          TEXT NOT NULL
);
CREATE INDEX idx_exercise_profile ON exercise (profile_id, created_at);

-- 作答與批改結果
CREATE TABLE attempt (
    id             INTEGER PRIMARY KEY,
    exercise_id    INTEGER NOT NULL REFERENCES exercise (id) ON DELETE CASCADE,
    answer_json    TEXT NOT NULL,
    score          REAL,
    feedback_json  TEXT,
    created_at     TEXT NOT NULL
);
CREATE INDEX idx_attempt_exercise ON attempt (exercise_id);

-- ---------------------------------------------------------------- 對話層

CREATE TABLE conversation (
    id          INTEGER PRIMARY KEY,
    profile_id  INTEGER NOT NULL REFERENCES profile (id) ON DELETE CASCADE,
    topic       TEXT,
    level       TEXT,                           -- 對話設定的難度上限
    created_at  TEXT NOT NULL
);

CREATE TABLE message (
    id               INTEGER PRIMARY KEY,
    conversation_id  INTEGER NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    role             TEXT NOT NULL,             -- user / assistant / system
    content          TEXT NOT NULL,
    -- AI 針對這句話的糾正（錯在哪、正確說法、涉及哪個文法點）
    corrections_json TEXT,
    created_at       TEXT NOT NULL
);
CREATE INDEX idx_message_conv ON message (conversation_id, created_at);
