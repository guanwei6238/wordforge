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
 * 一個文法點：定義加上「你學到哪」。
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
  level: string | null;
  /** seed（內建種子）/ import（匯入）/ manual（自己加） */
  origin: string;
  /** 還沒開始學就是 null */
  state: CardState | null;
  due: string | null;
  error_count: number;
  correct_count: number;
  /** 記憶穩定度（天）。撐得過三週不複習就算「會了」 */
  stability: number | null;
}

/** 撐得過這麼多天不複習就算「會了」，與詞彙量的定義一致。 */
export const GRAMMAR_KNOWN_DAYS = 21;

export function isGrammarKnown(g: GrammarView): boolean {
  return (g.stability ?? 0) >= GRAMMAR_KNOWN_DAYS;
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

/** 匯入一份文法清單（JSON）。回傳寫進去幾筆。 */
export function importGrammar(
  path: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<number> {
  return invoke("import_grammar", { profileId, path });
}
