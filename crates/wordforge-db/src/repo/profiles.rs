//! 使用者是誰、學什麼語言、每天學幾個。
//!
//! 這裡的東西全部是**設定**：換一台電腦要帶著走，但重匯字典不該動到它。
//! `native_lang` / `target_lang` 兩個欄位尤其重要——「載入哪個語言的字典
//! 就能學哪個語言」這個承諾靠它們流下去，曾經有很長一段時間它們
//! 只被寫進去、從來沒有被讀出來過。

use sqlx::Row;
use time::OffsetDateTime;
use wordforge_core::model::ProfileId;

use crate::ts;
use crate::{Db, DbError, Result};

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

/// 學習設定。存在 `profile.settings_json`，UI 可以調。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StudySettings {
    /// 每天引入幾張新卡
    pub new_per_day: i64,
    /// 每天最多複習幾張
    pub max_reviews_per_day: i64,
    /// FSRS 的目標記憶留存率。調高記得更牢但複習量大增。
    pub desired_retention: f64,
    /// 閱讀文章要有多少比例是你看得懂的字。
    ///
    /// 這是「90% 法則」的那個數字，但實際上多少最好因人而異：
    /// 想輕鬆讀順的人設高一點，想每篇都學到東西的人設低一點。
    /// 生詞的數量由這個值反推——設 0.90 的話 300 字的文章
    /// 會有約 30 個生詞詞元，設 0.96 只有 12 個。
    pub reading_coverage: f64,
    /// 閱讀測驗的文章字級（px）。
    ///
    /// 不做成「小/中/大」三段：合適的字級同時取決於螢幕、距離、
    /// 視力與語言（漢字要比拉丁字母大才看得清楚筆畫），
    /// 三段一定有人剛好卡在中間。存成數字，UI 給加減鈕。
    pub reading_font_size: i64,
}

impl Default for StudySettings {
    fn default() -> Self {
        Self {
            // 每張新卡當天要按兩三次才畢業，15 張大約是 10 分鐘的量
            new_per_day: 15,
            // 長假回來不要被幾百張淹沒
            max_reviews_per_day: 200,
            desired_retention: 0.9,
            // 0.96 落在「最適」區間中央：讀得動，又每篇都有幾個新字
            reading_coverage: 0.96,
            // 跟介面其他文字一樣大。兩欄版面下再放大會讓一行放不了
            // 幾個字，要一直換行反而更累；想大想小都自己按 A± 調。
            reading_font_size: 16,
        }
    }
}

impl StudySettings {
    /// 把使用者輸入夾到合理範圍。
    ///
    /// 留存率特別要夾：低於 0.7 會忘光，高於 0.97 複習量會爆炸，
    /// 而且 FSRS 的公式在 0 或 1 會直接壞掉。
    fn clamped(self) -> Self {
        Self {
            new_per_day: self.new_per_day.clamp(0, 500),
            max_reviews_per_day: self.max_reviews_per_day.clamp(10, 9_999),
            desired_retention: self.desired_retention.clamp(0.70, 0.97),
            // 低於 0.80 就不是「可理解輸入」而是查字典；
            // 高於 0.99 等於整篇都會，讀了學不到東西
            reading_coverage: self.reading_coverage.clamp(0.80, 0.99),
            // 小於 12px 標點看不清楚，大於 32px 一行放不了幾個字，
            // 眼睛要一直換行反而更累
            reading_font_size: self.reading_font_size.clamp(12, 32),
        }
    }
}

/// `study_settings` 從 JSON 撈回來的原始欄位。每個都可能是 NULL：
/// 舊的 profile 沒有新加的設定，缺的用預設值補。
type SettingsRow = (
    Option<i64>,
    Option<i64>,
    Option<f64>,
    Option<f64>,
    Option<i64>,
);

pub async fn study_settings(db: &Db, profile_id: ProfileId) -> Result<StudySettings> {
    let row: SettingsRow = sqlx::query_as(
        "SELECT CAST(json_extract(settings_json, '$.new_per_day') AS INTEGER),
                CAST(json_extract(settings_json, '$.max_reviews_per_day') AS INTEGER),
                CAST(json_extract(settings_json, '$.desired_retention') AS REAL),
                CAST(json_extract(settings_json, '$.reading_coverage') AS REAL),
                CAST(json_extract(settings_json, '$.reading_font_size') AS INTEGER)
         FROM profile WHERE id = ? AND json_valid(settings_json)",
    )
    .bind(profile_id.0)
    .fetch_optional(db.pool())
    .await?
    .unwrap_or((None, None, None, None, None));

    let d = StudySettings::default();
    Ok(StudySettings {
        new_per_day: row.0.unwrap_or(d.new_per_day),
        max_reviews_per_day: row.1.unwrap_or(d.max_reviews_per_day),
        desired_retention: row.2.unwrap_or(d.desired_retention),
        reading_coverage: row.3.unwrap_or(d.reading_coverage),
        reading_font_size: row.4.unwrap_or(d.reading_font_size),
    }
    .clamped())
}

