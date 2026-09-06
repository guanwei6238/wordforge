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
    /// 記憶穩定度（天）。越大表示越熟。這是**證據**：排程算出來的。
    pub stability: Option<f64>,
    /// 使用者自己按「我會了」的時間。這是**主張**：他說的話。
    ///
    /// 跟 `stability` 分開存，理由見 0022 migration：按一次「我會了」
    /// 只會把 stability 推到 3.17 天，離「已學會」的 21 天還很遠，
    /// 混在一起的結果是那一下按了等於沒按。
    pub known_at: Option<String>,
}

fn row_to_point(row: &sqlx::sqlite::SqliteRow) -> GrammarPoint {
    GrammarPoint {
        point: row.get("point"),
        state: row.get("state"),
        due: row.get("due"),
        error_count: row.get("error_count"),
        correct_count: row.get("correct_count"),
        stability: row.get("stability"),
        known_at: row.get("known_at"),
    }
}

const SELECT_POINT: &str = "SELECT point, state, step, stability, difficulty, due,
    last_review, scheduled_days, error_count, correct_count, known_at FROM grammar_point";

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

/// 使用者按下「我會了」／「還要練」。
///
/// 做兩件事，而且**必須是兩件**：
///
/// 1. 照樣送一次 FSRS 評分（會了＝答對、還要練＝答錯），排程才會跟著動
/// 2. 記下（或清掉）`known_at`——那是使用者說的話，不是算出來的
///
/// 只做第一件的話，按一次「我會了」只會把 stability 推到 3.17 天，
/// 而「已學會」的門檻是 21 天：按鈕看起來沒有反應，出題也不會知道
/// 他會這個句型。那正是這個功能原本壞掉的樣子。
pub async fn set_known(
    db: &Db,
    profile_id: ProfileId,
    point: &str,
    known: bool,
    scheduler: &Scheduler,
    now: OffsetDateTime,
) -> Result<()> {
    // 先跑排程：這一步會把列建出來，下面的 UPDATE 才找得到它
    record(db, profile_id, point, known, scheduler, now).await?;

    sqlx::query("UPDATE grammar_point SET known_at = ?3 WHERE profile_id = ?1 AND point = ?2")
        .bind(profile_id.0)
        .bind(point.trim())
        // 「還要練」要真的清掉，否則收不回自己標錯的那一下
        .bind(known.then(|| ts::to_sql(now)))
        .execute(db.pool())
        .await?;
    Ok(())
}

/// 現在該練的文法點，最久沒複習的排前面。
///
/// 出題時只送這幾個給模型——prompt 大小固定，
/// 練習做得再多也不會讓 token 膨脹。
///
/// **只回錯誤標籤（`kind='point'`），不回句型。** 句型走
/// [`due_patterns`]：它要另外受難度上限約束，而且拿去指派給翻譯題，
/// 跟「這幾個文法你最近常錯」是兩件事。
///
/// 定義已經被刪掉的識別碼仍然算標籤（`COALESCE(kind, 'point')`）——
/// 刪掉一份教材不該讓累積的學習歷史從排程裡消失。
pub async fn due_points(
    db: &Db,
    profile_id: ProfileId,
    lang: &str,
    now: OffsetDateTime,
    limit: i64,
) -> Result<Vec<String>> {
    let points: Vec<String> = sqlx::query_scalar(
        "SELECT p.point FROM grammar_point p
         LEFT JOIN grammar_def d ON d.lang = ?2 AND d.point = p.point
         WHERE p.profile_id = ?1 AND p.due <= ?3 AND COALESCE(d.kind, ?4) = ?4
         ORDER BY p.due ASC LIMIT ?5",
    )
    .bind(profile_id.0)
    .bind(lang)
    .bind(ts::to_sql(now))
    .bind(KIND_POINT)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;
    Ok(points)
}

