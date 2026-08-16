/** 牌組：依標籤加字、自動補充。 */
import { invoke } from "@tauri-apps/api/core";

import { DEFAULT_PROFILE_ID } from "./core";
import { targetLang } from "./languages";

export interface TagSummary {
  tag: string;
  total: number;
  in_deck: number;
}

export async function deckTags(
  lang?: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<TagSummary[]> {
  return invoke("deck_tags", { profileId, lang: lang ?? (await targetLang(profileId)) });
}

export async function addWordsByTag(
  tag: string,
  limit: number,
  lang?: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<number> {
  return invoke("add_words_by_tag", {
    profileId,
    lang: lang ?? (await targetLang(profileId)),
    tag,
    limit,
  });
}

/**
 * 自動補充：牌組裡未學的字少於 100 個時，從這個範圍自動補上。
 * 傳 null 關閉。回傳這次補了幾張。
 */
export function setRefillTag(
  tag: string | null,
  profileId = DEFAULT_PROFILE_ID,
): Promise<number> {
  return invoke("set_refill_tag", { profileId, tag });
}

export function getRefillTag(profileId = DEFAULT_PROFILE_ID): Promise<string | null> {
  return invoke("get_refill_tag", { profileId });
}
