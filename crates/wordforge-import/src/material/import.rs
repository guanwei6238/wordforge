//! 教材匯入流程：抽字 → 切塊 → 建詞表。

use std::path::Path;

use time::OffsetDateTime;
use wordforge_core::model::{LemmaId, ProfileId};
use wordforge_db::Db;
use wordforge_db::material::{self, MaterialId, NewMaterial};
use wordforge_db::repo::lemmas;

use super::{MaterialFormat, chunk, text};
use crate::Result;

/// 匯入教材時的選項。
#[derive(Debug, Clone)]
pub struct MaterialOptions<'a> {
    /// 顯示在清單上的名稱。留空就用檔名。
    pub title: Option<&'a str>,
    /// 教材的語言。**必須傳**，而且該是 profile 的目標語言——
    /// 這是「換一份教材就能學另一種語言」成立的地方。
    pub lang: &'a str,
    /// 使用者自己記的授權備註
    pub license_note: Option<&'a str>,
    /// 格式。`None` 就依副檔名猜。
    pub format: Option<MaterialFormat>,
}

/// 匯入結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct MaterialImport {
    pub material_id: i64,
    /// 抽出來的字元數
    pub chars: u64,
    pub chunks: u64,
    /// 詞表裡有幾個不同的詞（能對到字典的）
    pub vocab: u64,
    /// 在字典裡查不到的詞元數。這個數字大代表字典跟教材的語言對不上。
    pub unmatched_tokens: u64,
}

/// 把一份教材讀進資料庫。
pub async fn import_material(
    db: &Db,
    profile_id: ProfileId,
    path: &Path,
    opts: &MaterialOptions<'_>,
    now: OffsetDateTime,
) -> Result<MaterialImport> {
    let format = opts
        .format
        .unwrap_or_else(|| MaterialFormat::from_path(path));
    let body = text::extract(path, format)?;

    let title = opts
        .title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("未命名教材")
                .to_string()
        });

    let material_id = material::create(
        db,
        profile_id,
        NewMaterial {
            title: &title,
            kind: format.as_str(),
            lang: opts.lang,
            source_path: path.to_str(),
            license_note: opts.license_note,
        },
        now,
    )
    .await?;

    let chunks = chunk::split(&body);
    let written = material::add_chunks(db, material_id, &chunks).await?;

    let (vocab, unmatched) = build_vocab(db, material_id, opts.lang, &body).await?;

    Ok(MaterialImport {
        material_id: material_id.0,
        chars: body.chars().count() as u64,
        chunks: written,
        vocab,
        unmatched_tokens: unmatched,
    })
}

