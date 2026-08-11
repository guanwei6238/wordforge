# 應用程式圖示

標記是一個橘底白色的 W，用 [`make_icon.py`](make_icon.py) 純程式畫出來的
（只用標準函式庫，不需要 Pillow）。原始檔是程式碼而不是二進位圖檔，
所以要調色或改字形直接改常數就好，也不會在 git 歷史裡塞一堆圖片版本。

```bash
python3 make_icon.py icon.png          # 重畫 512×512 的來源圖
cd ../.. && npx tauri icon src-tauri/icons/icon.png   # 產生各平台尺寸
```

## 為什麼衍生檔案進版控

`.ico`、`.icns` 與各尺寸 PNG 都 commit 進來了，理由有兩個：

1. `generate_context!` 在**編譯期**就要讀到 `tauri.conf.json` 列出的每一個
   圖示檔。少一個，`cargo check` 就過不了——貢獻者 clone 下來第一件事
   就會撞牆。
2. 打包流程因此不需要額外一步，CI 也不必裝影像工具。

全部加起來不到 300 KB，換掉「clone 完就能建置」很划算。

行動平台的 `android/` 與 `ios/` 目錄沒有留——桌面版還沒做到 v1.0，
留著只是把 repo 撐大。要做的時候重跑 `npx tauri icon` 就會生出來。
