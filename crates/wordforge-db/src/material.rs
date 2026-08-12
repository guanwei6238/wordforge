//! 使用者匯入的教材：存放、檢索、詞表。
//!
//! 教材的用途跟字典相反。字典回答「這個字是什麼意思」，教材回答
//! 「這次出題只能用這裡面的東西」。所以這裡的檢索不追求語意相似，
//! 只要能穩定地挑出「含有這次要練的字」的段落就夠。

use serde::Serialize;
use time::OffsetDateTime;

use crate::{Db, Result, ts};
use wordforge_core::model::{LemmaId, ProfileId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct MaterialId(pub i64);

/// 一份教材的概況。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Material {
    pub id: i64,
    pub title: String,
    /// text / epub / pdf / subtitle / html
    pub kind: String,
    pub lang: String,
    pub source_path: Option<String>,
    /// 使用者自己記的授權備註。App 不散布教材，這欄是給他自己看的。
    pub license_note: Option<String>,
    pub created_at: String,
    pub chunk_count: i64,
    /// 這份教材出現過幾個不同的詞（能對到字典的）
    pub vocab_count: i64,
}

/// 新增一份教材時要填的東西。
#[derive(Debug, Clone)]
pub struct NewMaterial<'a> {
    pub title: &'a str,
    pub kind: &'a str,
    pub lang: &'a str,
    pub source_path: Option<&'a str>,
    pub license_note: Option<&'a str>,
}

pub async fn create(
    db: &Db,
    profile_id: ProfileId,
    material: NewMaterial<'_>,
    now: OffsetDateTime,
) -> Result<MaterialId> {
    let title = material.title.trim();
    if title.is_empty() {
        return Err(crate::DbError::Invalid("教材需要一個名稱".into()));
    }
    if material.lang.trim().is_empty() {
        return Err(crate::DbError::Invalid("教材需要標明語言".into()));
    }

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO material (profile_id, title, kind, lang, source_path, license_note, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(profile_id.0)
    .bind(title)
    .bind(material.kind)
    .bind(material.lang.trim())
    .bind(material.source_path)
    .bind(material.license_note)
    .bind(ts::to_sql(now))
    .fetch_one(db.pool())
    .await?;

    Ok(MaterialId(id))
}

/// 寫入切好的塊。
///
/// 用一個交易包起來：教材可能有上千塊，中途失敗留下半份教材
/// 比完全沒匯入更糟——使用者會以為匯好了，然後出題永遠只看得到前三章。
pub async fn add_chunks(db: &Db, material_id: MaterialId, chunks: &[String]) -> Result<Vec<i64>> {
    let mut tx = db.pool().begin().await?;
    let mut ids = Vec::with_capacity(chunks.len());

    for (ord, text) in chunks.iter().enumerate() {
        let token_count = wordforge_core::text::tokenize(text).len() as i64;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO material_chunk (material_id, ord, text, token_count)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (material_id, ord) DO UPDATE SET text = excluded.text,
                                                          token_count = excluded.token_count
             RETURNING id",
        )
        .bind(material_id.0)
        .bind(ord as i64)
        .bind(text)
        .bind(token_count)
        .fetch_one(&mut *tx)
        .await?;
        ids.push(id);
    }

    tx.commit().await?;
    Ok(ids)
}

/// 記錄每一塊用到哪些字，各幾次。
///
/// 記在塊上而不是整本書上，出題挑段落時才能問「哪些塊含有這個字」，
/// 而且因為存的是 base_form，課本裡的 `went` 找得到、`going` 不會誤中。
///
/// `per_chunk` 的索引要對應 [`add_chunks`] 回傳的 id 順序。
pub async fn set_chunk_vocab(db: &Db, per_chunk: &[(i64, Vec<(LemmaId, i64)>)]) -> Result<u64> {
    let mut tx = db.pool().begin().await?;
    let mut written = 0u64;

    for (chunk_id, counts) in per_chunk {
        sqlx::query("DELETE FROM material_chunk_vocab WHERE chunk_id = ?")
            .bind(chunk_id)
            .execute(&mut *tx)
            .await?;

        for (lemma, count) in counts {
            sqlx::query(
                "INSERT INTO material_chunk_vocab (chunk_id, lemma_id, count) VALUES (?, ?, ?)
                 ON CONFLICT (chunk_id, lemma_id) DO UPDATE SET count = excluded.count",
            )
            .bind(chunk_id)
            .bind(lemma.0)
            .bind(count)
            .execute(&mut *tx)
            .await?;
            written += 1;
        }
    }

    tx.commit().await?;
    Ok(written)
}