/// 更新學習設定，回傳實際存下來的值（已夾到合理範圍）。
pub async fn update_study_settings(
    db: &Db,
    profile_id: ProfileId,
    settings: StudySettings,
) -> Result<StudySettings> {
    let s = settings.clamped();
    sqlx::query(
        "UPDATE profile
         SET settings_json = json_set(
                 CASE WHEN json_valid(settings_json) THEN settings_json ELSE '{}' END,
                 '$.new_per_day', ?,
                 '$.max_reviews_per_day', ?,
                 '$.desired_retention', ?,
                 '$.reading_coverage', ?,
                 '$.reading_font_size', ?)
         WHERE id = ?",
    )
    .bind(s.new_per_day)
    .bind(s.max_reviews_per_day)
    .bind(s.desired_retention)
    .bind(s.reading_coverage)
    .bind(s.reading_font_size)
    .bind(profile_id.0)
    .execute(db.pool())
    .await?;
    Ok(s)
}

/// 重置清掉了多少東西。UI 要說得出「刪了幾張卡、幾份練習」，
/// 不然使用者按完只看到畫面變空，不知道發生了什麼。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct ResetSummary {
    pub cards: i64,
    pub reviews: i64,
    pub exercises: i64,
    pub attempts: i64,
    pub grammar_points: i64,
    pub llm_calls: i64,
}

/// 把這個 profile 的學習資料清空，回到剛安裝的狀態。
///
/// ## 刪什麼、不刪什麼
///
/// 刪：卡片、複習歷程、練習與批改、文法弱點、用量統計、對話，
/// 以及 `settings_json` 裡的一切（含分級測驗估出來的詞彙量）。
///
/// **不刪字典，也不刪教材**。那兩樣是使用者自己匯入的外部資料，
/// 重匯一份 Wiktionary 要好幾分鐘，而且跟「我想重新開始學」無關。
/// 想清掉字典的話那是「重新匯入」，不是「重置進度」。
///
/// 在同一個交易裡做完：中途失敗會留下卡片還在但練習沒了的半殘狀態，
/// 那比不刪更糟。
pub async fn reset_progress(db: &Db, profile_id: ProfileId) -> Result<ResetSummary> {
    let mut tx = db.pool().begin().await?;

    // 先數再刪。刪完就數不到了，而且 review_log / attempt 是靠
    // CASCADE 連帶刪掉的，`rows_affected` 根本看不到它們。
    let cards: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE profile_id = ?")
        .bind(profile_id.0)
        .fetch_one(&mut *tx)
        .await?;
    let reviews: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM review_log r
         JOIN card c ON c.id = r.card_id WHERE c.profile_id = ?",
    )
    .bind(profile_id.0)
    .fetch_one(&mut *tx)
    .await?;
    let exercises: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM exercise WHERE profile_id = ?")
        .bind(profile_id.0)
        .fetch_one(&mut *tx)
        .await?;
    let attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM attempt a
         JOIN exercise e ON e.id = a.exercise_id WHERE e.profile_id = ?",
    )
    .bind(profile_id.0)
    .fetch_one(&mut *tx)
    .await?;
    let grammar_points: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM grammar_point WHERE profile_id = ?")
            .bind(profile_id.0)
            .fetch_one(&mut *tx)
            .await?;
    let llm_calls: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM llm_call WHERE profile_id = ?")
        .bind(profile_id.0)
        .fetch_one(&mut *tx)
        .await?;

    // review_log 隨 card、attempt 隨 exercise、message 隨 conversation
    // 一起 CASCADE 掉，不必也不該自己刪。
    for table in [
        "card",
        "exercise",
        "grammar_point",
        "llm_call",
        "conversation",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE profile_id = ?"))
            .bind(profile_id.0)
            .execute(&mut *tx)
            .await?;
    }

    // 設定整份清掉，包含分級測驗的估計值與每日額度。
    // 只挑幾個 key 刪的話，之後新加的設定會被漏掉——而漏掉的樣子是
    // 「重置了但某個舊值還在」，很難查。
    sqlx::query("UPDATE profile SET settings_json = '{}' WHERE id = ?")
        .bind(profile_id.0)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(ResetSummary {
        cards,
        reviews,
        exercises,
        attempts,
        grammar_points,
        llm_calls,
    })
}

