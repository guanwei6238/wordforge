//! 資料存取函數。
//!
//! 刻意寫成自由函數而不是一堆 struct：查詢就是查詢，不需要為了物件導向
//! 包一層。要換掉實作時，呼叫端改 import 路徑即可。

use std::collections::HashSet;

use sqlx::Row;
use time::OffsetDateTime;
use wordforge_core::model::{
    Card, CardId, CardKind, CardState, LemmaId, MemoryState, ProfileId, Rating, ReviewLog,
};

use crate::ts::{self, ParseTs};
use crate::{Db, DbError, Result};

// ---------------------------------------------------------------- 列舉轉換

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

// ---------------------------------------------------------------- profile

pub mod profiles {
    use super::*;

    pub async fn create(
        db: &Db,
        name: &str,
        native_lang: &str,
        target_lang: &str,
        now: OffsetDateTime,
    ) -> Result<ProfileId> {
        let id = sqlx::query(
            "INSERT INTO profile (name, native_lang, target_lang, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(name)
        .bind(native_lang)
        .bind(target_lang)
        .bind(ts::to_sql(now))
        .execute(db.pool())
        .await?
        .last_insert_rowid();

        Ok(ProfileId(id))
    }

    pub async fn list(db: &Db) -> Result<Vec<(ProfileId, String)>> {
        let rows = sqlx::query("SELECT id, name FROM profile ORDER BY id")
            .fetch_all(db.pool())
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (ProfileId(r.get("id")), r.get("name")))
            .collect())
    }
}

// ---------------------------------------------------------------- lemma

/// 要寫入的新詞條。
#[derive(Debug, Clone)]
pub struct NewLemma<'a> {
    pub lang: &'a str,
    pub text: &'a str,
    pub pos: &'a str,
    pub freq_rank: Option<i64>,
    pub cefr: Option<&'a str>,
}

pub mod lemmas {
    use super::*;

    /// 寫入詞條；已存在則補上缺少的詞頻與 CEFR 資訊。
    ///
    /// `COALESCE(excluded.x, lemma.x)` 的用意：後匯入的來源若沒帶詞頻，
    /// 不該把先前來源帶進來的詞頻洗掉。
    pub async fn upsert(db: &Db, lemma: NewLemma<'_>) -> Result<LemmaId> {
        let normalized = wordforge_core::text::normalize(lemma.text);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, cefr)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (lang, text, pos) DO UPDATE SET
                 freq_rank = COALESCE(excluded.freq_rank, lemma.freq_rank),
                 cefr      = COALESCE(excluded.cefr, lemma.cefr)
             RETURNING id",
        )
        .bind(lemma.lang)
        .bind(lemma.text)
        .bind(&normalized)
        .bind(lemma.pos)
        .bind(lemma.freq_rank)
        .bind(lemma.cefr)
        .fetch_one(db.pool())
        .await?;

        Ok(LemmaId(id))
    }

    /// 登記一個表面形（`ran` → `run`），供詞形還原使用。
    pub async fn add_surface_form(
        db: &Db,
        lang: &str,
        form: &str,
        lemma_id: LemmaId,
        tag: &str,
    ) -> Result<()> {
        let normalized = wordforge_core::text::normalize(form);
        sqlx::query(
            "INSERT INTO surface_form (lang, form, normalized, lemma_id, tag)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (lang, normalized, lemma_id, tag) DO NOTHING",
        )
        .bind(lang)
        .bind(form)
        .bind(&normalized)
        .bind(lemma_id.0)
        .bind(tag)
        .execute(db.pool())
        .await?;
        Ok(())
    }

    /// 由任意詞形找出對應的 lemma。先查本身，再查表面形對照表。
    ///
    /// 一個詞形可能對到多個 lemma（`saw` = see 的過去式，也是「鋸子」）；
    /// 這裡回傳詞頻最高的那個，需要精確消歧時交給 LLM 或上下文判斷。
    pub async fn find_by_form(db: &Db, lang: &str, form: &str) -> Result<Option<LemmaId>> {
        let normalized = wordforge_core::text::normalize(form);
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM lemma
             WHERE lang = ? AND normalized = ?
             UNION
             SELECT l.id FROM lemma l
               JOIN surface_form s ON s.lemma_id = l.id
             WHERE s.lang = ? AND s.normalized = ?
             ORDER BY 1
             LIMIT 1",
        )
        .bind(lang)
        .bind(&normalized)
        .bind(lang)
        .bind(&normalized)
        .fetch_optional(db.pool())
        .await?;

        Ok(id.map(LemmaId))
    }
}

// ---------------------------------------------------------------- card

pub mod cards {
    use super::*;

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

