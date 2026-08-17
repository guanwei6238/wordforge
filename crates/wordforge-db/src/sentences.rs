//! 翻譯句子的複習排程。
//!
//! 規則刻意比 FSRS 簡單（理由寫在 `0014_sentence_review.sql`）：
//!
//! ```text
//! 答錯 → 明天再出現，錯誤次數加一
//! 答對 → 從此不再出現
//! 跳過 → 明天再出現，但什麼都不算
//! ```
//!
//! 「明天」用的是 UTC 午夜，跟每日新卡額度同一個定義——兩邊各算各的
//! 日界線的話，會出現「新卡說今天已經學完、句子說今天還沒開始」。

use sqlx::Row;
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

/// 今天不想寫這一句，明天再說。回傳有沒有真的推遲到。
///
/// 跟 [`miss`] 的差別是**只動 `due`**：
///
/// - `misses` 不加。跳過不是答錯——按了跳過的人沒有作答，
///   模型也沒有批改，沒有任何東西能說他寫錯了。加上去的話畫面會
///   顯示「錯過 5 次」，而他一次都沒寫過。
/// - `last_review` 不動。那一欄的意思是「上次真的練過這一句是什麼時候」，
///   而 [`practised_today`] 拿它擋「同一天重寫刷分」。跳過的人沒看到
///   參考答案，沒有東西可抄，不該被那道鎖擋住。
///
/// 沒有新欄位：這一整件事就是「今天不要出現，明天要」，而 `due`
/// 說的正是這句話。加一個 `skipped` 欄位只會多一份要跟 `due`
/// 保持一致的真相。
pub async fn skip(
    db: &Db,
    profile_id: ProfileId,
    exercise_id: i64,
    item_index: i64,
    now: OffsetDateTime,
) -> Result<bool> {
    let tomorrow = day_start(now) + Duration::days(1);
    let affected = sqlx::query(
        "UPDATE sentence_review SET due = ?
         WHERE profile_id = ? AND exercise_id = ? AND item_index = ?",
    )
    .bind(ts::to_sql(tomorrow))
    .bind(profile_id.0)
    .bind(exercise_id)
    .bind(item_index)
    .execute(db.pool())
    .await?
    .rows_affected();
    Ok(affected > 0)
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

/// 今天總共還有幾句要練。
///
/// 畫面一次只出一句，但仍然要說得出「還有幾句」——少了它，使用者只知道
/// 眼前這一句，不知道自己在半路還是最後一句，也就不知道還要花多久。
///
/// 分開查而不是把 `due` 撈回來數：那條清單會隨著練習量長大，而這裡
/// 要的只是一個數字。
pub async fn due_count(db: &Db, profile_id: ProfileId, now: OffsetDateTime) -> Result<i64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sentence_review WHERE profile_id = ? AND due <= ?",
    )
    .bind(profile_id.0)
    .bind(ts::to_sql(now))
    .fetch_one(db.pool())
    .await?;
    Ok(count)
}

/// 一次複習作答的紀錄。題目本文不在這裡，拿 `exercise_id` + `item_index` 取。
#[derive(Debug, Clone, PartialEq)]
pub struct SentenceAttempt {
    pub id: i64,
    pub exercise_id: i64,
    pub item_index: i64,
    pub answer: String,
    pub correct: bool,
    /// 口語說法。只在它跟正式說法不一樣時才有值。
    pub reference: Option<String>,
    pub reference_formal: Option<String>,
    pub comment: Option<String>,
    /// 逐處修正的原始 JSON（`[{original, corrected, explanation, ...}]`）。
    ///
    /// 這一層不解析它：形狀由 `wordforge-practice` 的 `Correction` 定義，
    /// 在這裡再寫一份只會有兩份會互相漂移的真相。
    pub corrections_json: String,
    pub created_at: String,
}

/// 一次複習作答要寫進紀錄的內容。
pub struct NewAttempt<'a> {
    pub exercise_id: i64,
    pub item_index: i64,
    pub answer: &'a str,
    pub correct: bool,
    pub reference: Option<&'a str>,
    pub reference_formal: Option<&'a str>,
    pub comment: Option<&'a str>,
    /// 這一句的逐處修正，已序列化。沒有就傳 `"[]"`。
    pub corrections_json: &'a str,
}

