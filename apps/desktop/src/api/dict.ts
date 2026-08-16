/** 查字典，以及把查到的字加進牌組。 */
import { invoke } from "@tauri-apps/api/core";

import type { CardKind } from "./cards";
import { DEFAULT_PROFILE_ID } from "./core";
import { targetLang } from "./languages";

export interface SearchHit {
  lemma_id: number;
  text: string;
  pos: string;
  freq_rank: number | null;
  cefr: string | null;
  gloss: string | null;
  translation: string | null;
  /** 分類標籤，如 zk / cet4 / oxford3000 */
  tags: string[];
  in_deck: boolean;
}

export interface ExampleView {
  text: string;
  translation: string | null;
}

export interface SenseView {
  gloss: string;
  translation: string | null;
  register: string | null;
  domain: string | null;
  /** 這條釋義所屬詞條的詞性；同一個字合併顯示後用它區分 */
  pos: string;
  examples: ExampleView[];
  /** CC BY-SA 要求顯示的出處 */
  attribution: string | null;
}

export interface PronunciationView {
  accent: string | null;
  ipa: string | null;
  audio_path: string | null;
  /** 有錄音網址但還沒下載 */
  has_audio_url: boolean;
  is_synthetic: boolean;
}

export interface WordDetail {
  lemma_id: number;
  text: string;
  pos: string;
  freq_rank: number | null;
  cefr: string | null;
  senses: SenseView[];
  pronunciations: PronunciationView[];
  /** [詞形, 標籤] */
  forms: [string, string][];
  tags: string[];
  in_deck: boolean;
}

/**
 * 詞條標籤的中文名稱。
 *
 * ECDICT 的標籤是簡稱（zk = 中考、gk = 高考），對台灣使用者要換成
 * 對應的本地說法，否則看不懂。沒收錄的標籤原樣顯示。
 */
export const TAG_LABELS: Record<string, string> = {
  zk: "國中",
  gk: "高中",
  cet4: "四級",
  cet6: "六級",
  ky: "考研",
  toefl: "托福",
  ielts: "雅思",
  gre: "GRE",
  oxford3000: "牛津核心",
  collins1: "★",
  collins2: "★★",
  collins3: "★★★",
  collins4: "★★★★",
  collins5: "★★★★★",
};

export function tagLabel(tag: string): string {
  return TAG_LABELS[tag] ?? tag;
}

export async function searchWords(
  query: string,
  lang?: string,
  limit = 30,
  profileId = DEFAULT_PROFILE_ID,
): Promise<SearchHit[]> {
  return invoke("search_words", {
    profileId,
    lang: lang ?? (await targetLang(profileId)),
    query,
    limit,
  });
}

export function wordDetail(
  lemmaId: number,
  profileId = DEFAULT_PROFILE_ID,
): Promise<WordDetail | null> {
  return invoke("word_detail", { profileId, lemmaId });
}

export function addLemmaToDeck(
  lemmaId: number,
  kinds: CardKind[] = ["recognition"],
  profileId = DEFAULT_PROFILE_ID,
): Promise<void> {
  return invoke("add_lemma_to_deck", { profileId, lemmaId, kinds });
}
