//! AI 後端的設定與連線測試。
//!
//! 金鑰**不進資料庫**：資料庫很可能被複製到雲端硬碟。另存權限 600 的檔案，
//! 送到 WebView 之前還要遮罩。

use std::path::PathBuf;

use tauri::{AppHandle, Manager};
use time::OffsetDateTime;
use wordforge_core::model::ProfileId;

use crate::commands::cards::day_start;
use crate::llm_settings::LlmSettings;
use crate::{AppState, CmdResult, CommandError};

/// LLM 設定檔放在 app 資料目錄，不進資料庫。
pub fn settings_dir(app: &AppHandle) -> CmdResult<PathBuf> {
    Ok(app.path().app_data_dir()?)
}

/// 這台機器上裝了哪些 AI CLI。
///
/// 設定頁一開就查，使用者不必自己猜「我到底有沒有裝」。
#[tauri::command]
pub async fn detect_ai_backends() -> Vec<wordforge_llm::CliAvailability> {
    wordforge_llm::detect_backends().await
}

#[tauri::command]
pub fn get_llm_settings(app: AppHandle) -> CmdResult<serde_json::Value> {
    Ok(LlmSettings::load(&settings_dir(&app)?).redacted())
}

/// 儲存 LLM 設定。
///
/// `api_key` 留空代表「不要動現有的」——前端拿到的是遮罩過的值，
/// 直接存回來會把真正的 key 洗掉。
#[tauri::command]
pub fn update_llm_settings(
    app: AppHandle,
    mut settings: LlmSettings,
) -> CmdResult<serde_json::Value> {
    let dir = settings_dir(&app)?;
    let existing = LlmSettings::load(&dir);

    if let Some(api) = settings.api.as_mut()
        && api.api_key.is_empty()
        && let Some(old) = existing.api.as_ref()
    {
        api.api_key = old.api_key.clone();
    }

    settings.save(&dir)?;
    Ok(settings.redacted())
}

/// 送一個極短的 prompt 確認後端真的能用。
///
/// 設定錯了要在這裡發現，而不是等使用者做完一整題才失敗。
#[tauri::command]
pub async fn test_llm(app: AppHandle) -> CmdResult<String> {
    let settings = LlmSettings::load(&settings_dir(&app)?);
    let Some(llm) = settings.build()? else {
        return Err(CommandError::new("還沒有設定 AI 後端"));
    };

    let req = wordforge_llm::ChatRequest {
        system: Some("你只輸出 JSON，不輸出任何其他文字。".into()),
        messages: vec![wordforge_llm::Message::user(
            r#"只輸出這個 JSON：{"ok": true}"#,
        )],
        json_only: true,
    };

    let resp = llm.chat(&req).await?;
    let value = resp
        .json()
        .map_err(|e| CommandError::new(format!("後端有回應，但格式看不懂：{e}")))?;

    if value.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        Ok(format!("連線正常（{}）", llm.model()))
    } else {
        Err(CommandError::new(format!(
            "後端回了東西但內容不對：{value}"
        )))
    }
}

/// 用量面板要的三份資料：今天、近七天、今天依用途拆開（用途、次數、總字元）。
type UsageReport = (
    wordforge_db::llm_usage::UsageSummary,
    wordforge_db::llm_usage::UsageSummary,
    Vec<(String, i64, i64)>,
);

/// 試跑一個模型，回報它在這台機器上能不能用。
///
/// 兩個 CLI 都沒有可以程式化查詢模型清單的方式，所以清單一定會過期。
/// 直接送一個最小 prompt 過去，成敗就是不會過期的答案。
#[tauri::command]
pub async fn probe_model(app: AppHandle, model: String) -> CmdResult<wordforge_llm::ModelProbe> {
    let settings = LlmSettings::load(&settings_dir(&app)?);
    let cli = settings
        .cli
        .ok_or_else(|| CommandError::new("目前的後端不是本機 CLI，沒有模型可以試"))?;
    Ok(wordforge_llm::probe_model(cli, &model).await)
}

/// LLM 用量：今天與最近七天。
#[tauri::command]
pub async fn llm_usage(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<UsageReport> {
    let now = OffsetDateTime::now_utc();
    let today = day_start(now);
    let week = today - time::Duration::days(6);

    Ok((
        wordforge_db::llm_usage::summary(&state.db, ProfileId(profile_id), today).await?,
        wordforge_db::llm_usage::summary(&state.db, ProfileId(profile_id), week).await?,
        wordforge_db::llm_usage::by_purpose(&state.db, ProfileId(profile_id), today).await?,
    ))
}
