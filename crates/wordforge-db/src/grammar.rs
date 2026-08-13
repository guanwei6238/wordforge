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

// ---------------------------------------------------------------- 文法點的定義

/// 一個文法點的定義：名稱、講解、例句。
///
/// 跟 [`GrammarPoint`]（掌握狀態）分開：定義是教材，狀態是每個人自己的。
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct GrammarDef {
    #[serde(default)]
    pub id: i64,
    pub lang: String,
    /// 受控識別碼，與 `grammar_point.point` 對應
    pub point: String,
    /// 給使用者看的名稱，用母語寫
    pub name: String,
    /// 母語講解。`None` 表示還沒講解過。
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    pub examples: Vec<GrammarExample>,
    /// 難度標示，由來源決定（CEFR 的 A2、JLPT 的 N4…）
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub sort_order: i64,
    /// seed（程式碼種子）/ import（匯入）/ manual（自己加）
    #[serde(default)]
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct GrammarExample {
    /// 目標語的例句
    pub text: String,
    /// 母語翻譯
    #[serde(default)]
    pub translation: Option<String>,
}

fn row_to_def(row: &sqlx::sqlite::SqliteRow) -> GrammarDef {
    let examples: String = row.get("examples_json");
    GrammarDef {
        id: row.get("id"),
        lang: row.get("lang"),
        point: row.get("point"),
        name: row.get("name"),
        explanation: row.get("explanation"),
        // 例句壞掉不該讓整頁打不開——那是附加內容，不是主線
        examples: serde_json::from_str(&examples).unwrap_or_default(),
        level: row.get("level"),
        sort_order: row.get("sort_order"),
        origin: row.get("origin"),
    }
}

const SELECT_DEF: &str = "SELECT id, lang, point, name, explanation, examples_json,
    level, sort_order, origin FROM grammar_def";

/// 某個語言的全部文法點定義，照 `sort_order` 排。
pub async fn list_defs(db: &Db, lang: &str) -> Result<Vec<GrammarDef>> {
    let rows = sqlx::query(&format!(
        "{SELECT_DEF} WHERE lang = ? ORDER BY sort_order, point"
    ))
    .bind(lang)
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(row_to_def).collect())
}

/// 只要識別碼。出題與正規化用得到，不必把講解一起撈出來。
pub async fn list_points(db: &Db, lang: &str) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT point FROM grammar_def WHERE lang = ? ORDER BY sort_order, point",
    )
    .bind(lang)
    .fetch_all(db.pool())
    .await?)
}

pub async fn get_def(db: &Db, lang: &str, point: &str) -> Result<Option<GrammarDef>> {
    let row = sqlx::query(&format!("{SELECT_DEF} WHERE lang = ? AND point = ?"))
        .bind(lang)
        .bind(point)
        .fetch_optional(db.pool())
        .await?;
    Ok(row.as_ref().map(row_to_def))
}

/// 新增或更新一個定義。回傳它的 id。
///
/// `(lang, point)` 是主鍵：同一個識別碼重複匯入會覆蓋，不會長出兩筆。
/// **講解與例句只在有給的時候才覆蓋**——匯入一份只有名稱的清單，
/// 不該把使用者辛苦生成的講解洗掉。
pub async fn upsert_def(db: &Db, def: &GrammarDef, now: OffsetDateTime) -> Result<i64> {
    let point = def.point.trim();
    let name = def.name.trim();
    if point.is_empty() || name.is_empty() {
        return Err(crate::DbError::Invalid(
            "文法點的識別碼與名稱不能是空的".into(),
        ));
    }

    let examples = serde_json::to_string(&def.examples).unwrap_or_else(|_| "[]".into());
    let ts = ts::to_sql(now);

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO grammar_def
             (lang, point, name, explanation, examples_json, level, sort_order, origin,
              created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
         ON CONFLICT (lang, point) DO UPDATE SET
             name          = excluded.name,
             -- 只在新的有內容時才覆蓋：匯入一份只有名稱的清單，
             -- 不該把已經生成好的講解洗掉
             explanation   = COALESCE(NULLIF(excluded.explanation, ''), grammar_def.explanation),
             examples_json = CASE WHEN excluded.examples_json = '[]'
                                  THEN grammar_def.examples_json
                                  ELSE excluded.examples_json END,
             level         = COALESCE(excluded.level, grammar_def.level),
             sort_order    = excluded.sort_order,
             updated_at    = excluded.updated_at
         RETURNING id",
    )
    .bind(&def.lang)
    .bind(point)
    .bind(name)
    .bind(def.explanation.as_deref())
    .bind(&examples)
    .bind(def.level.as_deref())
    .bind(def.sort_order)
    .bind(if def.origin.is_empty() {
        "manual"
    } else {
        &def.origin
    })
    .bind(&ts)
    .fetch_one(db.pool())
    .await?;

    Ok(id)
}

