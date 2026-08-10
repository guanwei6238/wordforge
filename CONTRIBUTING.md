# 貢獻指南

歡迎！這個專案的目標是做出一個真的能拿來學語言的工具，
所以任何「我實際使用時遇到的問題」都是有價值的 issue。

## 開發環境

```bash
git clone https://github.com/guanwei6238/wordforge
cd wordforge

# 核心邏輯：不需要任何系統依賴
cargo test

# 桌面 App：Linux 需要先裝 WebView 相關套件
sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev \
    patchelf build-essential curl wget file libxdo-dev libssl-dev
cd apps/desktop && npm install && npm run tauri dev
```

macOS 需要 Xcode Command Line Tools；Windows 需要
Microsoft C++ Build Tools 與 WebView2（Win11 已內建）。

## 提交前

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
cd apps/desktop && npm run typecheck
```

CI 會跑同樣的檢查。

## 程式碼風格

**註解寫「為什麼」，不寫「做什麼」。**

```rust
// ❌ 把 stability 限制在 0.01 到 36500 之間
// ✅ 極端輸入下公式會塌成 0 或負值，夾住避免後續除法爆炸
```

**核心層不准碰 I/O 與時鐘。** `wordforge-core` 的函數都是純函數，
時間一律由參數傳入。這是為了讓「連續答對八次間隔會遞增」
可以在毫秒內測完，而不是真的等八天。

**測試驗行為，不驗數值。** FSRS 權重之後可能換成個人化訓練的結果，
硬編數值的測試會全部失效，但「答對後間隔要變長」這個性質不會變。

**錯誤要能回答「使用者該怎麼辦」。** `thiserror` 的訊息會直接出現在 UI 上。

## 關於字典資料

**絕對不要**把字典資料檔提交進 repo，也不要加入爬取商業字典網站的程式碼。
理由與可用的替代來源見 [docs/dictionary-sources.md](docs/dictionary-sources.md)。

新增匯入器時，`SourceMeta` 的授權欄位請務必填正確——
UI 依賴它顯示出處，這是 CC BY-SA 的法律義務。

## Commit 訊息

用祈使句，說明改了什麼與為什麼：

```
srs: 修正同日重複複習高估 stability 的問題

原本走一般 recall 公式，導致五分鐘內重看一次就把間隔拉到數天。
改用 FSRS-5 的 short-term 公式。
```

不需要遵守 Conventional Commits，但請用前綴標出模組（`srs:`、`db:`、`ui:`）。

## Pull Request

- 一個 PR 做一件事
- 附上「怎麼驗證這個改動有效」
- 改動行為的 PR 要有測試
- 大改動請先開 issue 討論，避免白做工

## 授權

送出 PR 即表示同意你的貢獻以 [Apache License 2.0](LICENSE) 授權釋出。
