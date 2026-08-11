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
  /** 今天要複習的張數（不含新卡） */
  due_now: number;
  /** 今天還能引入幾張新卡 */
  new_today: number;
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

/** 恢復被收起來的卡，最常用的字優先 */
export function unsuspendCards(count: number, profileId = DEFAULT_PROFILE_ID): Promise<number> {
  return invoke("unsuspend_cards", { profileId, count });
}

/* ------------------------------------------------------------------ 查字典 */

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

export function searchWords(
  query: string,
  lang = "en",
  limit = 30,
  profileId = DEFAULT_PROFILE_ID,
): Promise<SearchHit[]> {
  return invoke("search_words", { profileId, lang, query, limit });
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

/* -------------------------------------------------------------------- 發音 */

/** 朗讀一個字。回傳時已經唸完。 */
export function speak(text: string, lang = "en"): Promise<void> {
  return invoke("speak", { text, lang });
}

export function speechAvailable(): Promise<boolean> {
  return invoke("speech_available");
}

export interface AudioProgress {
  total: number;
  downloaded: number;
  failed: number;
  skipped: number;
}

/** [有錄音的字數, 已下載的字數] */
export function audioStatus(profileId = DEFAULT_PROFILE_ID): Promise<[number, number]> {
  return invoke("audio_status", { profileId });
}

export function downloadAudio(
  limit = 500,
  profileId = DEFAULT_PROFILE_ID,
): Promise<AudioProgress> {
  return invoke("download_audio", { profileId, limit });
}

/** 把資料庫存的相對檔名換成 WebView 能讀的絕對路徑 */
export function audioFilePath(name: string): Promise<string> {
  return invoke("audio_file_path", { name });
}

export const AUDIO_PROGRESS_EVENT = "audio://progress";

/* ---------------------------------------------------------------- 分級測驗 */

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

export function placementItems(lang = "en"): Promise<PlacementItem[]> {
  return invoke("placement_items", { lang });
}

export function submitPlacement(
  answers: PlacementAnswer[],
  lang = "en",
  profileId = DEFAULT_PROFILE_ID,
): Promise<PlacementOutcome> {
  return invoke("submit_placement", { profileId, lang, answers });
}

/* -------------------------------------------------------------------- 牌組 */

export interface TagSummary {
  tag: string;
  total: number;
  in_deck: number;
}

export function deckTags(lang = "en", profileId = DEFAULT_PROFILE_ID): Promise<TagSummary[]> {
  return invoke("deck_tags", { profileId, lang });
}

export function addWordsByTag(
  tag: string,
  limit: number,
  lang = "en",
  profileId = DEFAULT_PROFILE_ID,
): Promise<number> {
  return invoke("add_words_by_tag", { profileId, lang, tag, limit });
}

/**
 * 自動補充：牌組裡未學的字少於 100 個時，從這個範圍自動補上。
 * 傳 null 關閉。回傳這次補了幾張。
 */
export function setRefillTag(
  tag: string | null,
  profileId = DEFAULT_PROFILE_ID,
): Promise<number> {
  return invoke("set_refill_tag", { profileId, tag });
}

export function getRefillTag(profileId = DEFAULT_PROFILE_ID): Promise<string | null> {
  return invoke("get_refill_tag", { profileId });
}

/* -------------------------------------------------------------------- 設定 */

export interface StudySettings {
  /** 每天引入幾張新卡 */
  new_per_day: number;
  /** 每天最多複習幾張 */
  max_reviews_per_day: number;
  /** FSRS 的目標記憶留存率 0.70~0.97 */
  desired_retention: number;
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

/* -------------------------------------------------------------------- 匯入 */

export interface SourceInfo {
  slug: string;
  name: string;
  license: string | null;
  attribution: string | null;
  imported_at: string;
  lemma_count: number;
}

export interface DictStats {
  lemmas: number;
  senses: number;
  with_audio: number;
  sources: SourceInfo[];
}

export interface ImportProgress {
  processed: number;
  imported: number;
  skipped: number;
  failed: number;
  bytes_read: number;
  bytes_total: number;
  cancelled: boolean;
}

/** 與 Rust 端的 `ImportKind` 對應 */
export type ImportKind =
  | "wiktionary_jsonl"
  | "csv"
  | "tsv"
  | "freq_ranked"
  | "freq_tab"
  | "freq_comma";

export const IMPORT_KINDS: { value: ImportKind; label: string; extensions: string[] }[] = [
  { value: "wiktionary_jsonl", label: "Wiktionary (kaikki JSONL)", extensions: ["jsonl", "json"] },
  { value: "csv", label: "單字表 CSV", extensions: ["csv"] },
  { value: "tsv", label: "單字表 TSV", extensions: ["tsv", "txt"] },
  { value: "freq_ranked", label: "詞頻表：一行一個字", extensions: ["txt"] },
  { value: "freq_tab", label: "詞頻表：字<TAB>次數", extensions: ["txt", "tsv"] },
  { value: "freq_comma", label: "詞頻表：字,次數", extensions: ["txt", "csv"] },
];

export function dictionaryStats(): Promise<DictStats> {
  return invoke("dictionary_stats");
}

/** 立刻回傳；進度透過 `import://progress` 等事件送達。 */
export function startImport(path: string, kind: ImportKind, lang = "en"): Promise<void> {
  return invoke("start_import", { path, kind, lang });
}

export function cancelImport(): Promise<void> {
  return invoke("cancel_import");
}

export function importRunning(): Promise<boolean> {
  return invoke("import_running");
}

export const IMPORT_EVENTS = {
  progress: "import://progress",
  done: "import://done",
  error: "import://error",
} as const;

/* ------------------------------------------------------------------ AI 練習 */

export type Backend = "none" | "cli" | "api";
export type CliPreset = "claude_code" | "codex" | "custom";

export interface CliConfig {
  preset: CliPreset;
  program: string;
  args: string[];
  system_flag: string | null;
  /** 指定模型的參數名（--model / -m） */
  model_flag: string | null;
  /** 要用哪個模型；留空用 CLI 自己的預設 */
  model: string;
  timeout_secs: number;
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

/** 送一個極短的 prompt 確認後端真的能用 */
export function testLlm(): Promise<string> {
  return invoke("test_llm");
}

export type ExerciseKind =
  | "translation_to_target"
  | "translation_to_native"
  | "cloze"
  | "reading"
  | "grammar";

export const EXERCISE_LABELS: Record<ExerciseKind, string> = {
  translation_to_native: "英翻中",
  translation_to_target: "中翻英",
  cloze: "克漏字",
  grammar: "文法練習",
  reading: "閱讀測驗",
};

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
  answer_index: number;
  explanation: string | null;
  grammar_point: string | null;
}

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
      new_words: NewWordHint[];
      questions: ChoiceItem[];
    }
  | { kind: "choices"; items: ChoiceItem[] };

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

export interface Feedback {
  score: number | null;
  items: ItemResult[];
  corrections: Correction[];
  /** LLM 判斷你不懂的字 */
  unknown_words: string[];
  /** 實際加進牌組的（字典查得到、還沒學過的） */
  added_to_deck: string[];
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
  profileId = DEFAULT_PROFILE_ID,
): Promise<ExerciseView> {
  return invoke("generate_exercise", { profileId, kind });
}

export function gradeExercise(
  input: GradeInput,
  profileId = DEFAULT_PROFILE_ID,
): Promise<Feedback> {
  return invoke("grade_exercise", { profileId, input });
}