/// 刪掉一個定義。**不動掌握狀態**——`grammar_point` 那邊的排程與對錯
/// 次數是學習歷史，刪掉一份教材不該把它抹掉。
pub async fn delete_def(db: &Db, lang: &str, point: &str) -> Result<bool> {
    let affected = sqlx::query("DELETE FROM grammar_def WHERE lang = ? AND point = ?")
        .bind(lang)
        .bind(point)
        .execute(db.pool())
        .await?
        .rows_affected();
    Ok(affected > 0)
}

/// 第一次使用某個語言時，把程式碼裡的種子寫進資料表；種子清單改版時
/// 補上缺的那些。回傳這次寫了幾筆。
///
/// ## 為什麼不是「有資料就不動」
///
/// 原本只要該語言有任何一筆定義就直接返回，於是種子清單改了之後，
/// **早就用過的資料庫永遠看不到新增的點**——只有全新安裝的人拿得到。
/// 那等於清單只能改給未來的使用者看。
///
/// ## 為什麼也不是「每次都補齊缺的」
///
/// 那樣使用者刪掉一個用不到的點之後，下次開 App 它就回來了，
/// 而且怎麼刪都刪不掉。所以補齊靠 [`SEED_VERSION`] 版號**只跑一次**：
/// 清單改版才補，補完記下版號。
///
/// [`SEED_VERSION`]: wordforge_core::grammar_points::SEED_VERSION
///
/// ## 補齊時碰什麼、不碰什麼
///
/// INSERT 缺的識別碼；另外把**我們自己種下的**那些列（`origin = 'seed'`）
/// 的 `level` 與 `sort_order` 對齊到新版種子。
///
/// 只補 `level` 是不夠的：既有資料庫裡那 26 筆的等級全是 NULL、順序是
/// 舊版的排法，只加新點的話文法頁會變成「新的十四個有分級、舊的沒有，
/// 而且順序還是亂的」——看起來像功能壞了一半，實際上就是壞了一半。
///
/// **`name`、`explanation`、`examples` 一律不碰。** 那些可能是使用者
/// 自己改的、或花了一次模型呼叫生出來的，沒有備份，洗掉就沒了。
/// `origin` 不是 `seed` 的列（使用者自己加的、匯入的）整列都不動。
///
/// 沒有種子的語言（日文、法文…）回傳 0，文法頁會是空的並提示匯入。
/// 那是誠實的：硬套英文的分類只會產生垃圾資料。
pub async fn seed_defs(db: &Db, lang: &str, now: OffsetDateTime) -> Result<usize> {
    use wordforge_core::grammar_points::SEED_VERSION;

    let seed = wordforge_core::grammar_points::seed_for(lang);
    if seed.is_empty() {
        return Ok(0);
    }

    let version_key = format!("grammar_seed:{lang}");
    let applied = crate::meta::get_i64(db, &version_key).await?;

    let existing: Vec<String> = sqlx::query_scalar("SELECT point FROM grammar_def WHERE lang = ?")
        .bind(lang)
        .fetch_all(db.pool())
        .await?;

    // 版號已經是最新的就什麼都不做。**這個判斷要在 `existing` 之前生效**，
    // 否則使用者刪掉的點每次啟動都會回來。
    if applied == Some(SEED_VERSION) {
        return Ok(0);
    }

    let mut written = 0usize;
    for (i, (point, name, level)) in seed.iter().enumerate() {
        if existing.iter().any(|e| e == point) {
            // 已經有了：只把等級與順序對齊，而且只碰我們自己種的那些列。
            // `name` 與講解不在 SET 裡——那是使用者的東西。
            sqlx::query(
                "UPDATE grammar_def
                 SET level = COALESCE(level, ?3), sort_order = ?4, updated_at = ?5
                 WHERE lang = ?1 AND point = ?2 AND origin = 'seed'",
            )
            .bind(lang)
            .bind(*point)
            .bind(*level)
            .bind(i as i64)
            .bind(ts::to_sql(now))
            .execute(db.pool())
            .await?;
            continue;
        }
        upsert_def(
            db,
            &GrammarDef {
                id: 0,
                lang: lang.to_string(),
                point: (*point).to_string(),
                name: (*name).to_string(),
                explanation: None,
                examples: Vec::new(),
                level: Some((*level).to_string()),
                sort_order: i as i64,
                origin: "seed".into(),
            },
            now,
        )
        .await?;
        written += 1;
    }

    crate::meta::set_i64(db, &version_key, SEED_VERSION).await?;
    Ok(written)
}

