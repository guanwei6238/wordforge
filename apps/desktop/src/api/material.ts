/** 教材：匯入自己的課本，出題只從那裡取材。 */
import { invoke } from "@tauri-apps/api/core";

import { DEFAULT_PROFILE_ID } from "./core";

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
