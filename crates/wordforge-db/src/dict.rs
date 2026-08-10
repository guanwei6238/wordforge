//! 字典內容的寫入與查詢。
//!
//! 寫入函數都吃 `&mut SqliteConnection` 而不是連線池，
//! 因為匯入時要把上千筆詞條包在同一個 transaction 裡才有合理的速度
//! （每筆各自 commit 的話，百萬筆詞條會跑上好幾個小時）。

use std::collections::HashMap;

use serde::Serialize;
use sqlx::{Row, SqliteConnection};
use time::OffsetDateTime;
use wordforge_core::model::LemmaId;

use crate::{Db, Result, ts};

/// 字典來源的識別碼。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SourceId(pub i64);

// ---------------------------------------------------------------- 寫入結構

#[derive(Debug, Clone)]
pub struct NewSource<'a> {
    pub slug: &'a str,
    pub name: &'a str,
    pub license: Option<&'a str>,
    pub attribution: Option<&'a str>,
    pub homepage: Option<&'a str>,
    pub version: Option<&'a str>,
}

#[derive(Debug, Clone, Default)]
pub struct NewSense<'a> {
    pub gloss: &'a str,
    pub gloss_lang: &'a str,
    pub translation: Option<&'a str>,
    pub register: Option<&'a str>,
    pub domain: Option<&'a str>,
    pub examples: Vec<NewExample<'a>>,
}

#[derive(Debug, Clone)]
pub struct NewExample<'a> {
    pub text: &'a str,
    pub translation: Option<&'a str>,
}

#[derive(Debug, Clone, Default)]
pub struct NewPronunciation<'a> {
    pub accent: Option<&'a str>,
    pub ipa: Option<&'a str>,
    pub audio_path: Option<&'a str>,
    pub audio_license: Option<&'a str>,
    pub is_synthetic: bool,
}

/// 一個完整詞條的寫入請求。
#[derive(Debug, Clone, Default)]
pub struct EntryWrite<'a> {
    pub lang: &'a str,
    pub headword: &'a str,
    pub pos: &'a str,
    pub freq_rank: Option<i64>,
    pub cefr: Option<&'a str>,
    pub senses: Vec<NewSense<'a>>,
    pub pronunciations: Vec<NewPronunciation<'a>>,
    /// (詞形, 標籤)，例如 `("ran", "past")`
    pub forms: Vec<(&'a str, &'a str)>,
}

// ---------------------------------------------------------------- 寫入

/// 登記匯入來源。同一個 slug 重複匯入會更新版本與時間，不會產生第二筆。
pub async fn upsert_source(db: &Db, src: NewSource<'_>, now: OffsetDateTime) -> Result<SourceId> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO dict_source (slug, name, license, attribution, homepage, version, imported_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (slug) DO UPDATE SET
             name        = excluded.name,
             license     = excluded.license,
             attribution = excluded.attribution,
             homepage    = excluded.homepage,
             version     = COALESCE(excluded.version, dict_source.version),
             imported_at = excluded.imported_at
         RETURNING id",
    )
    .bind(src.slug)
    .bind(src.name)
    .bind(src.license)
    .bind(src.attribution)
    .bind(src.homepage)
    .bind(src.version)
    .bind(ts::to_sql(now))
    .fetch_one(db.pool())
    .await?;

    Ok(SourceId(id))
}

