//! 卡片本身：建立、複習、今天該複習哪些。
//!
//! 這是使用者**最不能弄丟**的那一份資料——字典重匯得回來，
//! 「哪個字學到什麼程度」重匯不回來。所以這裡的每個寫入都要問一次
//! 「重跑一次會不會把進度打回原點」（見 [`ensure`]）。
//!
//! 熱路徑也在這裡：開 App 的第一個查詢是 [`due`]，
//! 十萬張卡的效能測試（`tests/perf.rs`）量的就是它。
//!
//! ## 旁邊那三個檔案
//!
//! - [`deck`]：牌組怎麼組起來（依標籤加字、暫停、擱置、補充）
//! - [`queue`]：今天發幾張（每日上限、加碼、空佇列的原因）
//! - [`knowledge`]：從卡片反推「他會哪些字」，出題與覆蓋率都靠它
//!
//! 三個子模組的東西都在這裡 re-export，所以 `cards::add_by_tag`
//! 這種既有的路徑一行都不用改。

mod deck;
mod knowledge;
mod queue;

pub use deck::{
    AddByTag, AutoRefill, add_by_tag, bury, count_other_languages, refill_if_needed, suspend,
    suspend_easy_new_cards, suspend_other_languages, tag_summary, unsuspend,
};
pub use knowledge::{
    known_lemma_ids, known_vocabulary, sample_known_words, shaky_words, words_with_few_sentences,
};
pub use queue::{QueueStatus, daily_counts, daily_queue, new_cards_introduced_today, queue_status};

use sqlx::Row;
use time::OffsetDateTime;
use wordforge_core::model::{
    Card, CardId, CardKind, CardState, LemmaId, MemoryState, ProfileId, Rating, ReviewLog,
};

use crate::ts::{self, ParseTs};
use crate::{Db, DbError, Result};

fn parse_kind(s: &str) -> Result<CardKind> {
    Ok(match s {
        "recognition" => CardKind::Recognition,
        "recall" => CardKind::Recall,
        "listening" => CardKind::Listening,
        "spelling" => CardKind::Spelling,
        other => {
            return Err(DbError::Decode {
                field: "card.kind",
                value: other.to_string(),
            });
        }
    })
}

fn parse_state(s: &str) -> Result<CardState> {
    Ok(match s {
        "new" => CardState::New,
        "learning" => CardState::Learning,
        "review" => CardState::Review,
        "relearning" => CardState::Relearning,
        other => {
            return Err(DbError::Decode {
                field: "card.state",
                value: other.to_string(),
            });
        }
    })
}

fn row_to_card(row: &sqlx::sqlite::SqliteRow) -> Result<Card> {
    let stability: Option<f64> = row.try_get("stability")?;
    let difficulty: Option<f64> = row.try_get("difficulty")?;
    let memory = match (stability, difficulty) {
        (Some(stability), Some(difficulty)) => Some(MemoryState {
            stability,
            difficulty,
        }),
        _ => None,
    };
    let last_review: Option<String> = row.try_get("last_review")?;

    Ok(Card {
        id: Some(CardId(row.try_get("id")?)),
        profile_id: ProfileId(row.try_get("profile_id")?),
        lemma_id: LemmaId(row.try_get("lemma_id")?),
        kind: parse_kind(row.try_get::<String, _>("kind")?.as_str())?,
        state: parse_state(row.try_get::<String, _>("state")?.as_str())?,
        memory,
        due: row.try_get::<String, _>("due")?.parse_ts("card.due")?,
        last_review: last_review
            .map(|s| s.parse_ts("card.last_review"))
            .transpose()?,
        step: row.try_get::<i64, _>("step")? as u8,
        reps: row.try_get::<i64, _>("reps")? as u32,
        lapses: row.try_get::<i64, _>("lapses")? as u32,
        scheduled_days: row.try_get("scheduled_days")?,
        suspended: row.try_get::<i64, _>("suspended")? != 0,
    })
}

