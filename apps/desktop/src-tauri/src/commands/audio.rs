//! 發音：系統 TTS 與離線發音檔。

use std::path::PathBuf;

use tauri::{AppHandle, Emitter, Manager};

use crate::{AppState, CmdResult, CommandError};

/// 朗讀一個字。
///
/// 目前用系統內建的語音合成（Linux 是 speech-dispatcher）。真人錄音品質好很多，
/// 之後會以 Wiktionary 的音檔為優先來源，TTS 退為後備。
#[tauri::command]
pub async fn speak(text: String, lang: String) -> CmdResult<()> {
    // 語音合成會阻塞到唸完，不能佔住 async runtime 的執行緒
    tauri::async_runtime::spawn_blocking(move || wordforge_tts::speak(&text, &lang))
        .await
        .map_err(|e| CommandError::new(e.to_string()))?
        .map_err(|e| CommandError::new(e.to_string()))
}

#[tauri::command]
pub fn speech_available() -> bool {
    wordforge_tts::is_available()
}

/// 音檔存放目錄：app 資料目錄下的 `audio/`。
pub fn audio_dir(app: &AppHandle) -> CmdResult<PathBuf> {
    Ok(app.path().app_data_dir()?.join("audio"))
}

/// 牌組裡有多少字有真人錄音、已經下載幾個。
#[tauri::command]
pub async fn audio_status(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<(i64, i64)> {
    Ok(wordforge_import::audio::audio_status(&state.db, profile_id).await?)
}

/// 幫牌組裡的字下載真人發音。
///
/// 只抓牌組裡、有網址、還沒下載的那些——完整音檔集有好幾 GB，
/// 但實際會聽到的只有這幾百個字。
#[tauri::command]
pub async fn download_audio(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    limit: i64,
) -> CmdResult<wordforge_import::audio::AudioProgress> {
    let dir = audio_dir(&app)?;
    let emitter = app.clone();
    Ok(
        wordforge_import::audio::download_for_deck(&state.db, profile_id, &dir, limit, move |p| {
            if let Err(e) = emitter.emit("audio://progress", p) {
                tracing::warn!(error = %e, "音檔進度事件送不出去");
            }
        })
        .await?,
    )
}

/// 把資料庫存的相對檔名換成前端能播放的絕對路徑。
#[tauri::command]
pub fn audio_file_path(app: AppHandle, name: String) -> CmdResult<String> {
    // 檔名一律由下載器用 id 組成，這裡再擋一次路徑穿越
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(CommandError::new("音檔名稱不合法"));
    }
    Ok(audio_dir(&app)?.join(name).to_string_lossy().into_owned())
}
