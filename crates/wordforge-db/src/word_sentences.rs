//! 「這個字我在哪句話裡用過」。
//!
//! 複習單字時看得到的是字典的釋義與別人寫的例句。真正記得住的是**自己
//! 做過的那一句**——翻譯題裡寫過的、閱讀文章裡讀到的。這個模組把單字接回
//! 那些句子，複習頁與字典頁共用同一份資料。
//!
//! 連結存的是 lemma 而不是字串：查 `ran` 要看得到練 `run` 時寫的句子。

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
    pub lemma_id: LemmaId,
    pub exercise_id: i64,
    pub text: &'a str,
    pub translation: Option<&'a str>,
    pub origin: &'a str,
    /// 這一句是那份練習的第幾題。翻譯題才有——閱讀與克漏字的句子
    /// 不是「一題」，對不回排程與批改結果。
    pub item_index: Option<i64>,
}

/// 記一句。同一份練習裡同一個字重複出現時只留一句。
///
/// 空句子直接忽略：出題偶爾會少一個欄位，而一個空白的「你寫過的句子」
/// 在畫面上看起來像壞掉。
pub async fn record(db: &Db, s: NewSentence<'_>, now: OffsetDateTime) -> Result<bool> {
    if s.text.trim().is_empty() {
        return Ok(false);
    }
    let affected = sqlx::query(
        "INSERT INTO word_sentence
             (profile_id, lemma_id, exercise_id, text, translation, origin, item_index, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (profile_id, lemma_id, exercise_id, text) DO UPDATE SET
             translation = COALESCE(excluded.translation, word_sentence.translation),
             item_index  = COALESCE(excluded.item_index, word_sentence.item_index)",
    )
    .bind(s.profile_id.0)
    .bind(s.lemma_id.0)
    .bind(s.exercise_id)
    .bind(s.text.trim())
    .bind(s.translation.map(str::trim).filter(|t| !t.is_empty()))
    .bind(s.origin)
    .bind(s.item_index)
    .bind(ts::to_sql(now))
    .execute(db.pool())
    .await?
    .rows_affected();
    Ok(affected > 0)
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
        "SELECT COUNT(*) FROM word_sentence
         WHERE profile_id = ? AND lemma_id IN ({placeholders})"
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
        "SELECT id, exercise_id, text, translation, origin, misses,
                grammar_points_json, created_at
         FROM word_sentence
         WHERE profile_id = ? AND lemma_id IN ({placeholders})
         ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
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
        "SELECT id, grammar_points_json FROM word_sentence
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
        sqlx::query("UPDATE word_sentence SET grammar_points_json = ? WHERE id = ?")
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
/// 句子不是一題，沒有對錯可言。同一句可能連到好幾個字（一句話裡有兩個
/// 目標詞），那時每一筆都要加，因為每一筆都是「那個字的那一句」。
pub async fn mark_missed(
    db: &Db,
    profile_id: ProfileId,
    exercise_id: i64,
    item_index: i64,
) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE word_sentence SET misses = misses + 1
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
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM word_sentence WHERE exercise_id = ?)")
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
        lemma: LemmaId,
        exercise: i64,
        text: &'a str,
        translation: Option<&'a str>,
    ) -> NewSentence<'a> {
        NewSentence {
            profile_id: profile,
            lemma_id: lemma,
            exercise_id: exercise,
            text,
            translation,
            origin: "translation",
            item_index: Some(0),
        }
    }

    #[tokio::test]
    async fn a_sentence_comes_back_for_its_word() {
        let (db, profile, lemma, exercise) = setup().await;
        record(
            &db,
            sentence(
                profile,
                lemma,
                exercise,
                "I borrowed a book from him.",
                Some("我跟他借了一本書"),
            ),
            t0(),
        )
        .await
        .unwrap();

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
        record(&db, sentence(profile, lemma, exercise, text, None), t0())
            .await
            .unwrap();
        record(
            &db,
            sentence(profile, lemma, exercise, text, Some("我借了一本書")),
            t0(),
        )
        .await
        .unwrap();

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

        // 句子是存在 base form 底下的
        record(
            &db,
            NewSentence {
                profile_id: profile,
                lemma_id: run,
                exercise_id: exercise,
                text: "She ran to the station.",
                translation: Some("她跑去車站"),
                origin: "translation",
                item_index: Some(0),
            },
            t0(),
        )
        .await
        .unwrap();

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
            !record(&db, sentence(profile, lemma, exercise, "   ", None), t0())
                .await
                .unwrap()
        );
        assert!(for_lemma(&db, profile, lemma, 10).await.unwrap().is_empty());
    }

    /// 刪掉練習，句子一起走——刪掉紀錄就是全部刪掉，不留沒有出處的殘影。
    #[tokio::test]
    async fn deleting_an_exercise_takes_its_sentences_with_it() {
        let (db, profile, lemma, exercise) = setup().await;
        record(
            &db,
            sentence(profile, lemma, exercise, "I borrowed it.", None),
            t0(),
        )
        .await
        .unwrap();
        assert!(has_any(&db, exercise).await.unwrap());

        exercises::delete(&db, profile, exercises::ExerciseId(exercise))
            .await
            .unwrap();

        assert!(for_lemma(&db, profile, lemma, 10).await.unwrap().is_empty());
        assert!(!has_any(&db, exercise).await.unwrap());
    }
}
