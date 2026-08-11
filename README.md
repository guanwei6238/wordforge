# Wordforge

> 開源、離線優先的語言學習桌面應用程式。
> 用你自己的字典、你自己的教材、你自己的 LLM，打造專屬於你的學習迴圈。

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/guanwei6238/wordforge/actions/workflows/ci.yml/badge.svg)](https://github.com/guanwei6238/wordforge/actions/workflows/ci.yml)

---

## 這是什麼

市面上的語言學習 App 各做一半：Anki 只管背單字、AI 聊天機器人不知道你會哪些字、閱讀器不會幫你複習。
Wordforge 把它們接成一條線：

**「你會的單字」是一份本地資料庫，所有功能都圍繞它運轉。**

| 功能 | 說明 |
| --- | --- |
| 📖 **字典匯入** | 匯入詞典資料（詞形、詞性、釋義、例句、IPA、發音音檔），作為單字的權威來源 |
| 🧠 **間隔重複** | 內建 FSRS-5 排程演算法（比 Anki 傳統 SM-2 更省時間），完整記錄複習歷程 |
| 📰 **90% 法則閱讀** | 根據你已掌握的詞彙產生閱讀理解文章，把生詞率控制在可理解輸入的甜蜜點 |
| 💬 **AI 對話練習** | 以你的詞彙量為上限對話，偏離時自動修正並把新詞加入牌組 |
| ✍️ **翻譯與寫作批改** | 出題、批改、逐句回饋，錯誤自動回饋到單字與文法弱點 |
| 🔤 **文法練習** | 依據錯誤紀錄針對弱點出題 |
| 📚 **自訂教材** | 匯入國中英文課本、原文書、文章，讓 AI **只依據該教材**出題（RAG） |

### 設計原則

1. **離線優先**：字典、單字庫、複習排程、發音全部在本機 SQLite + 本地檔案。沒有網路也能背單字。
2. **LLM 可插拔**：AI 功能需要模型，有四種接法——直接用本機已登入的
   `claude -p` / `codex exec`（**不必為同一個模型再開一份 API 帳單**）、
   填自己的 API key、或接本機 [Ollama](https://ollama.com) 完全離線。
   **沒有 LLM 時 App 仍然可用**，只是少了生成類功能。
3. **不綁架資料**：所有資料都在一個 SQLite 檔案裡，可匯出成 Anki `.apkg` / CSV，隨時搬走。
4. **尊重授權**：專案本身不散布任何有版權的字典內容，詳見下方說明。

---

## ⚠️ 關於字典資料的重要說明

Cambridge、Oxford、朗文等商業字典的釋義、例句與錄音**受著作權保護**，
爬取其網站或把內容打包進開源專案會有法律風險，也會讓專案無法被安心散布。

因此 Wordforge 的做法是：**程式碼提供匯入器，資料由使用者自行取得。**

專案內建的匯入器支援這些**授權明確、可自由散布**的來源：

| 來源 | 內容 | 授權 |
| --- | --- | --- |
| [Wiktionary / kaikki.org](https://kaikki.org/) | 釋義、詞性、詞形變化、IPA、例句 | CC BY-SA 4.0 |
| [Wikimedia Commons](https://commons.wikimedia.org/) | 真人發音音檔 | CC BY-SA / CC0 |
| [Open English WordNet](https://en-word.net/) | 同義詞、語意關係 | CC BY 4.0 |
| [CC-CEDICT](https://cc-cedict.org/) | 中英對照 | CC BY-SA 3.0 |
| [SUBTLEX / wordfreq](https://github.com/rspeer/wordfreq) | 詞頻排名（90% 法則的基礎） | 各自標示 |
| 通用 CSV / JSONL / StarDict | 你自己合法擁有的字典 | 由你負責 |

發音若字典未提供，可用本機 TTS（[Piper](https://github.com/rhasspy/piper) / eSpeak NG）即時合成。

詳見 [`docs/dictionary-sources.md`](docs/dictionary-sources.md)。

---

## 技術架構

```
┌─────────────────────────────────────────────┐
│  apps/desktop        Tauri v2 + React + TS  │  ← UI（桌面 App，Win/macOS/Linux）
└───────────────────────┬─────────────────────┘
                        │ Tauri command (IPC)
┌───────────────────────┴─────────────────────┐
│  wordforge-core   領域模型 / FSRS 排程 /     │
│                   90% 法則覆蓋率計算         │  ← 純 Rust，無 I/O，好測試
├─────────────────────────────────────────────┤
│  wordforge-db     SQLite (sqlx) + migrations │
├─────────────────────────────────────────────┤
│  wordforge-dict   字典格式解析器             │
│                   Wiktionary / CSV / 詞頻表  │
├─────────────────────────────────────────────┤
│  wordforge-import 批次匯入：transaction /    │
│                   進度回報 / 中斷 / 容錯     │
├─────────────────────────────────────────────┤
│  wordforge-llm    LLM 供應商抽象層           │
│                   Anthropic / OpenAI / Ollama│
└─────────────────────────────────────────────┘
```

核心邏輯與 UI 完全解耦，日後要做 CLI 或手機版不必重寫。

---

## 開發環境

需求：**Rust 1.85+**、**Node.js 20+**、**npm**。

Linux 另需 Tauri 的系統依賴：

```bash
sudo apt install libwebkit2gtk-4.1-dev librsvg2-dev patchelf libxdo-dev \
    build-essential curl wget file libssl-dev
```

> **不要裝 `libappindicator3-dev`**。那是 Tauri v1 的依賴，會跟現在系統上的
> `libayatana-appindicator3-1` 衝突。Tauri v2 只有在需要系統列圖示時才需要
> `libayatana-appindicator3-dev`，本專案沒有用到。

啟動：

```bash
# 只驗證 Rust 核心（不需 Tauri 系統依賴）
cargo test

# 完整桌面 App（開發模式）
cd apps/desktop && npm install && npm run tauri dev
```

---

## 命令列工具

首次載入一份完整字典要處理幾 GB 資料，跑在 GUI 裡會綁住視窗好幾十分鐘。
CLI 寫的是**同一個資料庫檔案**，匯入完打開 App 立刻就看得到：

```bash
cargo build --release -p wordforge-cli

# 英漢字典（含中文翻譯、音標、國中/高中/多益等考試標籤）
curl -LO https://raw.githubusercontent.com/skywind3000/ECDICT/master/ecdict.csv
./target/release/wordforge import ecdict ecdict.csv

# Wiktionary：補上英文定義與例句，與上面那份並存
./target/release/wordforge import wiktionary kaikki-en.jsonl --lang en

./target/release/wordforge stats           # 看字典規模與來源授權
./target/release/wordforge search run      # 查一個字
./target/release/wordforge path            # 資料庫在哪

# 依考試範圍批次建卡（依詞頻由常用排到罕見）
./target/release/wordforge deck tags                        # 有哪些範圍、各幾個字
./target/release/wordforge deck add --tag zk --limit 500    # 國中會考範圍前 500 個常用字
```

`zk` 國中會考、`gk` 學測、`cet4`/`cet6`、`ky` 考研、`toefl`、`ielts`、`gre`、
`oxford3000` 牛津核心三千——這些標籤來自 ECDICT。

## 專案狀態

**v1.0.1** — 匯入字典、查字典、FSRS 背單字、分級測驗、真人發音、AI 出題
（翻譯 / 閱讀 / 文法）、自訂教材都能用了。批改後會把你不會的字**自動排進複習**。
路線圖見 [`docs/roadmap.md`](docs/roadmap.md)，歡迎在 Issues 討論。

第一次使用請看[使用手冊](docs/manual.md)。簡單說：先匯入一份字典
（App 開啟後會有引導），然後就可以開始背單字。

### ⚠️ 這份程式碼沒有經過任何程式碼審查

講清楚一點：**程式碼是 AI 寫的，作者沒有逐行看過。**

作者做的是**驗收功能**——打開來用、確認該動的有動、遇到問題回報再修。
測試有 300 多項、CI 是綠的、clippy 沒有警告。但那些都是同一個 AI
自己寫的測試，驗的是它自己想得到要驗的東西。

所以下面這些事**沒有任何人檢查過**：

- 架構決策合不合理
- 邊界條件與錯誤處理有沒有漏
- 有沒有安全問題
- 效能會不會在某些情況下爆掉

一個真實的例子：v1.0.0 所有測試都過、CI 全綠，但裝起來從應用程式選單一開，
AI 後端就偵測不到——因為沒有任何一項測試是「用 GUI 那種最小 PATH 啟動」的。
那個 bug 要真的裝起來用才會浮出來。**功能驗收抓得到的東西，就是那麼多。**

所以：

- **請預期會遇到 bug**，尤其是在作者沒有的環境上（別的發行版、macOS、Windows）
- 遇到了請開 [Issue](https://github.com/guanwei6238/wordforge/issues)，
  貼上錯誤訊息與你的環境
- **直接發 PR 修掉更好**，不必先問。小修正不需要事前討論，
  改動比較大的話開個 Issue 聊一下方向比較不會白做工
- 沒把握修對也沒關係，附上重現步驟的 PR 一樣有價值
- **看得懂 Rust 的話，純粹來 review 也非常歡迎**——這個專案現在最缺的就是這個

要動手的話讀 [CONTRIBUTING.md](CONTRIBUTING.md)：怎麼跑測試、怎麼下 commit 訊息都寫在那。

## 文件

- [`docs/manual.md`](docs/manual.md) — **使用手冊**（給使用者，其他都是給開發者的）
- [`docs/architecture.md`](docs/architecture.md) — 架構決策與模組邊界
- [`docs/data-model.md`](docs/data-model.md) — SQLite schema 與設計理由
- [`docs/srs.md`](docs/srs.md) — FSRS-5 排程演算法說明
- [`docs/comprehensible-input.md`](docs/comprehensible-input.md) — 90% 法則怎麼算、怎麼出題
- [`docs/dictionary-sources.md`](docs/dictionary-sources.md) — 字典來源與授權
- [`docs/roadmap.md`](docs/roadmap.md) — 路線圖

## 貢獻

**歡迎 PR，尤其是修 bug 的。** 這份程式碼是 AI 寫的、沒有人逐行審查過
（見上面的[專案狀態](#專案狀態)），所以你發現的問題**很可能是真的問題，
不是你用錯**。

**純粹來 review 也歡迎**，不必附上修正。指出「這裡看起來不對」本身就有價值。

請先讀 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 授權

[Apache License 2.0](LICENSE) © Wordforge Contributors

本專案僅散布程式碼。使用者匯入的字典、教材、音檔之授權，由使用者自行確認與遵守。
