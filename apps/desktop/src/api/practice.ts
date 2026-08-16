/** AI 練習：出題、批改、練習紀錄，以及 AI 後端設定。 */
import { invoke } from "@tauri-apps/api/core";

import { DEFAULT_PROFILE_ID } from "./core";
import { languageName, type ProfileLanguages } from "./languages";

export type Backend = "none" | "cli" | "api";
export type CliPreset = "claude_code" | "codex" | "custom";

/** 推理強度怎麼傳給 CLI。claude 用獨立旗標，codex 走設定覆寫。 */
export type EffortStyle =
  | { kind: "unsupported" }
  | { kind: "flag"; value: string }
  | { kind: "config"; value: { flag: string; key: string } };

export interface CliConfig {
  preset: CliPreset;
  program: string;
  args: string[];
  system_flag: string | null;
  /** 指定模型的參數名（--model / -m） */
  model_flag: string | null;
  /** 要用哪個模型；留空用 CLI 自己的預設 */
  model: string;
  effort_style: EffortStyle;
  /** 推理強度；留空用 CLI 自己的預設 */
  effort: string;
  timeout_secs: number;
}

/** 這個 CLI 有哪些模型與推理強度可選。清單會過期，所以 UI 也要允許自訂。 */
export interface CliOptions {
  preset: CliPreset;
  models: string[];
  efforts: string[];
}

export interface ApiSettings {
  provider: "anthropic" | "open_ai_compatible" | "ollama";
  model: string;
  base_url: string | null;
  /** 讀出來永遠是空字串；要保留現有的 key 就別動它 */
  api_key: string;
  has_api_key?: boolean;
}

export interface LlmSettings {
  backend: Backend;
  cli: CliConfig;
  api: ApiSettings;
}

export interface CliAvailability {
  preset: CliPreset;
  label: string;
  program: string;
  installed: boolean;
  version: string | null;
  options: CliOptions;
}

/** 偵測這台機器上裝了哪些 AI CLI */
export function detectAiBackends(): Promise<CliAvailability[]> {
  return invoke("detect_ai_backends");
}

export function getLlmSettings(): Promise<LlmSettings> {
  return invoke("get_llm_settings");
}

export function updateLlmSettings(settings: LlmSettings): Promise<LlmSettings> {
  return invoke("update_llm_settings", { settings });
}

export interface ModelProbe {
  usable: boolean;
  /** 不能用的原因 */
  detail: string;
}

/**
 * 試跑一個模型看它能不能用。
 *
 * 兩個 CLI 都沒有可以程式化查詢模型清單的方式，所以下拉選單的清單一定會
 * 過期。直接送一個最小 prompt 過去，成敗就是不會過期的答案。要幾秒鐘。
 */
export function probeModel(model: string): Promise<ModelProbe> {
  return invoke("probe_model", { model });
}

/** 送一個極短的 prompt 確認後端真的能用 */
export function testLlm(): Promise<string> {
  return invoke("test_llm");
}

/** 一段期間的 LLM 用量 */
export interface UsageSummary {
  calls: number;
  /** 失敗的次數。重試會燒額度，所以單獨看得到 */
  failed: number;
  prompt_chars: number;
  response_chars: number;
  /** 後端有回報才有值。null 代表量不到，不是 0 */
  input_tokens: number | null;
  output_tokens: number | null;
  /** 有幾次呼叫回報了 token */
  calls_with_tokens: number;
}

/** [今天, 最近七天, 今天依用途拆開 [用途, 次數, 總字元]] */
export function llmUsage(
  profileId = DEFAULT_PROFILE_ID,
): Promise<[UsageSummary, UsageSummary, [string, number, number][]]> {
  return invoke("llm_usage", { profileId });
}

export const PURPOSE_LABELS: Record<string, string> = {
  generate: "出題",
  grade: "批改",
};

export type ExerciseKind =
  | "translation_to_target"
  | "translation_to_native"
  | "cloze"
  | "reading"
  | "grammar";

/**
 * 題型名稱。
 *
 * 翻譯題的名字得看使用者在學什麼——寫死「英翻中」的話，
 * 換一份日文字典之後畫面就在說謊。
 */
export function exerciseLabels(langs: ProfileLanguages): Record<ExerciseKind, string> {
  const target = languageName(langs.target);
  const native = languageName(langs.native);
  return {
    translation_to_native: `${target}翻${native}`,
    translation_to_target: `${native}翻${target}`,
    cloze: "克漏字",
    grammar: "文法練習",
    reading: "閱讀測驗",
  };
}

export interface PracticeStatus {
  llm_ready: boolean;
  vocabulary: number;
  weak_grammar: string[];
  recommended: ExerciseKind;
  /** [題型, 需要的最低詞彙量] */
  requirements: [ExerciseKind, number][];
}

export interface TranslationItem {
  source: string;
  target_word: string | null;
  reference: string | null;
}

