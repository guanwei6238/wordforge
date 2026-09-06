/** 文法點與情境主題，兩者都是使用者可以自己編輯的清單。 */
import { invoke } from "@tauri-apps/api/core";

import type { CardState } from "./cards";
import { DEFAULT_PROFILE_ID } from "./core";

export interface GrammarExample {
  /** 目標語的例句 */
  text: string;
  /** 母語翻譯 */
  translation: string | null;
}

/**
 * 怎麼在一句話裡認出一個句型。
 *
 * `positive` / `negative` 不是選配：一條打錯字的 regex 什麼都比對不到，
 * 於是難度檢查**永遠通過**，而畫面上完全正常。存檔與匯入時都會實跑一次。
 */
export interface Detector {
  /** 目前只有 "regex"。認不得的種類會在存檔／匯入時被擋下來 */
  type: string;
  value: string;
  /** 一定要命中的例句。**至少一句**，否則存不進去 */
  positive: string[];
  /** 一定不能命中的例句 */
  negative: string[];
}

/** 批改用的錯誤標籤 */
export const KIND_POINT = "point";
/** 教學用的句型 */
export const KIND_PATTERN = "pattern";

/**
 * 一個文法點或句型：定義加上「你學到哪」。
 *
 * 定義存在 `grammar_def`（可匯入、可編輯），掌握狀態存在 `grammar_point`
 * （FSRS 排程）。兩者分開是刻意的——刪掉一份教材不該抹掉學習歷史。
 */
export interface GrammarView {
  point: string;
  name: string;
  /** 還沒講解過就是 null */
  explanation: string | null;
  examples: GrammarExample[];
  /** 顯示用的等級名稱。程式不解讀它，排序看 level_ordinal */
  level: string | null;
  /** 分級刻度上的位置。null 表示沒分級，那時它不參與難度上限 */
  level_ordinal: number | null;
  /** point（錯誤標籤）或 pattern（句型） */
  kind: string;
  /**
   * 怎麼在句子裡認出這個句型。
   *
   * **空的表示偵測不到**——難度上限與「有沒有真的練到」對它都不生效。
   * 這件事一定要顯示出來，不然使用者會以為系統擋得住。
   */
  detectors: Detector[];
  /** seed（內建種子）/ import（匯入）/ manual（自己加）/ draft（AI 草擬） */
  origin: string;
  /** 還沒開始學就是 null */
  state: CardState | null;
  due: string | null;
  error_count: number;
  correct_count: number;
  /** 記憶穩定度（天）。這是**證據**：排程算出來的 */
  stability: number | null;
  /**
   * 使用者自己按過「我會了」的時間。這是**主張**：他說的話。
   *
   * 跟 stability 分開，因為按一次「我會了」只把 stability 推到 3.17 天，
   * 而門檻是 21 天——混在一起的話那一下按了等於沒按。
   */
  known_at: string | null;
}

/** 撐得過這麼多天不複習就算「會了」，與詞彙量的定義一致。 */
export const GRAMMAR_KNOWN_DAYS = 21;

/**
 * 他會這個嗎？**自己標記過**或**練到撐得過門檻**，兩者取聯集。
 *
 * 缺任何一邊都會漏：只看 stability 的話，剛按完「我會了」的撈不到
 * （按一次只到 3.17 天）；只看標記的話，練到滾瓜爛熟但從沒按過按鈕的
 * 撈不到。出題時「他已經會的句型」用的是同一個定義——兩邊不一致的話，
 * 就會變成「畫面說學會了、出題還當他不會」。
 */
export function isGrammarKnown(g: GrammarView): boolean {
  return g.known_at != null || (g.stability ?? 0) >= GRAMMAR_KNOWN_DAYS;
}

/** 這個句型偵測得到嗎？偵測不到的話難度上限對它不生效。 */
export function isDetectable(g: GrammarView): boolean {
  return g.detectors.length > 0;
}

/** 一條偵測規則的試打結果。 */
export interface DetectorCheck {
  /** 這條規則本身過不過（編譯 ＋ 控制組例句） */
  ok: boolean;
  /** 沒過的話，哪裡沒過 */
  problem: string | null;
  /** 試打的那一句有沒有被判定成這個句型。沒給句子就是 null */
  matched: boolean | null;
  /** 命中的是哪一段。看得到抓到什麼，才知道規則是不是抓太寬 */
  matched_text: string | null;
}

/**
 * 試跑一條偵測規則。
 *
 * **一定要走後端。** 瀏覽器的 `RegExp` 跟 Rust 的 `regex` crate 不是同一種
 * 方言（JS 有 lookahead、Rust 沒有），在前端試過就存進去的話，會拿到一條
 * 「編輯器裡看起來好好的、存進資料庫之後編譯失敗」的規則——而它失敗的
 * 樣子是安靜的：那個句型從此偵測不到。
 *
 * 走的是跟存檔、匯入完全同一個函式，所以這裡說會過，存檔就一定會過。
 */
export function checkDetector(
  detector: Detector,
  sentence: string,
): Promise<DetectorCheck> {
  return invoke("check_detector", { detector, sentence });
}

