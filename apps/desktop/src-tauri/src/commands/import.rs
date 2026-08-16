//! 匯入字典與詞頻表。
//!
//! 匯入會跑很久（Wiktionary 那份是幾百萬行），所以進度用事件回報，
//! 而且要能中止。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use tauri::{AppHandle, Emitter};
use wordforge_import::{FreqFormat, ImportOptions, ImportProgress, ProgressSink};

use crate::{AppState, CmdResult, CommandError};

/// 匯入的檔案格式。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportKind {
    /// kaikki.org 的 Wiktionary JSONL
    WiktionaryJsonl,
    Csv,
    Tsv,
    /// 一行一個字的排序詞頻表
    FreqRanked,
    /// `word<TAB>count`
    FreqTab,
    /// `word,count`
    FreqComma,
}

/// 把匯入進度轉成 Tauri 事件送給前端。
pub struct TauriProgress {
    app: AppHandle,
    cancel: Arc<AtomicBool>,
}

impl ProgressSink for TauriProgress {
    fn report(&self, progress: &ImportProgress) {
        // 發送失敗代表視窗已經關了，這時沒必要中斷匯入——
        // 讓它把當前批次寫完比較乾淨。
        if let Err(e) = self.app.emit("import://progress", progress) {
            tracing::warn!(error = %e, "進度事件送不出去");
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// 開始匯入。這個 command 會立刻回傳，實際工作在背景進行。
///
/// 匯入一份完整的 Wiktionary 要好幾分鐘，讓 `invoke` 一直卡著的話，
/// 前端不但拿不到進度，還會誤以為當機。
#[tauri::command]
pub async fn start_import(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    kind: ImportKind,
    lang: String,
) -> CmdResult<()> {
    if state
        .import_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(CommandError::new("已經有一個匯入正在進行中"));
    }
    state.import_cancel.store(false, Ordering::SeqCst);

    let db = state.db.clone();
    let cancel = Arc::clone(&state.import_cancel);
    let running = Arc::clone(&state.import_running);
    let path = PathBuf::from(path);

    tauri::async_runtime::spawn(async move {
        let sink = TauriProgress {
            app: app.clone(),
            cancel,
        };
        let opts = ImportOptions::default();

        let result = match kind {
            ImportKind::WiktionaryJsonl => {
                wordforge_import::import_wiktionary_jsonl(&db, &path, &lang, &opts, &sink).await
            }
            ImportKind::Csv | ImportKind::Tsv => {
                let delimiter = if matches!(kind, ImportKind::Tsv) {
                    b'\t'
                } else {
                    b','
                };
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("我的單字表")
                    .to_string();
                wordforge_import::import_csv(&db, &path, &lang, delimiter, &name, &opts, &sink)
                    .await
            }
            ImportKind::FreqRanked | ImportKind::FreqTab | ImportKind::FreqComma => {
                let format = match kind {
                    ImportKind::FreqRanked => FreqFormat::RankedList,
                    ImportKind::FreqTab => FreqFormat::TabCounts,
                    _ => FreqFormat::CommaCounts,
                };
                // 詞頻表只是更新既有詞條的排名，沒有逐筆進度可回報
                wordforge_import::import_freq_list(&db, &path, &lang, format)
                    .await
                    .map(|updated| ImportProgress {
                        processed: updated,
                        imported: updated,
                        ..Default::default()
                    })
            }
        };

        let emitted = match result {
            Ok(progress) => {
                tracing::info!(?progress, "匯入完成");
                app.emit("import://done", progress)
            }
            Err(e) => {
                tracing::error!(error = %e, "匯入失敗");
                app.emit("import://error", e.to_string())
            }
        };
        if let Err(e) = emitted {
            tracing::warn!(error = %e, "完成事件送不出去");
        }

        running.store(false, Ordering::SeqCst);
    });

    Ok(())
}

/// 要求中止匯入。已經寫入的批次會保留。
#[tauri::command]
pub fn cancel_import(state: tauri::State<'_, AppState>) {
    state.import_cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub fn import_running(state: tauri::State<'_, AppState>) -> bool {
    state.import_running.load(Ordering::SeqCst)
}