#[cfg(test)]
mod def_tests {
    use super::*;
    use crate::repo::profiles;

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    async fn setup() -> Db {
        let db = Db::open_in_memory().await.unwrap();
        profiles::create(&db, "我", "zh-TW", "en", t0())
            .await
            .unwrap();
        db
    }

    fn def(point: &str, name: &str) -> GrammarDef {
        GrammarDef {
            id: 0,
            lang: "en".into(),
            point: point.into(),
            name: name.into(),
            explanation: None,
            examples: Vec::new(),
            level: None,
            sort_order: 0,
            origin: "manual".into(),
        }
    }

    #[tokio::test]
    async fn definitions_round_trip_with_their_examples() {
        let db = setup().await;
        let mut d = def("conditionals", "條件句");
        d.explanation = Some("第二類條件句用來講與現在事實相反的假設。".into());
        d.examples = vec![GrammarExample {
            text: "If I had more time, I would learn Japanese.".into(),
            translation: Some("如果我有更多時間，我會學日文。".into()),
        }];
        upsert_def(&db, &d, t0()).await.unwrap();

        let got = get_def(&db, "en", "conditionals").await.unwrap().unwrap();
        assert_eq!(got.name, "條件句");
        assert_eq!(got.examples.len(), 1);
        assert_eq!(
            got.examples[0].translation.as_deref(),
            Some("如果我有更多時間，我會學日文。")
        );
    }

    /// 這條測試存在的理由：匯入一份只有名稱的清單，不該把使用者
    /// 辛苦生成的講解與例句洗掉。那種資料沒有備份，洗掉就沒了。
    #[tokio::test]
    async fn a_bare_import_does_not_wipe_an_existing_explanation() {
        let db = setup().await;

        let mut rich = def("tense", "時態");
        rich.explanation = Some("AI 生成的講解".into());
        rich.examples = vec![GrammarExample {
            text: "I went there yesterday.".into(),
            translation: None,
        }];
        upsert_def(&db, &rich, t0()).await.unwrap();

        // 之後匯入一份只有名稱的清單
        let mut bare = def("tense", "時態（新名稱）");
        bare.origin = "import".into();
        upsert_def(&db, &bare, t0()).await.unwrap();

        let got = get_def(&db, "en", "tense").await.unwrap().unwrap();
        assert_eq!(got.name, "時態（新名稱）", "名稱該更新");
        assert_eq!(
            got.explanation.as_deref(),
            Some("AI 生成的講解"),
            "講解被匯入洗掉了"
        );
        assert_eq!(got.examples.len(), 1, "例句被匯入洗掉了");
    }

    #[tokio::test]
    async fn seeding_only_happens_once() {
        let db = setup().await;

        let first = seed_defs(&db, "en", t0()).await.unwrap();
        assert!(first > 20, "英文種子應該有二十幾項，實際 {first}");

        // 使用者編輯過
        let mut edited = def("tense", "我自己改的名字");
        edited.explanation = Some("我自己寫的".into());
        upsert_def(&db, &edited, t0()).await.unwrap();

        let second = seed_defs(&db, "en", t0()).await.unwrap();
        assert_eq!(second, 0, "已經有資料就不該再種一次");

        let got = get_def(&db, "en", "tense").await.unwrap().unwrap();
        assert_eq!(got.name, "我自己改的名字", "使用者的編輯被種子蓋掉了");
    }

    /// 種子要帶 CEFR 等級。`level` 欄位資料表一直都有，但種子從來沒填過，
    /// 所以文法頁沒辦法說「這個點你現在還用不到」。
    #[tokio::test]
    async fn seeded_points_carry_their_level() {
        let db = setup().await;
        seed_defs(&db, "en", t0()).await.unwrap();

        let defs = list_defs(&db, "en").await.unwrap();
        assert!(
            defs.iter().all(|d| d.level.is_some()),
            "有種子的點都該有等級：{:?}",
            defs.iter()
                .filter(|d| d.level.is_none())
                .map(|d| &d.point)
                .collect::<Vec<_>>()
        );

        // 順序要是教材的順序，不是字母序——冠詞排在倒裝前面
        let order = |p: &str| defs.iter().position(|d| d.point == p).unwrap();
        assert!(
            order("articles") < order("inversion"),
            "A1 的點該排在 C1 前面"
        );
    }

