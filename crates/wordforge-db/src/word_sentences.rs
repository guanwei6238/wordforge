//! 做過的句子，以及「這個字我在哪句話裡用過」。
//!
//! 複習單字時看得到的是字典的釋義與別人寫的例句。真正記得住的是**自己
//! 做過的那一句**——翻譯題裡寫過的、閱讀文章裡讀到的。這個模組把單字接回
//! 那些句子，複習頁與字典頁共用同一份資料。
//!
//! ## 兩張表
//!
//! 句子存一份（`sentence`），「哪個字出現在哪一句」是一張倒排索引
//! （`sentence_lemma`）。所以寫入是兩步：[`record`] 記下句子拿到 id，
//! [`index`] 把它連到那句話裡出現的每個詞條。
//!
//! 索引存的是 lemma 而不是字串：查 `ran` 要看得到練 `run` 時寫的句子。
//! 索引也**不限使用者的牌組**——今天才學的字，回頭要看得到三個月前
//! 做過、明明用到它的句子。

use serde::Serialize;
use sqlx::Row;
use time::OffsetDateTime;
use wordforge_core::model::{LemmaId, ProfileId};

use crate::{Db, Result, ts};

/// 一句做過的句子。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WordSentence {
    pub id: i64,
    pub exercise_id: i64,
    /// 目標語言那一句
    pub text: String,
    /// 母語翻譯。閱讀文章對不齊時可能是整段，也可能沒有。
    pub translation: Option<String>,
    /// translation / reading / cloze
    pub origin: String,
    /// 這一句踩過哪些文法點（識別碼，如 `articles`）。
    ///
    /// 名稱在 `grammar_def` 裡查，不存副本——那份清單使用者可以改。
    pub grammar_points: Vec<String>,
    /// 這一句錯過幾次。
    ///
    /// 累計在這裡而不是讀 `sentence_review`：那張表是排程，句子寫對之後
    /// 整列會被刪掉，而「錯過三次才寫對」正是練起來之後最值得留著的訊號。
    pub misses: i64,
    pub created_at: String,
}

/// 要寫入的一句。
#[derive(Debug, Clone)]
pub struct NewSentence<'a> {
    pub profile_id: ProfileId,
    pub exercise_id: i64,
    pub text: &'a str,
    pub translation: Option<&'a str>,
    pub origin: &'a str,
    /// 這一句是那份練習的第幾題。翻譯題才有——閱讀與克漏字的句子
    /// 不是「一題」，對不回排程與批改結果。
    pub item_index: Option<i64>,
}

/// 記一句，回傳它的 id。同一份練習裡同一句只留一份。
///
/// 空句子直接忽略：出題偶爾會少一個欄位，而一個空白的「你寫過的句子」
/// 在畫面上看起來像壞掉。
///
/// 重跑（補寫舊資料、重算對齊）走 `ON CONFLICT`：譯文與題號會被更新，
/// 而 `misses` 與文法點留著——那是使用者練出來的紀錄，重算補不回來。
pub async fn record(db: &Db, s: NewSentence<'_>, now: OffsetDateTime) -> Result<Option<i64>> {
    if s.text.trim().is_empty() {
        return Ok(None);
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO sentence
             (profile_id, exercise_id, text, translation, origin, item_index, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (profile_id, exercise_id, text) DO UPDATE SET
             translation = COALESCE(excluded.translation, sentence.translation),
             item_index  = COALESCE(excluded.item_index, sentence.item_index)
         RETURNING id",
    )
    .bind(s.profile_id.0)
    .bind(s.exercise_id)
    .bind(s.text.trim())
    .bind(s.translation.map(str::trim).filter(|t| !t.is_empty()))
    .bind(s.origin)
    .bind(s.item_index)
    .bind(ts::to_sql(now))
    .fetch_one(db.pool())
    .await?;
    Ok(Some(id))
}

/// 把一句連到它裡面出現的詞條。
///
/// 重跑時**只加不減**：`INSERT OR IGNORE`。減的情況只有「這個字其實不在
/// 這句裡」，而那是索引建錯了，不是資料變了——真要修得整批重建。
pub async fn index(db: &Db, sentence_id: i64, lemma_ids: &[LemmaId]) -> Result<()> {
    for lemma_id in lemma_ids {
        sqlx::query("INSERT OR IGNORE INTO sentence_lemma (sentence_id, lemma_id) VALUES (?, ?)")
            .bind(sentence_id)
            .bind(lemma_id.0)
            .execute(db.pool())
            .await?;
    }
    Ok(())
}

/// 這個字做過的句子，新的在前。
pub async fn for_lemma(
    db: &Db,
    profile_id: ProfileId,
    lemma_id: LemmaId,
    limit: i64,
) -> Result<Vec<WordSentence>> {
    for_lemmas(db, profile_id, &[lemma_id], limit, 0).await
}

/// 這個詞族一共有幾句。
///
/// 分頁要靠它才說得出「第 2 / 5 頁」——常練的字會累積到十幾句，
/// 只給「還有更多」的話使用者不知道翻不翻得完。
pub async fn count_for_lemmas(
    db: &Db,
    profile_id: ProfileId,
    lemma_ids: &[LemmaId],
) -> Result<i64> {
    if lemma_ids.is_empty() {
        return Ok(0);
    }
    let placeholders = std::iter::repeat_n("?", lemma_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT COUNT(DISTINCT s.id) FROM sentence s
           JOIN sentence_lemma sl ON sl.sentence_id = s.id
         WHERE s.profile_id = ? AND sl.lemma_id IN ({placeholders})"
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql).bind(profile_id.0);
    for id in lemma_ids {
        q = q.bind(id.0);
    }
    Ok(q.fetch_one(db.pool()).await?)
}

/// 整個詞族做過的句子。
///
/// **一定要用這個而不是單一 id**：句子是用 `base_form` 正規化之後存的
/// （練 `ran` 存在 `run` 底下），而 UI 手上的是「使用者正在看的那個詞條」。
/// 字典裡 `ran` 自己也是四個獨立的詞條，拿它的 id 直接查會一句都查不到，
/// 而畫面上只會少一塊，看不出哪裡壞了。
pub async fn for_lemmas(
    db: &Db,
    profile_id: ProfileId,
    lemma_ids: &[LemmaId],
    limit: i64,
    offset: i64,
) -> Result<Vec<WordSentence>> {
    if lemma_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", lemma_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT DISTINCT s.id, s.exercise_id, s.text, s.translation, s.origin, s.misses,
                s.grammar_points_json, s.created_at
         FROM sentence s
           JOIN sentence_lemma sl ON sl.sentence_id = s.id
         WHERE s.profile_id = ? AND sl.lemma_id IN ({placeholders})
         ORDER BY s.created_at DESC, s.id DESC LIMIT ? OFFSET ?"
    );
    let mut q = sqlx::query(&sql).bind(profile_id.0);
    for id in lemma_ids {
        q = q.bind(id.0);
    }
    let rows = q
        .bind(limit.max(0))
        .bind(offset.max(0))
        .fetch_all(db.pool())
        .await?;

    Ok(rows
        .iter()
        .map(|row| WordSentence {
            id: row.get("id"),
            exercise_id: row.get("exercise_id"),
            text: row.get("text"),
            translation: row.get("translation"),
            origin: row.get("origin"),
            grammar_points: serde_json::from_str(&row.get::<String, _>("grammar_points_json"))
                .unwrap_or_default(),
            misses: row.get("misses"),
            created_at: row.get("created_at"),
        })
        .collect())
}

