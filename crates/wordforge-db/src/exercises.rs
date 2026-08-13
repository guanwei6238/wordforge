//! 練習與作答的存取。
//!
//! 題目內容以 JSON 存在 `exercise.payload_json`：題型會一直長出新的
//! （克漏字、配對、排序、聽寫…），每加一種就要一次 migration 並不划算。
//! 需要查詢的欄位（題型、覆蓋率、時間）才拉出來獨立成欄。

use serde::Serialize;
use sqlx::Row;
use time::OffsetDateTime;
use wordforge_core::model::ProfileId;

use crate::{Db, Result, ts};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExerciseId(pub i64);

/// 要寫入的新練習。
#[derive(Debug, Clone)]
pub struct NewExercise<'a> {
    pub profile_id: ProfileId,
    /// 對應 `wordforge_core::practice::ExerciseKind::as_str()`
    pub kind: &'a str,
    /// 題目內容，結構依題型而異
    pub payload_json: &'a str,
    /// 這次要教的目標詞
    pub target_words: &'a [String],
    /// 產生當下的已知詞覆蓋率，用來驗收 90% 法則
    pub coverage: Option<f64>,
    /// 產生用的模型，方便日後比較品質
    pub model: Option<&'a str>,
    pub material_id: Option<i64>,
    /// 這次用的情境主題，供之後輪換時避開
    pub topic: Option<&'a str>,
}

pub async fn create(db: &Db, ex: NewExercise<'_>, now: OffsetDateTime) -> Result<ExerciseId> {
    let targets = serde_json::to_string(ex.target_words).unwrap_or_else(|_| "[]".into());

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO exercise (profile_id, kind, material_id, payload_json,
                               target_lemmas_json, coverage, model, topic, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING id",
    )
    .bind(ex.profile_id.0)
    .bind(ex.kind)
    .bind(ex.material_id)
    .bind(ex.payload_json)
    .bind(&targets)
    .bind(ex.coverage)
    .bind(ex.model)
    .bind(ex.topic)
    .bind(ts::to_sql(now))
    .fetch_one(db.pool())
    .await?;

    Ok(ExerciseId(id))
}

/// 一次練習的完整內容。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExerciseRecord {
    pub id: i64,
    pub kind: String,
    pub payload_json: String,
    pub target_words: Vec<String>,
    pub coverage: Option<f64>,
    pub created_at: String,
    /// 已經作答過的話，最後一次的批改結果
    pub feedback_json: Option<String>,
}

fn row_to_record(row: &sqlx::sqlite::SqliteRow) -> ExerciseRecord {
    let targets: String = row.get("target_lemmas_json");
    ExerciseRecord {
        id: row.get("id"),
        kind: row.get("kind"),
        payload_json: row.get("payload_json"),
        target_words: serde_json::from_str(&targets).unwrap_or_default(),
        coverage: row.get("coverage"),
        created_at: row.get("created_at"),
        feedback_json: row.get("feedback_json"),
    }
}

const SELECT_EXERCISE: &str = "SELECT e.id, e.kind, e.payload_json, e.target_lemmas_json,
        e.coverage, e.created_at,
        (SELECT feedback_json FROM attempt WHERE exercise_id = e.id
         ORDER BY created_at DESC LIMIT 1) AS feedback_json
    FROM exercise e";

pub async fn get(db: &Db, id: ExerciseId) -> Result<Option<ExerciseRecord>> {
    let row = sqlx::query(&format!("{SELECT_EXERCISE} WHERE e.id = ?"))
        .bind(id.0)
        .fetch_optional(db.pool())
        .await?;
    Ok(row.as_ref().map(row_to_record))
}

/// 最近做過的練習，新的在前。`offset` 用來翻頁。
pub async fn recent(
    db: &Db,
    profile_id: ProfileId,
    limit: i64,
    offset: i64,
) -> Result<Vec<ExerciseRecord>> {
    let rows = sqlx::query(&format!(
        "{SELECT_EXERCISE} WHERE e.profile_id = ?
         ORDER BY e.created_at DESC, e.id DESC LIMIT ? OFFSET ?"
    ))
    .bind(profile_id.0)
    .bind(limit)
    .bind(offset.max(0))
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(row_to_record).collect())
}

