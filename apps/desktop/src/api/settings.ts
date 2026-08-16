/** 學習設定：每天幾張、留存率、閱讀字級。 */
import { invoke } from "@tauri-apps/api/core";

import { DEFAULT_PROFILE_ID } from "./core";

export interface StudySettings {
  /** 每天引入幾張新卡 */
  new_per_day: number;
  /** 每天最多複習幾張 */
  max_reviews_per_day: number;
  /** FSRS 的目標記憶留存率 0.70~0.97 */
  desired_retention: number;
  /** 閱讀文章要有多少比例是你看得懂的字 0.80~0.99 */
  reading_coverage: number;
  /** 閱讀測驗的文章字級（px）12~32 */
  reading_font_size: number;
}

export function getStudySettings(profileId = DEFAULT_PROFILE_ID): Promise<StudySettings> {
  return invoke("get_study_settings", { profileId });
}

/** 回傳實際存下來的值——超出合理範圍的會被後端夾住 */
export function updateStudySettings(
  settings: StudySettings,
  profileId = DEFAULT_PROFILE_ID,
): Promise<StudySettings> {
  return invoke("update_study_settings", { profileId, settings });
}