/// 寫入一個詞條。
///
/// 對「同一個來源」是冪等的：重新匯入一份更新的 dump 時，
/// 會先清掉這個來源先前寫在這個詞條上的釋義與發音再重寫，
/// 但**不會**動到其他來源的資料，也不會動到使用者的學習進度。
pub async fn write_entry(
    conn: &mut SqliteConnection,
    source: SourceId,
    entry: &EntryWrite<'_>,
) -> Result<LemmaId> {
    let normalized = wordforge_core::text::normalize(entry.headword);

    let lemma_id: i64 = sqlx::query_scalar(
        "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, cefr)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (lang, text, pos) DO UPDATE SET
             freq_rank = COALESCE(excluded.freq_rank, lemma.freq_rank),
             cefr      = COALESCE(excluded.cefr, lemma.cefr)
         RETURNING id",
    )
    .bind(entry.lang)
    .bind(entry.headword)
    .bind(&normalized)
    .bind(entry.pos)
    .bind(entry.freq_rank)
    .bind(entry.cefr)
    .fetch_one(&mut *conn)
    .await?;

    // 先清掉本來源的舊資料，避免重複匯入時釋義越疊越多。
    // example 掛在 sense 底下，會跟著 CASCADE 一起消失。
    sqlx::query("DELETE FROM sense WHERE lemma_id = ? AND source_id = ?")
        .bind(lemma_id)
        .bind(source.0)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM pronunciation WHERE lemma_id = ? AND source_id = ?")
        .bind(lemma_id)
        .bind(source.0)
        .execute(&mut *conn)
        .await?;

    for (order, sense) in entry.senses.iter().enumerate() {
        let sense_id: i64 = sqlx::query_scalar(
            "INSERT INTO sense (lemma_id, source_id, gloss, gloss_lang, translation,
                                register, domain, sort_order)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(lemma_id)
        .bind(source.0)
        .bind(sense.gloss)
        .bind(sense.gloss_lang)
        .bind(sense.translation)
        .bind(sense.register)
        .bind(sense.domain)
        .bind(order as i64)
        .fetch_one(&mut *conn)
        .await?;

        for ex in &sense.examples {
            sqlx::query(
                "INSERT INTO example (lemma_id, sense_id, source_id, text, translation)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(lemma_id)
            .bind(sense_id)
            .bind(source.0)
            .bind(ex.text)
            .bind(ex.translation)
            .execute(&mut *conn)
            .await?;
        }
    }

    for pron in &entry.pronunciations {
        sqlx::query(
            "INSERT INTO pronunciation (lemma_id, source_id, accent, ipa, audio_path,
                                        audio_license, is_synthetic)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(lemma_id)
        .bind(source.0)
        .bind(pron.accent)
        .bind(pron.ipa)
        .bind(pron.audio_path)
        .bind(pron.audio_license)
        .bind(pron.is_synthetic as i64)
        .execute(&mut *conn)
        .await?;
    }

    for (form, tag) in &entry.forms {
        let form_norm = wordforge_core::text::normalize(form);
        if form_norm.is_empty() || form_norm == normalized {
            continue; // 詞形跟原形一樣就沒有登記的價值
        }
        sqlx::query(
            "INSERT INTO surface_form (lang, form, normalized, lemma_id, tag)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (lang, normalized, lemma_id, tag) DO NOTHING",
        )
        .bind(entry.lang)
        .bind(form)
        .bind(&form_norm)
        .bind(lemma_id)
        .bind(tag)
        .execute(&mut *conn)
        .await?;
    }

    Ok(LemmaId(lemma_id))
}

/// 套用詞頻表。只更新已存在的詞條，不會憑空建立新詞條——
/// 詞頻表裡有一堆拼錯的字與專有名詞，不該讓它們污染字典。
///
/// 回傳實際更新的筆數。
pub async fn apply_freq_ranks(db: &Db, lang: &str, table: &HashMap<String, i64>) -> Result<u64> {
    let mut tx = db.pool().begin().await?;
    let mut updated = 0u64;

    for (word, rank) in table {
        let res = sqlx::query(
            "UPDATE lemma SET freq_rank = ?
             WHERE lang = ? AND normalized = ? AND (freq_rank IS NULL OR freq_rank > ?)",
        )
        .bind(rank)
        .bind(lang)
        .bind(word)
        .bind(rank)
        .execute(&mut *tx)
        .await?;
        updated += res.rows_affected();
    }

    tx.commit().await?;
    Ok(updated)
}

// ---------------------------------------------------------------- 查詢

/// 搜尋結果的一列。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchHit {
    pub lemma_id: i64,
    pub text: String,
    pub pos: String,
    pub freq_rank: Option<i64>,
    pub cefr: Option<String>,
    /// 第一個釋義，用於在清單上預覽
    pub gloss: Option<String>,
    pub translation: Option<String>,
    /// 這個字是否已經在學習者的牌組裡
    pub in_deck: bool,
}