/// 統計教材用到哪些字。
///
/// 用 `lemmas::base_form`：課本裡寫 `ran`、`went`，要記成 `run`、`go`，
/// 否則「這本課本我會幾成」會被詞形變化稀釋掉。
///
/// 這裡刻意不用 `family`（回傳所有可能的 lemma）。判斷「懂不懂」時
/// 多記幾個不會出錯，但詞表是要給人看的數字——`family` 會把 `at`
/// 連到 `@`、`A/T`、`(at)` 這些共用詞形的條目，六十個詞的課文
/// 會統計出一百八十個「字」。
async fn build_vocab(
    db: &Db,
    material_id: MaterialId,
    lang: &str,
    body: &str,
) -> Result<(u64, u64)> {
    use std::collections::HashMap;

    let tokens = wordforge_core::text::tokenize(body);
    let mut counts: HashMap<LemmaId, i64> = HashMap::new();
    // 同一個表面形在一本書裡會出現幾百次，查一次就好
    let mut resolved: HashMap<String, Option<LemmaId>> = HashMap::new();
    let mut unmatched = 0u64;

    for token in &tokens {
        if wordforge_core::wordlist::is_function_word(lang, token) {
            continue;
        }
        let id = match resolved.get(token) {
            Some(id) => *id,
            None => {
                let id = lemmas::base_form(db, lang, token).await?;
                resolved.insert(token.clone(), id);
                id
            }
        };
        match id {
            Some(id) => *counts.entry(id).or_insert(0) += 1,
            None => unmatched += 1,
        }
    }

    let counts: Vec<(LemmaId, i64)> = counts.into_iter().collect();
    let written = material::set_vocab(db, material_id, &counts).await?;
    Ok((written, unmatched))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordforge_db::dict::{EntryWrite, NewSense, NewSource};
    use wordforge_db::repo::profiles;

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    /// 建一個含指定詞條的資料庫。
    async fn setup(lang: &str, words: &[&str]) -> (Db, ProfileId) {
        let db = Db::open_in_memory().await.unwrap();
        let profile = profiles::create(&db, "我", "zh-TW", lang, t0())
            .await
            .unwrap();

        let source = wordforge_db::dict::upsert_source(
            &db,
            NewSource {
                slug: "t",
                name: "測試字典",
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
        for word in words {
            wordforge_db::dict::write_entry(
                &mut conn,
                source,
                &EntryWrite {
                    lang,
                    headword: word,
                    pos: "",
                    senses: vec![NewSense {
                        gloss: "g",
                        gloss_lang: "en",
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        drop(conn);
        (db, profile)
    }

    fn write_temp(name: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wordforge-material-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[tokio::test]
    async fn a_plain_text_book_becomes_chunks_and_vocabulary() {
        let (db, profile) = setup("en", &["weather", "market"]).await;
        let path = write_temp(
            "book.txt",
            "The weather is nice.\n\nThey went to the market.\n",
        );

        let result = import_material(
            &db,
            profile,
            &path,
            &MaterialOptions {
                title: Some("第一冊"),
                lang: "en",
                license_note: Some("自己買的"),
                format: None,
            },
            t0(),
        )
        .await
        .unwrap();

        assert!(result.chunks >= 1);
        assert!(result.vocab >= 2, "weather 與 market 都該進詞表");
        assert!(
            result.vocab <= 6,
            "詞表不該被同形異義的詞條灌水，實際 {}",
            result.vocab
        );

        let all = wordforge_db::material::list(&db, profile).await.unwrap();
        assert_eq!(all[0].title, "第一冊");
        assert_eq!(all[0].kind, "text");
    }

    /// 沒給名稱就用檔名，不要在清單上出現一堆「未命名」。
    #[tokio::test]
    async fn the_file_name_becomes_the_title_when_none_is_given() {
        let (db, profile) = setup("en", &["hello"]).await;
        let path = write_temp("Lesson3.txt", "hello world");

        import_material(
            &db,
            profile,
            &path,
            &MaterialOptions {
                title: None,
                lang: "en",
                license_note: None,
                format: None,
            },
            t0(),
        )
        .await
        .unwrap();

        let all = wordforge_db::material::list(&db, profile).await.unwrap();
        assert_eq!(all[0].title, "Lesson3");
    }

    /// 教材匯入不能只對英文成立。
    #[tokio::test]
    async fn a_japanese_book_builds_its_vocabulary_too() {
        let (db, profile) = setup("ja", &["天気", "市場"]).await;
        let path = write_temp("book_ja.txt", "今日の天気はいい。\n\n市場に行った。\n");

        let result = import_material(
            &db,
            profile,
            &path,
            &MaterialOptions {
                title: Some("日本語の教科書"),
                lang: "ja",
                license_note: None,
                format: None,
            },
            t0(),
        )
        .await
        .unwrap();

        assert!(result.chunks >= 1);
        let all = wordforge_db::material::list(&db, profile).await.unwrap();
        assert_eq!(all[0].lang, "ja");
    }

    /// 字典跟教材語言對不上的時候要看得出來，不要靜靜地建出一個空詞表。
    #[tokio::test]
    async fn a_mismatched_dictionary_shows_up_as_unmatched_tokens() {
        let (db, profile) = setup("en", &["weather"]).await;
        let path = write_temp("mismatch.txt", "Zzzz qqqq wwww vvvv.");

        let result = import_material(
            &db,
            profile,
            &path,
            &MaterialOptions {
                title: Some("對不上的書"),
                lang: "en",
                license_note: None,
                format: None,
            },
            t0(),
        )
        .await
        .unwrap();

        assert_eq!(result.vocab, 0);
        assert!(result.unmatched_tokens >= 4, "查不到的詞元要回報出來");
    }

    /// 課本裡的變化形要記在原形上，否則「這本書我會幾成」會被稀釋。
    #[tokio::test]
    async fn inflections_are_counted_under_their_base_form() {
        let (db, profile) = setup("en", &["go"]).await;
        // 變化形自己也是詞條，詞頻比原形低
        let go = lemmas::base_form(&db, "en", "go").await.unwrap().unwrap();
        lemmas::upsert(
            &db,
            wordforge_db::repo::NewLemma {
                lang: "en",
                text: "went",
                pos: "",
                freq_rank: Some(9_999),
                cefr: None,
            },
        )
        .await
        .unwrap();
        lemmas::add_surface_form(&db, "en", "went", go, "past")
            .await
            .unwrap();
        sqlx::query("UPDATE lemma SET freq_rank = 35 WHERE id = ?")
            .bind(go.0)
            .execute(db.pool())
            .await
            .unwrap();

        let path = write_temp("inflect.txt", "They went there. He will go too.");
        let id = import_material(
            &db,
            profile,
            &path,
            &MaterialOptions {
                title: Some("變化形"),
                lang: "en",
                license_note: None,
                format: None,
            },
            t0(),
        )
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT count FROM material_vocab WHERE material_id = ? AND lemma_id = ?",
        )
        .bind(id.material_id)
        .bind(go.0)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 2, "went 與 go 要算成同一個字");
    }

    #[tokio::test]
    async fn subtitles_lose_their_timecodes_on_the_way_in() {
        let (db, profile) = setup("en", &["hello"]).await;
        let path = write_temp(
            "show.srt",
            "1\n00:00:01,000 --> 00:00:04,000\nHello there.\n",
        );

        import_material(
            &db,
            profile,
            &path,
            &MaterialOptions {
                title: Some("影集"),
                lang: "en",
                license_note: None,
                format: None,
            },
            t0(),
        )
        .await
        .unwrap();

        let all = wordforge_db::material::list(&db, profile).await.unwrap();
        assert_eq!(all[0].kind, "subtitle");

        let chunk = wordforge_db::material::pick_chunk(
            &db,
            wordforge_db::material::MaterialId(all[0].id),
            &[],
            0,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(chunk.contains("Hello there."));
        assert!(!chunk.contains("-->"), "時間軸不該進教材：{chunk}");
    }
}