/// 現在該練的句型：到期的排前面，接著是**還沒練過的**，由簡入難。
///
/// ## 為什麼沒練過的也要進來
///
/// 只挑「到期」的話，一個句型永遠不會被第一次指派——`grammar_point`
/// 那一列要練過才會出現。整份課綱會靜靜躺在資料庫裡，一條都不會被教到。
/// 這正是這個專案踩過的那個坑的形狀：文法選單只列 `state != null` 的點，
/// 於是自己加的點練過才選得到，沒選過就永遠練不到。
///
/// ## 上限也在這裡生效
///
/// 超過 `ceiling` 的句型不會被指派——難度上限不只是「不要用」，
/// 也包含「不要現在教」。沒有分級的句型（`level_ordinal IS NULL`）
/// 一律通過：我們不知道它難不難，擋掉它等於憑空造出一條規則。
///
/// ## 這條查詢會排序，而且排得掉的只有一半
///
/// `EXPLAIN QUERY PLAN` 上有 `USE TEMP B-TREE FOR ORDER BY`：排序的第一個
/// 鍵是 `p.due`，它來自 JOIN 進來的另一張表，索引接不上。**這裡接受它**，
/// 理由是列數的上限不是由使用時間決定的——它是「這個語言有幾個句型」，
/// 也就是一份課綱的長度（幾百條），而且整條查詢每出一題才跑一次，
/// 旁邊就是一趟好幾秒的模型呼叫。
///
/// 會隨使用時間長大的是 `grammar_point`，而那一側走的是
/// `sqlite_autoindex_grammar_point_1`。
pub async fn due_patterns(
    db: &Db,
    profile_id: ProfileId,
    lang: &str,
    ceiling: Option<i64>,
    now: OffsetDateTime,
    limit: i64,
) -> Result<Vec<String>> {
    let points: Vec<String> = sqlx::query_scalar(
        "SELECT d.point FROM grammar_def d
         LEFT JOIN grammar_point p ON p.point = d.point AND p.profile_id = ?1
         WHERE d.lang = ?2 AND d.kind = ?3
           AND (?4 IS NULL OR d.level_ordinal IS NULL OR d.level_ordinal <= ?4)
           AND (p.due IS NULL OR p.due <= ?5)
         ORDER BY (p.due IS NULL), p.due ASC, d.level_ordinal ASC, d.sort_order ASC
         LIMIT ?6",
    )
    .bind(profile_id.0)
    .bind(lang)
    .bind(KIND_PATTERN)
    .bind(ceiling)
    .bind(ts::to_sql(now))
    .bind(limit)
    .fetch_all(db.pool())
    .await?;
    Ok(points)
}

