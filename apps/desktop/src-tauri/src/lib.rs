//! Tauri 後端：把核心 crate 的能力包成前端可呼叫的 command。
//!
//! 這一層刻意只做三件事：組裝依賴、轉換型別、把錯誤變成前端看得懂的字串。
//! 任何演算法都不該寫在這裡。

mod llm_settings;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use time::OffsetDateTime;
use wordforge_core::model::{CardId, CardKind, LemmaId, ProfileId, Rating};
use wordforge_core::placement::{self, PlacementAnswer, PlacementResult};
use wordforge_core::srs::Scheduler;
use wordforge_db::Db;
use wordforge_db::dict::PlacementItem;
use wordforge_db::dict::{DictStats, SearchHit, WordDetail};
use wordforge_db::repo::{cards, lemmas, profiles};
use wordforge_import::{FreqFormat, ImportOptions, ImportProgress, ProgressSink};
use wordforge_practice::{ExerciseView, Feedback, GradeInput, PracticeEngine};

use llm_settings::LlmSettings;

/// 「算是會了」的 stability 門檻（天）。撐得過三週不複習才計入詞彙量。
const KNOWN_STABILITY_DAYS: f64 = 21.0;

/// 今天的起點（UTC）。跨日換算之後會改成使用者所在時區。
fn day_start(now: OffsetDateTime) -> OffsetDateTime {
    now.replace_time(time::Time::MIDNIGHT)
}

/// 今天的日期字串，用來判斷「額外額度」是不是今天給的。
fn today_key(now: OffsetDateTime) -> String {
    now.date().to_string()
}

/// 今天實際可以引入幾張新卡 = 每日上限 + 使用者今天自己加開的額度。
///
/// 這個額度必須存起來，不能只存在某一次回應裡：
/// 取佇列、送出評分、算統計是三個獨立的查詢，
/// 只要有一個還用預設上限，「再學 10 個」就會在下一次重新整理時消失。
async fn todays_new_limit(db: &Db, profile_id: i64, now: OffsetDateTime) -> CmdResult<i64> {
    let settings = profiles::study_settings(db, ProfileId(profile_id)).await?;
    let extra = profiles::extra_new_today(db, ProfileId(profile_id), &today_key(now)).await?;
    Ok(settings.new_per_day + extra)
}

/// 依使用者設定的目標留存率建立排程器。
///
/// 每次複習都重新建一個：成本只有幾個 f64，換來的是設定改完立刻生效，
/// 不必處理「設定變了但 AppState 裡的 scheduler 還是舊的」。
async fn scheduler_for(db: &Db, profile_id: i64) -> CmdResult<Scheduler> {
    let settings = profiles::study_settings(db, ProfileId(profile_id)).await?;
    Scheduler::new(
        wordforge_core::srs::FsrsParams::default(),
        wordforge_core::srs::SchedulerConfig {
            desired_retention: settings.desired_retention,
            ..Default::default()
        },
    )
    .map_err(|e| CommandError::new(e.to_string()))
}