/// 查字典。
///
/// 排序刻意分三層：完全相符 → 詞形相符 → 前綴相符，
/// 同層之內用詞頻。使用者打 `run` 時要先看到 `run`，
/// 而不是 `runway`（即使 `runway` 剛好詞頻較高）。
pub async fn search(
    db: &Db,
    lang: &str,
    query: &str,
    profile_id: i64,
    limit: i64,
) -> Result<Vec<SearchHit>> {
    let normalized = wordforge_core::text::normalize(query);
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    // LIKE 的萬用字元必須跳脫，否則使用者輸入 `%` 會撈出整本字典
    let prefix = format!(
        "{}%",
        normalized
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );

    // 全部用裸 `?` 依序綁定。混用 `?N` 位置參數雖然能少 bind 幾次，
    // 但只要中間插入一個條件，整串索引就會錯位而且不會有編譯錯誤。
    let rows = sqlx::query(
        "SELECT l.id, l.text, l.pos, l.freq_rank, l.cefr,
                (SELECT gloss FROM sense WHERE lemma_id = l.id ORDER BY sort_order LIMIT 1) AS gloss,
                (SELECT translation FROM sense WHERE lemma_id = l.id
                   AND translation IS NOT NULL ORDER BY sort_order LIMIT 1) AS translation,
                EXISTS (SELECT 1 FROM card WHERE lemma_id = l.id AND profile_id = ?) AS in_deck,
                CASE
                    WHEN l.normalized = ? THEN 0
                    WHEN EXISTS (SELECT 1 FROM surface_form s
                                 WHERE s.lemma_id = l.id AND s.normalized = ?) THEN 1
                    ELSE 2
                END AS match_rank
         FROM lemma l
         WHERE l.lang = ?
           AND (l.normalized LIKE ? ESCAPE '\\'
                OR EXISTS (SELECT 1 FROM surface_form s
                           WHERE s.lemma_id = l.id AND s.normalized = ?))
         ORDER BY match_rank, l.freq_rank IS NULL, l.freq_rank, length(l.text), l.text
         LIMIT ?",
    )
    .bind(profile_id)
    .bind(&normalized)
    .bind(&normalized)
    .bind(lang)
    .bind(&prefix)
    .bind(&normalized)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SearchHit {
            lemma_id: r.get("id"),
            text: r.get("text"),
            pos: r.get("pos"),
            freq_rank: r.get("freq_rank"),
            cefr: r.get("cefr"),
            gloss: r.get("gloss"),
            translation: r.get("translation"),
            in_deck: r.get::<i64, _>("in_deck") != 0,
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SenseView {
    pub gloss: String,
    pub translation: Option<String>,
    pub register: Option<String>,
    pub domain: Option<String>,
    pub examples: Vec<ExampleView>,
    /// 來源標示，CC BY-SA 要求顯示
    pub attribution: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExampleView {
    pub text: String,
    pub translation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PronunciationView {
    pub accent: Option<String>,
    pub ipa: Option<String>,
    pub audio_path: Option<String>,
    pub is_synthetic: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WordDetail {
    pub lemma_id: i64,
    pub text: String,
    pub pos: String,
    pub freq_rank: Option<i64>,
    pub cefr: Option<String>,
    pub senses: Vec<SenseView>,
    pub pronunciations: Vec<PronunciationView>,
    pub forms: Vec<(String, String)>,
    pub in_deck: bool,
}

/// 取得一個詞條的完整內容。
pub async fn detail(db: &Db, lemma_id: i64, profile_id: i64) -> Result<Option<WordDetail>> {
    let Some(head) = sqlx::query(
        "SELECT l.id, l.text, l.pos, l.freq_rank, l.cefr,
                EXISTS (SELECT 1 FROM card WHERE lemma_id = l.id AND profile_id = ?) AS in_deck
         FROM lemma l WHERE l.id = ?",
    )
    .bind(profile_id)
    .bind(lemma_id)
    .fetch_optional(db.pool())
    .await?
    else {
        return Ok(None);
    };

    let sense_rows = sqlx::query(
        "SELECT s.id, s.gloss, s.translation, s.register, s.domain, d.attribution
         FROM sense s LEFT JOIN dict_source d ON d.id = s.source_id
         WHERE s.lemma_id = ? ORDER BY s.sort_order, s.id",
    )
    .bind(lemma_id)
    .fetch_all(db.pool())
    .await?;

    let mut senses = Vec::with_capacity(sense_rows.len());
    for row in sense_rows {
        let sense_id: i64 = row.get("id");
        let examples = sqlx::query("SELECT text, translation FROM example WHERE sense_id = ?")
            .bind(sense_id)
            .fetch_all(db.pool())
            .await?
            .into_iter()
            .map(|e| ExampleView {
                text: e.get("text"),
                translation: e.get("translation"),
            })
            .collect();

        senses.push(SenseView {
            gloss: row.get("gloss"),
            translation: row.get("translation"),
            register: row.get("register"),
            domain: row.get("domain"),
            examples,
            attribution: row.get("attribution"),
        });
    }

    let pronunciations = sqlx::query(
        "SELECT accent, ipa, audio_path, is_synthetic FROM pronunciation WHERE lemma_id = ?",
    )
    .bind(lemma_id)
    .fetch_all(db.pool())
    .await?
    .into_iter()
    .map(|p| PronunciationView {
        accent: p.get("accent"),
        ipa: p.get("ipa"),
        audio_path: p.get("audio_path"),
        is_synthetic: p.get::<i64, _>("is_synthetic") != 0,
    })
    .collect();

    let forms = sqlx::query("SELECT form, tag FROM surface_form WHERE lemma_id = ? ORDER BY form")
        .bind(lemma_id)
        .fetch_all(db.pool())
        .await?
        .into_iter()
        .map(|f| (f.get("form"), f.get("tag")))
        .collect();

    Ok(Some(WordDetail {
        lemma_id: head.get("id"),
        text: head.get("text"),
        pos: head.get("pos"),
        freq_rank: head.get("freq_rank"),
        cefr: head.get("cefr"),
        senses,
        pronunciations,
        forms,
        in_deck: head.get::<i64, _>("in_deck") != 0,
    }))
}

/// 字典規模統計，顯示在匯入畫面上。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DictStats {
    pub lemmas: i64,
    pub senses: i64,
    pub with_audio: i64,
    pub sources: Vec<SourceInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceInfo {
    pub slug: String,
    pub name: String,
    pub license: Option<String>,
    pub attribution: Option<String>,
    pub imported_at: String,
    pub lemma_count: i64,
}

pub async fn stats(db: &Db) -> Result<DictStats> {
    let lemmas: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM lemma")
        .fetch_one(db.pool())
        .await?;
    let senses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sense")
        .fetch_one(db.pool())
        .await?;
    let with_audio: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT lemma_id) FROM pronunciation WHERE audio_path IS NOT NULL",
    )
    .fetch_one(db.pool())
    .await?;

    let sources = sqlx::query(
        "SELECT d.slug, d.name, d.license, d.attribution, d.imported_at,
                (SELECT COUNT(DISTINCT lemma_id) FROM sense WHERE source_id = d.id) AS lemma_count
         FROM dict_source d ORDER BY d.imported_at DESC",
    )
    .fetch_all(db.pool())
    .await?
    .into_iter()
    .map(|r| SourceInfo {
        slug: r.get("slug"),
        name: r.get("name"),
        license: r.get("license"),
        attribution: r.get("attribution"),
        imported_at: r.get("imported_at"),
        lemma_count: r.get("lemma_count"),
    })
    .collect();

    Ok(DictStats {
        lemmas,
        senses,
        with_audio,
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::profiles;

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    async fn setup() -> (Db, SourceId, i64) {
        let db = Db::open_in_memory().await.unwrap();
        let profile = profiles::create(&db, "我", "zh-TW", "en", t0())
            .await
            .unwrap();
        let source = upsert_source(
            &db,
            NewSource {
                slug: "wiktionary-en",
                name: "Wiktionary (en)",
                license: Some("CC BY-SA 4.0"),
                attribution: Some("Wiktionary contributors"),
                homepage: None,
                version: Some("2026-08"),
            },
            t0(),
        )
        .await
        .unwrap();
        (db, source, profile.0)
    }

    fn run_entry<'a>() -> EntryWrite<'a> {
        EntryWrite {
            lang: "en",
            headword: "run",
            pos: "verb",
            freq_rank: Some(300),
            cefr: Some("A2"),
            senses: vec![NewSense {
                gloss: "To move swiftly on foot",
                gloss_lang: "en",
                translation: Some("跑"),
                examples: vec![NewExample {
                    text: "She ran to the station.",
                    translation: None,
                }],
                ..Default::default()
            }],
            pronunciations: vec![NewPronunciation {
                accent: Some("uk"),
                ipa: Some("/ɹʌn/"),
                ..Default::default()
            }],
            forms: vec![
                ("ran", "past"),
                ("running", "gerund"),
                ("run", "infinitive"),
            ],
        }
    }

    async fn write(db: &Db, source: SourceId, entry: &EntryWrite<'_>) -> LemmaId {
        let mut conn = db.pool().acquire().await.unwrap();
        write_entry(&mut conn, source, entry).await.unwrap()
    }

    #[tokio::test]
    async fn writes_a_full_entry() {
        let (db, source, profile) = setup().await;
        let id = write(&db, source, &run_entry()).await;

        let d = detail(&db, id.0, profile)
            .await
            .unwrap()
            .expect("應該查得到");
        assert_eq!(d.text, "run");
        assert_eq!(d.senses.len(), 1);
        assert_eq!(d.senses[0].translation.as_deref(), Some("跑"));
        assert_eq!(d.senses[0].examples[0].text, "She ran to the station.");
        assert_eq!(
            d.senses[0].attribution.as_deref(),
            Some("Wiktionary contributors"),
            "CC BY-SA 要求顯示出處"
        );
        assert_eq!(d.pronunciations[0].ipa.as_deref(), Some("/ɹʌn/"));
        assert!(!d.in_deck);
    }

    /// 跟原形相同的詞形不該進 surface_form。
    #[tokio::test]
    async fn skips_forms_identical_to_the_headword() {
        let (db, source, profile) = setup().await;
        let id = write(&db, source, &run_entry()).await;
        let d = detail(&db, id.0, profile).await.unwrap().unwrap();
        let forms: Vec<&str> = d.forms.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(forms, vec!["ran", "running"]);
    }

    /// 重新匯入更新版的 dump，不能讓釋義越疊越多。
    #[tokio::test]
    async fn reimporting_the_same_source_is_idempotent() {
        let (db, source, profile) = setup().await;
        write(&db, source, &run_entry()).await;
        let id = write(&db, source, &run_entry()).await;

        let d = detail(&db, id.0, profile).await.unwrap().unwrap();
        assert_eq!(d.senses.len(), 1, "重複匯入產生了重複釋義");
        assert_eq!(d.pronunciations.len(), 1);

        let examples: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM example")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(examples, 1, "例句應該跟著舊釋義一起被清掉");
    }

    /// 不同來源的資料要能並存，各自標示出處。
    #[tokio::test]
    async fn different_sources_coexist() {
        let (db, wiktionary, profile) = setup().await;
        let mine = upsert_source(
            &db,
            NewSource {
                slug: "my-notes",
                name: "我的單字表",
                license: None,
                attribution: None,
                homepage: None,
                version: None,
            },
            t0(),
        )
        .await
        .unwrap();

        write(&db, wiktionary, &run_entry()).await;
        let id = write(
            &db,
            mine,
            &EntryWrite {
                senses: vec![NewSense {
                    gloss: "課本第三課：跑步",
                    gloss_lang: "zh-TW",
                    ..Default::default()
                }],
                pronunciations: vec![],
                forms: vec![],
                ..run_entry()
            },
        )
        .await;

        let d = detail(&db, id.0, profile).await.unwrap().unwrap();
        assert_eq!(d.senses.len(), 2, "兩個來源的釋義都要在");
    }

    #[tokio::test]
    async fn search_ranks_exact_match_above_prefix() {
        let (db, source, profile) = setup().await;
        write(&db, source, &run_entry()).await;
        write(
            &db,
            source,
            &EntryWrite {
                headword: "runway",
                pos: "noun",
                freq_rank: Some(10), // 詞頻更高，但不是完全相符
                forms: vec![],
                ..run_entry()
            },
        )
        .await;

        let hits = search(&db, "en", "run", profile, 10).await.unwrap();
        assert_eq!(
            hits[0].text, "run",
            "完全相符必須排在詞頻更高的前綴相符之前"
        );
        assert_eq!(hits[1].text, "runway");
        assert_eq!(hits[0].translation.as_deref(), Some("跑"));
    }

    /// 查詞形變化要能找到原形。
    #[tokio::test]
    async fn search_resolves_inflected_forms() {
        let (db, source, profile) = setup().await;
        write(&db, source, &run_entry()).await;

        let hits = search(&db, "en", "Ran", profile, 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "run");
    }

    /// LIKE 的萬用字元必須跳脫，否則輸入 `%` 會把整本字典撈出來。
    #[tokio::test]
    async fn search_escapes_wildcards() {
        let (db, source, profile) = setup().await;
        write(&db, source, &run_entry()).await;

        assert!(
            search(&db, "en", "%", profile, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            search(&db, "en", "_un", profile, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(search(&db, "en", "", profile, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn freq_ranks_only_touch_existing_words() {
        let (db, source, profile) = setup().await;
        write(&db, source, &run_entry()).await;

        let table = HashMap::from([
            ("run".to_string(), 42i64),
            ("nonexistent".to_string(), 1i64),
        ]);
        let updated = apply_freq_ranks(&db, "en", &table).await.unwrap();

        assert_eq!(updated, 1, "詞頻表裡沒收錄的字不該憑空建立詞條");
        let d = detail(&db, 1, profile).await.unwrap().unwrap();
        assert_eq!(d.freq_rank, Some(42));

        // 已經有更好（更小）的排名時不覆蓋
        let worse = HashMap::from([("run".to_string(), 9000i64)]);
        apply_freq_ranks(&db, "en", &worse).await.unwrap();
        let d = detail(&db, 1, profile).await.unwrap().unwrap();
        assert_eq!(d.freq_rank, Some(42));
    }

    #[tokio::test]
    async fn stats_report_sources_and_counts() {
        let (db, source, _) = setup().await;
        write(&db, source, &run_entry()).await;

        let s = stats(&db).await.unwrap();
        assert_eq!(s.lemmas, 1);
        assert_eq!(s.senses, 1);
        assert_eq!(s.sources.len(), 1);
        assert_eq!(s.sources[0].slug, "wiktionary-en");
        assert_eq!(s.sources[0].lemma_count, 1);
        assert_eq!(s.sources[0].license.as_deref(), Some("CC BY-SA 4.0"));
    }

    #[tokio::test]
    async fn detail_returns_none_for_unknown_id() {
        let (db, _, profile) = setup().await;
        assert!(detail(&db, 999, profile).await.unwrap().is_none());
    }
}
