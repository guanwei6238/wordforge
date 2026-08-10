//! Tauri 後端：把核心 crate 的能力包成前端可呼叫的 command。
//!
//! 這一層刻意只做三件事：組裝依賴、轉換型別、把錯誤變成前端看得懂的字串。
//! 任何演算法都不該寫在這裡。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use time::OffsetDateTime;
use wordforge_core::model::{CardKind, LemmaId, ProfileId, Rating};
use wordforge_core::placement::{self, PlacementAnswer, PlacementResult};
use wordforge_core::srs::Scheduler;
use wordforge_db::Db;
use wordforge_db::dict::PlacementItem;
use wordforge_db::dict::{DictStats, SearchHit, WordDetail};
use wordforge_db::repo::{cards, lemmas, profiles};
use wordforge_import::{FreqFormat, ImportOptions, ImportProgress, ProgressSink};

/// 「算是會了」的 stability 門檻（天）。撐得過三週不複習才計入詞彙量。
const KNOWN_STABILITY_DAYS: f64 = 21.0;

/// 每天引入的新卡上限。
///
/// 一次把整個國中範圍 1600 個字設成到期，開啟 App 看到「待複習 1600」
/// 只會讓人直接關掉；FSRS 的排程也假設新卡是每天少量穩定引入的。
/// 15 張大約是每天 10 分鐘的量。
const NEW_CARDS_PER_DAY: i64 = 15;

/// 每天的複習上限，避免長假回來被幾百張卡淹沒。
const MAX_REVIEWS_PER_DAY: i64 = 200;

/// 今天的起點（UTC）。跨日換算之後會改成使用者所在時區。
fn day_start(now: OffsetDateTime) -> OffsetDateTime {
    now.replace_time(time::Time::MIDNIGHT)
}

pub struct AppState {
    db: Db,
    scheduler: Arc<Scheduler>,
    /// 匯入中斷旗標。使用者按下取消時設為 true，匯入迴圈在批次邊界檢查。
    import_cancel: Arc<AtomicBool>,
    /// 同時只允許一個匯入任務：兩個任務同時寫同一個 SQLite 檔只會互相卡住。
    import_running: Arc<AtomicBool>,
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
    wordforge_llm::LlmError,
    sqlx::Error,
    std::io::Error,
    anyhow::Error,
    tauri::Error,
);

type CmdResult<T> = std::result::Result<T, CommandError>;

/// 前端顯示複習卡所需的資料。
#[derive(Debug, Serialize)]
pub struct CardView {
    pub card_id: i64,
    pub lemma_id: i64,
    pub word: String,
    pub kind: String,
    pub state: String,
    pub gloss: Option<String>,
    pub translation: Option<String>,
    pub ipa: Option<String>,
    pub audio_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StudyStats {
    /// 今天要複習的張數（不含新卡）
    pub due_now: i64,
    /// 今天還能引入幾張新卡
    pub new_today: i64,
    pub known_words: i64,
    pub total_words: i64,
    pub reviews_today: i64,
}

#[derive(Debug, Deserialize)]
pub struct ReviewInput {
    pub card_id: i64,
    pub rating: u8,
    pub duration_ms: Option<u32>,
}

#[tauri::command]
async fn list_due_cards(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    limit: i64,
) -> CmdResult<Vec<CardView>> {
    let now = OffsetDateTime::now_utc();
    let due = cards::daily_queue(
        &state.db,
        ProfileId(profile_id),
        now,
        day_start(now),
        NEW_CARDS_PER_DAY,
        limit.min(MAX_REVIEWS_PER_DAY),
    )
    .await?;

    let mut views = Vec::with_capacity(due.len());
    for card in due {
        // 一張卡一次查詢；卡數上限是使用者設定的每日量（數十到數百），可接受。
        // 若日後成為瓶頸，改成一次 JOIN 撈回來即可。
        let row: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>)> =
            sqlx::query_as(
                "SELECT l.text,
                        (SELECT gloss FROM sense WHERE lemma_id = l.id ORDER BY sort_order LIMIT 1),
                        (SELECT translation FROM sense WHERE lemma_id = l.id ORDER BY sort_order LIMIT 1),
                        (SELECT ipa FROM pronunciation WHERE lemma_id = l.id LIMIT 1),
                        (SELECT audio_path FROM pronunciation WHERE lemma_id = l.id AND audio_path IS NOT NULL LIMIT 1)
                 FROM lemma l WHERE l.id = ?",
            )
            .bind(card.lemma_id.0)
            .fetch_optional(state.db.pool())
            .await?;

        let (word, gloss, translation, ipa, audio_path) =
            row.unwrap_or_else(|| ("?".into(), None, None, None, None));

        views.push(CardView {
            card_id: card.id.map(|c| c.0).unwrap_or_default(),
            lemma_id: card.lemma_id.0,
            word,
            kind: card.kind.as_str().to_string(),
            state: card.state.as_str().to_string(),
            gloss,
            translation,
            ipa,
            audio_path,
        });
    }
    Ok(views)
}

