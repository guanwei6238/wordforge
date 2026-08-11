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
            // 包一層子查詢有兩個理由：ORDER BY + LIMIT 要作用在挑選而不是插入，
            // 以及 SQLite 的 INSERT...SELECT 接 ON CONFLICT 需要語法上不含糊。
            let res = sqlx::query(&format!(
                "INSERT INTO card (profile_id, lemma_id, kind, state, due)
                 SELECT ?, pick.id, ?, 'new', ?
                 FROM (
                     SELECT id FROM lemma
                     WHERE lang = ? AND ' ' || tags || ' ' LIKE ? {exclusion}
                       AND (freq_rank IS NULL OR freq_rank >= ?)
                     ORDER BY freq_rank IS NULL, freq_rank, id
                     LIMIT ?
                 ) AS pick
                 WHERE true
                 ON CONFLICT (profile_id, lemma_id, kind) DO NOTHING"
            ))
            .bind(profile_id.0)
            .bind(kind.as_str())
            .bind(&due)
            .bind(lang)
            .bind(&pattern)
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
}
