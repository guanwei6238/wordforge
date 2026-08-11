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

    /// 學習設定。存在 `profile.settings_json`，UI 可以調。
    #[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct StudySettings {
        /// 每天引入幾張新卡
        pub new_per_day: i64,
        /// 每天最多複習幾張
        pub max_reviews_per_day: i64,
        /// FSRS 的目標記憶留存率。調高記得更牢但複習量大增。
        pub desired_retention: f64,
    }

    impl Default for StudySettings {
        fn default() -> Self {
            Self {
                // 每張新卡當天要按兩三次才畢業，15 張大約是 10 分鐘的量
                new_per_day: 15,
                // 長假回來不要被幾百張淹沒
                max_reviews_per_day: 200,
                desired_retention: 0.9,
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
            }
        }
    }

    pub async fn study_settings(db: &Db, profile_id: ProfileId) -> Result<StudySettings> {
        let row: (Option<i64>, Option<i64>, Option<f64>) = sqlx::query_as(
            "SELECT CAST(json_extract(settings_json, '$.new_per_day') AS INTEGER),
                    CAST(json_extract(settings_json, '$.max_reviews_per_day') AS INTEGER),
                    CAST(json_extract(settings_json, '$.desired_retention') AS REAL)
             FROM profile WHERE id = ? AND json_valid(settings_json)",
        )
        .bind(profile_id.0)
        .fetch_optional(db.pool())
        .await?
        .unwrap_or((None, None, None));

        let d = StudySettings::default();
        Ok(StudySettings {
            new_per_day: row.0.unwrap_or(d.new_per_day),
            max_reviews_per_day: row.1.unwrap_or(d.max_reviews_per_day),
            desired_retention: row.2.unwrap_or(d.desired_retention),
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
                     '$.desired_retention', ?)
             WHERE id = ?",
        )
        .bind(s.new_per_day)
        .bind(s.max_reviews_per_day)
        .bind(s.desired_retention)
        .bind(profile_id.0)
        .execute(db.pool())
        .await?;
        Ok(s)
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
    /// 一個表面形可能對應到的**所有** lemma。
    ///
    /// [`find_by_form`] 只回一個 id，而它挑的是「id 最小的那個」——
    /// 也就是匯入順序最早的那個，實際上等於字母序。這對判斷
    /// 「這個字他會不會」是錯的：`ran` 在字典裡自己也是一個詞條，
    /// 而 `ran` < `run`，所以會回 `ran` 而不是 `run`。學習者明明學過
    /// `run`，文章裡的 `ran` 卻被算成生字。`better`（該對到 `good`）、
    /// `studied`（該對到 `study`）都有同樣的問題。
    ///
    /// 挑「正確的那一個」需要真正的詞形還原，而那是有歧義的
    /// （`saw` 可以是 see 的過去式，也可以是「鋸子」）。判斷懂不懂
    /// 不需要解決這個歧義：整個家族有任何一個是他會的，就算他看得懂。
    pub async fn family(db: &Db, lang: &str, form: &str) -> Result<Vec<LemmaId>> {
        let normalized = wordforge_core::text::normalize(form);
        if normalized.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM lemma
             WHERE lang = ? AND normalized = ?
             UNION
             SELECT l.id FROM lemma l
               JOIN surface_form s ON s.lemma_id = l.id
             WHERE s.lang = ? AND s.normalized = ?",
        )
        .bind(lang)
        .bind(&normalized)
        .bind(lang)
        .bind(&normalized)
        .fetch_all(db.pool())
        .await?;

        Ok(ids.into_iter().map(LemmaId).collect())
    }

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

    /// [`add_by_tag`] 的參數。
    #[derive(Debug, Clone)]
    pub struct AddByTag<'a> {
        pub lang: &'a str,
        /// 考試範圍標籤，如 `zk`（國中會考）
        pub tag: &'a str,
        pub kinds: &'a [CardKind],
        /// 最多加入幾個字
        pub limit: i64,
        /// 排除 the / of / and 這類功能詞。除非有特別理由，都該是 `true`。
        pub skip_function_words: bool,
        /// 跳過比這個排名更常用的字。分級測驗的結果會填在這裡，
        /// 學過幾年英文的人不必從第一個字重背。
        pub min_freq_rank: i64,
        /// `limit` 的語意：
        /// - `false`：把這個範圍最常用的 `limit` 個字加進來（已有的會被跳過，
        ///   所以重複執行不會一直長）
        /// - `true`：加入 `limit` 個**還不在牌組裡**的字（補充用）
        pub skip_existing: bool,
    }

    /// 依標籤批次建卡，例如「把國中範圍的字全部加進牌組」。
    ///
    /// 依詞頻由常用到罕見加入——一次加一千個字，先學到的當然該是常用的那些。
    /// 已經在牌組裡的字不會被重置，回傳實際新增的張數。
    ///
    /// `skip_function_words` 預設應該給 `true`：依詞頻排下來，最前面清一色是
    /// `the`、`of`、`and`、`I`，做成單字卡學不到東西（理由見
    /// [`wordforge_core::wordlist`]）。
    pub async fn add_by_tag(
        db: &Db,
        profile_id: ProfileId,
        opts: AddByTag<'_>,
        now: OffsetDateTime,
    ) -> Result<u64> {
        let AddByTag {
            lang,
            tag,
            kinds,
            limit,
            skip_function_words,
            min_freq_rank,
            skip_existing,
        } = opts;
        // 標籤在資料庫裡存成 " zk gk "，前後補空白比對才不會讓 zk 誤中 zkk
        let pattern = format!("% {} %", tag.trim());
        let due = ts::to_sql(now);

        // 功能詞清單是編譯期常數，不是使用者輸入，直接內嵌成 SQL 字面值。
        // 用 bind 的話得動態組出上百個 `?`，反而更難讀。
        let exclusion = if skip_function_words {
            let list = wordforge_core::wordlist::function_words(lang);
            if list.is_empty() {
                String::new()
            } else {
                let quoted: Vec<String> = list.iter().map(|w| format!("'{w}'")).collect();
                format!("AND normalized NOT IN ({})", quoted.join(","))
            }
        } else {
            String::new()
        };

        let mut added = 0u64;
        let mut tx = db.pool().begin().await?;
        for kind in kinds {
            // 補充模式：先濾掉已經在牌組裡的字，LIMIT 才等於「真正新增幾個」
            let not_in_deck = if skip_existing {
                "AND NOT EXISTS (SELECT 1 FROM card c
                                 WHERE c.lemma_id = lemma.id AND c.profile_id = ?3
                                   AND c.kind = ?4)"
            } else {
                ""
            };
            // 包一層子查詢有兩個理由：ORDER BY + LIMIT 要作用在挑選而不是插入，
            // 以及 SQLite 的 INSERT...SELECT 接 ON CONFLICT 需要語法上不含糊。
            // 用編號參數而不是裸 `?`：`not_in_deck` 片段會插在中間，
            // 裸問號的順序會跟著條件有沒有出現而改變。
            let res = sqlx::query(&format!(
                "INSERT INTO card (profile_id, lemma_id, kind, state, due)
                 SELECT ?3, pick.id, ?4, 'new', ?5
                 FROM (
                     SELECT id FROM lemma
                     WHERE lang = ?1 AND ' ' || tags || ' ' LIKE ?2 {exclusion}
                       AND (freq_rank IS NULL OR freq_rank >= ?6)
                       {not_in_deck}
                     ORDER BY freq_rank IS NULL, freq_rank, id
                     LIMIT ?7
                 ) AS pick
                 WHERE true
                 ON CONFLICT (profile_id, lemma_id, kind) DO NOTHING"
            ))
            .bind(lang)
            .bind(&pattern)
            .bind(profile_id.0)
            .bind(kind.as_str())
            .bind(&due)
            .bind(min_freq_rank)
            .bind(limit)
            .execute(&mut *tx)
            .await?;
            added += res.rows_affected();
        }
        tx.commit().await?;

        Ok(added)
    }

    /// 把「其實早就會」的新卡收起來。
    ///
    /// 分級測驗說使用者大概掌握了前 N 個常用字，但牌組裡可能已經排了一堆
    /// 比 N 更常用的字。這些卡直接**暫停**而不是刪除——判斷可能不準，
    /// 使用者之後想學隨時可以恢復，複習歷程也不會消失。
    ///
    /// 只動從未複習過的卡，任何已經開始學的進度都保留。
    pub async fn suspend_easy_new_cards(
        db: &Db,
        profile_id: ProfileId,
        lang: &str,
        below_rank: i64,
    ) -> Result<u64> {
        let res = sqlx::query(
            "UPDATE card SET suspended = 1
             WHERE profile_id = ? AND suspended = 0 AND state = 'new' AND reps = 0
               AND lemma_id IN (
                   SELECT id FROM lemma
                   WHERE lang = ? AND freq_rank IS NOT NULL AND freq_rank < ?
               )",
        )
        .bind(profile_id.0)
        .bind(lang)
        .bind(below_rank)
        .execute(db.pool())
        .await?;
        Ok(res.rows_affected())
    }

    /// 牌組裡有幾張卡屬於別的語言。
    ///
    /// 換目標語言時要拿這個數字警告使用者：舊卡不會自己消失，
    /// 不講的話他明天打開 App 會看到一堆上一個語言的字混在複習裡。
    pub async fn count_other_languages(db: &Db, profile_id: ProfileId, lang: &str) -> Result<i64> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM card c JOIN lemma l ON l.id = c.lemma_id
             WHERE c.profile_id = ? AND c.suspended = 0 AND l.lang <> ?",
        )
        .bind(profile_id.0)
        .bind(lang)
        .fetch_one(db.pool())
        .await?;
        Ok(n)
    }

    /// 把別的語言的卡片收起來。
    ///
    /// 用 suspend 而不是刪除：使用者可能只是暫時換去學日文，
    /// 半年後回來時那些英文卡的複習歷史還在，不必從頭學。
    pub async fn suspend_other_languages(
        db: &Db,
        profile_id: ProfileId,
        lang: &str,
    ) -> Result<u64> {
        let res = sqlx::query(
            "UPDATE card SET suspended = 1
             WHERE profile_id = ? AND suspended = 0
               AND lemma_id IN (SELECT id FROM lemma WHERE lang <> ?)",
        )
        .bind(profile_id.0)
        .bind(lang)
        .execute(db.pool())
        .await?;
        Ok(res.rows_affected())
    }

    /// 今天還可以引入幾張新卡。
    ///
    /// 「今天引入的新卡」定義為 `review_log` 裡 `state = 'new'` 的紀錄——
    /// 那是一張卡的第一次複習，之後再怎麼重複都不會再算一次。
    pub async fn new_cards_introduced_today(
        db: &Db,
        profile_id: ProfileId,
        day_start: OffsetDateTime,
    ) -> Result<i64> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT r.card_id)
             FROM review_log r JOIN card c ON c.id = r.card_id
             WHERE c.profile_id = ? AND r.state = 'new' AND r.reviewed_at >= ?",
        )
        .bind(profile_id.0)
        .bind(ts::to_sql(day_start))
        .fetch_one(db.pool())
        .await?;
        Ok(n)
    }

    /// 今天該做的卡片佇列。
    ///
    /// 這跟單純的「due <= now」不一樣，差別就是這個 App 能不能天天用下去：
    ///
    /// 1. **學習中的卡最優先**：今天剛看過、幾分鐘後要再看一次的卡不能被排到後面，
    ///    否則當天根本記不起來。
    /// 2. **接著是到期的複習卡**：這些是已經投資過的記憶，錯過就要重學。
    /// 3. **最後才引入新卡，而且有每日上限**：一次把 1600 個字全設成到期，
    ///    開啟 App 看到「待複習 1600」只會讓人直接關掉。FSRS 的排程本來就
    ///    假設新卡是每天少量穩定引入的。
    pub async fn daily_queue(
        db: &Db,
        profile_id: ProfileId,
        now: OffsetDateTime,
        day_start: OffsetDateTime,
        new_per_day: i64,
        max_reviews: i64,
    ) -> Result<Vec<Card>> {
        let now_sql = ts::to_sql(now);

        // 學習中 + 到期複習，先來後到
        let mut queue: Vec<Card> = sqlx::query(&format!(
            "{SELECT_CARD} WHERE profile_id = ? AND suspended = 0
               AND state <> 'new' AND due <= ?
             ORDER BY due ASC LIMIT ?"
        ))
        .bind(profile_id.0)
        .bind(&now_sql)
        .bind(max_reviews.max(0))
        .fetch_all(db.pool())
        .await?
        .iter()
        .map(row_to_card)
        .collect::<Result<_>>()?;

        let introduced = new_cards_introduced_today(db, profile_id, day_start).await?;
        let remaining = (new_per_day - introduced).max(0);
        if remaining == 0 {
            return Ok(queue);
        }

        // 新卡依詞頻由常用到罕見引入，跟加入牌組時的順序一致
        let new_cards: Vec<Card> = sqlx::query(&format!(
            "{SELECT_CARD} AS c WHERE profile_id = ? AND suspended = 0
               AND state = 'new' AND due <= ?
             ORDER BY (SELECT freq_rank IS NULL FROM lemma WHERE id = c.lemma_id),
                      (SELECT freq_rank FROM lemma WHERE id = c.lemma_id),
                      c.id
             LIMIT ?"
        ))
        .bind(profile_id.0)
        .bind(&now_sql)
        .bind(remaining)
        .fetch_all(db.pool())
        .await?
        .iter()
        .map(row_to_card)
        .collect::<Result<_>>()?;

        queue.extend(new_cards);
        Ok(queue)
    }

    /// 佇列的完整狀態。
    ///
    /// 「沒有卡片可做」有好幾種原因，UI 必須分得出來，否則使用者只會看到
    /// 「今天的份做完了」而不知道其實整個牌組被收起來了。
    #[derive(Debug, Clone, PartialEq)]
    pub struct QueueStatus {
        /// 現在到期的複習卡
        pub due_reviews: i64,
        /// 今天還能引入幾張新卡（受每日上限限制）
        pub new_today: i64,
        /// 牌組裡還有幾張沒學過的新卡（不受每日上限限制，不含被收起的）
        pub new_in_deck: i64,
        /// 被收起來的卡（多半來自分級測驗判定「太簡單」）
        pub suspended: i64,
        /// 之後還有卡片要複習時，最近的一張是什麼時候
        pub next_due: Option<OffsetDateTime>,
    }

    pub async fn queue_status(
        db: &Db,
        profile_id: ProfileId,
        now: OffsetDateTime,
        day_start: OffsetDateTime,
        new_per_day: i64,
    ) -> Result<QueueStatus> {
        let now_sql = ts::to_sql(now);

        let due_reviews: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM card
             WHERE profile_id = ? AND suspended = 0 AND state <> 'new' AND due <= ?",
        )
        .bind(profile_id.0)
        .bind(&now_sql)
        .fetch_one(db.pool())
        .await?;

        let new_in_deck: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM card
             WHERE profile_id = ? AND suspended = 0 AND state = 'new'",
        )
        .bind(profile_id.0)
        .fetch_one(db.pool())
        .await?;

        let suspended: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE profile_id = ? AND suspended = 1")
                .bind(profile_id.0)
                .fetch_one(db.pool())
                .await?;

        let introduced = new_cards_introduced_today(db, profile_id, day_start).await?;
        let new_today = (new_per_day - introduced).max(0).min(new_in_deck);

        let next_due: Option<String> = sqlx::query_scalar(
            "SELECT MIN(due) FROM card
             WHERE profile_id = ? AND suspended = 0 AND state <> 'new' AND due > ?",
        )
        .bind(profile_id.0)
        .bind(&now_sql)
        .fetch_one(db.pool())
        .await?;

        Ok(QueueStatus {
            due_reviews,
            new_today,
            new_in_deck,
            suspended,
            next_due: next_due.map(|s| s.parse_ts("card.due")).transpose()?,
        })
    }

    /// 自動補充設定：牌組見底時要從哪個範圍補、補到剩幾張。
    #[derive(Debug, Clone, PartialEq)]
    pub struct AutoRefill<'a> {
        /// 從哪個範圍補（`cet4`、`gk`…）
        pub tag: &'a str,
        /// 牌組裡未學的新卡少於這個數量時就補到這個數量
        pub keep_ahead: i64,
        /// 跳過比這更常用的字（分級測驗的結果）
        pub min_freq_rank: i64,
    }

    /// 需要的話補充牌組，回傳實際加入的張數。
    ///
    /// 「學完了就自己接上新的」是這個 App 該有的行為：使用者的目標是學語言，
    /// 不是管理牌組。每次取佇列前檢查一次，成本只有一個 COUNT。
    ///
    /// 補充一樣依詞頻由常用到罕見，也一樣跳過功能詞。
    pub async fn refill_if_needed(
        db: &Db,
        profile_id: ProfileId,
        lang: &str,
        cfg: &AutoRefill<'_>,
        now: OffsetDateTime,
    ) -> Result<u64> {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM card
             WHERE profile_id = ? AND suspended = 0 AND state = 'new'",
        )
        .bind(profile_id.0)
        .fetch_one(db.pool())
        .await?;

        if waiting >= cfg.keep_ahead {
            return Ok(0);
        }

        add_by_tag(
            db,
            profile_id,
            AddByTag {
                lang,
                tag: cfg.tag,
                kinds: &[CardKind::Recognition],
                // 只補差額，而且是「還不在牌組裡的」那些
                limit: cfg.keep_ahead - waiting,
                skip_function_words: true,
                min_freq_rank: cfg.min_freq_rank,
                skip_existing: true,
            },
            now,
        )
        .await
    }

    /// 恢復被收起來的卡，最常用的字優先。
    ///
    /// 分級測驗的判斷可能不準，或者使用者就是想把那些字也複習一遍。
    /// 卡片當初只是暫停沒有刪除，所以恢復後進度完好。
    pub async fn unsuspend(db: &Db, profile_id: ProfileId, count: i64) -> Result<u64> {
        let res = sqlx::query(
            "UPDATE card SET suspended = 0
             WHERE id IN (
                 SELECT c.id FROM card c
                   JOIN lemma l ON l.id = c.lemma_id
                 WHERE c.profile_id = ? AND c.suspended = 1
                 ORDER BY l.freq_rank IS NULL, l.freq_rank, c.id
                 LIMIT ?
             )",
        )
        .bind(profile_id.0)
        .bind(count)
        .execute(db.pool())
        .await?;
        Ok(res.rows_affected())
    }

    /// 今天的待辦數量：到期複習幾張、還能引入幾張新卡。
    pub async fn daily_counts(
        db: &Db,
        profile_id: ProfileId,
        now: OffsetDateTime,
        day_start: OffsetDateTime,
        new_per_day: i64,
    ) -> Result<(i64, i64)> {
        let reviews: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM card
             WHERE profile_id = ? AND suspended = 0 AND state <> 'new' AND due <= ?",
        )
        .bind(profile_id.0)
        .bind(ts::to_sql(now))
        .fetch_one(db.pool())
        .await?;

        let waiting: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM card
             WHERE profile_id = ? AND suspended = 0 AND state = 'new'",
        )
        .bind(profile_id.0)
        .fetch_one(db.pool())
        .await?;

        let introduced = new_cards_introduced_today(db, profile_id, day_start).await?;
        let new_today = (new_per_day - introduced).max(0).min(waiting);

        Ok((reviews, new_today))
    }

    /// 每個標籤有幾個字、其中幾個已經在牌組裡。
    ///
    /// `min_freq_rank` 是分級測驗給的起點：比這更常用的字不列入計算，
    /// 否則牌組頁顯示「國中 1603 字」但實際只能加 870 個，數字對不起來。
    pub async fn tag_summary(
        db: &Db,
        profile_id: ProfileId,
        lang: &str,
        min_freq_rank: i64,
    ) -> Result<Vec<(String, i64, i64)>> {
        // 標籤是空白分隔的字串，SQLite 沒有 split，所以在 Rust 端展開。
        // 標籤種類只有十幾種，詞條數才是大的那一邊，撈回來的資料量不大。
        let rows = sqlx::query(
            "SELECT l.tags,
                    COUNT(*) AS total,
                    SUM(EXISTS (SELECT 1 FROM card c
                                WHERE c.lemma_id = l.id AND c.profile_id = ?)) AS in_deck
             FROM lemma l
             WHERE l.lang = ? AND l.tags <> ''
               AND (l.freq_rank IS NULL OR l.freq_rank >= ?)
             GROUP BY l.tags",
        )
        .bind(profile_id.0)
        .bind(lang)
        .bind(min_freq_rank)
        .fetch_all(db.pool())
        .await?;

        let mut totals: std::collections::BTreeMap<String, (i64, i64)> = Default::default();
        for row in rows {
            let tags: String = row.get("tags");
            let total: i64 = row.get("total");
            let in_deck: i64 = row.get::<Option<i64>, _>("in_deck").unwrap_or(0);
            for tag in tags.split_whitespace() {
                let e = totals.entry(tag.to_string()).or_insert((0, 0));
                e.0 += total;
                e.1 += in_deck;
            }
        }

        let mut out: Vec<(String, i64, i64)> = totals
            .into_iter()
            .map(|(tag, (total, in_deck))| (tag, total, in_deck))
            .collect();
        // 字多的標籤排前面，那通常是使用者真的會用的範圍
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Ok(out)
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

    /// 「把國中範圍的字加進牌組」是這個 App 最直接的用法之一。
    #[tokio::test]
    async fn add_by_tag_picks_frequent_words_first() {
        let (db, profile) = setup().await;

        // 三個國中字（詞頻不同）與一個高中字
        for (word, freq, tags) in [
            ("rare", 9000, " zk "),
            ("common", 100, " zk "),
            ("middle", 3000, " zk gk "),
            ("advanced", 50, " gk "),
        ] {
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ?)",
            )
            .bind(word)
            .bind(word)
            .bind(freq)
            .bind(tags)
            .execute(db.pool())
            .await
            .unwrap();
        }

        let added = cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 2,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();
        assert_eq!(added, 2);

        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id
             ORDER BY l.freq_rank",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            words,
            vec!["common", "middle"],
            "應該先加常用的字，而且高中字不該混進來"
        );
    }

    /// 重複執行不該把已經在複習的卡片打回新卡。
    #[tokio::test]
    async fn add_by_tag_never_resets_existing_cards() {
        let (db, profile) = setup().await;
        sqlx::query(
            "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
             VALUES ('en', 'apple', 'apple', '', 100, ' zk ')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE card SET state = 'review', stability = 30.0, reps = 5")
            .execute(db.pool())
            .await
            .unwrap();

        let added = cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        assert_eq!(added, 0, "已經在牌組裡的字不該重複加入");
        let (state, reps): (String, i64) = sqlx::query_as("SELECT state, reps FROM card")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(state, "review", "複習進度被重置了");
        assert_eq!(reps, 5);
    }

    /// 依詞頻加入時，最前面清一色是 the / of / and，
    /// 把它們做成單字卡是浪費使用者的時間。
    #[tokio::test]
    async fn add_by_tag_skips_function_words() {
        let (db, profile) = setup().await;
        for (word, freq) in [("the", 1), ("of", 2), ("water", 3), ("i", 4), ("book", 5)] {
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ' zk ')",
            )
            .bind(word)
            .bind(word)
            .bind(freq)
            .execute(db.pool())
            .await
            .unwrap();
        }

        cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: true,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id ORDER BY l.freq_rank",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(words, vec!["water", "book"], "功能詞不該進牌組");
    }

    /// 分級測驗說「你大概會前 2000 個字」，就不該再從第一個字開始排。
    #[tokio::test]
    async fn add_by_tag_can_skip_words_the_learner_already_knows() {
        let (db, profile) = setup().await;
        for (word, freq) in [("easy", 100), ("medium", 2500), ("hard", 9000)] {
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ' zk ')",
            )
            .bind(word)
            .bind(word)
            .bind(freq)
            .execute(db.pool())
            .await
            .unwrap();
        }

        cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 2_000,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id ORDER BY l.freq_rank",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(words, vec!["medium", "hard"], "太簡單的字不該再排進來");
    }

    /// 已經在牌組裡但其實早就會的新卡，應該能一次收起來。
    #[tokio::test]
    async fn easy_new_cards_can_be_suspended_in_bulk() {
        let (db, profile) = setup().await;
        for (word, freq) in [("easy", 100), ("hard", 9000)] {
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ' zk ')",
            )
            .bind(word)
            .bind(word)
            .bind(freq)
            .execute(db.pool())
            .await
            .unwrap();
        }
        cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        // 先讓 easy 有複習紀錄，確認有進度的卡不會被動到
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 10, 100)
            .await
            .unwrap();
        let easy = queue.iter().find(|c| c.lemma_id.0 == 1).unwrap();
        let (next, log) = Scheduler::default().review(easy, Rating::Good, t0(), None);
        cards::record_review(&db, &next, &log).await.unwrap();

        let suspended = cards::suspend_easy_new_cards(&db, profile, "en", 2_000)
            .await
            .unwrap();
        assert_eq!(suspended, 0, "已經開始學的卡不該被收起來");

        // 換一個乾淨的情境：沒複習過的簡單卡
        let (db2, profile2) = setup().await;
        sqlx::query(
            "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
             VALUES ('en', 'easy', 'easy', '', 100, ' zk ')",
        )
        .execute(db2.pool())
        .await
        .unwrap();
        cards::add_by_tag(
            &db2,
            profile2,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        let suspended = cards::suspend_easy_new_cards(&db2, profile2, "en", 2_000)
            .await
            .unwrap();
        assert_eq!(suspended, 1);
        let queue = cards::daily_queue(&db2, profile2, t0(), t0(), 10, 100)
            .await
            .unwrap();
        assert!(queue.is_empty(), "收起來的卡不該再出現在佇列裡");
    }

    /// 標籤比對必須精確：zk 不能命中 zkk。
    #[tokio::test]
    async fn tag_matching_is_exact() {
        let (db, profile) = setup().await;
        sqlx::query(
            "INSERT INTO lemma (lang, text, normalized, pos, tags)
             VALUES ('en', 'trap', 'trap', '', ' zkk ')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let added = cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();
        assert_eq!(added, 0);
    }

    #[tokio::test]
    async fn tag_summary_counts_words_and_deck_progress() {
        let (db, profile) = setup().await;
        for (word, tags) in [("a1", " zk gk "), ("a2", " zk "), ("a3", " gk ")] {
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', 1, ?)",
            )
            .bind(word)
            .bind(word)
            .bind(tags)
            .execute(db.pool())
            .await
            .unwrap();
        }
        cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 1,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        let summary = cards::tag_summary(&db, profile, "en", 0).await.unwrap();
        let zk = summary.iter().find(|(t, ..)| t == "zk").unwrap();
        let gk = summary.iter().find(|(t, ..)| t == "gk").unwrap();

        assert_eq!(zk.1, 2, "zk 有兩個字");
        assert_eq!(zk.2, 1, "其中一個已加入牌組");
        assert_eq!(gk.1, 2);
        assert_eq!(gk.2, 1, "同一個字同時屬於 zk 與 gk，兩邊都要算到");
    }

    /// 建立 n 張新卡，詞頻由 1 開始遞增。
    async fn seed_new_cards(db: &Db, profile: ProfileId, n: i64) {
        for i in 1..=n {
            let word = format!("w{i:04}");
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ' zk ')",
            )
            .bind(&word)
            .bind(&word)
            .bind(i)
            .execute(db.pool())
            .await
            .unwrap();
        }
        cards::add_by_tag(
            db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: n,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();
    }

    /// 一次加一千個字，第一天不該全部湧出來。
    #[tokio::test]
    async fn daily_queue_caps_new_cards() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 50).await;

        let queue = cards::daily_queue(&db, profile, t0(), t0(), 15, 200)
            .await
            .unwrap();

        assert_eq!(queue.len(), 15, "每日新卡上限沒生效");
        // 依詞頻由常用到罕見引入
        let ids: Vec<i64> = queue.iter().map(|c| c.lemma_id.0).collect();
        assert_eq!(ids, (1..=15).collect::<Vec<_>>());
    }

    /// 今天已經引入過的新卡要計入額度，否則關掉再開就能無限刷新。
    #[tokio::test]
    async fn introduced_new_cards_count_against_the_daily_limit() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 50).await;
        let scheduler = Scheduler::default();

        // 學掉 5 張
        let first = cards::daily_queue(&db, profile, t0(), t0(), 15, 200)
            .await
            .unwrap();
        for card in first.iter().take(5) {
            let (next, log) = scheduler.review(card, Rating::Good, t0(), None);
            cards::record_review(&db, &next, &log).await.unwrap();
        }

        assert_eq!(
            cards::new_cards_introduced_today(&db, profile, t0())
                .await
                .unwrap(),
            5
        );

        // 稍後再開，只剩 10 張新卡額度
        let later = t0() + Duration::minutes(30);
        let queue = cards::daily_queue(&db, profile, later, t0(), 15, 200)
            .await
            .unwrap();
        let new_count = queue.iter().filter(|c| c.state == CardState::New).count();
        assert_eq!(new_count, 10, "已引入的新卡沒有計入額度");

        // 隔天額度重置
        let tomorrow = t0() + Duration::days(1);
        let queue = cards::daily_queue(&db, profile, tomorrow, tomorrow, 15, 200)
            .await
            .unwrap();
        let new_count = queue.iter().filter(|c| c.state == CardState::New).count();
        assert_eq!(new_count, 15, "隔天額度應該重置");
    }

    /// 幾分鐘後要再看一次的卡，必須排在新卡前面。
    #[tokio::test]
    async fn learning_cards_come_before_new_ones() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 10).await;
        let scheduler = Scheduler::default();

        // 第一張按 Again，10 分鐘內要再出現
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 5, 200)
            .await
            .unwrap();
        let (next, log) = scheduler.review(&queue[0], Rating::Again, t0(), None);
        cards::record_review(&db, &next, &log).await.unwrap();

        let later = t0() + Duration::minutes(5);
        let queue = cards::daily_queue(&db, profile, later, t0(), 5, 200)
            .await
            .unwrap();

        assert_eq!(queue[0].state, CardState::Learning, "學習中的卡要排最前面");
        assert_eq!(queue[0].lemma_id, next.lemma_id);
    }

    #[tokio::test]
    async fn daily_counts_report_what_is_left_today() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 30).await;

        let (reviews, new_today) = cards::daily_counts(&db, profile, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(reviews, 0, "還沒有任何卡進入複習階段");
        assert_eq!(new_today, 15);

        // 牌組裡只剩 3 張新卡時，不該顯示 15
        let (db2, profile2) = setup().await;
        seed_new_cards(&db2, profile2, 3).await;
        let (_, new_today) = cards::daily_counts(&db2, profile2, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(new_today, 3);
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
            },
        )
        .await
        .unwrap();

        assert_eq!(s.new_per_day, 0, "0 是合法的（今天先不學新字）");
        assert_eq!(s.max_reviews_per_day, 10);
        assert!((s.desired_retention - 0.97).abs() < 1e-9);

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

    /// 佇列空掉的原因必須分得出來。
    ///
    /// 實際踩過：分級測驗把整個牌組收起來之後，UI 只說「今天的份做完了」，
    /// 使用者以為是自己學完了，其實是 296 張卡全被暫停。
    #[tokio::test]
    async fn queue_status_tells_apart_the_reasons_for_an_empty_queue() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 30).await;

        // 一、什麼都還沒做：有新卡可學
        let s = cards::queue_status(&db, profile, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(s.new_today, 15);
        assert_eq!(s.new_in_deck, 30);
        assert_eq!(s.suspended, 0);
        assert_eq!(s.next_due, None);

        // 二、今天的額度用完，但牌組裡還有字 → 這才是「做完了」
        let scheduler = Scheduler::default();
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 15, 200)
            .await
            .unwrap();
        for card in &queue {
            let (next, log) = scheduler.review(card, Rating::Easy, t0(), None);
            cards::record_review(&db, &next, &log).await.unwrap();
        }
        let s = cards::queue_status(&db, profile, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(s.new_today, 0, "今天的額度用完了");
        assert_eq!(s.new_in_deck, 15, "但牌組裡還有 15 個字在排隊");
        assert!(s.next_due.is_some(), "已學的卡有下次到期時間");

        // 三、剩下的新卡全被收起來 → 不是「做完了」，是沒東西可做
        cards::suspend_easy_new_cards(&db, profile, "en", 100_000)
            .await
            .unwrap();
        let s = cards::queue_status(&db, profile, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(s.new_in_deck, 0);
        assert_eq!(s.suspended, 15);
    }

    /// 學完了就該自己接上新的，不必使用者手動去牌組頁補。
    #[tokio::test]
    async fn refill_tops_up_the_deck_when_it_runs_low() {
        let (db, profile) = setup().await;
        // 字典裡有 100 個 cet4 的字
        for i in 1..=100 {
            let word = format!("w{i:04}");
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ' cet4 ')",
            )
            .bind(&word)
            .bind(&word)
            .bind(i)
            .execute(db.pool())
            .await
            .unwrap();
        }

        let cfg = cards::AutoRefill {
            tag: "cet4",
            keep_ahead: 20,
            min_freq_rank: 0,
        };

        // 牌組是空的 → 補到 20 張
        let added = cards::refill_if_needed(&db, profile, "en", &cfg, t0())
            .await
            .unwrap();
        assert_eq!(added, 20);

        // 還很滿 → 不動作
        let added = cards::refill_if_needed(&db, profile, "en", &cfg, t0())
            .await
            .unwrap();
        assert_eq!(added, 0, "牌組還夠的時候不該一直加");

        // 學掉 15 張之後剩 5 張 → 再補回 20
        let scheduler = Scheduler::default();
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 15, 100)
            .await
            .unwrap();
        for card in &queue {
            let (next, log) = scheduler.review(card, Rating::Easy, t0(), None);
            cards::record_review(&db, &next, &log).await.unwrap();
        }

        let added = cards::refill_if_needed(&db, profile, "en", &cfg, t0())
            .await
            .unwrap();
        assert_eq!(added, 15, "補回被學掉的那些");

        let status = cards::queue_status(&db, profile, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(status.new_in_deck, 20);
    }

    /// 補充也要尊重分級測驗的結果，別把已經會的字又塞回來。
    #[tokio::test]
    async fn refill_respects_the_placement_result() {
        let (db, profile) = setup().await;
        for i in 1..=50 {
            let word = format!("w{i:04}");
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ' cet4 ')",
            )
            .bind(&word)
            .bind(&word)
            .bind(i)
            .execute(db.pool())
            .await
            .unwrap();
        }

        cards::refill_if_needed(
            &db,
            profile,
            "en",
            &cards::AutoRefill {
                tag: "cet4",
                keep_ahead: 10,
                min_freq_rank: 30,
            },
            t0(),
        )
        .await
        .unwrap();

        let min_rank: i64 = sqlx::query_scalar(
            "SELECT MIN(l.freq_rank) FROM card c JOIN lemma l ON l.id = c.lemma_id",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(min_rank, 30, "不該補進比起始詞頻更常用的字");
    }

    /// 被收起來的卡要能恢復，而且從最常用的開始。
    #[tokio::test]
    async fn unsuspend_brings_back_the_most_useful_words_first() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 20).await;
        cards::suspend_easy_new_cards(&db, profile, "en", 100_000)
            .await
            .unwrap();
        assert_eq!(
            cards::queue_status(&db, profile, t0(), t0(), 15)
                .await
                .unwrap()
                .suspended,
            20
        );

        let restored = cards::unsuspend(&db, profile, 5).await.unwrap();
        assert_eq!(restored, 5);

        // 恢復的應該是詞頻 1~5（seed 依序給 freq_rank 1..n）
        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id
             WHERE c.suspended = 0 ORDER BY l.freq_rank",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(words, vec!["w0001", "w0002", "w0003", "w0004", "w0005"]);

        // 恢復後就能正常排進佇列
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 15, 200)
            .await
            .unwrap();
        assert_eq!(queue.len(), 5);
    }

    /// 超出每日上限繼續學：今天已引入的數量要算進去，不能重頭來過。
    #[tokio::test]
    async fn studying_more_extends_todays_quota() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 50).await;
        let scheduler = Scheduler::default();

        let first = cards::daily_queue(&db, profile, t0(), t0(), 15, 200)
            .await
            .unwrap();
        for card in &first {
            let (next, log) = scheduler.review(card, Rating::Easy, t0(), None);
            cards::record_review(&db, &next, &log).await.unwrap();
        }

        let introduced = cards::new_cards_introduced_today(&db, profile, t0())
            .await
            .unwrap();
        assert_eq!(introduced, 15);

        // 「再學 10 個」＝ 上限提高到 15 + 10
        let more = cards::daily_queue(&db, profile, t0(), t0(), introduced + 10, 200)
            .await
            .unwrap();
        let new_ones = more.iter().filter(|c| c.state == CardState::New).count();
        assert_eq!(new_ones, 10, "應該剛好多給 10 張，不是重新給 25 張");
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

    /// 設定頁的目標語言選單就是這份清單。
    #[tokio::test]
    async fn dictionary_languages_are_listed_by_size() {
        let (db, _) = setup().await;
        add_word(&db, "apple", 1).await;
        add_word(&db, "banana", 2).await;
        lemmas::upsert(
            &db,
            NewLemma {
                lang: "ja",
                text: "林檎",
                pos: "noun",
                freq_rank: Some(1),
                cefr: None,
            },
        )
        .await
        .unwrap();

        let langs = crate::dict::languages(&db).await.unwrap();
        assert_eq!(
            langs,
            vec![("en".to_string(), 2), ("ja".to_string(), 1)],
            "詞條多的排前面，使用者最可能要的排第一個"
        );
    }

    /// 學過 run 的人看到 ran 是懂的——90% 法則靠這件事成立。
    ///
    /// 這條測試存在的理由是它曾經是錯的：`find_by_form` 挑「id 最小的」，
    /// 而 `ran` 自己在字典裡也是一個詞條，且 `ran` < `run`，
    /// 所以查 `ran` 會回到 `ran` 而不是 `run`，學過的字被算成生字。
    #[tokio::test]
    async fn an_inflection_resolves_to_the_whole_family() {
        let (db, _) = setup().await;
        let run = add_word(&db, "run", 100).await;
        // 變化形自己也是詞條，而且拼字排在原形前面——這正是當初踩到的情況
        let ran_entry = add_word(&db, "ran", 900).await;
        lemmas::add_surface_form(&db, "en", "ran", run, "past")
            .await
            .unwrap();

        let family = lemmas::family(&db, "en", "ran").await.unwrap();
        assert!(family.contains(&run), "沒有把 ran 對回 run：{family:?}");
        assert!(family.contains(&ran_entry), "ran 自己那個詞條也該在家族裡");

        // 反過來：原形查得到自己
        assert!(
            lemmas::family(&db, "en", "run")
                .await
                .unwrap()
                .contains(&run)
        );
    }

    /// 大小寫與標點不能影響詞形比對。
    #[tokio::test]
    async fn family_lookup_normalizes_the_form() {
        let (db, _) = setup().await;
        let study = add_word(&db, "study", 100).await;
        lemmas::add_surface_form(&db, "en", "studied", study, "past")
            .await
            .unwrap();

        assert!(
            lemmas::family(&db, "en", "Studied,")
                .await
                .unwrap()
                .contains(&study),
            "文章裡的字會帶大寫與標點"
        );
        assert!(lemmas::family(&db, "en", "  ").await.unwrap().is_empty());
    }

    /// 別的語言的同拼字不能混進來。
    #[tokio::test]
    async fn family_lookup_stays_within_one_language() {
        let (db, _) = setup().await;
        let english = add_word(&db, "die", 100).await;
        let german = lemmas::upsert(
            &db,
            NewLemma {
                lang: "de",
                text: "die",
                pos: "article",
                freq_rank: Some(1),
                cefr: None,
            },
        )
        .await
        .unwrap();

        let family = lemmas::family(&db, "en", "die").await.unwrap();
        assert!(family.contains(&english));
        assert!(!family.contains(&german), "德文的 die 不該算進英文");
    }
}
