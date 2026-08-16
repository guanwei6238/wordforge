//! 教材：匯入自己的課本，出題只從那裡取材。

use time::OffsetDateTime;
use wordforge_core::model::ProfileId;

use crate::commands::cards::KNOWN_STABILITY_DAYS;
use crate::commands::profile::target_lang;
use crate::{AppState, CmdResult};

/// 匯入一份教材。
///
/// 語言用 profile 的目標語言，不讓前端傳——教材跟正在學的語言不一致的話，
/// 詞表會整份對不上，而那個失敗看起來像「匯入成功但沒有效果」。
#[tauri::command]
pub async fn import_material(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    path: String,
    title: Option<String>,
    license_note: Option<String>,
) -> CmdResult<wordforge_import::material::MaterialImport> {
    let lang = target_lang(&state.db, profile_id).await?;
    Ok(wordforge_import::material::import_material(
        &state.db,
        ProfileId(profile_id),
        std::path::Path::new(&path),
        &wordforge_import::material::MaterialOptions {
            title: title.as_deref(),
            lang: &lang,
            license_note: license_note.as_deref(),
            format: None,
        },
        OffsetDateTime::now_utc(),
    )
    .await?)
}

#[tauri::command]
pub async fn list_materials(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<Vec<wordforge_db::material::Material>> {
    Ok(wordforge_db::material::list(&state.db, ProfileId(profile_id)).await?)
}

#[tauri::command]
pub async fn delete_material(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    material_id: i64,
) -> CmdResult<bool> {
    Ok(wordforge_db::material::delete(
        &state.db,
        ProfileId(profile_id),
        wordforge_db::material::MaterialId(material_id),
    )
    .await?)
}

/// 這本教材的字我會了幾成。回傳 (總詞數, 已掌握)。
#[tauri::command]
pub async fn material_coverage(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    material_id: i64,
) -> CmdResult<(i64, i64)> {
    Ok(wordforge_db::material::coverage(
        &state.db,
        ProfileId(profile_id),
        wordforge_db::material::MaterialId(material_id),
        KNOWN_STABILITY_DAYS,
    )
    .await?)
}
