# 字典來源與授權

## 為什麼不能直接用 Cambridge / Oxford

商業字典的**釋義、例句、錄音**都是受著作權保護的原創內容。
單字本身（`apple` 這個字）不受保護，但「A round fruit with red or green skin」
這句定義是編輯寫出來的作品。

因此：

| 做法 | 可行性 |
| --- | --- |
| 爬取 dictionary.cambridge.org 並打包進 repo | ❌ 侵權，且違反其使用條款 |
| 在 App 內爬取，資料存在使用者電腦 | ⚠️ 灰色地帶，且違反使用條款 |
| 提供匯入器，使用者自行取得授權明確的資料 | ✅ 本專案的做法 |
| 使用者自己購買的電子字典（StarDict/MDict）匯入自用 | ✅ 由使用者負責 |

開源專案一旦夾帶侵權資料，任何人都不敢用、不敢 fork，
Linux 發行版也不會收錄。這個限制反而讓專案更好活。

## 建議的來源

### Wiktionary（首選）

- **內容**：釋義、詞性、詞形變化、IPA、例句、同義詞
- **授權**：CC BY-SA 4.0（需標示出處，衍生作品需同樣授權）
- **取得**：[kaikki.org](https://kaikki.org/) 提供整理好的 JSONL

```bash
wget https://kaikki.org/dictionary/English/kaikki.org-dictionary-English.jsonl
```

匯入器：[`wordforge_dict::kaikki`](../crates/wordforge-dict/src/kaikki.rs)

英文版約 100 萬個詞條、數 GB，逐行串流解析，不會吃光記憶體。

### Wikimedia Commons（發音）

- **內容**：真人錄製的單字發音
- **授權**：多為 CC BY-SA 或 CC0，**逐檔標示**
- kaikki 的 `sounds` 欄位已帶 `ogg_url` / `mp3_url`

音檔是否下載由使用者決定——完整下載可能數 GB。
`pronunciation.audio_license` 欄位會保留每個檔案的授權。

### Open English WordNet

- **內容**：同義詞、上下位關係、語意網路
- **授權**：CC BY 4.0
- **用途**：出題時產生合理的干擾選項（同義但不完全相同的字）

### CC-CEDICT

- **內容**：中英對照
- **授權**：CC BY-SA 3.0
- **用途**：中文母語者的翻譯欄位

### 詞頻表

90% 法則排序新詞的依據。可選：

- [wordfreq](https://github.com/rspeer/wordfreq)（多語言，MIT + 資料各自標示）
- SUBTLEX 系列（電影字幕語料，學術用途免費）
- Google Books Ngrams（CC BY 3.0）

匯入器：[`wordforge_dict::freq`](../crates/wordforge-dict/src/freq.rs)

### 自製 CSV

從課本抄下來的單字表、Anki 匯出的牌組：

```csv
word,pos,translation,gloss,example,ipa,cefr
apple,noun,蘋果,A round fruit,I ate an apple.,/ˈæp.əl/,A1
```

只有 `word` 必填。匯入器：[`wordforge_dict::tabular`](../crates/wordforge-dict/src/tabular.rs)

## 沒有音檔時的發音

本機 TTS 合成：

- **[Piper](https://github.com/rhasspy/piper)** — 神經網路 TTS，品質接近真人，
  單一語音模型約 60 MB，MIT 授權
- **eSpeak NG** — 極小、極快，但明顯是機器音，GPL-3.0

合成的發音會標記 `pronunciation.is_synthetic = 1`，
UI 應該讓使用者知道這不是真人錄音（學發音時這個差別很重要）。

## App 內的標示義務

CC BY-SA 要求標示出處。`dict_source` 表的 `attribution` 欄位
就是為此存在，UI 顯示釋義時必須帶上，例如：

> A round fruit with red or green skin
> — Wiktionary contributors, CC BY-SA 4.0

同時 CC BY-SA 有**傳染性**：如果使用者把匯入的釋義原樣匯出成教材分享出去，
那份教材也必須是 CC BY-SA。匯出功能應該提醒這一點。

## 給貢獻者

歡迎新增匯入器（StarDict、MDict、DSL、JMdict…）。
但**不要**：

- 把任何字典資料檔提交進 repo
- 加入爬取商業字典網站的程式碼
- 在預設設定裡指向來源不明的資料 dump