/// 記一句複習的作答。
pub async fn record_attempt(
    db: &Db,
    profile_id: ProfileId,
    attempt: NewAttempt<'_>,
    now: OffsetDateTime,
) -> Result<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO sentence_attempt
             (profile_id, exercise_id, item_index, answer, correct,
              reference, reference_formal, comment, corrections_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING id",
    )
    .bind(profile_id.0)
    .bind(attempt.exercise_id)
    .bind(attempt.item_index)
    .bind(attempt.answer)
    .bind(attempt.correct as i64)
    .bind(attempt.reference)
    .bind(attempt.reference_formal)
    .bind(attempt.comment)
    .bind(attempt.corrections_json)
    .bind(ts::to_sql(now))
    .fetch_one(db.pool())
    .await?;
    Ok(id)
}

/// 複習紀錄，新的在前。
///
/// 分頁：這張表每複習一句就長一列，是典型「長度由資料決定」的清單。
/// 總數由 [`attempt_count`] 給，少了它 UI 說不出「第 2 / 7 頁」。
pub async fn attempts(
    db: &Db,
    profile_id: ProfileId,
    limit: i64,
    offset: i64,
) -> Result<Vec<SentenceAttempt>> {
    // 欄位多，照 `exercises::attempts` 的做法一欄一欄取名字拿，
    // 不要排一個九元組——那種型別讀不出哪一欄是哪一欄，順序寫錯了
    // 編譯器也不會說話
    let rows = sqlx::query(
        "SELECT id, exercise_id, item_index, answer, correct,
                reference, reference_formal, comment, corrections_json, created_at
         FROM sentence_attempt
         WHERE profile_id = ?
         ORDER BY created_at DESC, id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(profile_id.0)
    .bind(limit.max(0))
    .bind(offset.max(0))
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .iter()
        .map(|row| SentenceAttempt {
            id: row.get("id"),
            exercise_id: row.get("exercise_id"),
            item_index: row.get("item_index"),
            answer: row.get("answer"),
            correct: row.get::<i64, _>("correct") != 0,
            reference: row.get("reference"),
            reference_formal: row.get("reference_formal"),
            comment: row.get("comment"),
            corrections_json: row.get("corrections_json"),
            created_at: row.get("created_at"),
        })
        .collect())
}