/** 分級刻度上的一格。設定頁的「我現在的程度」選單就是這個。 */
export interface LevelOption {
  level: string | null;
  ordinal: number;
  /** 這一級有幾個句型 */
  patterns: number;
  /** 其中有幾個偵測得到 */
  detectable: number;
}

/**
 * 這個語言的句型分級有哪幾格。
 *
 * 選項來自資料，程式不預設任何分級體系——匯入台灣課綱就得到國小國中
 * 高中，匯入 CEFR 就得到 A1..C2。
 */
export function grammarLevels(
  profileId = DEFAULT_PROFILE_ID,
): Promise<LevelOption[]> {
  return invoke("grammar_levels", { profileId });
}

/** 匯入一批句型之後的結果。 */
export interface PatternReport {
  written: number;
  /** 其中有幾筆偵測得到 */
  detectable: number;
  /** 沒通過控制組、被丟掉的偵測規則 */
  rejected: string[];
}

export function listGrammar(profileId = DEFAULT_PROFILE_ID): Promise<GrammarView[]> {
  return invoke("list_grammar", { profileId });
}

/** 新增或編輯一個文法點。語言由 profile 決定，不用傳。 */
export function saveGrammar(
  def: {
    point: string;
    name: string;
    explanation?: string | null;
    examples?: GrammarExample[];
    level?: string | null;
    level_ordinal?: number | null;
    kind?: string;
    detectors?: Detector[];
    sort_order?: number;
    origin?: string;
  },
  profileId = DEFAULT_PROFILE_ID,
): Promise<void> {
  return invoke("save_grammar", {
    profileId,
    def: {
      lang: "",
      explanation: null,
      examples: [],
      level: null,
      level_ordinal: null,
      kind: KIND_POINT,
      detectors: [],
      sort_order: 0,
      origin: "manual",
      ...def,
    },
  });
}

/** 刪掉一個文法點的定義。**不動掌握狀態**。 */
export function deleteGrammar(
  point: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<boolean> {
  return invoke("delete_grammar", { profileId, point });
}

/**
 * 一個情境主題。出題時用來輪換題材，避免每篇都在講校園生活。
 *
 * 清單存在 `topic` 資料表，可以增刪改——寫死的那份對準備多益的人、
 * 對醫生、對想練特定題材的人都不成立。
 */
export interface Topic {
  id: number;
  lang: string;
  /** 給模型看的描述，會直接進 prompt，寫具體一點比較有用 */
  text: string;
  /** 適用的題型。**空的表示全部題型都適用**，那是大多數 */
  kinds: string[];
  /** seed（內建種子）/ import（匯入）/ manual（自己加） */
  origin: string;
  sort_order: number;
  /** 關掉的仍然看得到，只是不會被拿去出題 */
  enabled: boolean;
}

/** 可以指定給主題的題型。空陣列＝全部適用。 */
export const TOPIC_KINDS: { value: string; label: string }[] = [
  { value: "reading", label: "閱讀" },
  { value: "cloze", label: "克漏字" },
  { value: "translation_to_target", label: "中翻英" },
  { value: "translation_to_native", label: "英翻中" },
];

/** 這個語言的全部主題，含停用的。 */
export function listTopics(profileId = DEFAULT_PROFILE_ID): Promise<Topic[]> {
  return invoke("list_topics", { profileId });
}

/**
 * 新增或編輯一個主題。語言由 profile 決定，不用傳。
 *
 * 有 `id` 就是編輯（改得動文字本身），沒有就是新增。
 */
export function saveTopic(
  topic: {
    id?: number;
    text: string;
    kinds?: string[];
    origin?: string;
    sort_order?: number;
    enabled?: boolean;
  },
  profileId = DEFAULT_PROFILE_ID,
): Promise<number> {
  return invoke("save_topic", {
    profileId,
    topic: {
      id: 0,
      lang: "",
      kinds: [],
      origin: "manual",
      sort_order: 0,
      enabled: true,
      ...topic,
    },
  });
}

export function deleteTopic(
  id: number,
  profileId = DEFAULT_PROFILE_ID,
): Promise<boolean> {
  return invoke("delete_topic", { profileId, id });
}

/** 請模型講解一個文法點，結果存進資料庫。要幾十秒。 */
export function explainGrammar(
  point: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<{ explanation: string | null; examples: GrammarExample[] }> {
  return invoke("explain_grammar", { profileId, point });
}

/**
 * 標記「我會了」或「還要練」。
 *
 * 走的是跟答題一樣的 FSRS 排程，所以自評與實際作答會匯流到同一個進度，
 * 不會變成兩套互相打架的狀態。
 */
export function setGrammarKnown(
  point: string,
  known: boolean,
  profileId = DEFAULT_PROFILE_ID,
): Promise<void> {
  return invoke("set_grammar_known", { profileId, point, known });
}

/**
 * 匯入一份文法／句型清單（JSON）。
 *
 * 每一條偵測規則都會當場實跑控制組，過不了的丟掉並列在 `rejected` 裡
 * ——一份 80 條的檔案不該因為一條打錯字而整份失敗，但被丟掉的一定要
 * 說出來，不然使用者會以為難度上限對那幾個句型有作用。
 */
export function importGrammar(
  path: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<PatternReport> {
  return invoke("import_grammar", { profileId, path });
}