    const SELECT_CARD: &str = "SELECT id, profile_id, lemma_id, kind, state, step, stability,
        difficulty, due, last_review, reps, lapses, scheduled_days, suspended FROM card";

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
            "{SELECT_CARD} WHERE profile_id = ? AND suspended = 0 AND due <= ?
             ORDER BY due ASC LIMIT ?"
        ))
        .bind(profile_id.0)
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

    /// 這位學習者「算是會了」的字。
    ///
    /// 定義：辨識卡已經畢業到長期複習，且 stability 達到門檻
    /// （預設 21 天 ≈ 撐得過三週不複習）。90% 法則的分母就是這份集合。
    pub async fn known_lemma_ids(
        db: &Db,
        profile_id: ProfileId,
        min_stability: f64,
    ) -> Result<HashSet<LemmaId>> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT lemma_id FROM card
             WHERE profile_id = ? AND kind = 'recognition' AND state = 'review'
               AND stability >= ?",
        )
        .bind(profile_id.0)
        .bind(min_stability)
        .fetch_all(db.pool())
        .await?;

        Ok(ids.into_iter().map(LemmaId).collect())
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;
    use wordforge_core::srs::Scheduler;

    async fn setup() -> (Db, ProfileId) {
        let db = Db::open_in_memory().await.unwrap();
        let profile = profiles::create(&db, "我", "zh-TW", "en", t0())
            .await
            .unwrap();
        (db, profile)
    }

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    async fn add_word(db: &Db, text: &str, freq: i64) -> LemmaId {
        lemmas::upsert(
            db,
            NewLemma {
                lang: "en",
                text,
                pos: "noun",
                freq_rank: Some(freq),
                cefr: None,
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn timestamps_round_trip_and_sort_lexicographically() {
        let a = ts::to_sql(t0());
        let b = ts::to_sql(t0() + Duration::milliseconds(500));
        assert!(a < b, "{a} 應該小於 {b}");
        assert_eq!(ts::from_sql(&a), Some(t0()));
    }

    #[tokio::test]
    async fn upsert_lemma_is_idempotent_and_backfills() {
        let (db, _) = setup().await;
        let first = add_word(&db, "apple", 500).await;

        // 第二次匯入沒帶詞頻，不該把既有詞頻洗掉
        let second = lemmas::upsert(
            &db,
            NewLemma {
                lang: "en",
                text: "apple",
                pos: "noun",
                freq_rank: None,
                cefr: Some("A1"),
            },
        )
        .await
        .unwrap();

        assert_eq!(first, second, "同一個字不該產生兩筆 lemma");
        let (freq, cefr): (Option<i64>, Option<String>) =
            sqlx::query_as("SELECT freq_rank, cefr FROM lemma WHERE id = ?")
                .bind(first.0)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(freq, Some(500));
        assert_eq!(cefr.as_deref(), Some("A1"));
    }

    #[tokio::test]
    async fn find_by_form_resolves_inflections() {
        let (db, _) = setup().await;
        let run = add_word(&db, "run", 300).await;
        lemmas::add_surface_form(&db, "en", "Running", run, "gerund")
            .await
            .unwrap();

        assert_eq!(
            lemmas::find_by_form(&db, "en", "run").await.unwrap(),
            Some(run)
        );
        // 大小寫與標點都該被正規化掉
        assert_eq!(
            lemmas::find_by_form(&db, "en", "running,").await.unwrap(),
            Some(run)
        );
        assert_eq!(
            lemmas::find_by_form(&db, "en", "nonexistent")
                .await
                .unwrap(),
            None
        );
    }

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

    #[tokio::test]
    async fn known_words_require_graduated_stable_cards() {
        let (db, profile) = setup().await;
        let word = add_word(&db, "apple", 500).await;
        let card = cards::ensure(&db, profile, word, CardKind::Recognition, t0())
            .await
            .unwrap();

        // 只學了一次、還在 learning：不算會
        let scheduler = Scheduler::default();
        let (after_again, log) = scheduler.review(&card, Rating::Again, t0(), None);
        cards::record_review(&db, &after_again, &log).await.unwrap();
        assert!(
            cards::known_lemma_ids(&db, profile, 21.0)
                .await
                .unwrap()
                .is_empty()
        );

        // 手動拉到高 stability 的 review 狀態：才算會
        sqlx::query("UPDATE card SET state = 'review', stability = 40.0 WHERE id = ?")
            .bind(card.id.unwrap().0)
            .execute(db.pool())
            .await
            .unwrap();
        let known = cards::known_lemma_ids(&db, profile, 21.0).await.unwrap();
        assert!(known.contains(&word));

        // 門檻拉高到 50 天就不算了
        assert!(
            cards::known_lemma_ids(&db, profile, 50.0)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
