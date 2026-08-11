//! 文法點的掌握狀態，用跟單字卡同一套 FSRS 排程。
//!
//! ## 為什麼文法點也需要間隔重複
//!
//! 「最近錯最多次的五個」不是好的出題依據：昨天剛練過的還是會被挑出來，
//! 而三週前錯過、之後都沒再碰的反而消失了。文法點跟單字一樣是記憶——
//! 錯了要盡快再遇到，對了可以拉遠，練熟了就不必再出。
//!
//! ## 對 token 的影響
//!
//! 出題時只送「今天到期」的幾個，不是整份歷史。
//! prompt 大小固定，練習做得再多也不會膨脹。

use serde::Serialize;
use sqlx::Row;
use time::OffsetDateTime;
use wordforge_core::model::{CardState, MemoryState, ProfileId, Rating};
use wordforge_core::srs::{ReviewState, Scheduler};

use crate::ts::{self, ParseTs};
use crate::{Db, Result};

/// 一個文法點目前的狀態。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GrammarPoint {
    pub point: String,
    pub state: String,
    pub due: String,
    /// 累計錯幾次
    pub error_count: i64,
    /// 累計對幾次
    pub correct_count: i64,
    /// 記憶穩定度（天）。越大表示越熟。
    pub stability: Option<f64>,
}

fn row_to_point(row: &sqlx::sqlite::SqliteRow) -> GrammarPoint {
    GrammarPoint {
        point: row.get("point"),
        state: row.get("state"),
        due: row.get("due"),
        error_count: row.get("error_count"),
        correct_count: row.get("correct_count"),
        stability: row.get("stability"),
    }
}

const SELECT_POINT: &str = "SELECT point, state, step, stability, difficulty, due,
    last_review, scheduled_days, error_count, correct_count FROM grammar_point";

/// 記錄一次結果並重新排程。
///
/// `correct` 對應 FSRS 的評分：答對是 Good、答錯是 Again。
/// 沒有用到 Hard / Easy——文法題只有對錯，硬套四級評分只是假精確。
pub async fn record(
    db: &Db,
    profile_id: ProfileId,
    point: &str,
    correct: bool,
    scheduler: &Scheduler,
    now: OffsetDateTime,
) -> Result<()> {
    let point = point.trim();
    if point.is_empty() {
        return Ok(());
    }

    // 先取出現況（沒有就是全新的）
    let existing = sqlx::query(&format!(
        "{SELECT_POINT} WHERE profile_id = ? AND point = ?"
    ))
    .bind(profile_id.0)
    .bind(point)
    .fetch_optional(db.pool())
    .await?;

    let current = match &existing {
        Some(row) => {
            let stability: Option<f64> = row.get("stability");
            let difficulty: Option<f64> = row.get("difficulty");
            let last_review: Option<String> = row.get("last_review");
            ReviewState {
                state: parse_state(row.get::<String, _>("state").as_str()),
                memory: match (stability, difficulty) {
                    (Some(stability), Some(difficulty)) => Some(MemoryState {
                        stability,
                        difficulty,
                    }),
                    _ => None,
                },
                due: row.get::<String, _>("due").parse_ts("grammar_point.due")?,
                last_review: last_review
                    .map(|s| s.parse_ts("grammar_point.last_review"))
                    .transpose()?,
                step: row.get::<i64, _>("step") as u8,
                scheduled_days: row.get("scheduled_days"),
            }
        }
        None => ReviewState::new(now),
    };

    let rating = if correct { Rating::Good } else { Rating::Again };
    let next = scheduler.schedule(current, rating, now);
    let memory = next.memory.expect("排程後一定有記憶狀態");

    sqlx::query(
        "INSERT INTO grammar_point (profile_id, point, state, step, stability, difficulty,
                                    due, last_review, scheduled_days,
                                    error_count, correct_count, first_seen)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (profile_id, point) DO UPDATE SET
             state          = excluded.state,
             step           = excluded.step,
             stability      = excluded.stability,
             difficulty     = excluded.difficulty,
             due            = excluded.due,
             last_review    = excluded.last_review,
             scheduled_days = excluded.scheduled_days,
             error_count    = grammar_point.error_count + excluded.error_count,
             correct_count  = grammar_point.correct_count + excluded.correct_count",
    )
    .bind(profile_id.0)
    .bind(point)
    .bind(next.state.as_str())
    .bind(next.step as i64)
    .bind(memory.stability)
    .bind(memory.difficulty)
    .bind(ts::to_sql(next.due))
    .bind(next.last_review.map(ts::to_sql))
    .bind(next.scheduled_days)
    .bind(i64::from(!correct))
    .bind(i64::from(correct))
    .bind(ts::to_sql(now))
    .execute(db.pool())
    .await?;

    Ok(())
}

/// 現在該練的文法點，最久沒複習的排前面。
///
/// 出題時只送這幾個給模型——prompt 大小固定，
/// 練習做得再多也不會讓 token 膨脹。
pub async fn due_points(
    db: &Db,
    profile_id: ProfileId,
    now: OffsetDateTime,
    limit: i64,
) -> Result<Vec<String>> {
    let points: Vec<String> = sqlx::query_scalar(
        "SELECT point FROM grammar_point
         WHERE profile_id = ? AND due <= ?
         ORDER BY due ASC LIMIT ?",
    )
    .bind(profile_id.0)
    .bind(ts::to_sql(now))
    .bind(limit)
    .fetch_all(db.pool())
    .await?;
    Ok(points)
}