/// 一共有幾份練習。分頁要靠它才說得出「第 2 頁 / 共 7 頁」。
pub async fn count(db: &Db, profile_id: ProfileId) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM exercise WHERE profile_id = ?")
            .bind(profile_id.0)
            .fetch_one(db.pool())
            .await?,
    )
}

/// 刪掉一份練習，連同它的作答紀錄（`attempt` 會 CASCADE）。
///
/// 綁 `profile_id` 而不是只用 id：這個參數從前端傳進來，
/// 少了這個條件就能刪到別人的練習。回傳有沒有真的刪到。
pub async fn delete(db: &Db, profile_id: ProfileId, id: ExerciseId) -> Result<bool> {
    let affected = sqlx::query("DELETE FROM exercise WHERE id = ? AND profile_id = ?")
        .bind(id.0)
        .bind(profile_id.0)
        .execute(db.pool())
        .await?
        .rows_affected();
    Ok(affected > 0)
}

/// 最近用過的情境主題，新的在後。用來輪換，避免每篇文章都在講校園生活。
///
/// `kinds` 限定只看哪些題型，理由跟 [`recent_target_words`] 一樣：
/// 記憶名額只有幾個，不限題型的話兩種題型會互相沖掉對方的歷史。
/// 閱讀與翻譯各自輪換自己的主題——它們的主題撞在一起沒有關係，
/// 一篇講旅行的文章配一題講旅行的翻譯反而是好事。
pub async fn recent_topics(
    db: &Db,
    profile_id: ProfileId,
    kinds: &[&str],
    limit: i64,
) -> Result<Vec<String>> {
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", kinds.len())
        .collect::<Vec<_>>()
        .join(",");

    let sql = format!(
        "SELECT topic FROM exercise
         WHERE profile_id = ? AND topic IS NOT NULL AND kind IN ({placeholders})
         ORDER BY created_at DESC LIMIT ?"
    );
    let mut q = sqlx::query_scalar::<_, String>(&sql).bind(profile_id.0);
    for kind in kinds {
        q = q.bind(*kind);
    }
    let rows: Vec<String> = q.bind(limit).fetch_all(db.pool()).await?;
    Ok(rows.into_iter().rev().collect())
}

/// 最近幾次出題教過的生詞。
///
/// ## 為什麼需要這個
///
/// 生詞是照詞頻決定性地挑出來的，而且**不會自動進牌組**——使用者讀完
/// 文章、從上下文看懂了、沒有標記任何字，那些字就永遠留在候選池裡。
/// 下一篇文章於是拿到一模一樣的六個字。
///
/// 排除最近教過的就會自然輪換。用歷史而不是亂數：亂數可能連續兩篇
/// 抽到同一個字，而且「教過的字隔幾篇再出現一次」本來就是我們要的
/// ——那是間隔重複，不是缺陷。
/// `kinds` 限定只看哪些題型。**一定要限定**：不限的話，中間穿插的文法題
/// 與翻譯題會佔掉記憶名額，把閱讀的歷史沖掉——做五題文法之後，
/// 下一篇閱讀就會拿回六篇前的同一批字。文法題存的 `target_words` 甚至
/// 是空的，佔了名額卻沒有排除任何東西。
pub async fn recent_target_words(
    db: &Db,
    profile_id: ProfileId,
    kinds: &[&str],
    limit: i64,
) -> Result<Vec<String>> {
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", kinds.len())
        .collect::<Vec<_>>()
        .join(",");

    let sql = format!(
        "SELECT target_lemmas_json FROM exercise
         WHERE profile_id = ? AND kind IN ({placeholders})
         ORDER BY created_at DESC LIMIT ?"
    );
    let mut q = sqlx::query_scalar::<_, String>(&sql).bind(profile_id.0);
    for kind in kinds {
        q = q.bind(*kind);
    }
    let rows: Vec<String> = q.bind(limit).fetch_all(db.pool()).await?;

    Ok(rows
        .iter()
        .filter_map(|json| serde_json::from_str::<Vec<String>>(json).ok())
        .flatten()
        .collect())
}

