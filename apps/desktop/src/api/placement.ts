/** 分級測驗：問幾個字估出詞彙量。 */
import { invoke } from "@tauri-apps/api/core";

import { DEFAULT_PROFILE_ID } from "./core";
import { targetLang } from "./languages";

export interface PlacementItem {
  lemma_id: number;
  text: string;
  freq_rank: number;
  band_index: number;
  translation: string | null;
}

export interface PlacementAnswer {
  band_index: number;
  known: boolean;
}

export interface FrequencyBand {
  start_rank: number;
  end_rank: number;
}

export interface PlacementOutcome {
  estimated_vocabulary: number;
  start_rank: number;
  /** [區間, 認識率] */
  band_rates: [FrequencyBand, number][];
  suspended_cards: number;
}

export async function placementItems(lang?: string): Promise<PlacementItem[]> {
  return invoke("placement_items", { lang: lang ?? (await targetLang()) });
}

export async function submitPlacement(
  answers: PlacementAnswer[],
  lang?: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<PlacementOutcome> {
  return invoke("submit_placement", {
    profileId,
    lang: lang ?? (await targetLang(profileId)),
    answers,
  });
}
