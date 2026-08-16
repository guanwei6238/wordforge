/**
 * Tauri command 的型別化封裝。
 *
 * 依畫面上的功能分檔（查字典、練習、匯入……），這裡把它們全部再匯出，
 * 所以 `import { ... } from "../api"` 一行都不用改。
 *
 * 這裡的型別必須與 `src-tauri/src/commands/` 底下的 struct 對應。
 * 日後可改用 tauri-specta 自動產生，先手動維持同步。
 */

export * from "./cards";
export * from "./core";
export * from "./languages";
export * from "./dict";
export * from "./audio";
export * from "./placement";
export * from "./deck";
export * from "./settings";
export * from "./importing";
export * from "./material";
export * from "./practice";
export * from "./grammar";
export * from "./reset";