#[tauri::command]
async fn review_card(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    input: ReviewInput,
) -> CmdResult<()> {
    let rating =
        Rating::from_i64(input.rating as i64).ok_or_else(|| CommandError::new("評分必須是 1~4"))?;

    let now = OffsetDateTime::now_utc();
    // 重新讀出卡片，避免前端送來過期狀態
    let due = cards::daily_queue(
        &state.db,
        ProfileId(profile_id),
        now,
        day_start(now),
        NEW_CARDS_PER_DAY,
        MAX_REVIEWS_PER_DAY,
    )
    .await?;
    let card = due
        .into_iter()
        .find(|c| c.id.map(|id| id.0) == Some(input.card_id))
        .ok_or_else(|| CommandError::new("找不到這張到期的卡片"))?;

    let (next, log) = state
        .scheduler
        .review(&card, rating, now, input.duration_ms);
    cards::record_review(&state.db, &next, &log).await?;
    Ok(())
}

#[tauri::command]
async fn add_word(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lang: String,
    word: String,
) -> CmdResult<i64> {
    let now = OffsetDateTime::now_utc();
    let lemma_id = match lemmas::find_by_form(&state.db, &lang, &word).await? {
        Some(id) => id,
        None => {
            lemmas::upsert(
                &state.db,
                wordforge_db::repo::NewLemma {
                    lang: &lang,
                    text: &word,
                    pos: "",
                    freq_rank: None,
                    cefr: None,
                },
            )
            .await?
        }
    };

    // 預設只建立辨識卡；主動回想卡等使用者在設定裡開啟
    cards::ensure(
        &state.db,
        ProfileId(profile_id),
        lemma_id,
        CardKind::Recognition,
        now,
    )
    .await?;

    Ok(lemma_id.0)
}

#[tauri::command]
async fn study_stats(state: tauri::State<'_, AppState>, profile_id: i64) -> CmdResult<StudyStats> {
    let now = OffsetDateTime::now_utc();
    let (due_now, new_today) = cards::daily_counts(
        &state.db,
        ProfileId(profile_id),
        now,
        day_start(now),
        NEW_CARDS_PER_DAY,
    )
    .await?;
    let known: std::collections::HashSet<LemmaId> =
        cards::known_lemma_ids(&state.db, ProfileId(profile_id), KNOWN_STABILITY_DAYS).await?;

    let total_words: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE profile_id = ?")
        .bind(profile_id)
        .fetch_one(state.db.pool())
        .await?;

    let reviews_today: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM review_log r JOIN card c ON c.id = r.card_id
         WHERE c.profile_id = ? AND r.reviewed_at >= ?",
    )
    .bind(profile_id)
    .bind(now.date().to_string())
    .fetch_one(state.db.pool())
    .await?;

    Ok(StudyStats {
        due_now,
        new_today,
        known_words: known.len() as i64,
        total_words,
        reviews_today,
    })
}

// ---------------------------------------------------------------- 查字典

#[tauri::command]
async fn search_words(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lang: String,
    query: String,
    limit: i64,
) -> CmdResult<Vec<SearchHit>> {
    Ok(wordforge_db::dict::search(&state.db, &lang, &query, profile_id, limit).await?)
}

#[tauri::command]
async fn word_detail(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lemma_id: i64,
) -> CmdResult<Option<WordDetail>> {
    Ok(wordforge_db::dict::detail(&state.db, lemma_id, profile_id).await?)
}

#[tauri::command]
async fn dictionary_stats(state: tauri::State<'_, AppState>) -> CmdResult<DictStats> {
    Ok(wordforge_db::dict::stats(&state.db).await?)
}

/// 把查到的字加進牌組。`kinds` 空著就只建立辨識卡。
#[tauri::command]
async fn add_lemma_to_deck(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lemma_id: i64,
    kinds: Vec<String>,
) -> CmdResult<()> {
    let now = OffsetDateTime::now_utc();
    let kinds: Vec<CardKind> = if kinds.is_empty() {
        vec![CardKind::Recognition]
    } else {
        kinds
            .iter()
            .map(|k| match k.as_str() {
                "recognition" => Ok(CardKind::Recognition),
                "recall" => Ok(CardKind::Recall),
                "listening" => Ok(CardKind::Listening),
                "spelling" => Ok(CardKind::Spelling),
                other => Err(CommandError::new(format!("未知的卡片類型：{other}"))),
            })
            .collect::<CmdResult<_>>()?
    };

    for kind in kinds {
        cards::ensure(
            &state.db,
            ProfileId(profile_id),
            LemmaId(lemma_id),
            kind,
            now,
        )
        .await?;
    }
    Ok(())
}

/// 一個標籤的字數與牌組進度。
#[derive(Debug, Serialize)]
pub struct TagSummary {
    pub tag: String,
    pub total: i64,
    pub in_deck: i64,
}

#[tauri::command]
async fn deck_tags(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lang: String,
) -> CmdResult<Vec<TagSummary>> {
    let rows = cards::tag_summary(&state.db, ProfileId(profile_id), &lang).await?;
    Ok(rows
        .into_iter()
        .map(|(tag, total, in_deck)| TagSummary {
            tag,
            total,
            in_deck,
        })
        .collect())
}