/// 這個 profile 在學什麼語言、母語是什麼。
///
/// 欄位一直都在，但先前所有地方都硬編 `"en"`——
/// 於是「換一份字典就能學另一種語言」這個設計目標名存實亡。
pub async fn languages(db: &Db, profile_id: ProfileId) -> Result<(String, String)> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT native_lang, target_lang FROM profile WHERE id = ?")
            .bind(profile_id.0)
            .fetch_optional(db.pool())
            .await?;
    Ok(row.unwrap_or_else(|| ("zh-TW".into(), "en".into())))
}

/// 改掉這個 profile 在學什麼語言。
///
/// 空字串會被拒絕：語言代碼一旦變成空的，之後每個字典查詢都會查不到，
/// 而且失敗的樣子是「一片空白」而不是報錯，很難查。
pub async fn set_languages(
    db: &Db,
    profile_id: ProfileId,
    native: &str,
    target: &str,
) -> Result<(String, String)> {
    let native = native.trim();
    let target = target.trim();
    if native.is_empty() || target.is_empty() {
        return Err(DbError::Invalid("語言代碼不能是空的".into()));
    }

    sqlx::query("UPDATE profile SET native_lang = ?, target_lang = ? WHERE id = ?")
        .bind(native)
        .bind(target)
        .bind(profile_id.0)
        .execute(db.pool())
        .await?;
    Ok((native.to_string(), target.to_string()))
}

/// 今天額外加開的新卡額度。
///
/// 存成 `{"extra_new": {"date": "2026-08-11", "count": 10}}`：
/// 帶著日期才能在隔天自動失效。只存數字的話，今天多學 30 個，
/// 之後每天都會變成 45 張。
pub async fn extra_new_today(db: &Db, profile_id: ProfileId, today: &str) -> Result<i64> {
    let row: Option<(Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT json_extract(settings_json, '$.extra_new.date'),
                CAST(json_extract(settings_json, '$.extra_new.count') AS INTEGER)
         FROM profile WHERE id = ? AND json_valid(settings_json)",
    )
    .bind(profile_id.0)
    .fetch_optional(db.pool())
    .await?;

    Ok(match row {
        Some((Some(date), Some(count))) if date == today => count.max(0),
        _ => 0,
    })
}

