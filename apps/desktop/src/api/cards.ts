/** 複習卡片：今天複習什麼、按下難易度之後怎麼排、暫時不想看的怎麼收。 */
import { invoke } from "@tauri-apps/api/core";

import { DEFAULT_PROFILE_ID } from "./core";
import { targetLang } from "./languages";

export type CardKind = "recognition" | "recall" | "listening" | "spelling";
export type CardState = "new" | "learning" | "review" | "relearning";

/** 對應 FSRS 的四級評分 */
export const RATING = {
  again: 1,
  hard: 2,
  good: 3,
  easy: 4,
} as const;

export type Rating = (typeof RATING)[keyof typeof RATING];

export interface CardView {
  card_id: number;
  lemma_id: number;
  word: string;
  kind: CardKind;
  state: CardState;
  gloss: string | null;
  translation: string | null;
  ipa: string | null;
  audio_path: string | null;
}

export interface StudyStats {
  /** 今天要複習的張數（不含新卡） */
  due_now: number;
  /** 今天還能引入幾張新卡 */
  new_today: number;
  known_words: number;
  total_words: number;
  reviews_today: number;
}

export function listDueCards(profileId = DEFAULT_PROFILE_ID, limit = 50): Promise<CardView[]> {
  return invoke("list_due_cards", { profileId, limit });
}

export function reviewCard(
  cardId: number,
  rating: Rating,
  durationMs?: number,
  profileId = DEFAULT_PROFILE_ID,
): Promise<void> {
  return invoke("review_card", {
    profileId,
    input: { card_id: cardId, rating, duration_ms: durationMs ?? null },
  });
}

export async function addWord(
  word: string,
  lang?: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<number> {
  return invoke("add_word", { profileId, lang: lang ?? (await targetLang(profileId)), word });
}

export function studyStats(profileId = DEFAULT_PROFILE_ID): Promise<StudyStats> {
  return invoke("study_stats", { profileId });
}

export interface QueueStatus {
  /** 現在到期的複習卡 */
  due_reviews: number;
  /** 今天還能引入幾張新卡 */
  new_today: number;
  /** 牌組裡還有幾張沒學過（不受每日上限限制） */
  new_in_deck: number;
  /** 被分級測驗收起來的卡 */
  suspended: number;
  /** 下一張卡到期時間（RFC 3339） */
  next_due: string | null;
  new_per_day: number;
}

export function queueStatus(profileId = DEFAULT_PROFILE_ID): Promise<QueueStatus> {
  return invoke("queue_status", { profileId });
}

/** 超出每日上限再多學幾個新字 */
export function studyMore(extra: number, profileId = DEFAULT_PROFILE_ID): Promise<CardView[]> {
  return invoke("study_more", { profileId, extra });
}

/**
 * 把一張卡藏到明天，排程不動。
 *
 * 跟 suspendCard 的差別是「會不會自己回來」：埋葬明天自動回來，
 * 暫停要到牌組頁主動恢復。
 */
export function buryCard(cardId: number, profileId = DEFAULT_PROFILE_ID): Promise<boolean> {
  return invoke("bury_card", { profileId, cardId });
}

/** 收起一張卡，要主動恢復才會回來 */
export function suspendCard(cardId: number, profileId = DEFAULT_PROFILE_ID): Promise<boolean> {
  return invoke("suspend_card", { profileId, cardId });
}

/** 恢復被收起來的卡，最常用的字優先 */
export function unsuspendCards(count: number, profileId = DEFAULT_PROFILE_ID): Promise<number> {
  return invoke("unsuspend_cards", { profileId, count });
}
