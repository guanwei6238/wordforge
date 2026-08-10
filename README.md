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
2. **LLM 可插拔**：AI 功能需要模型，你可以填自己的 API key（Anthropic / OpenAI 相容端點），
   或接本機 [Ollama](https://ollama.com) 完全離線。**沒有 LLM 時 App 仍然可用**，只是少了生成類功能。
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

## 專案狀態

🚧 **v0.2 開發中** — 已經可以匯入字典、查字典、用 FSRS 背單字。
AI 出題（v0.3）尚未接上。路線圖見 [`docs/roadmap.md`](docs/roadmap.md)，歡迎在 Issues 討論。

第一次使用：到「匯入」頁載入一份 [kaikki.org](https://kaikki.org/) 的 Wiktionary JSONL
或你自己的 CSV 單字表，就可以開始查字典與背單字。

## 文件

- [`docs/architecture.md`](docs/architecture.md) — 架構決策與模組邊界
- [`docs/data-model.md`](docs/data-model.md) — SQLite schema 與設計理由
- [`docs/srs.md`](docs/srs.md) — FSRS-5 排程演算法說明
- [`docs/comprehensible-input.md`](docs/comprehensible-input.md) — 90% 法則怎麼算、怎麼出題
- [`docs/dictionary-sources.md`](docs/dictionary-sources.md) — 字典來源與授權
- [`docs/roadmap.md`](docs/roadmap.md) — 路線圖

## 貢獻

歡迎 PR，請先讀 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 授權

[Apache License 2.0](LICENSE) © Wordforge Contributors

本專案僅散布程式碼。使用者匯入的字典、教材、音檔之授權，由使用者自行確認與遵守。