/// 依考試範圍批次加入單字，例如把國中會考範圍的字全部排進複習。
#[tauri::command]
async fn add_words_by_tag(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lang: String,
    tag: String,
    limit: i64,
) -> CmdResult<u64> {
    Ok(cards::add_by_tag(
        &state.db,
        ProfileId(profile_id),
        cards::AddByTag {
            lang: &lang,
            tag: &tag,
            kinds: &[CardKind::Recognition],
            limit,
            // 功能詞不做成卡片，理由見 wordforge_core::wordlist
            skip_function_words: true,
            // 分級測驗說已經會的字就不要再排進來
            min_freq_rank: start_rank(&state.db, profile_id).await?,
        },
        OffsetDateTime::now_utc(),
    )
    .await?)
}

// ---------------------------------------------------------------- 發音

/// 朗讀一個字。
///
/// 目前用系統內建的語音合成（Linux 是 speech-dispatcher）。真人錄音品質好很多，
/// 之後會以 Wiktionary 的音檔為優先來源，TTS 退為後備。
#[tauri::command]
async fn speak(text: String, lang: String) -> CmdResult<()> {
    // 語音合成會阻塞到唸完，不能佔住 async runtime 的執行緒
    tauri::async_runtime::spawn_blocking(move || wordforge_tts::speak(&text, &lang))
        .await
        .map_err(|e| CommandError::new(e.to_string()))?
        .map_err(|e| CommandError::new(e.to_string()))
}

#[tauri::command]
fn speech_available() -> bool {
    wordforge_tts::is_available()
}

// ---------------------------------------------------------------- 分級測驗

/// 每個詞頻層抽幾題。七層共 35 題，大約三分鐘。
const PLACEMENT_ITEMS_PER_BAND: i64 = 5;

#[tauri::command]
async fn placement_items(
    state: tauri::State<'_, AppState>,
    lang: String,
) -> CmdResult<Vec<PlacementItem>> {
    Ok(wordforge_db::dict::sample_for_placement(
        &state.db,
        &lang,
        &placement::default_bands(),
        PLACEMENT_ITEMS_PER_BAND,
    )
    .await?)
}

/// 收下測驗結果：估計詞彙量、記住起始詞頻，並把牌組裡太簡單的新卡收起來。
#[derive(Debug, Serialize)]
pub struct PlacementOutcome {
    #[serde(flatten)]
    pub result: PlacementResult,
    /// 被收起來的「早就會了」的卡片數
    pub suspended_cards: u64,
}

#[tauri::command]
async fn submit_placement(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lang: String,
    answers: Vec<PlacementAnswer>,
) -> CmdResult<PlacementOutcome> {
    let result = placement::estimate(&placement::default_bands(), &answers);

    // 起始詞頻存進 profile，之後加入新字都會從這裡開始
    sqlx::query(
        "UPDATE profile
         SET settings_json = json_set(
                 CASE WHEN json_valid(settings_json) THEN settings_json ELSE '{}' END,
                 '$.start_rank', ?,
                 '$.estimated_vocabulary', ?)
         WHERE id = ?",
    )
    .bind(result.start_rank)
    .bind(result.estimated_vocabulary)
    .bind(profile_id)
    .execute(state.db.pool())
    .await?;

    let suspended =
        cards::suspend_easy_new_cards(&state.db, ProfileId(profile_id), &lang, result.start_rank)
            .await?;

    Ok(PlacementOutcome {
        result,
        suspended_cards: suspended,
    })
}

/// 讀出 profile 設定裡的起始詞頻；沒做過測驗就是 0（從頭開始）。
async fn start_rank(db: &Db, profile_id: i64) -> CmdResult<i64> {
    let rank: Option<i64> = sqlx::query_scalar(
        "SELECT CAST(json_extract(settings_json, '$.start_rank') AS INTEGER)
         FROM profile WHERE id = ? AND json_valid(settings_json)",
    )
    .bind(profile_id)
    .fetch_optional(db.pool())
    .await?
    .flatten();
    Ok(rank.unwrap_or(0))
}

// ---------------------------------------------------------------- 匯入

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
struct TauriProgress {
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
async fn start_import(
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
fn cancel_import(state: tauri::State<'_, AppState>) {
    state.import_cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn import_running(state: tauri::State<'_, AppState>) -> bool {
    state.import_running.load(Ordering::SeqCst)
}

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
                scheduler: Arc::new(Scheduler::default()),
                import_cancel: Arc::new(AtomicBool::new(false)),
                import_running: Arc::new(AtomicBool::new(false)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_due_cards,
            review_card,
            add_word,
            study_stats,
            search_words,
            word_detail,
            dictionary_stats,
            add_lemma_to_deck,
            deck_tags,
            add_words_by_tag,
            speak,
            speech_available,
            placement_items,
            submit_placement,
            start_import,
            cancel_import,
            import_running,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 應用程式啟動失敗");
}
