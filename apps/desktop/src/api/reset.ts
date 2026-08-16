/** 重置學習資料，以及重置之後通知每一頁重新載入。 */
import { invoke } from "@tauri-apps/api/core";

import { DEFAULT_PROFILE_ID } from "./core";

export interface ResetSummary {
  cards: number;
  reviews: number;
  exercises: number;
  attempts: number;
  grammar_points: number;
  llm_calls: number;
}

/**
 * 把學習資料清空，回到剛安裝的狀態。
 *
 * **不刪字典也不刪教材**——重匯一份 Wiktionary 要好幾分鐘。
 * 呼叫端必須先讓使用者確認過。
 */
export async function resetProgress(profileId = DEFAULT_PROFILE_ID): Promise<ResetSummary> {
  const summary: ResetSummary = await invoke("reset_progress", { profileId });
  notifyDataReset();
  return summary;
}

/**
 * 資料被清空時，通知還掛在畫面上的頁面。
 *
 * 大部分頁面切走就卸載，回來時自然會重查。**練習頁不會**——出一題要
 * 幾十秒，切去別的分頁再回來題目不能消失，所以它一直掛著。
 * 代價是它看不到別的地方做了什麼：在設定頁按下重置之後，練習頁還
 * 顯示著已經不存在的題目與舊的詞彙量。
 *
 * 廣播在 `resetProgress` 裡發，不是由呼叫端負責——放在呼叫端的話，
 * 日後多一個地方能重置就會漏掉一次。
 */
const RESET_EVENT = "wordforge:data-reset";

export function notifyDataReset(): void {
  window.dispatchEvent(new Event(RESET_EVENT));
}

/** 訂閱重置事件，回傳取消訂閱的函式。 */
export function onDataReset(handler: () => void): () => void {
  window.addEventListener(RESET_EVENT, handler);
  return () => window.removeEventListener(RESET_EVENT, handler);
}