/// 加開額度，回傳今天累計加開了多少。
pub async fn add_extra_new_today(
    db: &Db,
    profile_id: ProfileId,
    today: &str,
    extra: i64,
) -> Result<i64> {
    let total = extra_new_today(db, profile_id, today).await? + extra.max(0);
    sqlx::query(
        "UPDATE profile
         SET settings_json = json_set(
                 CASE WHEN json_valid(settings_json) THEN settings_json ELSE '{}' END,
                 '$.extra_new', json_object('date', ?, 'count', ?))
         WHERE id = ?",
    )
    .bind(today)
    .bind(total)
    .bind(profile_id.0)
    .execute(db.pool())
    .await?;
    Ok(total)
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

#[cfg(test)]
mod tests {
    use crate::repo::fixture::*;
    use crate::repo::{NewLemma, cards, lemmas, profiles};
    use wordforge_core::model::{CardKind, Rating};
    use wordforge_core::srs::Scheduler;

    /// 重置要真的把學習資料清乾淨，但**不能碰字典與教材**——
    /// 那兩樣是使用者自己匯入的外部資料，重匯一份 Wiktionary 要好幾分鐘。
    #[tokio::test]
    async fn resetting_clears_the_learning_data_but_spares_the_dictionary() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 3).await;

        // 複習一張，留下 review_log
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 10, 200)
            .await
            .unwrap();
        let (next, log) = Scheduler::default().review(&queue[0], Rating::Good, t0(), None);
        cards::record_review(&db, &next, &log).await.unwrap();

        // 一份做過的練習
        let exercise = crate::exercises::create(
            &db,
            crate::exercises::NewExercise {
                profile_id: profile,
                kind: "reading",
                payload_json: "{}",
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
        crate::exercises::record_attempt(&db, exercise, "{}", Some(80.0), "{}", t0())
            .await
            .unwrap();

        profiles::update_study_settings(
            &db,
            profile,
            profiles::StudySettings {
                new_per_day: 42,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let lemmas_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM lemma")
            .fetch_one(db.pool())
            .await
            .unwrap();

        let summary = profiles::reset_progress(&db, profile).await.unwrap();
        assert_eq!(summary.cards, 3);
        assert_eq!(summary.reviews, 1, "複習歷程也要數進去");
        assert_eq!(summary.exercises, 1);
        assert_eq!(summary.attempts, 1);

        for table in ["card", "exercise", "grammar_point", "llm_call"] {
            let left: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM {table} WHERE profile_id = ?"
            ))
            .bind(profile.0)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(left, 0, "{table} 沒清乾淨");
        }
        // CASCADE 沒生效的話會留下孤兒
        for table in ["review_log", "attempt"] {
            let left: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert_eq!(left, 0, "{table} 應該隨著上層一起被 CASCADE 掉");
        }

        assert_eq!(
            profiles::study_settings(&db, profile).await.unwrap(),
            profiles::StudySettings::default(),
            "設定要回到預設，包含分級測驗估的詞彙量"
        );

        let lemmas_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM lemma")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(lemmas_after, lemmas_before, "字典被誤刪了");
    }

    #[tokio::test]
    async fn languages_come_from_the_profile() {
        let (db, profile) = setup().await;
        assert_eq!(
            profiles::languages(&db, profile).await.unwrap(),
            ("zh-TW".to_string(), "en".to_string())
        );

        let jp = profiles::create(&db, "日文", "zh-TW", "ja", t0())
            .await
            .unwrap();
        assert_eq!(profiles::languages(&db, jp).await.unwrap().1, "ja");
    }

    #[tokio::test]
    async fn study_settings_round_trip_with_sensible_defaults() {
        let (db, profile) = setup().await;

        let d = profiles::study_settings(&db, profile).await.unwrap();
        assert_eq!(d, profiles::StudySettings::default());

        let saved = profiles::update_study_settings(
            &db,
            profile,
            profiles::StudySettings {
                new_per_day: 40,
                max_reviews_per_day: 300,
                desired_retention: 0.85,
                ..d
            },
        )
        .await
        .unwrap();
        assert_eq!(saved.new_per_day, 40);

        let loaded = profiles::study_settings(&db, profile).await.unwrap();
        assert_eq!(loaded, saved);
    }

    /// 留存率超出範圍會讓 FSRS 的公式壞掉（0 或 1 直接是除以零），
    /// 而且 0.99 的複習量是 0.9 的好幾倍，不該讓使用者誤設。
    #[tokio::test]
    async fn study_settings_are_clamped_to_a_usable_range() {
        let (db, profile) = setup().await;

        let s = profiles::update_study_settings(
            &db,
            profile,
            profiles::StudySettings {
                new_per_day: -5,
                max_reviews_per_day: 0,
                desired_retention: 1.5,
                reading_coverage: 2.0,
                reading_font_size: 400,
            },
        )
        .await
        .unwrap();

        assert_eq!(s.new_per_day, 0, "0 是合法的（今天先不學新字）");
        assert_eq!(s.max_reviews_per_day, 10);
        assert!((s.desired_retention - 0.97).abs() < 1e-9);
        assert!((s.reading_coverage - 0.99).abs() < 1e-9);
        assert_eq!(s.reading_font_size, 32, "字級太大一行放不了幾個字");

        // 存進去的也必須是夾過的值，不能只在回傳時夾
        assert_eq!(profiles::study_settings(&db, profile).await.unwrap(), s);
    }

    /// 設定會直接影響佇列，不能只是存起來好看。
    #[tokio::test]
    async fn new_per_day_setting_changes_the_queue() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 100).await;

        let s = profiles::update_study_settings(
            &db,
            profile,
            profiles::StudySettings {
                new_per_day: 40,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let queue = cards::daily_queue(&db, profile, t0(), t0(), s.new_per_day, 200)
            .await
            .unwrap();
        assert_eq!(queue.len(), 40);
    }

    /// 「再學 10 個」必須留得住，而且隔天要自動回到預設。
    ///
    /// 實際踩過：額度只存在單次回應裡，前端接著重新取佇列時又回到每日上限 15，
    /// 而今天已經學滿 15 張，於是按了「再學 10 個」只跳出一張到期的舊卡。
    #[tokio::test]
    async fn extra_quota_persists_today_and_resets_tomorrow() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 100).await;

        assert_eq!(
            profiles::extra_new_today(&db, profile, "2026-08-11")
                .await
                .unwrap(),
            0
        );

        // 加開兩次，要累加而不是覆蓋
        profiles::add_extra_new_today(&db, profile, "2026-08-11", 10)
            .await
            .unwrap();
        let total = profiles::add_extra_new_today(&db, profile, "2026-08-11", 30)
            .await
            .unwrap();
        assert_eq!(total, 40);

        // 再讀一次還在（不是只存在於某一次回應裡）
        assert_eq!(
            profiles::extra_new_today(&db, profile, "2026-08-11")
                .await
                .unwrap(),
            40
        );

        // 隔天自動失效，否則今天多學 30 個會讓之後每天都是 45 張
        assert_eq!(
            profiles::extra_new_today(&db, profile, "2026-08-12")
                .await
                .unwrap(),
            0
        );

        // 額度確實反映在佇列上
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 15 + 40, 200)
            .await
            .unwrap();
        assert_eq!(queue.len(), 55);
    }

    /// 設定檔壞掉或還沒有任何設定時，不能讓整個佇列查詢失敗。
    #[tokio::test]
    async fn broken_settings_fall_back_to_no_extra_quota() {
        let (db, profile) = setup().await;
        sqlx::query("UPDATE profile SET settings_json = 'not json' WHERE id = ?")
            .bind(profile.0)
            .execute(db.pool())
            .await
            .unwrap();

        assert_eq!(
            profiles::extra_new_today(&db, profile, "2026-08-11")
                .await
                .unwrap(),
            0
        );
        // 寫入時會把壞掉的內容換成合法 JSON
        assert_eq!(
            profiles::add_extra_new_today(&db, profile, "2026-08-11", 5)
                .await
                .unwrap(),
            5
        );
    }

    /// 換語言是使用者真的會做的事，而且做完之後舊牌組不會自己消失。
    #[tokio::test]
    async fn switching_language_reports_the_leftover_deck() {
        let (db, profile) = setup().await;
        let english = add_word(&db, "apple", 500).await;
        let japanese = lemmas::upsert(
            &db,
            NewLemma {
                lang: "ja",
                text: "林檎",
                pos: "noun",
                freq_rank: Some(500),
                cefr: None,
            },
        )
        .await
        .unwrap();
        cards::ensure(&db, profile, english, CardKind::Recognition, t0())
            .await
            .unwrap();
        cards::ensure(&db, profile, japanese, CardKind::Recognition, t0())
            .await
            .unwrap();

        let (native, target) = profiles::set_languages(&db, profile, "zh-TW", "ja")
            .await
            .unwrap();
        assert_eq!((native.as_str(), target.as_str()), ("zh-TW", "ja"));
        assert_eq!(
            profiles::languages(&db, profile).await.unwrap(),
            ("zh-TW".to_string(), "ja".to_string()),
            "改完要真的存進去"
        );

        assert_eq!(
            cards::count_other_languages(&db, profile, "ja")
                .await
                .unwrap(),
            1,
            "那張英文卡還在牌組裡，必須講出來"
        );

        assert_eq!(
            cards::suspend_other_languages(&db, profile, "ja")
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            cards::count_other_languages(&db, profile, "ja")
                .await
                .unwrap(),
            0
        );

        // 收起來不是刪除：換回英文時那張卡還在
        let still_there: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE profile_id = ? AND suspended = 1")
                .bind(profile.0)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(still_there, 1);
    }

    /// 空的語言代碼會讓之後每個字典查詢都靜靜地查不到東西。
    #[tokio::test]
    async fn an_empty_language_code_is_rejected() {
        let (db, profile) = setup().await;
        assert!(
            profiles::set_languages(&db, profile, "zh-TW", "   ")
                .await
                .is_err()
        );
        assert_eq!(
            profiles::languages(&db, profile).await.unwrap().1,
            "en",
            "被拒絕的話原本的設定不能被改掉"
        );
    }

    /// 覆蓋率目標是使用者設定，而且要夾在有意義的範圍內。
    #[tokio::test]
    async fn reading_coverage_is_a_setting_with_sane_bounds() {
        let (db, profile) = setup().await;

        let d = profiles::study_settings(&db, profile).await.unwrap();
        assert_eq!(d.reading_coverage, 0.96, "預設落在最適區間");

        let saved = profiles::update_study_settings(
            &db,
            profile,
            profiles::StudySettings {
                reading_coverage: 0.90,
                ..d
            },
        )
        .await
        .unwrap();
        assert_eq!(saved.reading_coverage, 0.90);
        assert_eq!(
            profiles::study_settings(&db, profile)
                .await
                .unwrap()
                .reading_coverage,
            0.90,
            "要真的存進去"
        );

        // 低於 0.8 是查字典不是閱讀；高於 0.99 等於整篇都會
        for (input, want) in [(0.10, 0.80), (1.00, 0.99)] {
            let s = profiles::update_study_settings(
                &db,
                profile,
                profiles::StudySettings {
                    reading_coverage: input,
                    ..d
                },
            )
            .await
            .unwrap();
            assert_eq!(s.reading_coverage, want);
        }
    }
}
