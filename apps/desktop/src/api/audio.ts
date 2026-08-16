/** 發音：系統 TTS 與離線發音檔。 */
import { invoke } from "@tauri-apps/api/core";

import { DEFAULT_PROFILE_ID } from "./core";
import { targetLang } from "./languages";

/** 朗讀一個字。回傳時已經唸完。 */
export async function speak(text: string, lang?: string): Promise<void> {
  return invoke("speak", { text, lang: lang ?? (await targetLang()) });
}

export function speechAvailable(): Promise<boolean> {
  return invoke("speech_available");
}

export interface AudioProgress {
  total: number;
  downloaded: number;
  failed: number;
  skipped: number;
}

/** [有錄音的字數, 已下載的字數] */
export function audioStatus(profileId = DEFAULT_PROFILE_ID): Promise<[number, number]> {
  return invoke("audio_status", { profileId });
}

export function downloadAudio(
  limit = 500,
  profileId = DEFAULT_PROFILE_ID,
): Promise<AudioProgress> {
  return invoke("download_audio", { profileId, limit });
}

/** 把資料庫存的相對檔名換成 WebView 能讀的絕對路徑 */
export function audioFilePath(name: string): Promise<string> {
  return invoke("audio_file_path", { name });
}

export const AUDIO_PROGRESS_EVENT = "audio://progress";