/// `list` 撈回來的原始欄位：id、標題、格式、語言、來源路徑、授權備註、
/// 建立時間、段數、詞數。
type MaterialRow = (
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    i64,
    i64,
);

pub async fn list(db: &Db, profile_id: ProfileId) -> Result<Vec<Material>> {
    let rows: Vec<MaterialRow> = sqlx::query_as(
        "SELECT m.id, m.title, m.kind, m.lang, m.source_path, m.license_note, m.created_at,
                    (SELECT COUNT(*) FROM material_chunk WHERE material_id = m.id),
                    (SELECT COUNT(DISTINCT v.lemma_id)
                       FROM material_chunk_vocab v
                       JOIN material_chunk c ON c.id = v.chunk_id
                      WHERE c.material_id = m.id)
             FROM material m WHERE m.profile_id = ?
             ORDER BY m.created_at DESC",
    )
    .bind(profile_id.0)
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(id, title, kind, lang, source_path, license_note, created_at, chunks, vocab)| {
                Material {
                    id,
                    title,
                    kind,
                    lang,
                    source_path,
                    license_note,
                    created_at,
                    chunk_count: chunks,
                    vocab_count: vocab,
                }
            },
        )
        .collect())
}

/// 刪除一份教材。塊與詞表靠外鍵 CASCADE 一起走。
pub async fn delete(db: &Db, profile_id: ProfileId, material_id: MaterialId) -> Result<bool> {
    let res = sqlx::query("DELETE FROM material WHERE id = ? AND profile_id = ?")
        .bind(material_id.0)
        .bind(profile_id.0)
        .execute(db.pool())
        .await?;
    Ok(res.rows_affected() > 0)
}

/// 挑一塊教材餵給模型。
///
/// 沒有用 embedding：那需要一個本地模型，而且多語言 embedding 對小語種
/// 最弱，正好是這個專案最在意的地方。這裡用兩條規則就夠：
///
/// 1. **命中最多要練的字的塊優先**。出題本來就是要練那些字，
///    模型手上有原文才寫得出符合課本語感的句子。
/// 2. **同分時輪流**。用 `seed` 挑，避免每次都拿第一章——
///    課本前三頁被出到爛，後面永遠沒出現過。
///
/// 比對用逐塊詞表而不是文字 `LIKE`。詞表存的是 base_form，所以
/// 練 `go` 找得到寫 `went` 的段落；`LIKE '%go%'` 不但找不到，
/// 還會誤中 `going`、`good`、`ago`。
pub async fn pick_chunk(
    db: &Db,
    material_id: MaterialId,
    wanted: &[LemmaId],
    seed: u64,
) -> Result<Option<String>> {
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM material_chunk WHERE material_id = ?")
            .bind(material_id.0)
            .fetch_one(db.pool())
            .await?;
    if total == 0 {
        return Ok(None);
    }

    if !wanted.is_empty() {
        let placeholders = std::iter::repeat_n("?", wanted.len())
            .collect::<Vec<_>>()
            .join(",");
        // 命中幾個字就算幾分，只留最高分的那幾塊；同分的照 ord 排，
        // 再用 seed 在裡面輪，所以不會每次都拿同一段。
        //
        // 用 CTE 是為了讓要練的字只出現一組佔位符。混用 `?1` 與 `?`
        // 的話 sqlx 的綁定順序會跟 SQLite 的自動編號對不上，
        // 而且失敗的樣子是「查不到東西」而不是報錯，非常難查。
        let sql = format!(
            "WITH scored AS (
                 SELECT c.id AS id, c.ord AS ord, c.text AS text,
                        COUNT(DISTINCT v.lemma_id) AS hits
                 FROM material_chunk c
                 JOIN material_chunk_vocab v ON v.chunk_id = c.id
                 WHERE c.material_id = ? AND v.lemma_id IN ({placeholders})
                 GROUP BY c.id
             )
             SELECT text FROM scored
             WHERE hits = (SELECT MAX(hits) FROM scored)
             ORDER BY ord"
        );

        let mut q = sqlx::query_scalar::<_, String>(&sql).bind(material_id.0);
        for id in wanted {
            q = q.bind(id.0);
        }

        let matches = q.fetch_all(db.pool()).await?;
        if !matches.is_empty() {
            let idx = (seed as usize) % matches.len();
            return Ok(Some(matches[idx].clone()));
        }
    }

    // 一個字都沒中就輪流拿，讓整本書都會被用到
    let ord = (seed % total as u64) as i64;
    let text: Option<String> =
        sqlx::query_scalar("SELECT text FROM material_chunk WHERE material_id = ? AND ord = ?")
            .bind(material_id.0)
            .bind(ord)
            .fetch_optional(db.pool())
            .await?;
    Ok(text)
}

