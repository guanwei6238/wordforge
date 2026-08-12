# 工作指引

這份文件寫給在這個 repo 上工作的 AI 助理。人類貢獻者看
[CONTRIBUTING.md](../CONTRIBUTING.md)（環境、風格、commit 格式），
那些不在這裡重複。

這裡只講一件事：**這個專案已經踩過哪些坑，以及怎麼不要再踩。**
底下每一條規則都對應一個真的發生過的 bug。

---

## 最高原則：不要為了解決眼前的問題而寫死

寫死的東西不會報錯。它會安靜地把一整個設計目標變成裝飾品，
然後在幾週後由使用者發現。

這個專案的核心承諾是**「載入哪個語言的字典，就能學哪個語言」**。
但曾經有很長一段時間，`profile` 表的 `native_lang` / `target_lang`
兩個欄位**只出現在 INSERT 和測試 fixture 裡，從來沒有被讀出來過**——
每一個查詢都寫死 `"en"`。程式跑得好好的、測試全綠、功能都能用，
只是那個承諾是假的。

### 判斷寫死可不可以的三個問題

動手寫一個字面值之前，先回答：

1. **這個值換一個情境還成立嗎？** 語言、模型名稱、詞性清單、
   檔案格式、覆蓋率目標——這些都不成立。
2. **它會不會過期？** 模型名稱會、CLI 參數會、字典的欄位慣例會。
3. **壞掉的時候看得出來嗎？** 看不出來的最危險。

三個問題裡有任何一個答案不對，就不能寫死。

### 已經發生過的實例

下面每一條都已經修掉了，列在這裡是因為**它們都不是打字錯誤，
是當下「先讓功能動起來」的合理決定**——而那正是這類 bug 的來源。

| 曾經寫死了什麼 | 後果 |
| --- | --- |
| 各處的 `"en"` | 「換字典就能學別的語言」名存實亡 |
| 覆蓋率目標 `0.96`（曾是常數） | 使用者想調 90% 法則的門檻，調不了 |
| `gpt-5.6-luna` 當 codex 預設 | 在 codex-cli 0.142.5 上直接被拒絕 |
| 前後端各一份模型清單 | 兩邊長歪，而且都會過期 |
| 「太難」判定用寫死的 90% 難度帶 | 使用者把目標設 98% 時那個判斷完全不生效 |
| 生詞選詞完全決定性 | 每篇文章都拿到一模一樣的六個字 |
| `GRAMMAR_POINTS` 一份英文清單 | 學日文的人拿到英文文法術語 |

### 正確的做法長什麼樣

**讓資料決定，不要讓程式決定。** 設定頁的「我要學什麼語言」選項來自
`SELECT lang, COUNT(*) FROM lemma GROUP BY lang`——匯入了什麼就能學什麼，
程式不預設任何語言。

**清單一定要能自訂。** 模型選單有 curated 選項，但一定有「自訂…」，
而且文案明講清單會過期。更進一步：提供不會過期的驗證方式
（`probe_model` 直接送一個最小 prompt，成敗就是答案）。

**形狀不一樣的東西不要硬套同一個模型。** 推理強度在兩個 CLI 上長得不一樣：

```text
claude   --effort high                      獨立旗標
codex    -c model_reasoning_effort=high     設定覆寫
```

所以 `EffortStyle` 是個 enum（`Unsupported` / `Flag` / `Config`），
不是一個 `effort_flag: Option<String>`。硬套的話 codex 那條路就永遠是壞的。

**語言相關的東西要有 fallback，而且 fallback 不能是「什麼都不做」。**
`grammar_points::points_for(lang)` 對沒收錄的語言回空清單，此時
`normalize_point` 保留模型自己給的標籤，而不是硬套英文分類。
`wordlist::is_function_word` 對未知語言回 `false`——不會誤排除，
只是少了一層過濾。

**降級要保留核心功能。** 生詞選詞會做詞性配比，但詞性來自 Wiktionary；
只匯入 ECDICT 的人整份字典 `pos` 都是空的。曾經寫過「查不到詞性就跳過」，
那會讓那些人**一個生詞都拿不到**，文章覆蓋率衝回 99%，功能等於不存在。
正確做法：詞性配比是加分項，有生詞才是必要的。

---

## 動手之前先看真實資料

這個 repo 有一個 892 MB 的真實資料庫（`~/.local/share/org.wordforge.app/wordforge.db`），
224 萬個詞條。**用它驗證假設，不要用直覺。** 唯讀開啟：

```python
sqlite3.connect('file:...wordforge.db?mode=ro', uri=True)
```

真的發生過的事：

- 以為 `pos` 欄位有值 → ECDICT 那 77 萬筆全是空字串，詞性在 Wiktionary 那批
- 以為多詞條目很少 → 實際有 69 萬個，`search for` / `in spite of` 都在
- 以為 `find_by_form` 會回原形 → 它回 id 最小的，所以 `ran` 回 `ran` 不是 `run`
- 以為粗話會有 register 標記 → `bitch` 只標了 `countable, archaic, colloquial`

## 驗證方法本身也要驗證

`strings` 預設只掃 ASCII，所以拿它找中文字串**不管在不在都會回 0**。
我曾經用它「確認」一個修正不在 binary 裡，然後叫使用者不要安裝——結論是錯的。

**每個檢查都要先跑一次已知會通過的控制組。**

```bash
grep -ac "找不到指令" binary   # 控制組：一定存在，回 1 才代表方法可用
grep -ac "prompt 沒寫完" binary # 真正要查的
```