/// 全部文法點的狀況，還沒練熟的排前面。供 UI 顯示進度。
pub async fn all_points(db: &Db, profile_id: ProfileId) -> Result<Vec<GrammarPoint>> {
    let rows = sqlx::query(&format!(
        "{SELECT_POINT} WHERE profile_id = ?
         ORDER BY stability IS NULL DESC, stability ASC, point"
    ))
    .bind(profile_id.0)
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(row_to_point).collect())
}

fn parse_state(s: &str) -> CardState {
    match s {
        "learning" => CardState::Learning,
        "review" => CardState::Review,
        "relearning" => CardState::Relearning,
        // 認不得的值當成新的重新排程，比讓整個查詢失敗好
        _ => CardState::New,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::profiles;
    use time::Duration;

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    async fn setup() -> (Db, ProfileId, Scheduler) {
        let db = Db::open_in_memory().await.unwrap();
        let profile = profiles::create(&db, "我", "zh-TW", "en", t0())
            .await
            .unwrap();
        (db, profile, Scheduler::default())
    }

    #[tokio::test]
    async fn a_mistake_creates_a_point_and_schedules_it() {
        let (db, profile, sched) = setup().await;
        record(&db, profile, "tense", false, &sched, t0())
            .await
            .unwrap();

        let all = all_points(&db, profile).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].point, "tense");
        assert_eq!(all[0].error_count, 1);
        assert_eq!(all[0].correct_count, 0);

        // 剛錯過的東西應該很快就要再遇到
        let due = due_points(&db, profile, t0() + Duration::minutes(5), 10)
            .await
            .unwrap();
        assert_eq!(due, vec!["tense"]);
    }

    /// 答對要拉遠間隔，這正是「練熟的不再出現」的機制。
    #[tokio::test]
    async fn getting_it_right_pushes_the_interval_out() {
        let (db, profile, sched) = setup().await;

        record(&db, profile, "articles", false, &sched, t0())
            .await
            .unwrap();
        // 連續答對幾次
        let mut when = t0();
        for _ in 0..3 {
            when += Duration::days(1);
            record(&db, profile, "articles", true, &sched, when)
                .await
                .unwrap();
        }

        let all = all_points(&db, profile).await.unwrap();
        assert_eq!(all[0].error_count, 1);
        assert_eq!(all[0].correct_count, 3);
        assert!(
            all[0].stability.unwrap() > 1.0,
            "連續答對後穩定度應該明顯上升：{:?}",
            all[0].stability
        );

        // 隔天不該再被挑出來練
        assert!(
            due_points(&db, profile, when + Duration::days(1), 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 練熟之後又錯，間隔要縮回來。
    #[tokio::test]
    async fn a_relapse_brings_the_point_back() {
        let (db, profile, sched) = setup().await;
        let mut when = t0();
        for _ in 0..4 {
            record(&db, profile, "tense", true, &sched, when)
                .await
                .unwrap();
            when += Duration::days(3);
        }
        assert!(due_points(&db, profile, when, 10).await.unwrap().is_empty());

        record(&db, profile, "tense", false, &sched, when)
            .await
            .unwrap();
        assert_eq!(
            due_points(&db, profile, when + Duration::minutes(30), 10)
                .await
                .unwrap(),
            vec!["tense"],
            "又錯了就該馬上排回來"
        );
    }

    /// 出題只拿到期的幾個，prompt 大小才不會隨練習次數膨脹。
    #[tokio::test]
    async fn only_due_points_are_returned_and_the_count_is_bounded() {
        let (db, profile, sched) = setup().await;
        for point in ["tense", "articles", "plural", "word-order", "prepositions"] {
            record(&db, profile, point, false, &sched, t0())
                .await
                .unwrap();
        }

        let due = due_points(&db, profile, t0() + Duration::hours(1), 3)
            .await
            .unwrap();
        assert_eq!(due.len(), 3, "要能限制數量");
        assert_eq!(all_points(&db, profile).await.unwrap().len(), 5);
    }

    /// 同一個文法點不會產生第二筆，統計要累加。
    #[tokio::test]
    async fn repeated_results_accumulate_on_one_row() {
        let (db, profile, sched) = setup().await;
        let mut when = t0();
        for correct in [false, true, false, true, true] {
            record(&db, profile, "tense", correct, &sched, when)
                .await
                .unwrap();
            when += Duration::days(1);
        }

        let all = all_points(&db, profile).await.unwrap();
        assert_eq!(all.len(), 1, "不該產生重複的列");
        assert_eq!(all[0].error_count, 2);
        assert_eq!(all[0].correct_count, 3);
    }

    #[tokio::test]
    async fn blank_points_are_ignored() {
        let (db, profile, sched) = setup().await;
        record(&db, profile, "   ", false, &sched, t0())
            .await
            .unwrap();
        record(&db, profile, "", true, &sched, t0()).await.unwrap();
        assert!(all_points(&db, profile).await.unwrap().is_empty());
    }

    /// 沒練過的人不該拿到任何文法點。
    #[tokio::test]
    async fn a_fresh_profile_has_nothing_due() {
        let (db, profile, _) = setup().await;
        assert!(due_points(&db, profile, t0(), 10).await.unwrap().is_empty());
        assert!(all_points(&db, profile).await.unwrap().is_empty());
    }
}
