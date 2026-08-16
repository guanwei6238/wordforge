/**
 * 每個 API 檔共用的東西：錯誤形狀、預設 profile。
 *
 * 這裡的型別必須與 `src-tauri/src/commands/` 底下的 struct 對應。
 * 日後可改用 tauri-specta 自動產生，先手動維持同步。
 */
/**
 * Tauri command 的型別化封裝。
 *
 * 這裡的型別必須與 `src-tauri/src/lib.rs` 的 struct 對應。
 * 日後可改用 tauri-specta 自動產生，先手動維持同步。
 */
/** 後端錯誤統一是 `{ message }`。 */
export interface CommandError {
  message: string;
}

export function errorMessage(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) {
    return String((e as CommandError).message);
  }
  return String(e);
}

export const DEFAULT_PROFILE_ID = 1;