#[tauri::command]
async fn get_study_settings(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<profiles::StudySettings> {
    Ok(profiles::study_settings(&state.db, ProfileId(profile_id)).await?)
}

/// 更新學習設定，回傳實際存下來的值（超出合理範圍會被夾住）。
#[tauri::command]
async fn update_study_settings(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    settings: profiles::StudySettings,
) -> CmdResult<profiles::StudySettings> {
    Ok(profiles::update_study_settings(&state.db, ProfileId(profile_id), settings).await?)
}

pub struct AppState {
    db: Db,
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
    wordforge_practice::PracticeError,
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
    // 牌組見底前先自動補上新字，使用者不必自己去牌組頁加
    refill_deck(&state, profile_id).await?;

    let now = OffsetDateTime::now_utc();
    let due = cards::daily_queue(
        &state.db,
        ProfileId(profile_id),
        now,
        day_start(now),
        todays_new_limit(&state.db, profile_id, now).await?,
        limit.min(
            profiles::study_settings(&state.db, ProfileId(profile_id))
                .await?
                .max_reviews_per_day,
        ),
    )
    .await?;

    to_card_views(&state.db, due).await
}

/// 一張卡顯示時要補的欄位：字、字義、翻譯、IPA、錄音檔名。
type CardDisplayRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// 把卡片補上顯示所需的字義與發音。
async fn to_card_views(
    db: &Db,
    cards: Vec<wordforge_core::model::Card>,
) -> CmdResult<Vec<CardView>> {
    let mut views = Vec::with_capacity(cards.len());
    for card in cards {
        // 一張卡一次查詢；卡數上限是使用者設定的每日量（數十到數百），可接受。
        // 若日後成為瓶頸，改成一次 JOIN 撈回來即可。
        // 發音要跨「同一個字的所有詞條」找：卡片指向 ECDICT 建的 lemma，
        // 但真人錄音掛在 Wiktionary 建的那筆上，兩者 id 不同。
        let row: Option<CardDisplayRow> = sqlx::query_as(
                "WITH family AS (
                     SELECT id FROM lemma
                     WHERE lang = (SELECT lang FROM lemma WHERE id = ?1)
                       AND normalized = (SELECT normalized FROM lemma WHERE id = ?1)
                 )
                 SELECT l.text,
                        (SELECT gloss FROM sense WHERE lemma_id = l.id ORDER BY sort_order LIMIT 1),
                        (SELECT translation FROM sense WHERE lemma_id = l.id ORDER BY sort_order LIMIT 1),
                        (SELECT ipa FROM pronunciation
                         WHERE lemma_id IN (SELECT id FROM family) AND ipa IS NOT NULL LIMIT 1),
                        (SELECT audio_path FROM pronunciation
                         WHERE lemma_id IN (SELECT id FROM family) AND audio_path IS NOT NULL LIMIT 1)
                 FROM lemma l WHERE l.id = ?1",
            )
            .bind(card.lemma_id.0)
            .fetch_optional(db.pool())
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
        todays_new_limit(&state.db, profile_id, now).await?,
        profiles::study_settings(&state.db, ProfileId(profile_id))
            .await?
            .max_reviews_per_day,
    )
    .await?;
    let card = due
        .into_iter()
        .find(|c| c.id.map(|id| id.0) == Some(input.card_id))
        .ok_or_else(|| CommandError::new("找不到這張到期的卡片"))?;

    let (next, log) =
        scheduler_for(&state.db, profile_id)
            .await?
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
    let lemma_id = match lemmas::base_form(&state.db, &lang, &word).await? {
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
        todays_new_limit(&state.db, profile_id, now).await?,
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

/// 佇列狀態。「沒有卡片可做」有好幾種原因，UI 要分得出來。
#[derive(Debug, Serialize)]
pub struct QueueStatusView {
    pub due_reviews: i64,
    pub new_today: i64,
    pub new_in_deck: i64,
    pub suspended: i64,
    /// 下一張卡到期的時間（RFC 3339），沒有就是 null
    pub next_due: Option<String>,
    /// 每日新卡上限，讓 UI 說得出「今天的 15 張已經學完」
    pub new_per_day: i64,
}

#[tauri::command]
async fn queue_status(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<QueueStatusView> {
    refill_deck(&state, profile_id).await?;

    let now = OffsetDateTime::now_utc();
    let new_per_day = todays_new_limit(&state.db, profile_id, now).await?;
    let s = cards::queue_status(
        &state.db,
        ProfileId(profile_id),
        now,
        day_start(now),
        new_per_day,
    )
    .await?;

    Ok(QueueStatusView {
        due_reviews: s.due_reviews,
        new_today: s.new_today,
        new_in_deck: s.new_in_deck,
        suspended: s.suspended,
        next_due: s.next_due.and_then(|d| {
            d.format(&time::format_description::well_known::Rfc3339)
                .ok()
        }),
        new_per_day,
    })
}

/// 超出每日上限再多學幾個新字。
///
/// 每日上限是為了讓學習可持續，但今天特別有空的時候不該被擋住——
/// 這是使用者自己的選擇，不是系統該替他決定的事。
#[tauri::command]
async fn study_more(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    extra: i64,
) -> CmdResult<Vec<CardView>> {
    let now = OffsetDateTime::now_utc();
    let extra = extra.clamp(0, 500);

    // 累加到今天的額度上並存起來。前端接著會重新取佇列，
    // 那個查詢也讀同一個設定，兩邊才會一致。
    let settings = profiles::study_settings(&state.db, ProfileId(profile_id)).await?;
    let total_extra =
        profiles::add_extra_new_today(&state.db, ProfileId(profile_id), &today_key(now), extra)
            .await?;

    let cards = cards::daily_queue(
        &state.db,
        ProfileId(profile_id),
        now,
        day_start(now),
        settings.new_per_day + total_extra,
        settings.max_reviews_per_day,
    )
    .await?;
    to_card_views(&state.db, cards).await
}

/// 恢復被分級測驗收起來的卡。
#[tauri::command]
async fn unsuspend_cards(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    count: i64,
) -> CmdResult<u64> {
    Ok(cards::unsuspend(&state.db, ProfileId(profile_id), count).await?)
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
    // 扣掉分級測驗判定「已經會了」的字，顯示的數字才是真正能加的
    let min_rank = start_rank(&state.db, profile_id).await?;
    let rows = cards::tag_summary(&state.db, ProfileId(profile_id), &lang, min_rank).await?;
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
            skip_existing: false,
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

/// 音檔存放目錄：app 資料目錄下的 `audio/`。
fn audio_dir(app: &AppHandle) -> CmdResult<PathBuf> {
    Ok(app.path().app_data_dir()?.join("audio"))
}

/// 牌組裡有多少字有真人錄音、已經下載幾個。
#[tauri::command]
async fn audio_status(state: tauri::State<'_, AppState>, profile_id: i64) -> CmdResult<(i64, i64)> {
    Ok(wordforge_import::audio::audio_status(&state.db, profile_id).await?)
}

/// 幫牌組裡的字下載真人發音。
///
/// 只抓牌組裡、有網址、還沒下載的那些——完整音檔集有好幾 GB，
/// 但實際會聽到的只有這幾百個字。
#[tauri::command]
async fn download_audio(
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
fn audio_file_path(app: AppHandle, name: String) -> CmdResult<String> {
    // 檔名一律由下載器用 id 組成，這裡再擋一次路徑穿越
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(CommandError::new("音檔名稱不合法"));
    }
    Ok(audio_dir(&app)?.join(name).to_string_lossy().into_owned())
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

/// 牌組裡未學的字少於這個數量時，自動補充。
///
/// 100 大約是一週的量：夠讓使用者不會突然沒東西學，
/// 又不會一次塞進幾千張讓「還剩幾個字」失去意義。
const REFILL_KEEP_AHEAD: i64 = 100;

/// 這個 profile 在學什麼語言。
///
/// 先前每個地方都硬編 `"en"`——那讓「換一份字典就能學另一種語言」
/// 這個設計目標名存實亡。
async fn target_lang(db: &Db, profile_id: i64) -> CmdResult<String> {
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
async fn profile_languages(
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
async fn set_profile_languages(
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
async fn suspend_other_language_cards(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<u64> {
    let target = target_lang(&state.db, profile_id).await?;
    Ok(cards::suspend_other_languages(&state.db, ProfileId(profile_id), &target).await?)
}

/// 匯入了哪些語言的字典。設定頁的目標語言選單用它，
/// 使用者才不會選到一個沒有字典的語言。
#[tauri::command]
async fn dictionary_languages(state: tauri::State<'_, AppState>) -> CmdResult<Vec<(String, i64)>> {
    Ok(wordforge_db::dict::languages(&state.db).await?)
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
async fn probe_model(app: AppHandle, model: String) -> CmdResult<wordforge_llm::ModelProbe> {
    let settings = LlmSettings::load(&settings_dir(&app)?);
    let cli = settings
        .cli
        .ok_or_else(|| CommandError::new("目前的後端不是本機 CLI，沒有模型可以試"))?;
    Ok(wordforge_llm::probe_model(cli, &model).await)
}

/// LLM 用量：今天與最近七天。
#[tauri::command]
async fn llm_usage(state: tauri::State<'_, AppState>, profile_id: i64) -> CmdResult<UsageReport> {
    let now = OffsetDateTime::now_utc();
    let today = day_start(now);
    let week = today - time::Duration::days(6);

    Ok((
        wordforge_db::llm_usage::summary(&state.db, ProfileId(profile_id), today).await?,
        wordforge_db::llm_usage::summary(&state.db, ProfileId(profile_id), week).await?,
        wordforge_db::llm_usage::by_purpose(&state.db, ProfileId(profile_id), today).await?,
    ))
}

/// 把一張卡藏到明天。排程不動。
///
/// 「明天」用使用者的午夜而不是「24 小時後」——晚上十一點埋一張卡，
/// 使用者的預期是明天早上會看到，不是明天晚上。
#[tauri::command]
async fn bury_card(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    card_id: i64,
) -> CmdResult<bool> {
    let now = OffsetDateTime::now_utc();
    let tomorrow = day_start(now) + time::Duration::days(1);
    Ok(cards::bury(&state.db, ProfileId(profile_id), CardId(card_id), tomorrow).await?)
}

/// 收起一張卡，要到牌組頁主動恢復才會回來。
#[tauri::command]
async fn suspend_card(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    card_id: i64,
) -> CmdResult<bool> {
    Ok(cards::suspend(&state.db, ProfileId(profile_id), CardId(card_id)).await?)
}

// ------------------------------------------------------------------ 教材

/// 匯入一份教材。
///
/// 語言用 profile 的目標語言，不讓前端傳——教材跟正在學的語言不一致的話，
/// 詞表會整份對不上，而那個失敗看起來像「匯入成功但沒有效果」。
#[tauri::command]
async fn import_material(
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
async fn list_materials(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<Vec<wordforge_db::material::Material>> {
    Ok(wordforge_db::material::list(&state.db, ProfileId(profile_id)).await?)
}

#[tauri::command]
async fn delete_material(
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
async fn material_coverage(
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

/// 讀出自動補充要用哪個範圍。沒設定就不補。
async fn refill_tag(db: &Db, profile_id: i64) -> CmdResult<Option<String>> {
    let tag: Option<String> = sqlx::query_scalar(
        "SELECT json_extract(settings_json, '$.refill_tag')
         FROM profile WHERE id = ? AND json_valid(settings_json)",
    )
    .bind(profile_id)
    .fetch_optional(db.pool())
    .await?
    .flatten();
    Ok(tag.filter(|t| !t.is_empty()))
}

/// 需要的話補充牌組。每次取佇列前呼叫，成本是一個 COUNT。
async fn refill_deck(state: &AppState, profile_id: i64) -> CmdResult<u64> {
    let Some(tag) = refill_tag(&state.db, profile_id).await? else {
        return Ok(0);
    };
    let lang = target_lang(&state.db, profile_id).await?;
    Ok(cards::refill_if_needed(
        &state.db,
        ProfileId(profile_id),
        &lang,
        &cards::AutoRefill {
            tag: &tag,
            keep_ahead: REFILL_KEEP_AHEAD,
            min_freq_rank: start_rank(&state.db, profile_id).await?,
        },
        OffsetDateTime::now_utc(),
    )
    .await?)
}

/// 設定（或關閉）自動補充的範圍。
#[tauri::command]
async fn set_refill_tag(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    tag: Option<String>,
) -> CmdResult<u64> {
    sqlx::query(
        "UPDATE profile
         SET settings_json = json_set(
                 CASE WHEN json_valid(settings_json) THEN settings_json ELSE '{}' END,
                 '$.refill_tag', ?)
         WHERE id = ?",
    )
    .bind(tag.as_deref().unwrap_or(""))
    .bind(profile_id)
    .execute(state.db.pool())
    .await?;

    // 設定完立刻補一次，使用者不用等到下次開啟
    refill_deck(&state, profile_id).await
}

#[tauri::command]
async fn get_refill_tag(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<Option<String>> {
    refill_tag(&state.db, profile_id).await
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

// ---------------------------------------------------------------- AI 練習

/// LLM 設定檔放在 app 資料目錄，不進資料庫。
fn settings_dir(app: &AppHandle) -> CmdResult<PathBuf> {
    Ok(app.path().app_data_dir()?)
}

/// 這台機器上裝了哪些 AI CLI。
///
/// 設定頁一開就查，使用者不必自己猜「我到底有沒有裝」。
#[tauri::command]
async fn detect_ai_backends() -> Vec<wordforge_llm::CliAvailability> {
    wordforge_llm::detect_backends().await
}

#[tauri::command]
fn get_llm_settings(app: AppHandle) -> CmdResult<serde_json::Value> {
    Ok(LlmSettings::load(&settings_dir(&app)?).redacted())
}

/// 儲存 LLM 設定。
///
/// `api_key` 留空代表「不要動現有的」——前端拿到的是遮罩過的值，
/// 直接存回來會把真正的 key 洗掉。
#[tauri::command]
fn update_llm_settings(app: AppHandle, mut settings: LlmSettings) -> CmdResult<serde_json::Value> {
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
async fn test_llm(app: AppHandle) -> CmdResult<String> {
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

/// 目前能不能出題，以及會出什麼樣的題。
#[derive(Debug, Serialize)]
pub struct PracticeStatus {
    pub llm_ready: bool,
    pub vocabulary: i64,
    pub weak_grammar: Vec<String>,
    /// 依現在的程度，系統會出哪種題
    pub recommended: String,
    /// 每種題型需要的最低詞彙量，讓 UI 說得出「再多學 N 個字就能做閱讀測驗」
    pub requirements: Vec<(String, i64)>,
}

#[tauri::command]
async fn practice_status(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<PracticeStatus> {
    use wordforge_core::practice::ExerciseKind::*;

    let settings = LlmSettings::load(&settings_dir(&app)?);
    let llm_ready = settings.build().map(|p| p.is_some()).unwrap_or(false);

    // 沒有 LLM 也要能顯示程度，讓使用者知道設定完會拿到什麼
    let dummy = wordforge_llm::CliLlm::new(wordforge_llm::CliConfig::claude_code())
        .map_err(|e| CommandError::new(e.to_string()))?;
    let engine = PracticeEngine::for_profile(&state.db, &dummy, profile_id).await?;
    let learner = engine
        .learner_profile(profile_id, OffsetDateTime::now_utc())
        .await?;

    Ok(PracticeStatus {
        llm_ready,
        vocabulary: learner.vocabulary,
        weak_grammar: learner.weak_grammar.clone(),
        recommended: wordforge_core::practice::recommend_kind(&learner)
            .as_str()
            .to_string(),
        requirements: [
            TranslationToNative,
            TranslationToTarget,
            Cloze,
            Grammar,
            Reading,
        ]
        .into_iter()
        .map(|k| (k.as_str().to_string(), k.min_vocabulary()))
        .collect(),
    })
}

#[tauri::command]
async fn generate_exercise(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    kind: Option<String>,
    // 指定教材時，模型只能從那本書取材
    material_id: Option<i64>,
) -> CmdResult<ExerciseView> {
    let settings = LlmSettings::load(&settings_dir(&app)?);
    let llm = settings
        .build()?
        .ok_or_else(|| CommandError::new("還沒有設定 AI 後端，請先到設定頁選一個"))?;

    let kind = match kind.as_deref() {
        None | Some("") | Some("auto") => None,
        Some(k) => Some(parse_exercise_kind(k)?),
    };

    let engine = PracticeEngine::for_profile(&state.db, llm.as_ref(), profile_id)
        .await?
        .with_material(material_id);
    Ok(engine
        .generate(profile_id, kind, OffsetDateTime::now_utc())
        .await?)
}

#[tauri::command]
async fn grade_exercise(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    input: GradeInput,
) -> CmdResult<Feedback> {
    let settings = LlmSettings::load(&settings_dir(&app)?);
    let llm = settings
        .build()?
        .ok_or_else(|| CommandError::new("還沒有設定 AI 後端"))?;

    let engine = PracticeEngine::for_profile(&state.db, llm.as_ref(), profile_id).await?;
    Ok(engine
        .grade(profile_id, &input, OffsetDateTime::now_utc())
        .await?)
}

fn parse_exercise_kind(s: &str) -> CmdResult<wordforge_core::practice::ExerciseKind> {
    use wordforge_core::practice::ExerciseKind::*;
    Ok(match s {
        "translation_to_target" => TranslationToTarget,
        "translation_to_native" => TranslationToNative,
        "cloze" => Cloze,
        "reading" => Reading,
        "grammar" => Grammar,
        other => return Err(CommandError::new(format!("未知的題型：{other}"))),
    })
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
            queue_status,
            study_more,
            unsuspend_cards,
            set_refill_tag,
            get_refill_tag,
            get_study_settings,
            update_study_settings,
            profile_languages,
            set_profile_languages,
            suspend_other_language_cards,
            dictionary_languages,
            probe_model,
            llm_usage,
            bury_card,
            suspend_card,
            import_material,
            list_materials,
            delete_material,
            material_coverage,
            detect_ai_backends,
            get_llm_settings,
            update_llm_settings,
            test_llm,
            practice_status,
            generate_exercise,
            grade_exercise,
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
            audio_status,
            download_audio,
            audio_file_path,
            start_import,
            cancel_import,
            import_running,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 應用程式啟動失敗");
}