同一類錯誤：`grep -c` 在零匹配時 **exit 1**，所以
`cargo clippy | grep -c "^error" && git commit` 會在 clippy 乾淨時
跳過 commit。那次 tag 推出去了但改動留在工作區。

---

## 效能問題要量，不要猜

`crates/wordforge-db/tests/perf.rs` 建十萬張卡量熱路徑，**必須用 `--release`**
（debug 的數字差 4～8 倍，沒有參考價值）。

它抓到過一個我自己剛引入的退化：把 `buried_until` 放進 `idx_card_due` 的
第三欄。它是範圍條件，一進索引 SQLite 就沒辦法再用索引替 `due` 排序，
退化成 `USE TEMP B-TREE FOR ORDER BY`，開 App 的第一個查詢慢四倍。

**改索引之後跑 `EXPLAIN QUERY PLAN`。** 看到 `SCAN` 或
`USE TEMP B-TREE FOR ORDER BY` 就是有問題。

**加外鍵一定要加索引。** 曾經因為 `example.sense_id` 沒有索引，
重新匯入時 CASCADE 每刪一筆就全表掃描 71.9 萬列，讀了 604 GB。
`repo.rs` 有一條測試掃 `PRAGMA foreign_key_list` 檢查每個外鍵都有索引，
別讓它失效。

---

## LLM 相關

**prompt 講的話要能在本地驗收。** 「請讓覆蓋率達到 96%」只是請求；
`measure_coverage()` 在本地實算才是保證。這個模式要保持：
凡是能在本地驗的，就不要只相信模型。

**但驗收的定義要跟 prompt 說的一致。** 曾經 prompt 告訴模型
「他掌握約 5200 個單字」，而驗收只認「stability ≥ 21 天的卡片」——
使用者一張都不到門檻，覆蓋率永遠 0%，每篇都重寫三次。
一題 98 秒，而且驗收本身完全沒有作用。

**量不到就回 `None`，不要回 0。** CLI 後端不回報 token 數。
`0` 看起來像「沒用到」，實際是「量不到」——兩件事差很多。
用量統計主要顯示字元數（每個後端都量得到），token 有回報時才另外顯示。

**錯誤訊息要說得出下一步。** `HTTP 127` 對使用者是天書。
現在會說明是缺哪個直譯器、為什麼從選單啟動才會發生、可以怎麼辦，
並保留原始訊息。

**呼叫模型的測試用 `FakeLlm`。** 真的要打模型的測試標 `#[ignore]`
並在檔頭寫清楚它會消耗訂閱額度（`crates/wordforge-practice/tests/live.rs`）。

---

## 測試

**測試資料要像真實資料。** 生詞選詞那個 bug 是測試抓到的，
而它抓得到的唯一原因是測試 fixture 的 `pos` 剛好是空字串——
跟 ECDICT 一樣。如果當初填了詞性，那個問題會一路溜到使用者手上。

**測試名稱要講清楚在驗什麼行為**，不是驗什麼函數：

```rust
// ❌ fn test_find_by_form()
// ✅ fn an_inflection_resolves_to_the_whole_family()
```

**修 bug 一定要留一條會紅的測試**，而且註解寫清楚它為什麼存在：

```rust
/// 這條測試存在的理由是它曾經是錯的：`find_by_form` 挑 id 最小的，
/// 而 `ran` 自己在字典裡也是一個詞條，且 `ran` < `run`，
/// 所以查 `ran` 會回到 `ran` 而不是 `run`，學過的字被算成生字。
```

**測試行為改變時，先想「是測試錯了還是程式錯了」。** 加生詞自動進牌組時
有一條翻譯題的測試掛掉——那不是測試過時，是我的實作錯了：
翻譯題的 `target_words` 是複習字，不該當成新教的字。

---

## 提交前

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps/desktop && npx tsc --noEmit && npm run build
```

**發版時三個檔案的版號要一起改**：`Cargo.toml`、
`apps/desktop/src-tauri/tauri.conf.json`、`apps/desktop/package.json`。
漏掉的話 tag 說 1.1.1、打包出來的檔名是 1.1.0。

**打完 tag 驗證它指到對的 commit**：

```bash
[ "$(git rev-parse v1.2.3^{})" = "$(git rev-parse HEAD)" ] && echo 一致
```

**圖示檔要進版控。** `generate_context!` 在編譯期就要讀到
`tauri.conf.json` 列的每一個圖示。曾經被 `.gitignore` 擋掉，
結果新 clone 連 `cargo check` 都過不了。改動打包設定之後，
實際 clone 一份驗證。

---

## 不能碰的界線

- **不要把字典或教材資料提交進 repo**，也不要寫爬商業字典網站的程式碼。
  理由見 [docs/dictionary-sources.md](../docs/dictionary-sources.md)。
- **API 金鑰不進 SQLite**。資料庫很可能被複製到雲端硬碟；金鑰另存
  權限 600 的檔案，送到 WebView 之前要遮罩。
- **`wordforge-core` 不碰 I/O 與時鐘。** 時間一律由參數傳入。

---

## 講話要誠實

這個專案的 README 明講「程式碼是 AI 寫的，作者沒有逐行看過」。
同樣的標準適用於每一次回報：

- 沒驗證過的不要說「已完成」。macOS 與 Windows 的安裝檔只確認打包流程
  跑得完，**沒有實機測試過**，release notes 就是這樣寫的。
- 量不出來的不要假裝量得出來。「high effort 的題目比較好」——
  本地只驗得到生詞覆蓋率，四種設定算出來都是 99% 以上，
  所以那個差別**程式量不出來**，只能請使用者自己讀了決定。
- 自己弄錯的直接講，不要繞。方法錯了就說方法錯了，重新驗一次。
