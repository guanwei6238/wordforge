/** 學什麼語言、換語言、語言代碼怎麼顯示。 */
import { invoke } from "@tauri-apps/api/core";

import { DEFAULT_PROFILE_ID } from "./core";

export interface ProfileLanguages {
  native: string;
  target: string;
}

export function profileLanguages(profileId = DEFAULT_PROFILE_ID): Promise<ProfileLanguages> {
  return invoke("profile_languages", { profileId });
}

/**
 * 目標語言的快取。
 *
 * 「載入哪個語言的字典就能學哪個語言」是這個專案的設計目標，
 * 所以查字典、分級測驗、朗讀都必須跟著 profile 走，不能寫死 en。
 * 這個值在一次啟動內不會變，查一次就夠——但每個呼叫端各自去查
 * 會變成每張卡一次 IPC，所以在這裡共用同一個 Promise。
 */
let languagesCache: Promise<ProfileLanguages> | null = null;

export function currentLanguages(profileId = DEFAULT_PROFILE_ID): Promise<ProfileLanguages> {
  languagesCache ??= profileLanguages(profileId).catch((e) => {
    // 查失敗就讓下一次重試，不要把錯誤永久快取起來
    languagesCache = null;
    throw e;
  });
  return languagesCache;
}

/** 使用者正在學的語言。各 API 的 `lang` 參數省略時就用它。 */
export async function targetLang(profileId = DEFAULT_PROFILE_ID): Promise<string> {
  return (await currentLanguages(profileId)).target;
}

export interface LanguageChange {
  languages: ProfileLanguages;
  /** 屬於其他語言、還沒被收起來的卡片數 */
  other_language_cards: number;
}

/** 換正在學的語言。舊牌組不會自動處理，回傳值會說還有幾張別的語言的卡。 */
export async function setProfileLanguages(
  native: string,
  target: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<LanguageChange> {
  const changed: LanguageChange = await invoke("set_profile_languages", {
    profileId,
    native,
    target,
  });
  // 快取的是舊語言，不清掉的話畫面會有一半還在講上一個語言
  languagesCache = Promise.resolve(changed.languages);
  return changed;
}

/** 把別的語言的卡片收起來（不是刪除，之後換回來還在）。回傳收了幾張。 */
export function suspendOtherLanguageCards(profileId = DEFAULT_PROFILE_ID): Promise<number> {
  return invoke("suspend_other_language_cards", { profileId });
}

/** 匯入了哪些語言的字典：[語言代碼, 詞條數] */
export function dictionaryLanguages(): Promise<[string, number][]> {
  return invoke("dictionary_languages");
}

/** 語言代碼的中文名稱。沒收錄的代碼原樣顯示，總比顯示不出來好。 */
const LANGUAGE_NAMES: Record<string, string> = {
  en: "英文",
  ja: "日文",
  ko: "韓文",
  fr: "法文",
  de: "德文",
  es: "西班牙文",
  it: "義大利文",
  ru: "俄文",
  "zh-TW": "繁體中文",
  "zh-CN": "簡體中文",
};

export function languageName(code: string): string {
  return LANGUAGE_NAMES[code] ?? LANGUAGE_NAMES[code.split("-")[0]] ?? code;
}
