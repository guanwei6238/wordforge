/**
 * Tauri command 的型別化封裝。
 *
 * 這裡的型別必須與 `src-tauri/src/lib.rs` 的 struct 對應。
 * 日後可改用 tauri-specta 自動產生，先手動維持同步。
 */
import { invoke } from "@tauri-apps/api/core";

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
  due_now: number;
  known_words: number;
  total_words: number;
  reviews_today: number;
}

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

export function addWord(word: string, lang = "en", profileId = DEFAULT_PROFILE_ID): Promise<number> {
  return invoke("add_word", { profileId, lang, word });
}

export function studyStats(profileId = DEFAULT_PROFILE_ID): Promise<StudyStats> {
  return invoke("study_stats", { profileId });
}
