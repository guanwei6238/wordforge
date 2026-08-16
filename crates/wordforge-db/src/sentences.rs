//! 翻譯句子的複習排程。
//!
//! 規則只有兩條，刻意比 FSRS 簡單（理由寫在 `0014_sentence_review.sql`）：
//!
//! ```text
//! 答錯 → 明天再出現
//! 答對 → 從此不再出現
//! ```
//!
//! 「明天」用的是 UTC 午夜，跟每日新卡額度同一個定義——兩邊各算各的
//! 日界線的話，會出現「新卡說今天已經學完、句子說今天還沒開始」。

use time::{Duration, OffsetDateTime};

use crate::ts::{self, ParseTs};
use crate::{Db, Result};
use wordforge_core::model::ProfileId;

/// 一句待複習的翻譯。
///
/// 句子本文不在這裡：它在 `exercise.payload_json` 裡，呼叫端拿
/// `exercise_id` 去取。存兩份只會有兩份互相漂移的真相。
#[derive(Debug, Clone, PartialEq)]
pub struct DueSentence {
    pub exercise_id: i64,
    pub item_index: i64,
    pub due: OffsetDateTime,
    /// 錯過幾次
    pub misses: i64,
}

/// 這一天的起點。跟 `commands::cards::day_start` 是同一個定義。
fn day_start(now: OffsetDateTime) -> OffsetDateTime {
    now.replace_time(time::Time::MIDNIGHT)
}

/// 排一句到明天。已經在排程裡的就往後推，錯誤次數加一。
///
/// 「明天」而不是「24 小時後」：晚上十一點做錯的句子，一小時後就回來
/// 不叫複習。日界線一致才說得出「今天還有幾句要練」。
pub async fn miss(
    db: &Db,
    profile_id: ProfileId,
    exercise_id: i64,
    item_index: i64,
    now: OffsetDateTime,
) -> Result<()> {
    let tomorrow = day_start(now) + Duration::days(1);
    sqlx::query(
        "INSERT INTO sentence_review (profile_id, exercise_id, item_index, due, last_review, misses)
         VALUES (?, ?, ?, ?, ?, 1)
         ON CONFLICT (profile_id, exercise_id, item_index) DO UPDATE SET
             due         = excluded.due,
             last_review = excluded.last_review,
             misses      = sentence_review.misses + 1",
    )
    .bind(profile_id.0)
    .bind(exercise_id)
    .bind(item_index)
    .bind(ts::to_sql(tomorrow))
    .bind(ts::to_sql(now))
    .execute(db.pool())
    .await?;
    Ok(())
}

/// 這份練習有沒有任何一句在排程裡。
///
/// 補寫舊資料時用來略過已經排過的：`miss` 是 `misses + 1`，
/// 對已經排好的練習再跑一次會讓次數憑空多一次。
pub async fn has_any(db: &Db, exercise_id: i64) -> Result<bool> {
    let found: i64 =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sentence_review WHERE exercise_id = ?)")
            .bind(exercise_id)
            .fetch_one(db.pool())
            .await?;
    Ok(found != 0)
}

/// 這一句練起來了，從排程裡拿掉。
///
/// 刪掉而不是標記完成：這張表的意義就是「還沒練起來的句子」，
/// 留著已完成的紀錄只會讓「今天還有幾句」的查詢愈來愈慢。
/// 想知道「這句以前錯過幾次」的話，那段歷史在 `attempt` 裡。
pub async fn pass(
    db: &Db,
    profile_id: ProfileId,
    exercise_id: i64,
    item_index: i64,
) -> Result<bool> {
    let affected = sqlx::query(
        "DELETE FROM sentence_review
         WHERE profile_id = ? AND exercise_id = ? AND item_index = ?",
    )
    .bind(profile_id.0)
    .bind(exercise_id)
    .bind(item_index)
    .execute(db.pool())
    .await?
    .rows_affected();
    Ok(affected > 0)
}

/// 今天該練的句子，最久沒練的排前面。
pub async fn due(
    db: &Db,
    profile_id: ProfileId,
    now: OffsetDateTime,
    limit: i64,
) -> Result<Vec<DueSentence>> {
    let rows: Vec<(i64, i64, String, i64)> = sqlx::query_as(
        "SELECT exercise_id, item_index, due, misses FROM sentence_review
         WHERE profile_id = ? AND due <= ?
         ORDER BY due, id LIMIT ?",
    )
    .bind(profile_id.0)
    .bind(ts::to_sql(now))
    .bind(limit.max(0))
    .fetch_all(db.pool())
    .await?;

    rows.into_iter()
        .map(|(exercise_id, item_index, due, misses)| {
            Ok(DueSentence {
                exercise_id,
                item_index,
                due: due.parse_ts("sentence_review.due")?,
                misses,
            })
        })
        .collect()
}

