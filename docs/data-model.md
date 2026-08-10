# 資料模型

完整 schema 見 [`crates/wordforge-db/migrations/0001_init.sql`](../crates/wordforge-db/migrations/0001_init.sql)。
這份文件說明**為什麼**這樣設計。

## 三個分層

```
字典層  dict_source → lemma → sense → example
                        ↓        pronunciation
                   surface_form

學習層  profile → card → review_log

教材層  material → material_chunk / material_vocab
        exercise → attempt
        conversation → message
```

字典層與學習層透過 `card.lemma_id` 相連。**換一份字典不該弄丟複習歷程**，
所以學習進度不存在字典表上。

## 關鍵決策

### lemma 與 surface_form 分開

`run` 是 lemma，`ran` / `running` / `runs` 是 surface form。
分開的理由：使用者在文章中讀到 `ran` 時，複習進度應該記在 `run` 上。

一個 surface form 可能對到多個 lemma（`saw` = see 的過去式，也是「鋸子」）。
目前 `find_by_form` 回傳詞頻最高的那個；精確消歧需要上下文，留給日後處理。

### 一個字拆成多張卡

`card` 的唯一鍵是 `(profile_id, lemma_id, kind)`。四種 kind：

| kind | 訓練 | 提示 → 作答 |
| --- | --- | --- |
| `recognition` | 閱讀 | 看到 `apple` → 想出意思 |
| `recall` | 寫作口說 | 看到「蘋果」→ 想出 `apple` |
| `listening` | 聽力 | 聽到發音 → 認出字 |
| `spelling` | 拼寫 | 聽到發音 → 拼出來 |

「看得懂」和「講得出」是不同強度的記憶，混成一張卡會讓 FSRS 的 stability 失準。
預設只開 `recognition`，其餘由使用者選擇。

### 時間一律存固定寬度的 UTC 字串

格式：`2026-08-10T12:34:56.000000Z`

`due` 欄位靠字串比較排序。如果有的值帶毫秒、有的不帶，
`'...:00Z' > '...:00.5Z'` 會成立（因為 `Z` 的碼位大於 `.`），排序就錯了。
固定六位微秒可以杜絕這個問題。轉換函數在 `repo.rs` 的 `ts` 模組。

### pos 用空字串而不是 NULL

`UNIQUE (lang, text, pos)` 在 SQLite 中，兩個 `NULL` 不算相同，
去重會失效。未分類的詞性一律存 `''`。

### 練習內容用 JSON 欄位

`exercise.payload_json` 不拆成正規化的表。題型會一直長出新的
（克漏字、配對、排序、聽寫…），每加一種就要一次 migration 不划算。
需要查詢的欄位（`kind`、`coverage`、`created_at`）才拉出來當獨立欄位。

### review_log 保留所有 FSRS 輸入

`stability` / `difficulty` / `elapsed_days` / `scheduled_days` 全都存。
這些不是為了顯示，而是為了將來能用**你自己的**複習歷程重新訓練 FSRS 權重。
少存一個欄位，之後就沒辦法回頭訓練。

### 音檔不進資料庫

`pronunciation.audio_path` 存相對於 app 資料目錄的路徑。
一份完整的 Wiktionary 音檔集可以到數 GB，塞進 SQLite 會讓資料庫檔案
難以備份，也拖慢每一次查詢。

## 「已知詞」的定義

90% 法則需要一個明確的分母。目前的定義寫在
`repo::cards::known_lemma_ids`：

> `recognition` 卡 + `state = 'review'` + `stability >= 21 天`

也就是「已經畢業到長期複習，而且三週不看也還記得」。
門檻是參數，使用者可以調——嚴格一點會讓產生的文章更簡單。

## 索引

只建了三個熱路徑索引：

- `idx_card_due (profile_id, suspended, due)` — 每次開 App 的第一個查詢
- `idx_surface_lookup (lang, normalized)` — 分析文章時每個詞都要查一次
- `idx_lemma_freq (lang, freq_rank)` — 挑選要教的新詞

其餘等出現實際瓶頸再加。過早建索引只會拖慢字典匯入。

## 全文檢索

尚未加入。FTS5 是否可用取決於編譯進來的 SQLite 是否啟用該模組，
在跨平台打包時是個變數。目前用 `normalized` 欄位加索引做前綴比對，
等真的需要「在釋義中搜尋」時再處理。