/// 使用者**已經會的句型**：標記過「我會了」，或練到撐得過門檻的。
///
/// 取兩者的**聯集**，因為它們是兩件不同的真話：`known_at` 是他說的，
/// `stability` 是排程算的。缺任何一邊都會漏——只看 stability 的話，
/// 剛按完「我會了」的句型撈不到（按一次只到 3.17 天）；只看 known_at
/// 的話，練到滾瓜爛熟但從沒按過按鈕的句型撈不到。
///
/// ## 這份清單是做什麼用的
///
/// 兩件事，而且是這整套機制真正貼合使用者的地方：
///
/// 1. **出題時列給模型看**——「這些他讀得懂，放心用」。沒有這份清單的話，
///    模型只知道什麼不能用，不知道什麼好用，於是句子會退化成一種形狀。
/// 2. **從禁止清單裡扣掉**——他自己標記會了的句型，就算級數高過上限
///    也不該被擋。上限是「還沒教到哪」的預設值，不是「他不會什麼」的事實；
///    使用者親手標記的才是事實。
///
/// `stability_days` 要跟 UI 上「已學會」的定義**同一個數字**。兩邊各用
/// 各的門檻，就會出現「畫面說學會了，出題還當他不會」——這個專案踩過
/// 那個坑：prompt 說他掌握 5200 個字，驗收只認 stability ≥ 21 天的卡片，
/// 結果覆蓋率永遠 0%。
///
/// 排序由難到易：模型最需要知道的是**他會的上緣在哪**。
pub async fn known_patterns(
    db: &Db,
    profile_id: ProfileId,
    lang: &str,
    stability_days: f64,
) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query(
        "SELECT d.point AS point, d.name AS name
         FROM grammar_def d
         JOIN grammar_point p ON p.point = d.point AND p.profile_id = ?1
         WHERE d.lang = ?2 AND d.kind = ?3
           AND (p.known_at IS NOT NULL OR p.stability >= ?4)
         ORDER BY d.level_ordinal DESC, d.sort_order, d.point",
    )
    .bind(profile_id.0)
    .bind(lang)
    .bind(KIND_PATTERN)
    .bind(stability_days)
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .iter()
        .map(|row| (row.get("point"), row.get("name")))
        .collect())
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
        let due = due_points(&db, profile, "en", t0() + Duration::minutes(5), 10)
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
            due_points(&db, profile, "en", when + Duration::days(1), 10)
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
        assert!(
            due_points(&db, profile, "en", when, 10)
                .await
                .unwrap()
                .is_empty()
        );

        record(&db, profile, "tense", false, &sched, when)
            .await
            .unwrap();
        assert_eq!(
            due_points(&db, profile, "en", when + Duration::minutes(30), 10)
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

        let due = due_points(&db, profile, "en", t0() + Duration::hours(1), 3)
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
        assert!(
            due_points(&db, profile, "en", t0(), 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(all_points(&db, profile).await.unwrap().is_empty());
    }
}

// ---------------------------------------------------------------- 文法點的定義

/// 一個文法點的定義：名稱、講解、例句。
///
/// 跟 [`GrammarPoint`]（掌握狀態）分開：定義是教材，狀態是每個人自己的。
#[derive(Debug, Clone, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct GrammarDef {
    #[serde(default)]
    pub id: i64,
    /// 語言代碼。**匯入時不必寫**——`import_grammar` 一律用 profile 的
    /// 目標語言覆寫它，讓檔案指定只會讓那份定義消失在另一個語言底下。
    ///
    /// 沒有這個 `default` 的話，文件寫的最小格式
    /// `[{"point": …, "name": …}]` 會直接解析失敗（missing field `lang`），
    /// 而錯誤訊息看起來像使用者的檔案寫錯了。
    #[serde(default)]
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
    /// 難度標示，由來源決定（CEFR 的 A2、JLPT 的 N4…）。
    ///
    /// **這是顯示用的名稱，程式不解讀它**——字串沒有順序，
    /// 排得出先後的是 [`GrammarDef::level_ordinal`]。
    #[serde(default)]
    pub level: Option<String>,
    /// 分級刻度上的位置。`None` 表示沒有分級，那時這一筆不參與難度上限
    /// ——我們不知道它難不難，猜一個等於憑空造出一條規則。
    #[serde(default)]
    pub level_ordinal: Option<i64>,
    /// `point`（批改用的錯誤標籤）或 `pattern`（教學用的句型）。
    ///
    /// 兩者要的粒度相反，見 0021 migration 的說明。預設是 `point`：
    /// 既有的資料與舊格式的匯入檔案都是錯誤標籤。
    #[serde(default = "default_kind")]
    pub kind: String,
    /// 怎麼在句子裡認出這個句型。空的就是「偵測不到」——
    /// 那時它只能靠模型自報，難度上限對它不生效。
    #[serde(default)]
    pub detectors: Vec<wordforge_core::patterns::Detector>,
    #[serde(default)]
    pub sort_order: i64,
    /// seed（程式碼種子）/ import（匯入）/ manual（自己加）
    #[serde(default)]
    pub origin: String,
}

/// 批改用的錯誤標籤。
pub const KIND_POINT: &str = "point";
/// 教學用的句型。
pub const KIND_PATTERN: &str = "pattern";

fn default_kind() -> String {
    KIND_POINT.to_string()
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
    let detectors: String = row.get("detectors_json");
    GrammarDef {
        id: row.get("id"),
        lang: row.get("lang"),
        point: row.get("point"),
        name: row.get("name"),
        explanation: row.get("explanation"),
        // 例句壞掉不該讓整頁打不開——那是附加內容，不是主線
        examples: serde_json::from_str(&examples).unwrap_or_default(),
        level: row.get("level"),
        level_ordinal: row.get("level_ordinal"),
        kind: row.get("kind"),
        // 偵測器讀不出來時**要出聲**。它壞掉的症狀是「難度檢查安靜地
        // 不生效」，跟例句壞掉（少看幾句）完全不是同一個等級的事。
        detectors: match serde_json::from_str(&detectors) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(
                    point = %row.get::<String, _>("point"),
                    error = %e,
                    "偵測器讀不出來，這個句型的難度檢查不會生效"
                );
                Vec::new()
            }
        },
        sort_order: row.get("sort_order"),
        origin: row.get("origin"),
    }
}