/// 記下這一句踩到的文法點。
///
/// 合併而不是覆蓋：同一句練好幾次，每次錯的點不一定一樣，而使用者要看的
/// 是「這句我在哪些地方栽過」。
pub async fn add_grammar_points(
    db: &Db,
    profile_id: ProfileId,
    exercise_id: i64,
    item_index: i64,
    points: &[String],
) -> Result<()> {
    if points.is_empty() {
        return Ok(());
    }
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, grammar_points_json FROM sentence
         WHERE profile_id = ? AND exercise_id = ? AND item_index = ?",
    )
    .bind(profile_id.0)
    .bind(exercise_id)
    .bind(item_index)
    .fetch_all(db.pool())
    .await?;

    for (id, existing) in rows {
        let mut merged: Vec<String> = serde_json::from_str(&existing).unwrap_or_default();
        for point in points {
            if !merged.iter().any(|p| p == point) {
                merged.push(point.clone());
            }
        }
        sqlx::query("UPDATE sentence SET grammar_points_json = ? WHERE id = ?")
            .bind(serde_json::to_string(&merged).unwrap_or_else(|_| "[]".into()))
            .bind(id)
            .execute(db.pool())
            .await?;
    }
    Ok(())
}

/// 這一句又錯了一次。
///
/// 對到的是「那份練習的第幾題」，所以只有翻譯題會呼叫——閱讀與克漏字的
/// 句子不是一題，沒有對錯可言。
pub async fn mark_missed(
    db: &Db,
    profile_id: ProfileId,
    exercise_id: i64,
    item_index: i64,
) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE sentence SET misses = misses + 1
         WHERE profile_id = ? AND exercise_id = ? AND item_index = ?",
    )
    .bind(profile_id.0)
    .bind(exercise_id)
    .bind(item_index)
    .execute(db.pool())
    .await?
    .rows_affected())
}

