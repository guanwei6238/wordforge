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

  // ---- 句子難度上限。三個都可以是 null，null ＝這一項不限制。
  //
  // 預設全是 null：沒匯入句型之前這套機制無事可做，開著只會讓題目
  // 莫名其妙被退回去重寫，而使用者不知道是自己沒設定過的東西在擋。
  /**
   * 句型分級的上限（`level_ordinal`）。
   *
   * 超過這一級的句型：出題時列進「不要用」，產生之後本地實測，
   * 命中就退回重寫；也不會被指派為「這次要練的句型」。
   */
  pattern_ceiling: number | null;
  /**
   * 一句話最多幾個詞 3~60。
   *
   * 這是**全覆蓋但粗**的那一層：句型偵測只抓得到收錄過的東西，
   * 而沒收錄的難句正是最需要被擋下來的。
   */
  sentence_max_words: number | null;
  /** 一句話最多幾個子句 0~10。0 就是「只給單句」 */
  sentence_max_clauses: number | null;
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