const SELECT_DEF: &str = "SELECT id, lang, point, name, explanation, examples_json,
    level, level_ordinal, kind, detectors_json, sort_order, origin FROM grammar_def";

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

/// 批改用的錯誤標籤清單。
///
/// **只回 `kind='point'`**。句型不能混進來：那份清單會原樣列進批改
/// prompt，上百條課綱句型會同時撐爆 prompt、並且把「最常錯的文法點」
/// 稀釋成一堆各錯一次的標籤——0005 與 0011 的註解各講過一次這件事。
pub async fn list_points(db: &Db, lang: &str) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT point FROM grammar_def WHERE lang = ? AND kind = ?
         ORDER BY sort_order, point",
    )
    .bind(lang)
    .bind(KIND_POINT)
    .fetch_all(db.pool())
    .await?)
}

/// 這個語言的**全部**識別碼，錯誤標籤與句型都算。
///
/// ## 跟 [`list_points`] 的分工
///
/// `list_points` 是**給模型挑的選單**（批改時「從這份清單挑一個標籤」），
/// 所以要窄——句型混進去會撐爆 prompt 並稀釋排程。
///
/// 這一份是**認回來用的字典**：把模型回報的標籤收斂到某個真實存在的
/// 識別碼。它必須寬，因為指定句型出的練習，題目標籤就是那個句型的
/// 識別碼——拿窄的那份去比對會認不出來，然後**整份練習的對錯被無聲
/// 丟掉**：使用者答完五題，畫面一切正常，什麼都沒記到。
pub async fn list_all_points(db: &Db, lang: &str) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT point FROM grammar_def WHERE lang = ? ORDER BY sort_order, point",
    )
    .bind(lang)
    .fetch_all(db.pool())
    .await?)
}

/// 這個語言的句型，轉成偵測層看得懂的形狀。
///
/// 只撈 `kind='pattern'`：錯誤標籤沒有偵測器，也不參與難度上限。
/// 回傳的是**定義**，還沒編譯——編譯（含控制組自我檢查）在
/// [`wordforge_core::patterns::PatternSet::compile`]。
pub async fn pattern_defs(
    db: &Db,
    lang: &str,
) -> Result<Vec<wordforge_core::patterns::PatternDef>> {
    let defs = list_defs_of_kind(db, lang, KIND_PATTERN).await?;
    Ok(defs
        .into_iter()
        .map(|d| wordforge_core::patterns::PatternDef {
            point: d.point,
            name: d.name,
            level_ordinal: d.level_ordinal,
            level: d.level,
            detectors: d.detectors,
        })
        .collect())
}

/// 某個語言、某一種 kind 的全部定義。
pub async fn list_defs_of_kind(db: &Db, lang: &str, kind: &str) -> Result<Vec<GrammarDef>> {
    let rows = sqlx::query(&format!(
        "{SELECT_DEF} WHERE lang = ? AND kind = ? ORDER BY level_ordinal, sort_order, point"
    ))
    .bind(lang)
    .bind(kind)
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(row_to_def).collect())
}

/// 分級刻度上的一格。設定頁的「我現在的程度」選單就是這個。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LevelOption {
    /// 顯示名稱（`"國中七年級"`）。可能是 NULL——那時只有序號。
    pub level: Option<String>,
    pub ordinal: i64,
    /// 這一級有幾個句型。使用者要看得出「選這一級等於開放多少東西」。
    pub patterns: i64,
    /// 其中有幾個**偵測得到**。
    ///
    /// 這個數字一定要露出來：偵測不到的句型只能靠模型自報，
    /// 難度上限對它不生效。兩個數字差很多的話，使用者該知道
    /// 這一級的保護其實很薄，而不是以為系統擋得住。
    pub detectable: i64,
}

