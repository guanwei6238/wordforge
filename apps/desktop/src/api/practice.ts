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
  /**
   * 參考答案的口語說法——只在它跟正式說法不一樣時才有值。
   *
   * 舊的批改紀錄只有這一欄（那時它是唯一的參考答案，語氣不明），
   * 選擇題也只有這一欄（正確選項沒有語體之分）。所以顯示時要能
   * 退回它，見 `components/Reference.tsx`。
   */
  reference: string | null;
  /** 參考答案的正式說法。翻譯題有批改時這一欄才有。 */
  reference_formal: string | null;
  comment: string | null;
}

export interface Correction {
  /** 第幾題（從 1 起算）。模型漏填時後端會用 original 比對回去 */
  index: number | null;
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
  /** 做過幾次 */
  attempts: number;
  /**
   * 最後一次還有幾題沒全對。
   *
   * null 是沒作答過，或那次批改沒有逐題結果（模型偶爾只給總分）——
   * 那時候說不出「還有幾題」，UI 就不要顯示，不要猜成 0。
   */
  pending: number | null;
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

/**
 * 一次作答：你當時寫了什麼、模型當時怎麼講。
 *
 * `answer` 與 `feedback` 是後端解析好的，不是 JSON 字串——欄位長什麼樣
 * 只有一份定義。壞掉的紀錄會是 null，不會讓整份查詢失敗。
 */
export interface AttemptView {
  attempt_id: number;
  created_at: string;
  score: number | null;
  answer: GradeInput | null;
  feedback: Feedback | null;
}

/** 一份練習做過幾次，舊的在前——62 → 85 → 100 讀起來才是一條線。 */
export function listAttempts(exerciseId: number): Promise<AttemptView[]> {
  return invoke("list_attempts", { exerciseId });
}

/**
 * 一句做過的句子。
 *
 * 這是「我真的用過這個字」的紀錄，跟字典的例句分開——那些是別人寫的。
 */
export interface WordSentence {
  id: number;
  exercise_id: number;
  /** 目標語言那一句 */
  text: string;
  /** 母語翻譯。閱讀文章對不齊時可能是整段，也可能沒有。 */
  translation: string | null;
  /** translation / reading / cloze */
  origin: string;
  /**
   * 這一句踩過哪些文法點（識別碼，如 `articles`）。
   *
   * 名稱要到文法頁的清單查——那份清單使用者可以改，存副本只會漂移。
   */
  grammar_points: string[];
  /**
   * 這一句錯過幾次。
   *
   * 累計在句子上而不是讀排程表：句子寫對之後排程那一列會被刪掉，
   * 而「錯過三次才寫對」正是練起來之後最值得留著的訊號。
   */
  misses: number;
  created_at: string;
}

export const SENTENCE_ORIGIN_LABELS: Record<string, string> = {
  translation: "翻譯題",
  reading: "閱讀",
  cloze: "克漏字",
};

/** 一頁的「你做過的句子」。total 讓 UI 說得出「第 2 / 5 句」。 */
export interface SentencePage {
  items: WordSentence[];
  total: number;
}

/**
 * 這個字在哪幾句話裡用過。第一次呼叫會順便補寫既有練習的連結。
 *
 * 查詢會展開整個詞族：句子存在原形底下（練 `ran` 記在 `run`），
 * 而這裡傳進去的是使用者正在看的那個詞條。
 */
export function wordSentences(
  lemmaId: number,
  limit = 3,
  offset = 0,
  profileId = DEFAULT_PROFILE_ID,
): Promise<SentencePage> {
  return invoke("word_sentences", { profileId, lemmaId, limit, offset });
}

/**
 * 今天要重練的一句翻譯。
 *
 * 刻意沒有參考答案、也沒有你上次寫的東西：今天要問的是「隔一天之後
 * 你自己想得出來嗎」。把上次的作答擺在旁邊，複習就退化成抄寫。
 */
export interface DueSentence {
  exercise_id: number;
  item_index: number;
  kind: ExerciseKind;
  source: string;
  target_word: string | null;
  /** 錯過幾次 */
  misses: number;
}

/** 今天要練的句子：這一頁的份量，加上今天總共還有幾句。 */
export interface DueSentencePage {
  items: DueSentence[];
  /** 今天總共還有幾句，不是這一頁有幾句 */
  total: number;
}

/**
 * 今天該重練的句子。答錯的明天回來，答對的從此不再出現。
 *
 * 一句每天只出現一次——當天反覆重寫刷到全對，看起來是 100 分，
 * 實際上只是背下剛看到的參考答案。
 *
 * 預設一輪三句：一次把二十句攤開來只會讓人不想開始，而三句是
 * 「看得完」與「一次模型呼叫批得完」的交集——批改是按輪送的，
 * 一句一次等於一句燒掉一次完整的請求。
 *
 * 一輪只會拿到**同一個翻譯方向**的句子（批改的 prompt 開頭就寫著方向，
 * 混在一起送等於告訴模型一件錯的事）。另一個方向不會被漏掉，
 * 這一輪送完就輪到它。
 *
 * 要知道還剩幾句的話看 `total`，那個數字跟這一輪拿幾句無關。
 */
export function dueSentences(
  limit = 3,
  profileId = DEFAULT_PROFILE_ID,
): Promise<DueSentencePage> {
  return invoke("due_sentences", { profileId, limit });
}

/** 一句複習的批改結果。 */
export interface DueSentenceResult {
  exercise_id: number;
  item_index: number;
  correct: boolean;
  /** 口語說法。只在它跟正式說法不一樣時才有值。 */
  reference: string | null;
  reference_formal: string | null;
  comment: string | null;
  /**
   * 逐處修正：你寫的哪一段、該改成什麼、為什麼。
   *
   * `comment` 只是一句摘要（「缺少正在進行式」），這一份才說得出
   * 「`dealing with` 要改成 `is addressing`」。
   */
  corrections: Correction[];
}

/**
 * 批改這一輪的複習句子。**一次模型呼叫**，而且不寫進練習紀錄。
 *
 * 跟 `regradeItems` 的分工：那個是「重寫某一份練習裡的某幾題」，會合併
 * 回那份練習、重算分數、在練習紀錄裡多一筆；這個是複習，紀錄自己一份
 * （`listSentenceAttempts`），練習紀錄完全不動。
 */
export function gradeDueSentences(
  items: { exercise_id: number; item_index: number; answer: string }[],
  profileId = DEFAULT_PROFILE_ID,
): Promise<DueSentenceResult[]> {
  return invoke("grade_due_sentences", { profileId, items });
}

/** 複習紀錄的一列：題目、你寫了什麼、對不對。 */
export interface SentenceAttempt {
  id: number;
  exercise_id: number;
  item_index: number;
  /** 題目那一句。那份練習被刪掉時是空字串。 */
  source: string;
  kind: ExerciseKind | "";
  answer: string;
  correct: boolean;
  reference: string | null;
  reference_formal: string | null;
  comment: string | null;
  /** 逐處修正。0020 之前的紀錄沒有這一欄，那時是空陣列。 */
  corrections: Correction[];
  created_at: string;
}

/** 一次送出的複習，含那一輪的每一句。 */
export interface SentenceAttemptBatch {
  /** 這一次送出的時間，同時是這一組的識別 */
  created_at: string;
  items: SentenceAttempt[];
}

export interface SentenceAttemptPage {
  items: SentenceAttemptBatch[];
  /** 複習過幾次（幾組）。分頁數的是這個。 */
  total: number;
  /** 總共練過幾句 */
  sentences: number;
}

/**
 * 複習紀錄一頁幾**次**（不是幾句）。
 *
 * 三次就好：一次送出最多三句，而每一句都攤著題目、你的作答、兩種語體的
 * 參考答案、評語與逐處修正——十次會是一片捲不完的牆。這裡跟練習紀錄
 * （一頁十份）不一樣，因為那邊一列只有一行摘要。
 */
export const REVIEW_LOG_PAGE = 3;

/**
 * 複習紀錄，以每次送出為一組，新的在前。
 *
 * 一輪三句攤成三列的話，一頁十列只看得到三次多一點的複習——
 * 而使用者記得的是「剛剛那一次」，不是「第 7 句」。
 */
export function listSentenceAttempts(
  limit = REVIEW_LOG_PAGE,
  offset = 0,
  profileId = DEFAULT_PROFILE_ID,
): Promise<SentenceAttemptPage> {
  return invoke("list_sentence_attempts", { profileId, limit, offset });
}

/**
 * 刪掉複習紀錄。傳一整組的 id 就是刪掉那一次送出。
 *
 * **只刪紀錄，不動排程**：那一句還沒練起來的話，明天照樣要練。
 */
export function deleteSentenceAttempts(
  ids: number[],
  profileId = DEFAULT_PROFILE_ID,
): Promise<number> {
  return invoke("delete_sentence_attempts", { profileId, ids });
}

/**
 * 今天不寫這一句，明天再出現。
 *
 * 跟送出不一樣的地方：不打模型（沒作答就沒東西可批改，所以是即時的），
 * 也**不算答錯**——錯誤次數不變，不會被記成「錯過 N 次」。
 */
export function skipSentence(
  exerciseId: number,
  itemIndex: number,
  profileId = DEFAULT_PROFILE_ID,
): Promise<boolean> {
  return invoke("skip_sentence", { profileId, exerciseId, itemIndex });
}

/**
 * 只重寫沒全對的那幾題，其餘沿用上一次的批改。
 *
 * 只有翻譯題適用：選擇題本地判分，整份「再做一次」不必打模型，很便宜。
 * 分數會用「答對幾題 ÷ 總題數」重算，所以 100 分＝每一題都對。
 */
export function regradeItems(
  exerciseId: number,
  items: { index: number; answer: string }[],
  profileId = DEFAULT_PROFILE_ID,
): Promise<Feedback> {
  return invoke("regrade_items", { profileId, exerciseId, items });
}

/** 刪掉單獨一次作答，練習本身留著。 */
export function deleteAttempt(
  attemptId: number,
  profileId = DEFAULT_PROFILE_ID,
): Promise<boolean> {
  return invoke("delete_attempt", { profileId, attemptId });
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