/// 「這張卡沒有被埋葬」。
///
/// 埋葬存的是到期時間而不是布林值，所以判斷要跟 `now` 比。
/// 這樣就不需要一支「每天清掉埋葬旗標」的排程工作——
/// 那種工作在桌面應用程式上特別不可靠，使用者可能三天沒開 App。
///
/// 沒被埋葬的卡存空字串而不是 NULL。空字串排在任何 RFC 3339 時間戳
/// 之前，所以這個條件對它永遠成立——而且是**純範圍條件**，
/// 索引接得下去。寫成 `IS NULL OR <= ?` 的話那個 OR 會讓索引
/// 在這一欄斷掉，連帶後面的 due 也用不到：十萬張卡實測 200 ms 起跳。
const NOT_BURIED: &str = "buried_until <= ?";

const SELECT_CARD: &str = "SELECT id, profile_id, lemma_id, kind, state, step, stability,
    difficulty, due, last_review, reps, lapses, scheduled_days, suspended,
    buried_until FROM card";

/// 取得（必要時建立）某個字的某種卡片。
pub async fn ensure(
    db: &Db,
    profile_id: ProfileId,
    lemma_id: LemmaId,
    kind: CardKind,
    now: OffsetDateTime,
) -> Result<Card> {
    sqlx::query(
        "INSERT INTO card (profile_id, lemma_id, kind, state, due)
         VALUES (?, ?, ?, 'new', ?)
         ON CONFLICT (profile_id, lemma_id, kind) DO NOTHING",
    )
    .bind(profile_id.0)
    .bind(lemma_id.0)
    .bind(kind.as_str())
    .bind(ts::to_sql(now))
    .execute(db.pool())
    .await?;

    let row = sqlx::query(&format!(
        "{SELECT_CARD} WHERE profile_id = ? AND lemma_id = ? AND kind = ?"
    ))
    .bind(profile_id.0)
    .bind(lemma_id.0)
    .bind(kind.as_str())
    .fetch_one(db.pool())
    .await?;

    row_to_card(&row)
}

/// 今天該複習的卡，最早到期的排前面。
pub async fn due(
    db: &Db,
    profile_id: ProfileId,
    now: OffsetDateTime,
    limit: i64,
) -> Result<Vec<Card>> {
    let rows = sqlx::query(&format!(
        "{SELECT_CARD} WHERE profile_id = ? AND suspended = 0 AND {NOT_BURIED} AND due <= ?
         ORDER BY due ASC LIMIT ?"
    ))
    .bind(profile_id.0)
    // NOT_BURIED 與 due 各要一個 now
    .bind(ts::to_sql(now))
    .bind(ts::to_sql(now))
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    rows.iter().map(row_to_card).collect()
}