export interface ChoiceItem {
  question: string;
  options: string[];
  /**
   * 每個選項一句說明，與 options 平行。
   *
   * 這是「針對你的作答說明」的來源：選擇題在本地判分，模型沒看過你
   * 選了什麼，所以解說在出題時就每個選項各備一句，判分時挑你按的
   * 那一句。舊的練習紀錄沒有這個欄位，所以可能是空陣列。
   */
  option_notes: string[];
  answer_index: number;
  explanation: string | null;
  grammar_point: string | null;
  /** easy / medium / hard。模型沒給就是 null，不顯示徽章 */
  difficulty: string | null;
}

/** 難度標籤。沒收錄的值原樣顯示，總比顯示不出來好。 */
export const DIFFICULTY_LABELS: Record<string, string> = {
  easy: "基礎",
  medium: "進階",
  hard: "推論",
};

export interface NewWordHint {
  word: string;
  gloss: string | null;
}

export type ExerciseBody =
  | { kind: "translation"; to_target: boolean; items: TranslationItem[] }
  | {
      kind: "reading";
      title: string;
      passage: string;
      /** 整篇的母語翻譯。作答前不顯示，解析時才展開 */
      translation: string | null;
      new_words: NewWordHint[];
      questions: ChoiceItem[];
    }
  | {
      kind: "cloze";
      title: string;
      /** 挖好空格的短文，空格是 `{{1}}`、`{{2}}` */
      passage: string;
      translation: string | null;
      /** 第 k 題對應 `{{k}}` 那一格 */
      items: ChoiceItem[];
    }
  | { kind: "choices"; items: ChoiceItem[] };

/** 克漏字空格的標記，與 `wordforge_core::practice` 那一份對應。 */
export const BLANK_PATTERN = /\{\{\s*(\d+)\s*\}\}/g;

export interface ExerciseView {
  exercise_id: number;
  kind: ExerciseKind;
  body: ExerciseBody;
  target_words: string[];
  coverage: number | null;
}

export interface ItemResult {
  index: number;
  correct: boolean;
  reference: string | null;
  comment: string | null;
}

export interface Correction {
  original: string;
  corrected: string;
  grammar_point: string | null;
  severity: string | null;
  explanation: string | null;
}

/** 閱讀解析的一條字詞說明。由本地字典查出，不是模型寫的。 */
export interface GlossaryNote {
  /** 正規化後的比對鍵。要拿使用者點到的字對這一欄，不是 text */
  term: string;
  /** 字典收錄的原形，顯示用 */
  text: string;
  gloss: string | null;
  translation: string | null;
  /** 多詞片語，單看每個字查不出這個意思 */
  is_phrase: boolean;
  /** 不在你的已知詞裡，也就是 90% 法則裡那不足 10% */
  is_unknown: boolean;
}

export interface Feedback {
  score: number | null;
  items: ItemResult[];
  corrections: Correction[];
  /** LLM 判斷你不懂的字 */
  unknown_words: string[];
  /** 實際加進牌組的（字典查得到、還沒學過的） */
  added_to_deck: string[];
  /** 這篇刻意要教的新字。跟 unknown_words 分開：那些是你答錯時露出來的 */
  taught_words: string[];
  /** 文章裡的生字與片語，本地字典查的 */
  glossary: GlossaryNote[];
}

export interface GradeInput {
  exercise_id: number;
  answers: string[];
  choices: (number | null)[];
  /** 你自己點「這個字我不會」 */
  marked_unknown: string[];
}

export function practiceStatus(profileId = DEFAULT_PROFILE_ID): Promise<PracticeStatus> {
  return invoke("practice_status", { profileId });
}

export function generateExercise(
  kind: ExerciseKind | "auto" = "auto",
  materialId: number | null = null,
  /** 文法題只練這一個點。null 就用今天到期的弱點 */
  grammarPoint: string | null = null,
  profileId = DEFAULT_PROFILE_ID,
): Promise<ExerciseView> {
  return invoke("generate_exercise", { profileId, kind, materialId, grammarPoint });
}

export function gradeExercise(
  input: GradeInput,
  profileId = DEFAULT_PROFILE_ID,
): Promise<Feedback> {
  return invoke("grade_exercise", { profileId, input });
}

/** 練習紀錄的一列。清單上不帶題目內容，重做時再用 loadExercise 取。 */
export interface ExerciseSummary {
  exercise_id: number;
  kind: ExerciseKind;
  created_at: string;
  coverage: number | null;
  /** 做過才有分數。null 代表出了題但沒作答 */
  score: number | null;
  title: string;
}

export interface ExercisePage {
  items: ExerciseSummary[];
  /** 全部有幾份，用來算頁數。少了它說不出「第 2 頁 / 共 7 頁」 */
  total: number;
}

export function listExercises(
  limit = 10,
  offset = 0,
  profileId = DEFAULT_PROFILE_ID,
): Promise<ExercisePage> {
  return invoke("list_exercises", { profileId, limit, offset });
}

/** 刪掉一份練習紀錄，連同它的作答。回傳有沒有真的刪到。 */
export function deleteExercise(
  exerciseId: number,
  profileId = DEFAULT_PROFILE_ID,
): Promise<boolean> {
  return invoke("delete_exercise", { profileId, exerciseId });
}

/** 取回一份做過的練習，原封不動再做一次。送出後照常由 LLM 批改。 */
export function loadExercise(exerciseId: number): Promise<ExerciseView> {
  return invoke("load_exercise", { exerciseId });
}
