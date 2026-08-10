# 應用程式圖示

`icon.png` 是**編譯期就需要**的檔案：Tauri 的 `generate_context!` 會讀它，
少了它連 `cargo check` 都過不了。目前放的是用
[`make_placeholder_icon.py`](make_placeholder_icon.py) 生出來的佔位圖
（純標準函式庫，不需要 Pillow）：

```bash
python3 make_placeholder_icon.py icon.png
```

有正式 logo 之後，準備一張 1024×1024 的 PNG 並執行：

```bash
cd apps/desktop
npx tauri icon path/to/logo.png
```

這會產生各平台需要的所有尺寸（`.ico`、`.icns`、多組 `.png`）。
那些衍生檔案不進版控，打包發行版之前重跑一次即可。
