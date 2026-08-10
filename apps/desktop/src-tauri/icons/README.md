# 應用程式圖示

圖示是二進位檔，不進版控。準備一張 1024×1024 的 PNG 後執行：

```bash
cd apps/desktop
npx tauri icon path/to/logo.png
```

指令會在這個目錄產生各平台需要的尺寸，並請把它們加進
`src-tauri/tauri.conf.json` 的 `bundle.icon` 陣列。
打包發行版（`npm run tauri build`）之前必須先完成這一步。