/// 這份教材的字，學習者已經會了幾成。
///
/// 用來回答「這本課本我還差多少才讀得動」。
pub async fn coverage(
    db: &Db,
    profile_id: ProfileId,
    material_id: MaterialId,
    min_stability: f64,
) -> Result<(i64, i64)> {
    let row: (i64, i64) = sqlx::query_as(
        "WITH words AS (
             SELECT DISTINCT v.lemma_id AS lemma_id
             FROM material_chunk_vocab v
             JOIN material_chunk c ON c.id = v.chunk_id
             WHERE c.material_id = ?2
         )
         SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN EXISTS (
                    SELECT 1 FROM card c
                    WHERE c.profile_id = ?1 AND c.lemma_id = words.lemma_id
                      AND c.state = 'review' AND c.stability >= ?3
                ) THEN 1 ELSE 0 END), 0)
         FROM words",
    )
    .bind(profile_id.0)
    .bind(material_id.0)
    .bind(min_stability)
    .fetch_one(db.pool())
    .await?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::{EntryWrite, NewSense, NewSource};
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

    async fn add_material(db: &Db, profile: ProfileId, lang: &str) -> MaterialId {
        create(
            db,
            profile,
            NewMaterial {
                title: "第一冊",
                kind: "pdf",
                lang,
                source_path: Some("/tmp/book.pdf"),
                license_note: Some("自己買的，不外流"),
            },
            t0(),
        )
        .await
        .unwrap()
    }

    /// 建幾個詞條並回傳 id，讓測試能用真的 lemma。
    async fn words(db: &Db, lang: &str, texts: &[&str]) -> Vec<LemmaId> {
        let source = crate::dict::upsert_source(
            db,
            NewSource {
                slug: "t",
                name: "測試",
                license: None,
                attribution: None,
                homepage: None,
                version: None,
            },
            t0(),
        )
        .await
        .unwrap();

        let mut conn = db.pool().acquire().await.unwrap();
        let mut ids = Vec::new();
        for text in texts {
            ids.push(
                crate::dict::write_entry(
                    &mut conn,
                    source,
                    &EntryWrite {
                        lang,
                        headword: text,
                        pos: "",
                        senses: vec![NewSense {
                            gloss: "g",
                            gloss_lang: "en",
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    crate::dict::WriteMode::Replace,
                )
                .await
                .unwrap(),
            );
        }
        ids
    }

    #[tokio::test]
    async fn a_material_round_trips_with_its_chunks() {
        let (db, profile) = setup().await;
        let id = add_material(&db, profile, "en").await;
        add_chunks(&db, id, &["first chunk".into(), "second chunk".into()])
            .await
            .unwrap();

        let all = list(&db, profile).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].chunk_count, 2);
        assert_eq!(all[0].license_note.as_deref(), Some("自己買的，不外流"));
    }

    /// 沒有名稱的教材在清單上分不出來，寧可拒絕。
    #[tokio::test]
    async fn a_material_needs_a_title_and_a_language() {
        let (db, profile) = setup().await;
        let bad = NewMaterial {
            title: "   ",
            kind: "text",
            lang: "en",
            source_path: None,
            license_note: None,
        };
        assert!(create(&db, profile, bad, t0()).await.is_err());
    }

    /// 出題要練的字如果在課本裡出現過，就該拿那一段給模型。
    #[tokio::test]
    async fn chunk_selection_prefers_the_words_being_practised() {
        let (db, profile) = setup().await;
        let id = add_material(&db, profile, "en").await;
        let ids = words(&db, "en", &["weather", "reluctant", "market"]).await;
        let chunks = add_chunks(
            &db,
            id,
            &[
                "The weather is nice today.".into(),
                "She was reluctant to answer.".into(),
                "They went to the market.".into(),
            ],
        )
        .await
        .unwrap();
        set_chunk_vocab(
            &db,
            &[
                (chunks[0], vec![(ids[0], 1)]),
                (chunks[1], vec![(ids[1], 1)]),
                (chunks[2], vec![(ids[2], 1)]),
            ],
        )
        .await
        .unwrap();

        let picked = pick_chunk(&db, id, &[ids[1]], 0).await.unwrap().unwrap();
        assert!(picked.contains("reluctant"), "{picked}");
    }

    /// 命中越多要練的字的塊越優先，不是「第一個中的就算」。
    #[tokio::test]
    async fn the_chunk_hitting_the_most_words_wins() {
        let (db, profile) = setup().await;
        let id = add_material(&db, profile, "en").await;
        let ids = words(&db, "en", &["apple", "banana"]).await;
        let chunks = add_chunks(
            &db,
            id,
            &["only apple here".into(), "apple and banana here".into()],
        )
        .await
        .unwrap();
        set_chunk_vocab(
            &db,
            &[
                (chunks[0], vec![(ids[0], 1)]),
                (chunks[1], vec![(ids[0], 1), (ids[1], 1)]),
            ],
        )
        .await
        .unwrap();

        for seed in 0..4 {
            let picked = pick_chunk(&db, id, &[ids[0], ids[1]], seed)
                .await
                .unwrap()
                .unwrap();
            assert!(
                picked.contains("banana"),
                "應該挑命中兩個字的那塊：{picked}"
            );
        }
    }

    /// 詞表存的是原形，所以練 `go` 要找得到寫 `went` 的段落。
    ///
    /// 這正是換掉字面 `LIKE` 的理由——`LIKE '%go%'` 找不到 `went`，
    /// 反而會誤中 `going`、`good`、`ago`。
    #[tokio::test]
    async fn an_inflected_passage_is_found_by_its_base_form() {
        let (db, profile) = setup().await;
        let id = add_material(&db, profile, "en").await;
        let ids = words(&db, "en", &["go", "weather"]).await;
        let chunks = add_chunks(
            &db,
            id,
            &[
                "The weather was good.".into(),
                "They went to school yesterday.".into(),
            ],
        )
        .await
        .unwrap();
        set_chunk_vocab(
            &db,
            &[
                (chunks[0], vec![(ids[1], 1)]),
                (chunks[1], vec![(ids[0], 1)]),
            ],
        )
        .await
        .unwrap();

        let picked = pick_chunk(&db, id, &[ids[0]], 0).await.unwrap().unwrap();
        assert!(
            picked.contains("went"),
            "第二塊裡沒有 go 這個字串，但詞表記的是原形：{picked}"
        );
    }

    /// 一個字都沒中的時候也要給東西，而且要輪流，
    /// 否則課本前三頁被出到爛、後面永遠沒出現過。
    #[tokio::test]
    async fn chunk_selection_rotates_when_nothing_matches() {
        let (db, profile) = setup().await;
        let id = add_material(&db, profile, "en").await;
        add_chunks(&db, id, &["one".into(), "two".into(), "three".into()])
            .await
            .unwrap();

        let mut picked = std::collections::HashSet::new();
        for seed in 0..6 {
            let chunk = pick_chunk(&db, id, &[], seed).await.unwrap().unwrap();
            picked.insert(chunk);
        }
        assert_eq!(picked.len(), 3, "三塊都該輪到：{picked:?}");
    }

    #[tokio::test]
    async fn an_empty_material_yields_no_chunk() {
        let (db, profile) = setup().await;
        let id = add_material(&db, profile, "en").await;
        assert!(pick_chunk(&db, id, &[], 0).await.unwrap().is_none());
    }

    /// 教材是分語言的：日文課本不該出現在英文 profile 的清單上判斷裡。
    #[tokio::test]
    async fn materials_remember_their_language() {
        let (db, profile) = setup().await;
        add_material(&db, profile, "ja").await;
        let all = list(&db, profile).await.unwrap();
        assert_eq!(all[0].lang, "ja");
    }

    #[tokio::test]
    async fn deleting_a_material_takes_its_chunks_with_it() {
        let (db, profile) = setup().await;
        let id = add_material(&db, profile, "en").await;
        add_chunks(&db, id, &["a".into()]).await.unwrap();

        assert!(delete(&db, profile, id).await.unwrap());
        let left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM material_chunk WHERE material_id = ?")
                .bind(id.0)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(left, 0, "外鍵 CASCADE 要真的生效");
    }

    #[tokio::test]
    async fn vocabulary_coverage_counts_known_words() {
        let (db, profile) = setup().await;
        let id = add_material(&db, profile, "en").await;

        let ids = words(&db, "en", &["apple", "banana"]).await;
        let chunks = add_chunks(&db, id, &["apple banana".into(), "apple".into()])
            .await
            .unwrap();
        // 同一個字出現在兩塊裡，整本書的詞數不能重複計算
        set_chunk_vocab(
            &db,
            &[
                (chunks[0], vec![(ids[0], 1), (ids[1], 1)]),
                (chunks[1], vec![(ids[0], 1)]),
            ],
        )
        .await
        .unwrap();

        let (total, known) = coverage(&db, profile, id, 21.0).await.unwrap();
        assert_eq!(total, 2, "apple 出現在兩塊裡，只能算一個字");
        assert_eq!(known, 0, "還沒學過任何一個");
        assert_eq!(list(&db, profile).await.unwrap()[0].vocab_count, 2);
    }
}