/// 這個語言的句型分級有哪幾格。
///
/// **選項來自資料，程式不預設任何分級體系**——跟「設定頁的語言選單
/// 來自字典裡有什麼」是同一件事。匯入台灣課綱就得到國小國中高中，
/// 匯入 CEFR 就得到 A1..C2，程式兩種都不認識。
pub async fn level_options(db: &Db, lang: &str) -> Result<Vec<LevelOption>> {
    let rows = sqlx::query(
        "SELECT level_ordinal AS ordinal,
                MIN(level) AS level,
                COUNT(*) AS patterns,
                SUM(CASE WHEN detectors_json <> '[]' THEN 1 ELSE 0 END) AS detectable
         FROM grammar_def
         WHERE lang = ? AND kind = ? AND level_ordinal IS NOT NULL
         GROUP BY level_ordinal
         ORDER BY level_ordinal",
    )
    .bind(lang)
    .bind(KIND_PATTERN)
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .iter()
        .map(|row| LevelOption {
            level: row.get("level"),
            ordinal: row.get("ordinal"),
            patterns: row.get("patterns"),
            detectable: row.get("detectable"),
        })
        .collect())
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
    let detectors = serde_json::to_string(&def.detectors).unwrap_or_else(|_| "[]".into());
    let ts = ts::to_sql(now);
    let kind = if def.kind.is_empty() {
        KIND_POINT
    } else {
        def.kind.as_str()
    };

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO grammar_def
             (lang, point, name, explanation, examples_json, level, sort_order, origin,
              level_ordinal, kind, detectors_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?10, ?11, ?12, ?9, ?9)
         ON CONFLICT (lang, point) DO UPDATE SET
             name          = excluded.name,
             -- 只在新的有內容時才覆蓋：匯入一份只有名稱的清單，
             -- 不該把已經生成好的講解洗掉
             explanation   = COALESCE(NULLIF(excluded.explanation, ''), grammar_def.explanation),
             examples_json = CASE WHEN excluded.examples_json = '[]'
                                  THEN grammar_def.examples_json
                                  ELSE excluded.examples_json END,
             level         = COALESCE(excluded.level, grammar_def.level),
             level_ordinal = COALESCE(excluded.level_ordinal, grammar_def.level_ordinal),
             kind          = excluded.kind,
             -- 跟例句同一個道理：匯入一份沒有偵測器的清單，不該把
             -- 已經寫好、已經通過控制組的那些洗掉
             detectors_json = CASE WHEN excluded.detectors_json = '[]'
                                   THEN grammar_def.detectors_json
                                   ELSE excluded.detectors_json END,
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
    .bind(def.level_ordinal)
    .bind(kind)
    .bind(&detectors)
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
                level: Some((*level).to_string()),
                sort_order: i as i64,
                origin: "seed".into(),
                // 種子是錯誤標籤，不是句型：它們不參與難度上限，
                // 也不該讓 CEFR 的刻度跟匯入的課綱刻度混在一起。
                kind: KIND_POINT.into(),
                ..GrammarDef::default()
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
    use time::Duration;

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
            lang: "en".into(),
            point: point.into(),
            name: name.into(),
            origin: "manual".into(),
            kind: KIND_POINT.into(),
            ..GrammarDef::default()
        }
    }

    /// 一個看得到、驗得過的句型。`positive` 一定要真的命中——
    /// 測試資料要像真實資料，而真實資料是會被 `compile_detector` 擋下來的。
    fn pattern(point: &str, ordinal: i64, regex: &str, positive: &str) -> GrammarDef {
        GrammarDef {
            lang: "en".into(),
            point: point.into(),
            name: point.into(),
            kind: KIND_PATTERN.into(),
            level: Some(format!("第 {ordinal} 級")),
            level_ordinal: Some(ordinal),
            detectors: vec![wordforge_core::patterns::Detector {
                kind: "regex".into(),
                value: regex.into(),
                positive: vec![positive.into()],
                negative: Vec::new(),
            }],
            origin: "import".into(),
            ..GrammarDef::default()
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

    /// 這條測試存在的理由：`lang` 曾經沒有 `default`，於是文件與 UI 上
    /// 寫的最小格式 `[{"point": …, "name": …}]` **解析不了**——
    /// 匯入會回「missing field `lang`」，而那看起來像使用者的檔案寫錯了。
    /// 語言本來就由 profile 決定，檔案裡不該有它。
    #[test]
    fn the_documented_minimal_import_format_parses() {
        let json = r#"[{"point": "there-be", "name": "there is / there are"}]"#;
        let defs: Vec<GrammarDef> = serde_json::from_str(json).expect("最小格式該解析得了");
        assert_eq!(defs[0].point, "there-be");
        assert_eq!(defs[0].kind, KIND_POINT, "沒寫 kind 就是錯誤標籤");
        assert!(defs[0].lang.is_empty(), "語言由 profile 決定，檔案裡不該有");
    }

    #[tokio::test]
    async fn a_definition_needs_an_identifier_and_a_name() {
        let db = setup().await;
        assert!(upsert_def(&db, &def("  ", "時態"), t0()).await.is_err());
        assert!(upsert_def(&db, &def("tense", " "), t0()).await.is_err());
    }

    /// 這條測試存在的理由：句型與錯誤標籤住在同一張表，而它們要的粒度
    /// 相反。上百條課綱句型如果混進批改用的標籤清單，會同時撐爆 prompt
    /// 並把「最常錯的文法點」稀釋成一堆各錯一次的東西——0005 的註解
    /// 已經為了這件事把排程從「數次數」改成 FSRS 一次了。
    #[tokio::test]
    async fn a_sentence_pattern_is_not_offered_as_an_error_label() {
        let db = setup().await;
        upsert_def(&db, &def("tense", "時態"), t0()).await.unwrap();
        upsert_def(
            &db,
            &pattern(
                "conditional-2",
                3,
                r"\bif\b[^.?!]*\bwould\b",
                "If I knew, I would say.",
            ),
            t0(),
        )
        .await
        .unwrap();

        let labels = list_points(&db, "en").await.unwrap();
        assert_eq!(labels, vec!["tense".to_string()]);

        let patterns = pattern_defs(&db, "en").await.unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].point, "conditional-2");
        assert_eq!(patterns[0].level_ordinal, Some(3));
        assert_eq!(patterns[0].detectors.len(), 1);
    }

    /// 同一件事的排程那一側：句型練過之後會在 `grammar_point` 留下紀錄，
    /// 而那張表沒有 kind 欄位。少了 JOIN 的話，句型會跑進
    /// 「這幾個文法你最近常錯」餵給批改 prompt。
    #[tokio::test]
    async fn a_due_pattern_does_not_leak_into_the_weak_point_list() {
        let db = setup().await;
        let profile = ProfileId(1);
        let scheduler = Scheduler::default();
        upsert_def(&db, &def("tense", "時態"), t0()).await.unwrap();
        upsert_def(
            &db,
            &pattern(
                "conditional-2",
                3,
                r"\bif\b[^.?!]*\bwould\b",
                "If I knew, I would say.",
            ),
            t0(),
        )
        .await
        .unwrap();

        record(&db, profile, "tense", false, &scheduler, t0())
            .await
            .unwrap();
        record(&db, profile, "conditional-2", false, &scheduler, t0())
            .await
            .unwrap();

        let later = t0() + Duration::days(1);
        let weak = due_points(&db, profile, "en", later, 10).await.unwrap();
        assert_eq!(weak, vec!["tense".to_string()], "句型混進錯誤標籤了");
    }

    /// 這條測試存在的理由是它曾經是同一個形狀的錯：文法選單只列
    /// `state != null` 的點，於是自己加的點**練過才選得到**，
    /// 而沒選過就永遠練不到。句型如果只從 `grammar_point` 撈到期的，
    /// 整份匯入的課綱會靜靜躺著，一條都不會被第一次指派。
    #[tokio::test]
    async fn a_pattern_never_practised_is_still_offered() {
        let db = setup().await;
        upsert_def(
            &db,
            &pattern("there-be", 1, r"\bthere\s+(is|are)\b", "There is a cat."),
            t0(),
        )
        .await
        .unwrap();

        let due = due_patterns(&db, ProfileId(1), "en", None, t0(), 5)
            .await
            .unwrap();
        assert_eq!(due, vec!["there-be".to_string()]);
    }

    /// 難度上限不只是「不要用」，也包含「不要現在教」。
    #[tokio::test]
    async fn a_pattern_above_the_ceiling_is_not_assigned() {
        let db = setup().await;
        upsert_def(
            &db,
            &pattern("there-be", 1, r"\bthere\s+(is|are)\b", "There is a cat."),
            t0(),
        )
        .await
        .unwrap();
        upsert_def(
            &db,
            &pattern(
                "inversion",
                8,
                r"^never\s+\w+\s+(i|he|she|we|they)\b",
                "Never have I seen it.",
            ),
            t0(),
        )
        .await
        .unwrap();

        let due = due_patterns(&db, ProfileId(1), "en", Some(3), t0(), 5)
            .await
            .unwrap();
        assert_eq!(due, vec!["there-be".to_string()]);

        // 上限拉高就教得到
        let due = due_patterns(&db, ProfileId(1), "en", Some(8), t0(), 5)
            .await
            .unwrap();
        assert_eq!(due.len(), 2);
    }

    /// 到期的排在還沒練過的前面：忘掉的東西比沒學過的東西急。
    #[tokio::test]
    async fn a_pattern_that_is_due_comes_before_one_never_seen() {
        let db = setup().await;
        let scheduler = Scheduler::default();
        upsert_def(
            &db,
            &pattern("there-be", 1, r"\bthere\s+(is|are)\b", "There is a cat."),
            t0(),
        )
        .await
        .unwrap();
        upsert_def(
            &db,
            &pattern("past-simple", 2, r"\b\w+ed\b", "I walked home."),
            t0(),
        )
        .await
        .unwrap();

        // 第 2 級的那個練過而且答錯了，所以很快就到期
        record(&db, ProfileId(1), "past-simple", false, &scheduler, t0())
            .await
            .unwrap();

        let later = t0() + Duration::days(1);
        let due = due_patterns(&db, ProfileId(1), "en", None, later, 5)
            .await
            .unwrap();
        assert_eq!(due[0], "past-simple", "到期的要排在沒學過的前面");
    }

    /// 跟講解、例句同一個道理：重匯一份沒有偵測器的清單，不該把已經
    /// 寫好、已經通過控制組的 regex 洗掉。那些東西沒有備份。
    #[tokio::test]
    async fn a_bare_reimport_keeps_the_detectors() {
        let db = setup().await;
        let full = pattern("there-be", 1, r"\bthere\s+(is|are)\b", "There is a cat.");
        upsert_def(&db, &full, t0()).await.unwrap();

        let mut bare = full.clone();
        bare.detectors = Vec::new();
        bare.name = "there is / there are".into();
        upsert_def(&db, &bare, t0()).await.unwrap();

        let got = get_def(&db, "en", "there-be").await.unwrap().unwrap();
        assert_eq!(got.name, "there is / there are", "名稱該更新");
        assert_eq!(got.detectors.len(), 1, "偵測器被洗掉了");
    }

    /// 這條測試存在的理由是它曾經是錯的：按「我會了」只送一次 FSRS 的
    /// Good，而初始 stability 是 3.17 天、「已學會」的門檻是 21 天。
    /// 於是**按一次等於沒按**——畫面仍然顯示「在學」，出題也不知道
    /// 他會這個句型，而使用者不知道自己按的那一下去了哪裡。
    #[tokio::test]
    async fn one_click_on_i_know_this_is_enough_to_count() {
        let db = setup().await;
        let scheduler = Scheduler::default();
        upsert_def(
            &db,
            &pattern("there-be", 1, r"\bthere\s+(is|are)\b", "There is a cat."),
            t0(),
        )
        .await
        .unwrap();

        set_known(&db, ProfileId(1), "there-be", true, &scheduler, t0())
            .await
            .unwrap();

        let known = known_patterns(&db, ProfileId(1), "en", 21.0).await.unwrap();
        assert_eq!(known.len(), 1, "按了一次「我會了」卻撈不到");
        assert_eq!(known[0].0, "there-be");

        // 排程照樣要動——自評與作答匯流到同一個進度，不是兩套
        let state = all_points(&db, ProfileId(1)).await.unwrap();
        assert_eq!(state[0].correct_count, 1);
        assert!(
            state[0].stability.is_some_and(|s| s < 21.0),
            "證據還沒到門檻，但主張已經在了"
        );
    }

    /// 收得回來：標錯的那一下要按得掉，否則自評變成單向閥門。
    #[tokio::test]
    async fn marking_it_for_practice_again_takes_the_claim_back() {
        let db = setup().await;
        let scheduler = Scheduler::default();
        upsert_def(
            &db,
            &pattern("there-be", 1, r"\bthere\s+(is|are)\b", "There is a cat."),
            t0(),
        )
        .await
        .unwrap();

        set_known(&db, ProfileId(1), "there-be", true, &scheduler, t0())
            .await
            .unwrap();
        set_known(&db, ProfileId(1), "there-be", false, &scheduler, t0())
            .await
            .unwrap();

        assert!(
            known_patterns(&db, ProfileId(1), "en", 21.0)
                .await
                .unwrap()
                .is_empty(),
            "按了「還要練」卻收不回標記"
        );
    }

    /// 練到撐得過門檻的也算——兩個來源取聯集。
    /// 只認自評的話，練了半年卻從沒按過按鈕的句型會被當成他不會。
    #[tokio::test]
    async fn practising_it_until_it_sticks_also_counts_as_known() {
        let db = setup().await;
        let scheduler = Scheduler::default();
        upsert_def(
            &db,
            &pattern("there-be", 1, r"\bthere\s+(is|are)\b", "There is a cat."),
            t0(),
        )
        .await
        .unwrap();

        // 沒有按過任何按鈕，只是一直答對
        let mut when = t0();
        for _ in 0..6 {
            record(&db, ProfileId(1), "there-be", true, &scheduler, when)
                .await
                .unwrap();
            when += Duration::days(30);
        }

        let state = all_points(&db, ProfileId(1)).await.unwrap();
        assert!(state[0].known_at.is_none(), "他從來沒按過按鈕");
        assert!(
            state[0].stability.is_some_and(|s| s >= 21.0),
            "答對六次之後穩定度是 {:?}",
            state[0].stability
        );
        assert_eq!(
            known_patterns(&db, ProfileId(1), "en", 21.0)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// 分級選單來自資料，程式不預設任何分級體系——匯入台灣課綱就得到
    /// 國小國中高中，匯入 CEFR 就得到 A1..C2，程式兩種都不認識。
    #[tokio::test]
    async fn the_level_menu_comes_from_whatever_was_imported() {
        let db = setup().await;
        let mut p1 = pattern("there-be", 1, r"\bthere\s+(is|are)\b", "There is a cat.");
        p1.level = Some("國小".into());
        let mut p2 = pattern("past-simple", 1, r"\b\w+ed\b", "I walked home.");
        p2.level = Some("國小".into());
        let mut p3 = pattern("relative", 4, r"\b(who|which)\b", "The man who came.");
        p3.level = Some("國中".into());
        // 偵測不到的那一條也要算進總數，但不能算進 detectable
        let mut p4 = pattern("subjunctive", 4, r"\bwish\b", "I wish I could.");
        p4.level = Some("國中".into());
        p4.detectors = Vec::new();
        for d in [&p1, &p2, &p3, &p4] {
            upsert_def(&db, d, t0()).await.unwrap();
        }

        let levels = level_options(&db, "en").await.unwrap();
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].level.as_deref(), Some("國小"));
        assert_eq!(levels[0].ordinal, 1);
        assert_eq!(levels[0].patterns, 2);
        assert_eq!(levels[1].level.as_deref(), Some("國中"));
        assert_eq!(levels[1].patterns, 2);
        assert_eq!(
            levels[1].detectable, 1,
            "偵測不到的句型要看得出來，否則使用者會以為系統擋得住"
        );
    }
}