/// 複習紀錄總共幾筆。
pub async fn attempt_count(db: &Db, profile_id: ProfileId) -> Result<i64> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sentence_attempt WHERE profile_id = ?")
            .bind(profile_id.0)
            .fetch_one(db.pool())
            .await?;
    Ok(count)
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

    /// 跳過只是「今天不要出現」：明天照樣回來，而且**不算答錯**。
    ///
    /// 錯誤次數會顯示在畫面上（「錯過 3 次」），跳過也加一的話，
    /// 一個一次都沒寫過的句子會被標成錯過很多次。
    #[tokio::test]
    async fn skipping_a_sentence_defers_it_without_counting_a_mistake() {
        let (db, profile, exercise) = setup().await;
        miss(&db, profile, exercise, 0, t0()).await.unwrap();
        let day2 = t0() + Duration::days(1);

        assert!(skip(&db, profile, exercise, 0, day2).await.unwrap());
        assert!(
            due(&db, profile, day2, 10).await.unwrap().is_empty(),
            "跳過之後今天就不該再出現"
        );

        let day3 = due(&db, profile, day2 + Duration::days(1), 10)
            .await
            .unwrap();
        assert_eq!(day3.len(), 1, "明天要自己回來");
        assert_eq!(day3[0].misses, 1, "跳過不是答錯，次數不該變");

        // 跳過的人沒看到參考答案，沒有東西可抄——不該被「今天練過了」鎖住
        assert!(
            !practised_today(&db, profile, exercise, 0, day2)
                .await
                .unwrap(),
            "跳過不算今天練過"
        );
    }

    /// 已經寫對、從排程裡消失的句子，跳過它不該把它變回來。
    #[tokio::test]
    async fn skipping_a_sentence_that_is_not_scheduled_changes_nothing() {
        let (db, profile, exercise) = setup().await;

        assert!(
            !skip(&db, profile, exercise, 3, t0()).await.unwrap(),
            "沒排程的句子沒有東西可推遲"
        );
        assert!(
            due(&db, profile, t0() + Duration::days(30), 10)
                .await
                .unwrap()
                .is_empty(),
            "更不該憑空長出一筆排程"
        );
    }

    /// 「還有幾句」要跟 `due` 的取件範圍一致：畫面一次只拿一句，
    /// 但數字要算到今天所有到期的，而且不能把明天才回來的算進去。
    #[tokio::test]
    async fn the_count_matches_what_is_due_today_not_what_fits_on_screen() {
        let (db, profile, exercise) = setup().await;
        for i in 0..3 {
            miss(&db, profile, exercise, i, t0()).await.unwrap();
        }
        let tomorrow = t0() + Duration::days(1);
        // 明天做錯的那一句要到後天才算數
        miss(&db, profile, exercise, 9, tomorrow).await.unwrap();

        assert_eq!(due_count(&db, profile, t0()).await.unwrap(), 0);
        assert_eq!(due_count(&db, profile, tomorrow).await.unwrap(), 3);
        assert_eq!(
            due(&db, profile, tomorrow, 1).await.unwrap().len(),
            1,
            "畫面只拿一句，但總數仍然是 3"
        );
        assert_eq!(
            due_count(&db, profile, tomorrow + Duration::days(1))
                .await
                .unwrap(),
            4
        );
    }

    /// 這句 SQL 的查詢計畫，串成一行方便斷言。
    async fn plan(db: &Db, sql: &str) -> String {
        let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
            .fetch_all(db.pool())
            .await
            .unwrap();
        rows.iter()
            .map(|r| r.get::<String, _>("detail"))
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// 複習紀錄的清單查詢要**完全走在索引上**。
    ///
    /// 這張表每複習一句就長一列，是典型會一直長大的那種。`SCAN` 或
    /// `USE TEMP B-TREE FOR ORDER BY` 在資料少的時候完全看不出來，
    /// 一年之後才變成「打開紀錄要等一下」——而那時候很難想到是這裡。
    ///
    /// 控制組先跑一個一定會退化的查詢：驗證方法本身要先驗證過，
    /// 否則這條測試可能只是永遠通過。
    #[tokio::test]
    async fn the_review_log_query_stays_on_its_index() {
        let (db, _, _) = setup().await;

        let control = plan(&db, "SELECT * FROM sentence_attempt ORDER BY answer").await;
        assert!(
            control.contains("SCAN") && control.contains("TEMP B-TREE"),
            "控制組沒有退化，這個檢查方法看不出問題：{control}"
        );

        let listing = plan(
            &db,
            "SELECT id, answer FROM sentence_attempt
             WHERE profile_id = 1 ORDER BY created_at DESC, id DESC LIMIT 10 OFFSET 0",
        )
        .await;
        assert!(!listing.contains("SCAN"), "清單查詢在掃全表：{listing}");
        assert!(
            !listing.contains("TEMP B-TREE"),
            "排序沒有走索引，建了暫存 B-tree：{listing}"
        );

        // 刪練習時 CASCADE 會照 exercise_id 找子列，那條也不能是全表掃描
        let cascade = plan(&db, "SELECT 1 FROM sentence_attempt WHERE exercise_id = 1").await;
        assert!(!cascade.contains("SCAN"), "CASCADE 會掃全表：{cascade}");
    }

    /// 複習紀錄自己一張表，而且要分頁。
    #[tokio::test]
    async fn review_attempts_come_back_newest_first_with_a_total() {
        let (db, profile, exercise) = setup().await;
        for i in 0..5 {
            record_attempt(
                &db,
                profile,
                NewAttempt {
                    exercise_id: exercise,
                    item_index: i,
                    answer: &format!("答案 {i}"),
                    correct: i % 2 == 0,
                    reference: None,
                    reference_formal: Some("正式說法"),
                    comment: None,
                    corrections_json: "[]",
                },
                t0() + Duration::minutes(i),
            )
            .await
            .unwrap();
        }

        assert_eq!(attempt_count(&db, profile).await.unwrap(), 5);
        let page = attempts(&db, profile, 2, 0).await.unwrap();
        assert_eq!(page.len(), 2, "一頁兩筆");
        assert_eq!(page[0].answer, "答案 4", "新的在前");
        assert_eq!(page[0].reference_formal.as_deref(), Some("正式說法"));

        let second = attempts(&db, profile, 2, 2).await.unwrap();
        assert_eq!(second[0].answer, "答案 2", "翻頁要接得上");
    }

    /// 練習被刪掉時，它的複習紀錄也要跟著走——題目都沒了，
    /// 「你當時翻得對不對」沒有東西可以對照。
    #[tokio::test]
    async fn deleting_an_exercise_takes_its_review_attempts_with_it() {
        let (db, profile, exercise) = setup().await;
        record_attempt(
            &db,
            profile,
            NewAttempt {
                exercise_id: exercise,
                item_index: 0,
                answer: "寫了什麼",
                correct: false,
                reference: None,
                reference_formal: None,
                comment: None,
                corrections_json: "[]",
            },
            t0(),
        )
        .await
        .unwrap();

        exercises::delete(&db, profile, exercises::ExerciseId(exercise))
            .await
            .unwrap();

        assert_eq!(attempt_count(&db, profile).await.unwrap(), 0);
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
