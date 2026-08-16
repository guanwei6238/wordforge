/** 匯入字典與詞頻表，含進度事件。 */
import { invoke } from "@tauri-apps/api/core";

import { targetLang } from "./languages";

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