/// 寫入一次複習：更新卡片狀態並附上複習紀錄。兩者必須同進同退。
pub async fn record_review(db: &Db, card: &Card, log: &ReviewLog) -> Result<()> {
    let card_id = card.id.ok_or(DbError::NotFound {
        entity: "card",
        id: -1,
    })?;

    let mut tx = db.pool().begin().await?;

    sqlx::query(
        "UPDATE card SET state = ?, step = ?, stability = ?, difficulty = ?, due = ?,
                         last_review = ?, reps = ?, lapses = ?, scheduled_days = ?
         WHERE id = ?",
    )
    .bind(card.state.as_str())
    .bind(card.step as i64)
    .bind(card.memory.map(|m| m.stability))
    .bind(card.memory.map(|m| m.difficulty))
    .bind(ts::to_sql(card.due))
    .bind(card.last_review.map(ts::to_sql))
    .bind(card.reps as i64)
    .bind(card.lapses as i64)
    .bind(card.scheduled_days)
    .bind(card_id.0)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO review_log (card_id, rating, state, stability, difficulty,
                                 elapsed_days, scheduled_days, reviewed_at, duration_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(card_id.0)
    .bind(log.rating as i64)
    .bind(log.state.as_str())
    .bind(log.memory.stability)
    .bind(log.memory.difficulty)
    .bind(log.elapsed_days)
    .bind(log.scheduled_days)
    .bind(ts::to_sql(log.reviewed_at))
    .bind(log.duration_ms.map(|d| d as i64))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// 複習歷程，供 FSRS optimizer 重新訓練個人化權重使用。
pub async fn review_history(
    db: &Db,
    profile_id: ProfileId,
) -> Result<Vec<(CardId, Rating, OffsetDateTime)>> {
    let rows = sqlx::query(
        "SELECT r.card_id, r.rating, r.reviewed_at
         FROM review_log r JOIN card c ON c.id = r.card_id
         WHERE c.profile_id = ?
         ORDER BY r.reviewed_at",
    )
    .bind(profile_id.0)
    .fetch_all(db.pool())
    .await?;

    rows.into_iter()
        .map(|r| {
            let rating =
                Rating::from_i64(r.try_get::<i64, _>("rating")?).ok_or(DbError::Decode {
                    field: "review_log.rating",
                    value: "超出 1..4".into(),
                })?;
            Ok((
                CardId(r.try_get("card_id")?),
                rating,
                r.try_get::<String, _>("reviewed_at")?
                    .parse_ts("review_log.reviewed_at")?,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use time::Duration;
    use wordforge_core::model::{CardKind, CardState, Rating};
    use wordforge_core::srs::Scheduler;

    use crate::repo::cards;
    use crate::repo::fixture::*;

    #[tokio::test]
    async fn ensure_card_is_idempotent() {
        let (db, profile) = setup().await;
        let apple = add_word(&db, "apple", 500).await;

        let a = cards::ensure(&db, profile, apple, CardKind::Recognition, t0())
            .await
            .unwrap();
        let b = cards::ensure(&db, profile, apple, CardKind::Recognition, t0())
            .await
            .unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.state, CardState::New);

        // 不同 kind 是不同的卡
        let c = cards::ensure(&db, profile, apple, CardKind::Recall, t0())
            .await
            .unwrap();
        assert_ne!(a.id, c.id);
    }

    #[tokio::test]
    async fn review_updates_card_and_appends_log() {
        let (db, profile) = setup().await;
        let apple = add_word(&db, "apple", 500).await;
        let card = cards::ensure(&db, profile, apple, CardKind::Recognition, t0())
            .await
            .unwrap();

        let scheduler = Scheduler::default();
        let (next, log) = scheduler.review(&card, Rating::Easy, t0(), Some(1_200));
        cards::record_review(&db, &next, &log).await.unwrap();

        let reloaded = cards::ensure(&db, profile, apple, CardKind::Recognition, t0())
            .await
            .unwrap();
        assert_eq!(reloaded.state, CardState::Review);
        assert_eq!(reloaded.reps, 1);
        assert!(reloaded.memory.unwrap().stability > 0.0);
        assert_eq!(reloaded.due, next.due, "時間必須原樣往返");

        let history = cards::review_history(&db, profile).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].1, Rating::Easy);
    }

    #[tokio::test]
    async fn due_query_respects_time_and_suspension() {
        let (db, profile) = setup().await;
        let a = add_word(&db, "alpha", 1).await;
        let b = add_word(&db, "beta", 2).await;
        cards::ensure(&db, profile, a, CardKind::Recognition, t0())
            .await
            .unwrap();
        let card_b = cards::ensure(
            &db,
            profile,
            b,
            CardKind::Recognition,
            t0() + Duration::days(3),
        )
        .await
        .unwrap();

        // 此刻只有 alpha 到期
        let now_due = cards::due(&db, profile, t0(), 10).await.unwrap();
        assert_eq!(now_due.len(), 1);
        assert_eq!(now_due[0].lemma_id, a);

        // 三天後兩張都到期，且照 due 由早到晚排序
        let later = cards::due(&db, profile, t0() + Duration::days(3), 10)
            .await
            .unwrap();
        assert_eq!(later.len(), 2);
        assert_eq!(later[0].lemma_id, a);

        // 暫停的卡不該出現
        sqlx::query("UPDATE card SET suspended = 1 WHERE id = ?")
            .bind(card_b.id.unwrap().0)
            .execute(db.pool())
            .await
            .unwrap();
        let filtered = cards::due(&db, profile, t0() + Duration::days(3), 10)
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);
    }
}