/// 這份練習已經記過句子了沒有。補寫舊資料時用來略過做過的。
pub async fn has_any(db: &Db, exercise_id: i64) -> Result<bool> {
    let found: i64 =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sentence WHERE exercise_id = ?)")
            .bind(exercise_id)
            .fetch_one(db.pool())
            .await?;
    Ok(found != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exercises::{self, NewExercise};
    use crate::repo::{NewLemma, lemmas, profiles};

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    /// 回傳 (資料庫, profile, `borrow` 的 lemma, 一份練習)
    async fn setup() -> (Db, ProfileId, LemmaId, i64) {
        let db = Db::open_in_memory().await.unwrap();
        let profile = profiles::create(&db, "我", "zh-TW", "en", t0())
            .await
            .unwrap();
        let lemma = lemmas::upsert(
            &db,
            NewLemma {
                lang: "en",
                text: "borrow",
                pos: "verb",
                freq_rank: Some(1000),
                cefr: None,
            },
        )
        .await
        .unwrap();
        let exercise = exercises::create(
            &db,
            NewExercise {
                profile_id: profile,
                kind: "translation_to_target",
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
        (db, profile, lemma, exercise.0)
    }

    fn sentence<'a>(
        profile: ProfileId,
        exercise: i64,
        text: &'a str,
        translation: Option<&'a str>,
    ) -> NewSentence<'a> {
        NewSentence {
            profile_id: profile,
            exercise_id: exercise,
            text,
            translation,
            origin: "translation",
            item_index: Some(0),
        }
    }

    /// 寫入是兩步：記下句子拿到 id，再把它連到出現的詞條。
    async fn store(db: &Db, s: NewSentence<'_>, lemma: LemmaId) -> Option<i64> {
        let id = record(db, s, t0()).await.unwrap();
        if let Some(id) = id {
            index(db, id, &[lemma]).await.unwrap();
        }
        id
    }

    #[tokio::test]
    async fn a_sentence_comes_back_for_its_word() {
        let (db, profile, lemma, exercise) = setup().await;
        store(
            &db,
            sentence(
                profile,
                exercise,
                "I borrowed a book from him.",
                Some("我跟他借了一本書"),
            ),
            lemma,
        )
        .await;

        let got = for_lemma(&db, profile, lemma, 10).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "I borrowed a book from him.");
        assert_eq!(got[0].translation.as_deref(), Some("我跟他借了一本書"));
    }

    /// 同一份練習裡同一個字出現兩次只留一句，不然複習畫面會是同一句重複三行。
    #[tokio::test]
    async fn the_same_sentence_is_only_kept_once() {
        let (db, profile, lemma, exercise) = setup().await;
        let text = "I borrowed a book.";
        store(&db, sentence(profile, exercise, text, None), lemma).await;
        store(
            &db,
            sentence(profile, exercise, text, Some("我借了一本書")),
            lemma,
        )
        .await;

        let got = for_lemma(&db, profile, lemma, 10).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].translation.as_deref(),
            Some("我借了一本書"),
            "第二次帶了翻譯就該補上去"
        );
    }

    /// 這條測試存在的理由是它曾經是錯的：句子用 `base_form` 正規化之後
    /// 存進去（練 `ran` 存在 `run` 底下），但讀取端拿的是「使用者正在看的
    /// 那個詞條」的 id。字典裡 `ran` 自己也是獨立的詞條——實測那份 224 萬詞
    /// 的字典裡有四個——所以在字典頁點 `ran` 會一句都查不到，
    /// 而畫面上只是少一塊，看不出哪裡壞了。
    #[tokio::test]
    async fn an_inflection_finds_the_sentences_filed_under_its_base_form() {
        let (db, profile, run, exercise) = setup().await;
        // `ran` 在字典裡自己也是一個詞條，跟 `run` 是不同的 id
        let ran = lemmas::upsert(
            &db,
            NewLemma {
                lang: "en",
                text: "ran",
                pos: "verb",
                freq_rank: None,
                cefr: None,
            },
        )
        .await
        .unwrap();
        assert_ne!(ran, run);
        lemmas::add_surface_form(&db, "en", "ran", run, "past")
            .await
            .unwrap();

        // 句子的索引是建在 base form 底下的
        store(
            &db,
            NewSentence {
                profile_id: profile,
                exercise_id: exercise,
                text: "She ran to the station.",
                translation: Some("她跑去車站"),
                origin: "translation",
                item_index: Some(0),
            },
            run,
        )
        .await;

        let family = lemmas::family(&db, "en", "ran").await.unwrap();
        let got = for_lemmas(&db, profile, &family, 10, 0).await.unwrap();
        assert_eq!(got.len(), 1, "查 ran 該看得到掛在 run 底下的句子");
        assert_eq!(got[0].text, "She ran to the station.");
    }

    /// 空句子不要記：畫面上一個空白的「你寫過的句子」看起來像壞掉。
    #[tokio::test]
    async fn an_empty_sentence_is_ignored() {
        let (db, profile, lemma, exercise) = setup().await;
        assert!(
            store(&db, sentence(profile, exercise, "   ", None), lemma)
                .await
                .is_none()
        );
        assert!(for_lemma(&db, profile, lemma, 10).await.unwrap().is_empty());
    }

    /// 刪掉練習，句子一起走——刪掉紀錄就是全部刪掉，不留沒有出處的殘影。
    #[tokio::test]
    async fn deleting_an_exercise_takes_its_sentences_with_it() {
        let (db, profile, lemma, exercise) = setup().await;
        store(
            &db,
            sentence(profile, exercise, "I borrowed it.", None),
            lemma,
        )
        .await;
        assert!(has_any(&db, exercise).await.unwrap());

        exercises::delete(&db, profile, exercises::ExerciseId(exercise))
            .await
            .unwrap();

        assert!(for_lemma(&db, profile, lemma, 10).await.unwrap().is_empty());
        assert!(!has_any(&db, exercise).await.unwrap());
    }
}
