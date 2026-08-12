-- 文法點的「定義」：名稱、講解、例句。
--
-- 原本這份清單寫死在 `wordforge_core::grammar_points::ENGLISH_POINTS`，
-- 只有英文，而且使用者改不了。於是：
--
--   * 學日文的人拿到空清單——助詞、活用、敬語一個都沒有
--   * 想加一個自己常錯的點（比方說「時態一致」）沒有地方加
--   * 想調整某個點的講解也不行
--
-- 改成資料表之後，跟字典一樣「匯入什麼就能學什麼」：程式碼只提供
-- 英文那份當種子，其餘由使用者匯入或自己編輯。
--
-- 這張表存的是**定義**；`grammar_point`（0005）存的是**你的掌握狀態**
-- （FSRS 排程、對錯次數）。兩者刻意分開：定義是跨 profile 共用的教材，
-- 掌握狀態是每個人自己的。它們靠 (lang, point) 對應，不是外鍵——
-- 使用者刪掉一個定義不該連帶抹掉學習歷史。
CREATE TABLE grammar_def (
    id          INTEGER PRIMARY KEY,
    lang        TEXT NOT NULL,
    -- 受控識別碼（tense、articles…）。跟 grammar_point.point 對應。
    point       TEXT NOT NULL,
    -- 給使用者看的名稱，用母語寫（「時態」「冠詞 a / an / the」）
    name        TEXT NOT NULL,
    -- 母語講解。可以是空的——按「請 AI 講解」之後才有。
    explanation TEXT,
    -- 例句，JSON 陣列：[{"text": "目標語例句", "translation": "母語翻譯"}]
    --
    -- 用 JSON 而不是獨立的表：例句只會整組讀寫、從來不會單獨查詢，
    -- 拆一張表只是多一次 JOIN。
    examples_json TEXT NOT NULL DEFAULT '[]',
    -- 難度標示，由來源決定（CEFR 的 A2、JLPT 的 N4…）。沒有就留空。
    level       TEXT,
    -- 排序用。匯入時照檔案順序給，使用者可以再調。
    sort_order  INTEGER NOT NULL DEFAULT 0,
    -- 這筆從哪來：seed（程式碼種子）/ import（匯入）/ manual（自己加）
    origin      TEXT NOT NULL DEFAULT 'manual',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,

    UNIQUE (lang, point)
);

-- 熱路徑：開文法頁時列出某個語言的全部定義
CREATE INDEX idx_grammar_def_lang ON grammar_def (lang, sort_order);