/// 最近做過的題型，用來避免連續出同一種。
pub async fn recent_kinds(db: &Db, profile_id: ProfileId, limit: i64) -> Result<Vec<String>> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT kind FROM exercise WHERE profile_id = ? ORDER BY created_at DESC LIMIT ?",
    )
    .bind(profile_id.0)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;
    // 呼叫端要的是時間順序（舊 → 新）
    Ok(rows.into_iter().rev().collect())
}

/// 記錄一次作答與批改結果。
pub async fn record_attempt(
    db: &Db,
    exercise_id: ExerciseId,
    answer_json: &str,
    score: Option<f64>,
    feedback_json: &str,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO attempt (exercise_id, answer_json, score, feedback_json, created_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(exercise_id.0)
    .bind(answer_json)
    .bind(score)
    .bind(feedback_json)
    .bind(ts::to_sql(now))
    .execute(db.pool())
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::profiles;

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    async fn setup() -> (Db, ProfileId) {
        let db = Db::open_in_memory().await.unwrap();
        let profile = profiles::create(&db, "我", "zh-TW", "en", t0())
            .await
            .unwrap();
        (db, profile)
    }

    async fn add(db: &Db, profile: ProfileId, kind: &str, at: OffsetDateTime) -> ExerciseId {
        create(
            db,
            NewExercise {
                profile_id: profile,
                kind,
                payload_json: r#"{"items":[]}"#,
                target_words: &["apple".to_string()],
                coverage: Some(0.96),
                model: Some("test-model"),
                material_id: None,
                topic: Some("校園生活"),
            },
            at,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn exercises_round_trip() {
        let (db, profile) = setup().await;
        let id = add(&db, profile, "reading", t0()).await;

        let got = get(&db, id).await.unwrap().unwrap();
        assert_eq!(got.kind, "reading");
        assert_eq!(got.target_words, vec!["apple"]);
        assert_eq!(got.coverage, Some(0.96));
        assert_eq!(got.feedback_json, None, "還沒作答");
    }

    /// 生詞是決定性挑出來的，而且不會自動進牌組——沒有這個查詢的話
    /// 每一篇文章都會拿到一模一樣的字。
    #[tokio::test]
    async fn recent_target_words_come_back_across_exercises() {
        let (db, profile) = setup().await;

        for (i, words) in [vec!["alpha", "beta"], vec!["gamma"]]
            .into_iter()
            .enumerate()
        {
            create(
                &db,
                NewExercise {
                    profile_id: profile,
                    kind: "reading",
                    payload_json: "{}",
                    target_words: &words.iter().map(|w| w.to_string()).collect::<Vec<_>>(),
                    coverage: None,
                    model: None,
                    material_id: None,
                    topic: None,
                },
                t0() + time::Duration::minutes(i as i64),
            )
            .await
            .unwrap();
        }

        let mut recent = recent_target_words(&db, profile, &["reading"], 5)
            .await
            .unwrap();
        recent.sort();
        assert_eq!(recent, vec!["alpha", "beta", "gamma"]);

        // 只看最近一篇時，更早的字要能重新被挑到
        let latest = recent_target_words(&db, profile, &["reading"], 1)
            .await
            .unwrap();
        assert_eq!(latest, vec!["gamma"]);
    }

    /// 中間穿插的文法題不能把閱讀的生詞歷史沖掉。
    ///
    /// 文法題存的 target_words 是空的，佔了記憶名額卻沒排除任何東西——
    /// 做五題文法之後，下一篇閱讀就會拿回六篇前的同一批字。
    #[tokio::test]
    async fn other_exercise_kinds_do_not_flush_the_reading_history() {
        let (db, profile) = setup().await;

        create(
            &db,
            NewExercise {
                profile_id: profile,
                kind: "reading",
                payload_json: "{}",
                target_words: &["alpha".to_string()],
                coverage: None,
                model: None,
                material_id: None,
                topic: None,
            },
            t0(),
        )
        .await
        .unwrap();

        // 之後做了五題文法
        for i in 1..=5 {
            create(
                &db,
                NewExercise {
                    profile_id: profile,
                    kind: "grammar",
                    payload_json: "{}",
                    target_words: &[],
                    coverage: None,
                    model: None,
                    material_id: None,
                    topic: None,
                },
                t0() + time::Duration::minutes(i),
            )
            .await
            .unwrap();
        }

        let recent = recent_target_words(&db, profile, &["reading"], 5)
            .await
            .unwrap();
        assert_eq!(recent, vec!["alpha"], "閱讀的歷史被文法題沖掉了");
    }

    #[tokio::test]
    async fn attempts_attach_to_their_exercise() {
        let (db, profile) = setup().await;
        let id = add(&db, profile, "translation_to_target", t0()).await;

        record_attempt(
            &db,
            id,
            r#"{"answers":["I ate an apple"]}"#,
            Some(80.0),
            r#"{"score":80,"corrections":[]}"#,
            t0(),
        )
        .await
        .unwrap();

        let got = get(&db, id).await.unwrap().unwrap();
        assert!(got.feedback_json.unwrap().contains("\"score\":80"));
    }

    /// 同一題重做時，拿到的該是最後一次的批改。
    #[tokio::test]
    async fn the_latest_attempt_wins() {
        let (db, profile) = setup().await;
        let id = add(&db, profile, "grammar", t0()).await;

        record_attempt(&db, id, "{}", Some(40.0), r#"{"score":40}"#, t0())
            .await
            .unwrap();
        record_attempt(
            &db,
            id,
            "{}",
            Some(90.0),
            r#"{"score":90}"#,
            t0() + time::Duration::minutes(5),
        )
        .await
        .unwrap();

        let got = get(&db, id).await.unwrap().unwrap();
        assert!(got.feedback_json.unwrap().contains("90"));
    }

    #[tokio::test]
    async fn recent_kinds_come_back_in_chronological_order() {
        let (db, profile) = setup().await;
        add(&db, profile, "reading", t0()).await;
        add(&db, profile, "grammar", t0() + time::Duration::minutes(1)).await;
        add(&db, profile, "cloze", t0() + time::Duration::minutes(2)).await;

        let kinds = recent_kinds(&db, profile, 10).await.unwrap();
        assert_eq!(kinds, vec!["reading", "grammar", "cloze"], "舊到新");
    }

    /// 文法弱點現在存在 grammar_point 表（有 FSRS 排程），
    /// 這裡只驗證批改結果有被完整保存下來。
    #[tokio::test]
    async fn feedback_is_stored_verbatim() {
        let (db, profile) = setup().await;
        let id = add(&db, profile, "translation_to_target", t0()).await;

        record_attempt(
            &db,
            id,
            "{}",
            Some(60.0),
            r#"{"corrections":[
                 {"grammar_point":"past tense"},
                 {"grammar_point":"articles"},
                 {"grammar_point":"past tense"}
               ]}"#,
            t0(),
        )
        .await
        .unwrap();

        let stored = get(&db, id).await.unwrap().unwrap().feedback_json.unwrap();
        assert!(stored.contains("past tense"));
        assert!(stored.contains("articles"));
    }

    /// 批改結果格式壞掉不該讓整個查詢失敗。
    #[tokio::test]
    async fn malformed_feedback_is_skipped() {
        let (db, profile) = setup().await;
        let id = add(&db, profile, "grammar", t0()).await;

        record_attempt(&db, id, "{}", None, "not json at all", t0())
            .await
            .unwrap();
        record_attempt(
            &db,
            id,
            "{}",
            None,
            r#"{"corrections":[{"grammar_point":"tense"}]}"#,
            t0() + time::Duration::minutes(1),
        )
        .await
        .unwrap();

        let stored = get(&db, id).await.unwrap().unwrap().feedback_json.unwrap();
        assert!(stored.contains("tense"), "最後一次的批改要保留下來");
    }

    /// 主題要記下來才輪換得了。
    #[tokio::test]
    async fn recent_topics_come_back_in_chronological_order() {
        let (db, profile) = setup().await;
        for (i, topic) in ["校園生活", "職場", "旅行"].iter().enumerate() {
            create(
                &db,
                NewExercise {
                    profile_id: profile,
                    kind: "reading",
                    payload_json: "{}",
                    target_words: &[],
                    coverage: None,
                    model: None,
                    material_id: None,
                    topic: Some(topic),
                },
                t0() + time::Duration::minutes(i as i64),
            )
            .await
            .unwrap();
        }

        let topics = recent_topics(&db, profile, &["reading"], 10).await.unwrap();
        assert_eq!(topics, vec!["校園生活", "職場", "旅行"], "舊到新");
    }

    /// 主題記憶只有幾個名額，不限題型的話兩種題型會互相沖掉對方的歷史：
    /// 翻譯連出幾題就把閱讀的主題擠光，下一篇文章又回到六篇前的題材。
    /// 這跟 `recent_target_words` 要限定題型是同一個理由。
    #[tokio::test]
    async fn topic_memory_is_not_shared_across_exercise_kinds() {
        let (db, profile) = setup().await;
        for (i, (kind, topic)) in [
            ("reading", "校園生活"),
            ("translation_to_target", "職場"),
            ("translation_to_native", "旅行"),
        ]
        .iter()
        .enumerate()
        {
            create(
                &db,
                NewExercise {
                    profile_id: profile,
                    kind,
                    payload_json: "{}",
                    target_words: &[],
                    coverage: None,
                    model: None,
                    material_id: None,
                    topic: Some(topic),
                },
                t0() + time::Duration::minutes(i as i64),
            )
            .await
            .unwrap();
        }

        let reading = recent_topics(&db, profile, &["reading"], 10).await.unwrap();
        assert_eq!(reading, vec!["校園生活"], "翻譯的主題不該算進閱讀的歷史");

        let translation = recent_topics(
            &db,
            profile,
            &["translation_to_target", "translation_to_native"],
            10,
        )
        .await
        .unwrap();
        assert_eq!(
            translation,
            vec!["職場", "旅行"],
            "翻譯的兩個方向算同一組，閱讀的不算"
        );

        assert!(
            recent_topics(&db, profile, &[], 10)
                .await
                .unwrap()
                .is_empty(),
            "沒指定題型時回空的，不要靜靜地回全部"
        );
    }

    #[tokio::test]
    async fn a_fresh_profile_has_no_history() {
        let (db, profile) = setup().await;
        assert!(recent_kinds(&db, profile, 5).await.unwrap().is_empty());
        assert!(recent(&db, profile, 5, 0).await.unwrap().is_empty());
        assert!(
            recent_topics(&db, profile, &["reading"], 5)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(count(&db, profile).await.unwrap(), 0);
    }

    /// 分頁不能重複也不能漏掉。同一秒建立的練習用 id 當第二排序鍵，
    /// 否則 SQLite 的順序不保證穩定，翻頁時同一份會出現兩次。
    #[tokio::test]
    async fn paging_walks_every_exercise_exactly_once() {
        let (db, profile) = setup().await;
        for _ in 0..7 {
            add(&db, profile, "reading", t0()).await;
        }

        assert_eq!(count(&db, profile).await.unwrap(), 7);

        let mut seen = Vec::new();
        for page in 0..3 {
            let ids: Vec<i64> = recent(&db, profile, 3, page * 3)
                .await
                .unwrap()
                .iter()
                .map(|r| r.id)
                .collect();
            seen.extend(ids);
        }

        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 7, "翻頁時有練習重複或漏掉");
    }

    /// 刪練習要連作答一起走，而且不能刪到別的 profile 的。
    #[tokio::test]
    async fn deleting_an_exercise_takes_its_attempts_with_it() {
        let (db, profile) = setup().await;
        let id = add(&db, profile, "reading", t0()).await;
        record_attempt(&db, id, "{}", Some(80.0), "{}", t0())
            .await
            .unwrap();

        let other = profiles::create(&db, "別人", "zh-TW", "en", t0())
            .await
            .unwrap();
        assert!(
            !delete(&db, other, id).await.unwrap(),
            "別的 profile 不該刪得掉"
        );
        assert!(get(&db, id).await.unwrap().is_some());

        assert!(delete(&db, profile, id).await.unwrap());
        assert!(get(&db, id).await.unwrap().is_none());

        let attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attempt")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(attempts, 0, "作答紀錄應該隨著練習一起 CASCADE");

        // 已經不存在的再刪一次不該報錯，只要說「沒刪到」
        assert!(!delete(&db, profile, id).await.unwrap());
    }
}