/// 這一句今天練過了沒有。
///
/// 擋的是「同一天反覆重寫同一句刷到全對」：那看起來是 100 分，
/// 實際上只是背下了剛剛看到的參考答案。
pub async fn practised_today(
    db: &Db,
    profile_id: ProfileId,
    exercise_id: i64,
    item_index: i64,
    now: OffsetDateTime,
) -> Result<bool> {
    let last: Option<String> = sqlx::query_scalar(
        "SELECT last_review FROM sentence_review
         WHERE profile_id = ? AND exercise_id = ? AND item_index = ?",
    )
    .bind(profile_id.0)
    .bind(exercise_id)
    .bind(item_index)
    .fetch_optional(db.pool())
    .await?;

    let Some(last) = last else {
        return Ok(false);
    };
    Ok(last.parse_ts("sentence_review.last_review")? >= day_start(now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exercises::{self, NewExercise};
    use crate::repo::profiles;

    /// 2023-11-14T22:13:20Z——刻意挑一個**接近日界線**的時刻。
    ///
    /// 這裡差一點就寫成 `t0() + 6h` 當作「當天稍晚」，而那已經是隔天
    /// 早上四點了。測試自己踩到的正是這個功能要處理的邊界：
    /// 「明天」是日界線，不是 24 小時後。
    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    async fn setup() -> (Db, ProfileId, i64) {
        let db = Db::open_in_memory().await.unwrap();
        let profile = profiles::create(&db, "我", "zh-TW", "en", t0())
            .await
            .unwrap();
        let exercise = exercises::create(
            &db,
            NewExercise {
                profile_id: profile,
                kind: "translation_to_target",
                payload_json: r#"{"kind":"translation","to_target":true,"items":[]}"#,
                target_words: &[],
                coverage: None,
                model: None,
                material_id: None,
                topic: None,
            },
            t0(),
        )
        .await
        .unwrap();
        (db, profile, exercise.0)
    }

    /// 做錯的句子今天不再出現，明天才回來。
    #[tokio::test]
    async fn a_missed_sentence_comes_back_tomorrow_not_today() {
        let (db, profile, exercise) = setup().await;
        miss(&db, profile, exercise, 2, t0()).await.unwrap();

        assert!(
            due(&db, profile, t0(), 10).await.unwrap().is_empty(),
            "今天做錯的句子今天不該再出現"
        );
        // 當天再晚也不行（t0 是 22:13，加一小時仍是同一天）
        assert!(
            due(&db, profile, t0() + Duration::hours(1), 10)
                .await
                .unwrap()
                .is_empty()
        );

        let tomorrow = due(&db, profile, t0() + Duration::days(1), 10)
            .await
            .unwrap();
        assert_eq!(tomorrow.len(), 1);
        assert_eq!(tomorrow[0].item_index, 2);
        assert_eq!(tomorrow[0].misses, 1);
    }

    /// 再錯一次就再排一天，錯誤次數累積。
    #[tokio::test]
    async fn missing_the_same_sentence_again_pushes_it_out_again() {
        let (db, profile, exercise) = setup().await;
        miss(&db, profile, exercise, 0, t0()).await.unwrap();
        let day2 = t0() + Duration::days(1);
        miss(&db, profile, exercise, 0, day2).await.unwrap();

        assert!(
            due(&db, profile, day2, 10).await.unwrap().is_empty(),
            "當天又做錯一次，當天就不該再出現"
        );
        let day3 = due(&db, profile, day2 + Duration::days(1), 10)
            .await
            .unwrap();
        assert_eq!(day3[0].misses, 2, "錯誤次數要累積");
    }

    /// 做對就從此不再出現——「練到會了」是這條排程的終點。
    #[tokio::test]
    async fn a_passed_sentence_never_comes_back() {
        let (db, profile, exercise) = setup().await;
        miss(&db, profile, exercise, 1, t0()).await.unwrap();
        assert!(pass(&db, profile, exercise, 1).await.unwrap());

        assert!(
            due(&db, profile, t0() + Duration::days(30), 10)
                .await
                .unwrap()
                .is_empty()
        );
        // 沒排程的句子做對了也不該報錯，只是「沒東西可刪」
        assert!(!pass(&db, profile, exercise, 1).await.unwrap());
    }

    /// 同一天不能再練一次：那只是背下剛看到的參考答案。
    #[tokio::test]
    async fn a_sentence_practised_today_is_locked_until_tomorrow() {
        let (db, profile, exercise) = setup().await;
        miss(&db, profile, exercise, 0, t0()).await.unwrap();

        assert!(
            practised_today(&db, profile, exercise, 0, t0() + Duration::hours(1))
                .await
                .unwrap()
        );
        assert!(
            !practised_today(&db, profile, exercise, 0, t0() + Duration::days(1))
                .await
                .unwrap()
        );
        // 從來沒練過的句子當然不算今天練過
        assert!(
            !practised_today(&db, profile, exercise, 5, t0())
                .await
                .unwrap()
        );
    }

    /// 刪掉練習，它的句子排程要跟著走——否則「今天要練的句子」
    /// 會指向一份已經不存在的練習。
    #[tokio::test]
    async fn deleting_an_exercise_takes_its_sentences_with_it() {
        let (db, profile, exercise) = setup().await;
        miss(&db, profile, exercise, 0, t0()).await.unwrap();
        miss(&db, profile, exercise, 1, t0()).await.unwrap();

        exercises::delete(&db, profile, exercises::ExerciseId(exercise))
            .await
            .unwrap();

        assert!(
            due(&db, profile, t0() + Duration::days(1), 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
