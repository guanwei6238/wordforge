-- 情境主題：出題時用來輪換題材。
--
-- 原本寫死在 `wordforge_core::practice::TOPICS`，十二個固定的項目，
-- 使用者改不了。問題跟文法點（0011）當初一模一樣：
--
--   * 準備多益的人不需要「校園生活：課程、考試、社團」
--   * 醫生想練的科別情境一個都沒有
--   * 想加一個自己的題材沒有地方加
--
-- 而且那份清單是給閱讀寫的，翻譯沿用時會拿到「報導一則虛構但合理的
-- 地方新聞」這種**體裁**當情境——出翻譯題時是歪的。所以這裡多一個
-- `kinds`：這個主題適合哪些題型。
--
-- 跟 `grammar_def` 一樣照 `lang`（目標語言）分組，程式碼只提供種子。
CREATE TABLE topic (
    id         INTEGER PRIMARY KEY,
    lang       TEXT NOT NULL,
    -- 給模型看的主題描述，用母語寫（「旅行：訂房、機場、迷路」）。
    -- 它會直接進 prompt，所以寫得具體一點比較有用。
    text       TEXT NOT NULL,
    -- 適用的題型，JSON 陣列（["reading","cloze"]）。
    -- **空陣列表示全部題型都適用**，那是最常見的情況。
    --
    -- 用 JSON 而不是關聯表：這個欄位只會整組讀寫，
    -- 過濾在 Rust 那邊做（主題總共幾十個，不是需要索引的規模）。
    kinds_json TEXT NOT NULL DEFAULT '[]',
    -- seed（程式碼種子）/ import（匯入）/ manual（自己加）
    origin     TEXT NOT NULL DEFAULT 'manual',
    -- 排序用，也決定輪換的起點順序
    sort_order INTEGER NOT NULL DEFAULT 0,
    -- 關掉但不刪除。刪掉之後種子補齊不會把它加回來（版號只跑一次），
    -- 但使用者想暫時停用某個題材時，關掉比刪掉容易反悔。
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,

    UNIQUE (lang, text)
);

-- 熱路徑：出題時撈某個語言可用的主題
CREATE INDEX idx_topic_lang ON topic (lang, sort_order);