    /// 這條測試存在的理由是它曾經是錯的：`seed_defs` 只要該語言有任何
    /// 一筆定義就直接返回，於是種子清單改版之後，**早就用過的資料庫
    /// 永遠看不到新增的點**——只有全新安裝的人拿得到。
    #[tokio::test]
    async fn a_new_seed_version_tops_up_an_existing_database() {
        let db = setup().await;

        // 一個「舊版」的資料庫：只有幾個點，版號停在 1
        for (i, (point, name)) in [("tense", "時態"), ("articles", "冠詞")].iter().enumerate() {
            let mut d = def(point, name);
            d.origin = "seed".into();
            d.sort_order = i as i64;
            upsert_def(&db, &d, t0()).await.unwrap();
        }
        let mut edited = def("tense", "我自己改的名字");
        edited.explanation = Some("我自己寫的".into());
        upsert_def(&db, &edited, t0()).await.unwrap();
        crate::meta::set_i64(&db, "grammar_seed:en", 1)
            .await
            .unwrap();

        let added = seed_defs(&db, "en", t0()).await.unwrap();
        assert!(added > 0, "改版之後該補上缺的點");

        let defs = list_defs(&db, "en").await.unwrap();
        assert!(
            defs.iter().any(|d| d.point == "reported-speech"),
            "新增的點沒有補進來"
        );

        // 名稱與講解是使用者的東西，不能碰
        let tense = get_def(&db, "en", "tense").await.unwrap().unwrap();
        assert_eq!(tense.name, "我自己改的名字", "使用者改的名稱被蓋掉了");
        assert_eq!(
            tense.explanation.as_deref(),
            Some("我自己寫的"),
            "使用者的講解被蓋掉了"
        );

        // 但等級與順序要對齊：只補新的點的話，舊的那些永遠沒有等級、
        // 順序還是舊版的排法，文法頁等於只做了一半
        assert!(
            defs.iter().all(|d| d.level.is_some()),
            "既有的點沒有補上等級：{:?}",
            defs.iter()
                .filter(|d| d.level.is_none())
                .map(|d| &d.point)
                .collect::<Vec<_>>()
        );
        let order = |p: &str| defs.iter().position(|d| d.point == p).unwrap();
        assert!(
            order("articles") < order("tense"),
            "既有的點沒有照新版順序重排"
        );

        // 補完就記下版號，再跑一次不該重複做事
        assert_eq!(seed_defs(&db, "en", t0()).await.unwrap(), 0);
    }

    /// 補齊只跑一次，否則使用者刪掉用不到的點之後，下次開 App 它就回來了，
    /// 而且怎麼刪都刪不掉。
    #[tokio::test]
    async fn a_deleted_point_does_not_come_back_on_the_next_launch() {
        let db = setup().await;
        seed_defs(&db, "en", t0()).await.unwrap();

        assert!(delete_def(&db, "en", "inversion").await.unwrap());
        seed_defs(&db, "en", t0()).await.unwrap();

        let defs = list_defs(&db, "en").await.unwrap();
        assert!(
            !defs.iter().any(|d| d.point == "inversion"),
            "刪掉的點又被種回來了"
        );
    }

    /// 沒有種子的語言開箱是空的——硬套英文的分類只會產生垃圾資料。
    #[tokio::test]
    async fn a_language_without_a_seed_starts_empty() {
        let db = setup().await;
        assert_eq!(seed_defs(&db, "ja", t0()).await.unwrap(), 0);
        assert!(list_defs(&db, "ja").await.unwrap().is_empty());
    }

    /// 刪掉定義不該抹掉學習歷史——那是使用者練出來的，教材是可替換的。
    #[tokio::test]
    async fn deleting_a_definition_keeps_the_learning_history() {
        let db = setup().await;
        let profile = ProfileId(1);
        upsert_def(&db, &def("tense", "時態"), t0()).await.unwrap();

        let scheduler = Scheduler::default();
        record(&db, profile, "tense", false, &scheduler, t0())
            .await
            .unwrap();

        assert!(delete_def(&db, "en", "tense").await.unwrap());
        assert!(get_def(&db, "en", "tense").await.unwrap().is_none());

        let points = all_points(&db, profile).await.unwrap();
        assert_eq!(points.len(), 1, "掌握狀態被連帶刪掉了");
        assert_eq!(points[0].error_count, 1);
    }

    #[tokio::test]
    async fn a_definition_needs_an_identifier_and_a_name() {
        let db = setup().await;
        assert!(upsert_def(&db, &def("  ", "時態"), t0()).await.is_err());
        assert!(upsert_def(&db, &def("tense", " "), t0()).await.is_err());
    }
}
