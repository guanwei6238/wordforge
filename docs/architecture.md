# 架構

## 全貌

```
┌──────────────────────────────────────────────────┐
│ apps/desktop            React + TypeScript (Vite) │
│                         ↕ Tauri IPC (invoke)      │
│ apps/desktop/src-tauri  command 層：組裝、轉型別   │
└───────────────────────┬──────────────────────────┘
                        │
   ┌────────────────────┼────────────────────┐
   ▼                    ▼                    ▼
wordforge-core     wordforge-db        wordforge-llm
（純運算）          （SQLite）          （HTTP / 本機模型）
   ▲                    ▲
   │            wordforge-import
   │           （批次寫入 / 進度 / 中斷）
   │                    ▲
   └──── wordforge-dict ┘
          （格式解析）
```

## 模組邊界

| Crate | 負責 | 明確**不**負責 |
| --- | --- | --- |
| `wordforge-core` | FSRS 排程、覆蓋率計算、領域型別、文字正規化 | 任何 I/O、資料庫、網路、時鐘 |
| `wordforge-db` | SQLite schema、migration、查詢 | 演算法、業務規則 |
| `wordforge-dict` | 把外部字典格式解析成 `DictEntry` | 寫入資料庫、下載檔案 |
| `wordforge-import` | 批次 transaction、進度回報、中斷、容錯 | 解析格式、SQL |
| `wordforge-llm` | 供應商協定、prompt 模板 | 決定「該出什麼題」的教學邏輯 |
| `src-tauri` | command 註冊、狀態管理、錯誤轉字串 | 上述任何一項 |

### 為什麼核心層不准碰時鐘

`Scheduler::review()` 的 `now` 是參數而不是 `OffsetDateTime::now_utc()`。
這讓「連續答對八次，間隔要遞增」這種行為可以在毫秒內測完，
而不需要真的等八天。所有時間相關的決策都往外推到呼叫端。

## 資料流：一次複習

```
使用者按下「記得」
  → App.tsx grade(RATING.good)
  → invoke("review_card")
  → src-tauri: 重新從 DB 讀出卡片（不信任前端送來的狀態）
  → core: Scheduler::review(card, Good, now) → (新卡片, 複習紀錄)
  → db: 一個 transaction 同時更新 card 與寫入 review_log
```

前端送來的只有 `card_id` 與評分，卡片狀態一律以資料庫為準——
否則視窗開著放一天再按下按鈕，就會用過期的 stability 算出錯誤間隔。

## 資料流：匯入一份字典

一份完整的英文 Wiktionary 有上百萬筆、數 GB，逐筆 commit 會跑上好幾個小時，
所以匯入不是一個 for 迴圈：

```
1. dict   : 逐行串流解析 JSONL（不整份載入記憶體）
2. import : 每 1000 筆包成一個 transaction
3. import : 每 2000 筆回報一次進度（每筆都發事件會淹沒 UI）
4. db     : write_entry 對同一來源冪等——重匯不會讓釋義越疊越多
5. import : 每個批次結束時檢查取消旗標，已 commit 的批次保留
```

解析失敗的行只計入 `failed` 並跳過；資料庫錯誤才中止整批。
幾百萬行裡有幾行壞掉是常態，不該讓整份匯入白費。

## 資料流：產生一篇閱讀理解

這是唯一一條需要「生成後驗收」的流程：

```
1. db   : 取得已知詞集合（recognition 卡、review 狀態、stability ≥ 21 天）
2. core : coverage::unknown_token_budget(300 詞, 96%) → 12 個生詞額度
3. core : coverage::select_target_words(候選, 已知, 額度/2) → 6 個目標新詞
4. llm  : prompts::reading_comprehension(...) → 呼叫模型
5. core : coverage::analyze(產生的文章, 已知詞) → 實際覆蓋率
6.      : 落在目標帶內 → 存檔；否則帶著超標的詞 retry（最多 2 次）
```

第 5 步是關鍵：prompt 只能提高命中率，本地計算才是保證。
沒有這一步，「90% 法則」就只是一句寫在 prompt 裡的祈禱。

## 為什麼選 Tauri

- 後端邏輯用 Rust，跟 SQLite、字典解析、大檔串流處理的需求相符
- 打包體積約 10 MB 等級（Electron 動輒 150 MB 起跳），字典資料才是大宗
- UI 用 Web 技術，貢獻門檻低
- 系統 WebView，不必為了顯示介面附帶整個瀏覽器

代價是各平台 WebView 行為有差異，複雜的 UI 需要跨平台測試。

## 離線與線上的界線

| 功能 | 需要網路 |
| --- | --- |
| 背單字、複習、查字典、聽發音 | ❌ |
| 匯入字典 / 教材 | ❌（檔案已下載的情況下） |
| 閱讀理解、AI 對話、批改、文法出題 | ✅ 或改用本機 Ollama |

App 在完全沒有設定 LLM 的情況下必須可用。任何讓「沒有 API key 就不能開啟」
的設計都是錯的。

## 尚未決定

- **RAG 的向量檢索**：`material_chunk.embedding` 欄位已預留，但用哪個
  embedding 模型（本機 vs API）還沒決定。教材量小的情況下，
  先用關鍵字檢索 + 全文塞入可能就夠了。
- **TTS**：Piper（品質好、要下載模型）或 eSpeak NG（小、機械音）。
- **FSRS 個人化訓練**：需要移植 optimizer，或呼叫外部工具。
- **同步**：目前只有單機。若要做，優先考慮「使用者自備儲存空間」
  （WebDAV / 雲端硬碟資料夾），不架伺服器。
