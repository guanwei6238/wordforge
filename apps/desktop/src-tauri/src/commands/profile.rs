//! 使用者設定：學什麼語言、每天學幾個、要不要重來一次。
//!
//! 換語言是這裡最需要小心的一條路：舊牌組不會自己消失，
//! 所以要說得出「還有幾張別的語言的卡」讓使用者自己決定。

use serde::Serialize;
use wordforge_core::model::ProfileId;
use wordforge_db::Db;
use wordforge_db::repo::{cards, profiles};

use crate::{AppState, CmdResult};

/// 這個 profile 在學什麼語言。
///
/// 先前每個地方都硬編 `"en"`——那讓「換一份字典就能學另一種語言」
/// 這個設計目標名存實亡。
pub async fn target_lang(db: &Db, profile_id: i64) -> CmdResult<String> {
    Ok(profiles::languages(db, ProfileId(profile_id)).await?.1)
}

/// 這個 profile 的母語與目標語言。
#[derive(Debug, Serialize)]
pub struct ProfileLanguages {
    pub native: String,
    pub target: String,
}

/// 前端要拿這個當各處 `lang` 參數的預設值，而不是自己寫死 `"en"`。
#[tauri::command]
pub async fn profile_languages(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<ProfileLanguages> {
    let (native, target) = profiles::languages(&state.db, ProfileId(profile_id)).await?;
    Ok(ProfileLanguages { native, target })
}

/// 換語言之後的狀況：新的語言設定，加上還有幾張別的語言的卡混在牌組裡。
#[derive(Debug, Serialize)]
pub struct LanguageChange {
    pub languages: ProfileLanguages,
    /// 屬於其他語言、還沒被收起來的卡片數
    pub other_language_cards: i64,
}

/// 改掉正在學的語言。
///
/// 不會自動處理舊牌組——那是使用者的資料，該由他決定要不要收起來。
/// 但一定要把數量回報出去，否則他明天會看到一堆上個語言的字。
#[tauri::command]
pub async fn set_profile_languages(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    native: String,
    target: String,
) -> CmdResult<LanguageChange> {
    let (native, target) =
        profiles::set_languages(&state.db, ProfileId(profile_id), &native, &target).await?;
    let other_language_cards =
        cards::count_other_languages(&state.db, ProfileId(profile_id), &target).await?;
    Ok(LanguageChange {
        languages: ProfileLanguages { native, target },
        other_language_cards,
    })
}

/// 把別的語言的卡片收起來，回傳收了幾張。
#[tauri::command]
pub async fn suspend_other_language_cards(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<u64> {
    let target = target_lang(&state.db, profile_id).await?;
    Ok(cards::suspend_other_languages(&state.db, ProfileId(profile_id), &target).await?)
}

#[tauri::command]
pub async fn get_study_settings(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<profiles::StudySettings> {
    Ok(profiles::study_settings(&state.db, ProfileId(profile_id)).await?)
}

/// 更新學習設定，回傳實際存下來的值（超出合理範圍會被夾住）。
#[tauri::command]
pub async fn update_study_settings(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    settings: profiles::StudySettings,
) -> CmdResult<profiles::StudySettings> {
    Ok(profiles::update_study_settings(&state.db, ProfileId(profile_id), settings).await?)
}

/// 把這個 profile 的學習資料清空。
///
/// **不刪字典也不刪教材**：那是使用者自己匯入的外部資料，重匯一份
/// Wiktionary 要好幾分鐘，而且跟「我想重新開始學」是兩件事。
/// 前端必須先讓使用者確認過才呼叫這個。
#[tauri::command]
pub async fn reset_progress(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<profiles::ResetSummary> {
    let summary = profiles::reset_progress(&state.db, ProfileId(profile_id)).await?;
    tracing::warn!(?summary, profile_id, "使用者重置了學習資料");
    Ok(summary)
}
