import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 會啟動這個 dev server，port 必須固定，
// 否則 tauri.conf.json 的 devUrl 會對不上。
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // src-tauri 由 cargo 自己監看，vite 不需要跟著重載
      ignored: ["**/src-tauri/**"],
    },
  },
});
