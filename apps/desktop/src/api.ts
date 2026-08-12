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

/* ------------------------------------------------------------------ 語言 */

export interface ProfileLanguages {
  native: string;
  target: string;
}

export function profileLanguages(profileId = DEFAULT_PROFILE_ID): Promise<ProfileLanguages> {
  return invoke("profile_languages", { profileId });
}

/**
 * 目標語言的快取。
 *
 * 「載入哪個語言的字典就能學哪個語言」是這個專案的設計目標，
 * 所以查字典、分級測驗、朗讀都必須跟著 profile 走，不能寫死 en。
 * 這個值在一次啟動內不會變，查一次就夠——但每個呼叫端各自去查
 * 會變成每張卡一次 IPC，所以在這裡共用同一個 Promise。
 */
let languagesCache: Promise<ProfileLanguages> | null = null;

export function currentLanguages(profileId = DEFAULT_PROFILE_ID): Promise<ProfileLanguages> {
  languagesCache ??= profileLanguages(profileId).catch((e) => {
    // 查失敗就讓下一次重試，不要把錯誤永久快取起來
    languagesCache = null;
    throw e;
  });
  return languagesCache;
}

/** 使用者正在學的語言。各 API 的 `lang` 參數省略時就用它。 */
export async function targetLang(profileId = DEFAULT_PROFILE_ID): Promise<string> {
  return (await currentLanguages(profileId)).target;
}

export interface LanguageChange {
  languages: ProfileLanguages;
  /** 屬於其他語言、還沒被收起來的卡片數 */
  other_language_cards: number;
}

/** 換正在學的語言。舊牌組不會自動處理，回傳值會說還有幾張別的語言的卡。 */
export async function setProfileLanguages(
  native: string,
  target: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<LanguageChange> {
  const changed: LanguageChange = await invoke("set_profile_languages", {
    profileId,
    native,
    target,
  });
  // 快取的是舊語言，不清掉的話畫面會有一半還在講上一個語言
  languagesCache = Promise.resolve(changed.languages);
  return changed;
}

/** 把別的語言的卡片收起來（不是刪除，之後換回來還在）。回傳收了幾張。 */
export function suspendOtherLanguageCards(profileId = DEFAULT_PROFILE_ID): Promise<number> {
  return invoke("suspend_other_language_cards", { profileId });
}

/** 匯入了哪些語言的字典：[語言代碼, 詞條數] */
export function dictionaryLanguages(): Promise<[string, number][]> {
  return invoke("dictionary_languages");
}

/** 語言代碼的中文名稱。沒收錄的代碼原樣顯示，總比顯示不出來好。 */
const LANGUAGE_NAMES: Record<string, string> = {
  en: "英文",
  ja: "日文",
  ko: "韓文",
  fr: "法文",
  de: "德文",
  es: "西班牙文",
  it: "義大利文",
  ru: "俄文",
  "zh-TW": "繁體中文",
  "zh-CN": "簡體中文",
};

export function languageName(code: string): string {
  return LANGUAGE_NAMES[code] ?? LANGUAGE_NAMES[code.split("-")[0]] ?? code;
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

export async function searchWords(
  query: string,
  lang?: string,
  limit = 30,
  profileId = DEFAULT_PROFILE_ID,
): Promise<SearchHit[]> {
  return invoke("search_words", {
    profileId,
    lang: lang ?? (await targetLang(profileId)),
    query,
    limit,
  });
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
export async function speak(text: string, lang?: string): Promise<void> {
  return invoke("speak", { text, lang: lang ?? (await targetLang()) });
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

/* -------------------------------------------------------------------- 牌組 */

export interface TagSummary {
  tag: string;
  total: number;
  in_deck: number;
}

export async function deckTags(
  lang?: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<TagSummary[]> {
  return invoke("deck_tags", { profileId, lang: lang ?? (await targetLang(profileId)) });
}

export async function addWordsByTag(
  tag: string,
  limit: number,
  lang?: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<number> {
  return invoke("add_words_by_tag", {
    profileId,
    lang: lang ?? (await targetLang(profileId)),
    tag,
    limit,
  });
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
  /** 閱讀文章要有多少比例是你看得懂的字 0.80~0.99 */
  reading_coverage: number;
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
export async function startImport(path: string, kind: ImportKind, lang?: string): Promise<void> {
  return invoke("start_import", { path, kind, lang: lang ?? (await targetLang()) });
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

/* ------------------------------------------------------------------ 教材 */

export interface Material {
  id: number;
  title: string;
  /** text / epub / pdf / subtitle / html */
  kind: string;
  lang: string;
  source_path: string | null;
  /** 你自己記的授權備註。App 不散布教材，這欄只給你自己看 */
  license_note: string | null;
  created_at: string;
  chunk_count: number;
  vocab_count: number;
}

export interface MaterialImport {
  material_id: number;
  chars: number;
  chunks: number;
  vocab: number;
  /** 字典裡查不到的詞元數。這個數字大代表字典跟教材的語言對不上 */
  unmatched_tokens: number;
}

export const MATERIAL_KIND_LABELS: Record<string, string> = {
  text: "純文字",
  epub: "EPUB",
  pdf: "PDF",
  subtitle: "字幕",
  html: "HTML",
};

/** 教材的語言用 profile 的目標語言，不由前端指定 */
export function importMaterial(
  path: string,
  title?: string,
  licenseNote?: string,
  profileId = DEFAULT_PROFILE_ID,
): Promise<MaterialImport> {
  return invoke("import_material", {
    profileId,
    path,
    title: title ?? null,
    licenseNote: licenseNote ?? null,
  });
}

export function listMaterials(profileId = DEFAULT_PROFILE_ID): Promise<Material[]> {
  return invoke("list_materials", { profileId });
}

export function deleteMaterial(
  materialId: number,
  profileId = DEFAULT_PROFILE_ID,
): Promise<boolean> {
  return invoke("delete_material", { profileId, materialId });
}

/** [這本書有幾個字, 你已經掌握幾個] */
export function materialCoverage(
  materialId: number,
  profileId = DEFAULT_PROFILE_ID,
): Promise<[number, number]> {
  return invoke("material_coverage", { profileId, materialId });
}

/* ------------------------------------------------------------------ AI 練習 */

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

/** 閱讀解析的一條字詞說明。由本地字典查出，不是模型寫的。 */
export interface GlossaryNote {
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
  profileId = DEFAULT_PROFILE_ID,
): Promise<ExerciseView> {
  return invoke("generate_exercise", { profileId, kind, materialId });
}

export function gradeExercise(
  input: GradeInput,
  profileId = DEFAULT_PROFILE_ID,
): Promise<Feedback> {
  return invoke("grade_exercise", { profileId, input });
}
