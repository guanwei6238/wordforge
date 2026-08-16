//! Tauri 後端：把核心 crate 的能力包成前端可呼叫的 command。
//!
//! 這一層刻意只做三件事：組裝依賴、轉換型別、把錯誤變成前端看得懂的字串。
//! 任何演算法都不該寫在這裡。

mod commands;
mod llm_settings;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;
use tauri::Manager;
use time::OffsetDateTime;
use wordforge_db::Db;
use wordforge_db::repo::profiles;

pub struct AppState {
    pub(crate) db: Db,
    /// 匯入中斷旗標。使用者按下取消時設為 true，匯入迴圈在批次邊界檢查。
    pub(crate) import_cancel: Arc<AtomicBool>,
    /// 同時只允許一個匯入任務：兩個任務同時寫同一個 SQLite 檔只會互相卡住。
    pub(crate) import_running: Arc<AtomicBool>,
}

/// Tauri command 的錯誤型別。前端只需要一段可顯示的訊息。
///
/// 這裡逐一列出來源錯誤而不用泛型 blanket impl：
/// `impl<E: Display> From<E>` 會在日後替 `CommandError` 加上 `Display` 時
/// 與標準庫的 `From<T> for T` 撞在一起。
#[derive(Debug, Serialize)]
pub struct CommandError {
    message: String,
}

impl CommandError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

macro_rules! from_error {
    ($($ty:ty),* $(,)?) => {
        $(impl From<$ty> for CommandError {
            fn from(e: $ty) -> Self {
                Self::new(e.to_string())
            }
        })*
    };
}

from_error!(
    wordforge_db::DbError,
    wordforge_dict::DictError,
    wordforge_import::ImportError,
    wordforge_practice::PracticeError,
    wordforge_llm::LlmError,
    sqlx::Error,
    std::io::Error,
    anyhow::Error,
    tauri::Error,
);

pub(crate) type CmdResult<T> = std::result::Result<T, CommandError>;

// ---------------------------------------------------------------- 查字典

// ---------------------------------------------------------------- 發音

// ---------------------------------------------------------------- 分級測驗

// ------------------------------------------------------------------ 教材

// ---------------------------------------------------------------- AI 練習

// ---------------------------------------------------------------- 文法

// ---------------------------------------------------------------- 情境主題

// ---------------------------------------------------------------- 匯入

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wordforge=info".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db_path = dir.join("wordforge.db");
            tracing::info!(path = %db_path.display(), "開啟資料庫");

            // Tauri 的 setup 是同步的，這裡阻塞等待初始化完成；
            // 資料庫還沒開好就讓 UI 出現只會得到一堆錯誤。
            let db = tauri::async_runtime::block_on(Db::open(&db_path))?;

            // 首次啟動時建立預設 profile
            tauri::async_runtime::block_on(async {
                if profiles::list(&db).await?.is_empty() {
                    profiles::create(&db, "我", "zh-TW", "en", OffsetDateTime::now_utc()).await?;
                }
                Ok::<_, wordforge_db::DbError>(())
            })?;

            app.manage(AppState {
                db,
                import_cancel: Arc::new(AtomicBool::new(false)),
                import_running: Arc::new(AtomicBool::new(false)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::cards::list_due_cards,
            commands::cards::review_card,
            commands::cards::add_word,
            commands::cards::study_stats,
            commands::cards::queue_status,
            commands::cards::study_more,
            commands::cards::unsuspend_cards,
            commands::cards::set_refill_tag,
            commands::cards::get_refill_tag,
            commands::profile::get_study_settings,
            commands::profile::update_study_settings,
            commands::profile::profile_languages,
            commands::profile::set_profile_languages,
            commands::profile::suspend_other_language_cards,
            commands::dict::dictionary_languages,
            commands::llm::probe_model,
            commands::llm::llm_usage,
            commands::cards::bury_card,
            commands::cards::suspend_card,
            commands::material::import_material,
            commands::material::list_materials,
            commands::material::delete_material,
            commands::material::material_coverage,
            commands::llm::detect_ai_backends,
            commands::llm::get_llm_settings,
            commands::llm::update_llm_settings,
            commands::llm::test_llm,
            commands::practice::practice_status,
            commands::practice::generate_exercise,
            commands::practice::grade_exercise,
            commands::practice::list_exercises,
            commands::practice::delete_exercise,
            commands::grammar::list_grammar,
            commands::grammar::save_grammar,
            commands::grammar::delete_grammar,
            commands::topics::list_topics,
            commands::topics::save_topic,
            commands::topics::delete_topic,
            commands::grammar::explain_grammar,
            commands::grammar::set_grammar_known,
            commands::grammar::import_grammar,
            commands::practice::load_exercise,
            commands::profile::reset_progress,
            commands::dict::search_words,
            commands::dict::word_detail,
            commands::dict::dictionary_stats,
            commands::dict::add_lemma_to_deck,
            commands::dict::deck_tags,
            commands::dict::add_words_by_tag,
            commands::audio::speak,
            commands::audio::speech_available,
            commands::placement::placement_items,
            commands::placement::submit_placement,
            commands::audio::audio_status,
            commands::audio::download_audio,
            commands::audio::audio_file_path,
            commands::import::start_import,
            commands::import::cancel_import,
            commands::import::import_running,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 應用程式啟動失敗");
}
