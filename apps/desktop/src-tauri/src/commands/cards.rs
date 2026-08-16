//! 複習卡片：今天要複習什麼、按下難易度之後怎麼排。
//!
//! 這一層只做轉換：排程的算法在 `wordforge-core`，查詢在 `wordforge-db`。

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use wordforge_core::model::{CardId, CardKind, LemmaId, ProfileId, Rating};
use wordforge_core::srs::Scheduler;
use wordforge_db::Db;
use wordforge_db::repo::{cards, lemmas, profiles};

use crate::commands::placement::start_rank;
use crate::commands::profile::target_lang;
use crate::{AppState, CmdResult, CommandError};

/// 「算是會了」的 stability 門檻（天）。撐得過三週不複習才計入詞彙量。
pub const KNOWN_STABILITY_DAYS: f64 = 21.0;

/// 今天的起點（UTC）。跨日換算之後會改成使用者所在時區。
pub fn day_start(now: OffsetDateTime) -> OffsetDateTime {
    now.replace_time(time::Time::MIDNIGHT)
}

/// 今天的日期字串，用來判斷「額外額度」是不是今天給的。
pub fn today_key(now: OffsetDateTime) -> String {
    now.date().to_string()
}

/// 今天實際可以引入幾張新卡 = 每日上限 + 使用者今天自己加開的額度。
///
/// 這個額度必須存起來，不能只存在某一次回應裡：
/// 取佇列、送出評分、算統計是三個獨立的查詢，
/// 只要有一個還用預設上限，「再學 10 個」就會在下一次重新整理時消失。
pub async fn todays_new_limit(db: &Db, profile_id: i64, now: OffsetDateTime) -> CmdResult<i64> {
    let settings = profiles::study_settings(db, ProfileId(profile_id)).await?;
    let extra = profiles::extra_new_today(db, ProfileId(profile_id), &today_key(now)).await?;
    Ok(settings.new_per_day + extra)
}

/// 依使用者設定的目標留存率建立排程器。
///
/// 每次複習都重新建一個：成本只有幾個 f64，換來的是設定改完立刻生效，
/// 不必處理「設定變了但 AppState 裡的 scheduler 還是舊的」。
pub async fn scheduler_for(db: &Db, profile_id: i64) -> CmdResult<Scheduler> {
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
pub async fn list_due_cards(
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
pub async fn to_card_views(
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
pub async fn review_card(
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
pub async fn add_word(
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
pub async fn study_stats(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<StudyStats> {
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
pub async fn queue_status(
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
pub async fn study_more(
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
pub async fn unsuspend_cards(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    count: i64,
) -> CmdResult<u64> {
    Ok(cards::unsuspend(&state.db, ProfileId(profile_id), count).await?)
}

/// 把一張卡藏到明天。排程不動。
///
/// 「明天」用使用者的午夜而不是「24 小時後」——晚上十一點埋一張卡，
/// 使用者的預期是明天早上會看到，不是明天晚上。
#[tauri::command]
pub async fn bury_card(
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
pub async fn suspend_card(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    card_id: i64,
) -> CmdResult<bool> {
    Ok(cards::suspend(&state.db, ProfileId(profile_id), CardId(card_id)).await?)
}

/// 牌組裡未學的字少於這個數量時，自動補充。
///
/// 100 大約是一週的量：夠讓使用者不會突然沒東西學，
/// 又不會一次塞進幾千張讓「還剩幾個字」失去意義。
pub const REFILL_KEEP_AHEAD: i64 = 100;

/// 讀出自動補充要用哪個範圍。沒設定就不補。
pub async fn refill_tag(db: &Db, profile_id: i64) -> CmdResult<Option<String>> {
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
pub async fn refill_deck(state: &AppState, profile_id: i64) -> CmdResult<u64> {
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
pub async fn set_refill_tag(
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
pub async fn get_refill_tag(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<Option<String>> {
    refill_tag(&state.db, profile_id).await
}
