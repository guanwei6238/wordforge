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
    /// 真人錄音的來源網址。匯入時只記網址，實際下載是另一個步驟。
    pub audio_url: Option<&'a str>,
    /// 已下載的本機檔案，相對於 app 資料目錄
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
    /// 分類標籤（`zk`、`cet4`、`oxford3000`…）
    pub tags: Vec<&'a str>,
}

/// 一筆詞條寫進來的時候，要不要先清掉這個來源在同一個 lemma 上的舊資料。
///
/// 這是個 enum 而不是 `bool`，因為兩個模式的正確用法差很多，
/// 而且用錯的後果是**安靜地掉資料**：
///
/// lemma 的鍵是 `(lang, text, pos)`，但一份 dump 裡同一組鍵可以出現好幾次——
/// Wiktionary 的 `cat` 就有多個詞源各自一筆 `pos="noun"`（動物、catapult 的
/// 縮寫、category 的縮寫…）。整批匯入時如果每筆都 `Replace`，
/// 最後處理的那個詞源會把前面所有詞源的釋義刪光。真的發生過：
/// 資料庫裡 `cat` 只剩下一堆縮寫，「貓」整組不見了。
#[derive(Debug)]
pub enum WriteMode<'a> {
    /// 先刪掉這個來源寫在這個 lemma 上的舊釋義與發音，再寫新的。
    ///
    /// 單筆寫入用這個（教材匯入、測試 fixture）。
    Replace,
    /// 整份 dump 匯入。同一個 lemma 在這一輪裡**只清一次**，
    /// 之後遇到的詞源都接在後面。
    ///
    /// `seen` 記的就是「這一輪已經清過哪些 lemma」，
    /// 由呼叫端擁有並跨批次沿用。不用「開始前把整個來源清光」是因為
    /// 那樣中途取消會只剩半份字典；這個做法沒碰到的詞條仍保有舊資料。
    Batch(&'a mut std::collections::HashSet<i64>),
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
/// 對「同一個來源」是冪等的，但冪等的做法由 `mode` 決定，
/// 兩種模式的差別與踩過的坑見 [`WriteMode`]。
/// 兩種模式都**不會**動到其他來源的資料，也不會動到使用者的學習進度。
pub async fn write_entry(
    conn: &mut SqliteConnection,
    source: SourceId,
    entry: &EntryWrite<'_>,
    mode: WriteMode<'_>,
) -> Result<LemmaId> {
    let normalized = wordforge_core::text::normalize(entry.headword);
    // 前後各補一個空白，這樣 LIKE '% zk %' 不會誤中 'zkk'
    let tags = if entry.tags.is_empty() {
        String::new()
    } else {
        format!(" {} ", entry.tags.join(" "))
    };

    let lemma_id: i64 = sqlx::query_scalar(
        "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, cefr, tags)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (lang, text, pos) DO UPDATE SET
             freq_rank = COALESCE(excluded.freq_rank, lemma.freq_rank),
             cefr      = COALESCE(excluded.cefr, lemma.cefr),
             -- 空字串代表這個來源沒有標籤，不該把別的來源標好的洗掉
             tags      = CASE WHEN excluded.tags = '' THEN lemma.tags ELSE excluded.tags END
         RETURNING id",
    )
    .bind(entry.lang)
    .bind(entry.headword)
    .bind(&normalized)
    .bind(entry.pos)
    .bind(entry.freq_rank)
    .bind(entry.cefr)
    .bind(&tags)
    .fetch_one(&mut *conn)
    .await?;

    // Batch 模式下，這一輪第一次碰到這個 lemma 時才清——
    // 之後的詞源都是接在後面。這就是「多詞源不互相洗掉」的機制本身。
    let appending = match mode {
        WriteMode::Replace => false,
        WriteMode::Batch(seen) => !seen.insert(lemma_id),
    };

    if !appending {
        // 先清掉本來源的舊資料，避免重複寫入時釋義越疊越多。
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
    }

    // 接在既有釋義後面時要延續編號，不然第二個詞源的第一條又是 0，
    // 之後 `ORDER BY sort_order` 會把兩個詞源交錯著吐出來。
    let base_order: i64 = if appending {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT MAX(sort_order) FROM sense WHERE lemma_id = ? AND source_id = ?",
        )
        .bind(lemma_id)
        .bind(source.0)
        .fetch_one(&mut *conn)
        .await?
        .map_or(0, |m| m + 1)
    } else {
        0
    };

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
        .bind(base_order + order as i64)
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
        // 接在後面時，同一個詞的每個詞源通常都帶同一組 IPA，
        // 直接插會讓字典頁列出四五個一模一樣的發音。
        // 這張表沒有唯一約束（發音本來就可以有多筆），所以在這裡擋。
        if appending {
            let dup: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM pronunciation
                  WHERE lemma_id = ? AND source_id = ?
                    AND accent IS ? AND ipa IS ?
                  LIMIT 1",
            )
            .bind(lemma_id)
            .bind(source.0)
            .bind(pron.accent)
            .bind(pron.ipa)
            .fetch_optional(&mut *conn)
            .await?;
            if dup.is_some() {
                continue;
            }
        }
        sqlx::query(
            "INSERT INTO pronunciation (lemma_id, source_id, accent, ipa, audio_url,
                                        audio_path, audio_license, is_synthetic)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(lemma_id)
        .bind(source.0)
        .bind(pron.accent)
        .bind(pron.ipa)
        .bind(pron.audio_url)
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
    /// 分類標籤（`zk`、`cet4`…）
    pub tags: Vec<String>,
    /// 這個字是否已經在學習者的牌組裡
    pub in_deck: bool,
}

/// 查字典的 SQL。抽成常數是為了讓測試能對它跑 `EXPLAIN QUERY PLAN`——
/// 這個查詢的重點是「有沒有走索引」，複製一份到測試裡就失去意義了。
const SEARCH_SQL: &str = "WITH matched(id, match_rank) AS (
             SELECT id, 0 FROM lemma WHERE lang = ? AND normalized = ?
             UNION ALL
             SELECT lemma_id, 1 FROM surface_form WHERE lang = ? AND normalized = ?
             UNION ALL
             SELECT id, 2 FROM lemma
               WHERE lang = ? AND normalized >= ? AND normalized < ?
         ),
         best(id, match_rank) AS (
             SELECT id, MIN(match_rank) FROM matched GROUP BY id
         ),
         top(id, match_rank) AS (
             SELECT b.id, b.match_rank
             FROM best b JOIN lemma l ON l.id = b.id
             ORDER BY b.match_rank, l.freq_rank IS NULL, l.freq_rank,
                      length(l.text), l.text
             LIMIT ?
         )
         SELECT l.id, l.text, l.pos, l.freq_rank, l.cefr, l.tags, t.match_rank,
                (SELECT gloss FROM sense WHERE lemma_id = l.id ORDER BY sort_order LIMIT 1) AS gloss,
                (SELECT translation FROM sense WHERE lemma_id = l.id
                   AND translation IS NOT NULL ORDER BY sort_order LIMIT 1) AS translation,
                EXISTS (SELECT 1 FROM card WHERE lemma_id = l.id AND profile_id = ?) AS in_deck
         FROM top t JOIN lemma l ON l.id = t.id
         ORDER BY t.match_rank, l.freq_rank IS NULL, l.freq_rank,
                  length(l.text), l.text";

/// 前綴範圍查詢的上界。
///
/// `U+10FFFF` 是 Unicode 的最大碼位，所以任何以 `prefix` 開頭的字串
/// 都排在 `prefix + U+10FFFF` 之前。用它取代 `LIKE 'prefix%'`，
/// 就能走索引，也不必處理 `%` 與 `_` 的跳脫。
fn prefix_upper_bound(prefix: &str) -> String {
    format!("{prefix}\u{10FFFF}")
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
    let upper = prefix_upper_bound(&normalized);

    // 這個查詢的形狀是為了「能用索引」而長成這樣，不是為了好看：
    //
    // 1. 前綴比對用範圍條件而不是 `LIKE 'x%'`。SQLite 的 LIKE 只在很嚴格的
    //    條件下才會走索引，而且**只要出現 ESCAPE 子句就一定退化成全表掃描**。
    //    77 萬詞條掃一次要 1.5 秒，打字時每個字母都卡一下。
    //    改成 `normalized >= 'run' AND normalized < 'run\u{10FFFF}'` 之後
    //    走的是 idx_lemma_normalized，順便也不用煩惱 `%`、`_` 的跳脫。
    //
    // 2. 三種比對各自成為一個能吃索引的子查詢再 UNION，而不是用 `OR` 串起來。
    //
    // 3. 先排序取前 N 筆（`top`），才去撈釋義。相關子查詢很貴，
    //    只該對真正要顯示的那幾筆執行。
    let rows = sqlx::query(SEARCH_SQL)
        .bind(lang)
        .bind(&normalized)
        .bind(lang)
        .bind(&normalized)
        .bind(lang)
        .bind(&normalized)
        .bind(&upper)
        // 多撈幾倍再去重：同一個字常被不同來源拆成好幾筆詞條
        .bind(limit.saturating_mul(4))
        .bind(profile_id)
        .fetch_all(db.pool())
        .await?;

    // 同一個字只留一筆。ECDICT 不標詞性、Wiktionary 把 run 拆成 noun 與 verb，
    // 直接顯示會變成三筆長得很像的結果。結果已照相關性排序，
    // 留下的第一筆就是最好的代表（有詞頻的來源排在前面）。
    let mut seen = std::collections::HashSet::new();
    let hits = rows
        .into_iter()
        .map(|r| SearchHit {
            lemma_id: r.get("id"),
            text: r.get("text"),
            pos: r.get("pos"),
            freq_rank: r.get("freq_rank"),
            cefr: r.get("cefr"),
            gloss: r.get("gloss"),
            translation: r.get("translation"),
            tags: split_tags(r.get::<String, _>("tags")),
            in_deck: r.get::<i64, _>("in_deck") != 0,
        })
        .filter(|h| seen.insert(wordforge_core::text::normalize(&h.text)))
        .take(limit.max(0) as usize)
        .collect();

    Ok(hits)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SenseView {
    pub gloss: String,
    pub translation: Option<String>,
    pub register: Option<String>,
    pub domain: Option<String>,
    /// 這條釋義所屬詞條的詞性。合併顯示後，用它區分 noun / verb 的釋義。
    pub pos: String,
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
    /// 有網址但沒有 `audio_path`，代表這個字有真人錄音、只是還沒下載
    pub has_audio_url: bool,
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
    pub tags: Vec<String>,
    pub in_deck: bool,
}

fn split_tags(raw: String) -> Vec<String> {
    raw.split_whitespace().map(str::to_string).collect()
}

/// 取得一個詞條的完整內容。
///
/// **會把同一個字的所有詞條合在一起顯示。** 不同來源對詞性的處理不一樣：
/// ECDICT 多半不標詞性，Wiktionary 則把 `run` 拆成 noun 與 verb 兩筆。
/// 資料層分開存是對的（詞性確實不同），但使用者查 `run` 只想看到一頁，
/// 上面同時有中文翻譯和英文定義，而不是兩三筆長得很像的結果。
pub async fn detail(db: &Db, lemma_id: i64, profile_id: i64) -> Result<Option<WordDetail>> {
    // 同一個字的所有詞條（同語言、同正規化拼寫，不分詞性與來源）。
    // 代表詞條選有詞頻的那筆——那通常是資料比較完整的來源。
    let family: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM lemma
         WHERE lang = (SELECT lang FROM lemma WHERE id = ?)
           AND normalized = (SELECT normalized FROM lemma WHERE id = ?)
         ORDER BY freq_rank IS NULL, freq_rank, id",
    )
    .bind(lemma_id)
    .bind(lemma_id)
    .fetch_all(db.pool())
    .await?;

    if family.is_empty() {
        return Ok(None);
    }
    let ids = bind_list(&family);

    let head = sqlx::query(&format!(
        "SELECT l.id, l.text, l.pos, l.freq_rank, l.cefr, l.tags,
                EXISTS (SELECT 1 FROM card WHERE lemma_id IN ({ids}) AND profile_id = ?) AS in_deck
         FROM lemma l WHERE l.id = ?"
    ))
    .bind(profile_id)
    .bind(family[0])
    .fetch_one(db.pool())
    .await?;

    let sense_rows = sqlx::query(&format!(
        "SELECT s.id, s.gloss, s.translation, s.register, s.domain, l.pos, d.attribution
         FROM sense s
           JOIN lemma l ON l.id = s.lemma_id
           LEFT JOIN dict_source d ON d.id = s.source_id
         WHERE s.lemma_id IN ({ids})
         ORDER BY l.freq_rank IS NULL, l.freq_rank, s.lemma_id, s.sort_order, s.id"
    ))
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
            pos: row.get("pos"),
            examples,
            attribution: row.get("attribution"),
        });
    }

    let pronunciations = sqlx::query(&format!(
        "SELECT DISTINCT accent, ipa, audio_path, audio_url, is_synthetic
         FROM pronunciation WHERE lemma_id IN ({ids})"
    ))
    .fetch_all(db.pool())
    .await?
    .into_iter()
    .map(|p| PronunciationView {
        accent: p.get("accent"),
        ipa: p.get("ipa"),
        audio_path: p.get("audio_path"),
        has_audio_url: p.get::<Option<String>, _>("audio_url").is_some(),
        is_synthetic: p.get::<i64, _>("is_synthetic") != 0,
    })
    .collect();

    let forms = sqlx::query(&format!(
        "SELECT DISTINCT form, tag FROM surface_form WHERE lemma_id IN ({ids}) ORDER BY form"
    ))
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
        tags: split_tags(head.get::<String, _>("tags")),
        in_deck: head.get::<i64, _>("in_deck") != 0,
    }))
}

/// 把一串 id 拼成可以直接放進 `IN (...)` 的字面值。
///
/// 這些 id 全部來自資料庫本身（上一個查詢的結果），不是使用者輸入，
/// 所以直接內嵌不會有注入問題；用 bind 反而要動態組出對應數量的 `?`。
fn bind_list(ids: &[i64]) -> String {
    ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// 分級測驗的一題。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlacementItem {
    pub lemma_id: i64,
    pub text: String,
    pub freq_rank: i64,
    /// 對應 [`wordforge_core::placement`] 分層的索引
    pub band_index: usize,
    /// 作答後才顯示，用來讓使用者對照自己判斷得準不準
    pub translation: Option<String>,
}

/// 從每個詞頻層抽幾個字出來當測驗題目。
///
/// 抽樣時排除功能詞與沒有翻譯的詞條：問「你認識 the 嗎」量不到任何東西，
/// 而答完之後要能顯示意思讓使用者對照，沒有翻譯就辦不到。
pub async fn sample_for_placement(
    db: &Db,
    lang: &str,
    bands: &[wordforge_core::placement::FrequencyBand],
    per_band: i64,
) -> Result<Vec<PlacementItem>> {
    let function_words = wordforge_core::wordlist::function_words(lang);
    let exclusion = if function_words.is_empty() {
        String::new()
    } else {
        let quoted: Vec<String> = function_words.iter().map(|w| format!("'{w}'")).collect();
        format!("AND l.normalized NOT IN ({})", quoted.join(","))
    };

    let mut items = Vec::new();
    for (band_index, band) in bands.iter().enumerate() {
        // RANDOM() 在這裡是安全的：freq_rank 範圍先用索引縮到幾千筆，
        // 排序的是那幾千筆而不是整本字典。
        // 過濾條件是實際抽樣之後補上的：原本會抽到 Montgomery、Abu、
        // Englishman 這類專有名詞，還有 B、nov 這種單字母與縮寫。
        // 「你認識 Montgomery 嗎」量不到詞彙量，只會讓估計失準。
        let rows = sqlx::query(&format!(
            "SELECT l.id, l.text, l.freq_rank,
                    (SELECT translation FROM sense
                     WHERE lemma_id = l.id AND translation IS NOT NULL
                     ORDER BY sort_order LIMIT 1) AS translation
             FROM lemma l
             WHERE l.lang = ? AND l.freq_rank BETWEEN ? AND ? {exclusion}
               AND length(l.text) >= 3
               AND l.text = lower(l.text)   -- 大寫開頭幾乎都是專有名詞
               AND l.text NOT LIKE '% %'    -- 片語不適合當單字測驗
               AND l.text NOT LIKE '%.%'    -- 縮寫
               AND EXISTS (SELECT 1 FROM sense
                           WHERE lemma_id = l.id AND translation IS NOT NULL)
             ORDER BY RANDOM()
             LIMIT ?"
        ))
        .bind(lang)
        .bind(band.start_rank)
        .bind(band.end_rank)
        .bind(per_band)
        .fetch_all(db.pool())
        .await?;

        for row in rows {
            items.push(PlacementItem {
                lemma_id: row.get("id"),
                text: row.get("text"),
                freq_rank: row.get("freq_rank"),
                band_index,
                translation: row.get("translation"),
            });
        }
    }

    Ok(items)
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

/// 解析用的字條：一個詞或片語，加上它的釋義。
///
/// 這裡刻意不標「是不是片語」——中日文的片語沒有空格，從字串猜不出來。
/// 呼叫端知道自己是拿單字還是 n-gram 來查的，由它去標。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GlossEntry {
    /// 比對用的鍵值，跟傳進來的 term 相同
    pub term: String,
    /// 字典裡的原始拼寫
    pub text: String,
    /// 目標語言的定義（Wiktionary）
    pub gloss: Option<String>,
    /// 母語翻譯（ECDICT），有的話 UI 優先顯示這個
    pub translation: Option<String>,
}

/// 一次查一批詞的釋義。
///
/// 閱讀解析要標出「這篇文章裡你不會的字」與「這裡有片語」，一篇 300 字的
/// 文章會產生近千個候選（單字 + 2..4 詞的 n-gram）。一個一個查是上千次
/// round-trip，所以這裡用 `IN (...)` 一次撈完。
///
/// 比對用 `normalized` 而不是 `text`：文章裡是 `Searched For`，
/// 字典裡是 `search for`。
pub async fn glossary(db: &Db, lang: &str, terms: &[String]) -> Result<Vec<GlossEntry>> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }

    // SQLite 的參數上限是 32766，分批查才不會在長文章上炸掉
    const CHUNK: usize = 900;
    let mut out = Vec::new();

    for chunk in terms.chunks(CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        // 同一個拼寫在 lemma 表裡通常有好幾列（詞性各一列，大小寫各一列），
        // 這裡要明確挑出「對學習者最有用」的那一列，理由見 `rank` 註解。
        let sql = format!(
            "WITH ranked AS (
                 SELECT l.id, l.normalized, l.text,
                        ROW_NUMBER() OVER (
                            PARTITION BY l.normalized
                            ORDER BY
                                -- 1. 有母語翻譯的排前面。學習者要看的是這個，
                                --    而專有名詞條目幾乎都只有目標語言釋義。
                                NOT EXISTS (SELECT 1 FROM sense s
                                             WHERE s.lemma_id = l.id
                                               AND s.translation IS NOT NULL
                                               AND s.translation <> ''),
                                -- 2. 專有名詞排後面。
                                --    `pos` 沒有跨字典的正規化，這裡的
                                --    'name' 是 Wiktionary/kaikki 的寫法；
                                --    別的字典可能寫 'proper noun'、'propn'
                                --    或什麼都不寫。所以這是**加分項不是保證**，
                                --    認不出來時下一條規則接手。
                                --    留著的理由：日文中文這種沒有大小寫的語言，
                                --    規則 3 完全失效，這是唯一的訊號。
                                l.pos = 'name',
                                -- 3. 拼寫跟正規化形不同的排後面（Straight、CAT）。
                                --    比 lower() 好：正規化是這個專案自己的規則，
                                --    lower() 只處理 ASCII。沒有大小寫的語言
                                --    這條恆為 false，等於少一層過濾而已，
                                --    不會挑錯——這是刻意的降級。
                                l.text <> l.normalized,
                                l.id
                        ) AS rn
                 FROM lemma l
                 WHERE l.lang = ? AND l.normalized IN ({placeholders})
             )
             SELECT r.normalized,
                    r.text,
                    -- 目標語言的定義可能在另一列（翻譯在 ECDICT、定義在
                    -- Wiktionary），所以獨立取，但用同一套排序決定先後。
                    (SELECT s.gloss FROM sense s
                       JOIN ranked g ON g.id = s.lemma_id
                      WHERE g.normalized = r.normalized AND s.gloss_lang = ?
                      ORDER BY g.rn, s.sort_order LIMIT 1),
                    (SELECT s.translation FROM sense s
                      WHERE s.lemma_id = r.id
                        AND s.translation IS NOT NULL AND s.translation <> ''
                      ORDER BY s.sort_order LIMIT 1)
             FROM ranked r
             WHERE r.rn = 1"
        );

        let mut q =
            sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(&sql).bind(lang);
        for term in chunk {
            q = q.bind(term);
        }
        // 綁定順序照 SQL 文字順序：CTE 的 lang、各個 term、gloss 子查詢的 lang
        q = q.bind(lang);

        for (term, text, gloss, translation) in q.fetch_all(db.pool()).await? {
            out.push(GlossEntry {
                term,
                text,
                gloss,
                translation,
            });
        }
    }

    Ok(out)
}

/// 字典裡收了哪些語言，以及各有幾個詞條。
///
/// 這份清單就是「你現在能學哪些語言」——設定頁的目標語言選單直接用它，
/// 使用者不會選到一個沒有字典的語言然後看到空白畫面。
pub async fn languages(db: &Db) -> Result<Vec<(String, i64)>> {
    Ok(
        sqlx::query_as("SELECT lang, COUNT(*) FROM lemma GROUP BY lang ORDER BY COUNT(*) DESC")
            .fetch_all(db.pool())
            .await?,
    )
}

/// 字典裡有沒有東西。
///
/// 開機畫面只需要這一個布林值，但它原本是靠 [`stats`] 取得的——而那個
/// 函式為了「每個來源有幾個詞條」要對 285 萬列的 `sense` 做
/// `COUNT(DISTINCT lemma_id)`，實測 1.3 秒（冷快取更久）。複習頁在那之前
/// 是一片空白，使用者感受到的是「點複習要等十秒」。
///
/// `EXISTS` 一找到第一列就停，實測 0.0 ms。要一個布林值就不要撈統計。
pub async fn has_entries(db: &Db) -> Result<bool> {
    let found: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM lemma LIMIT 1)")
        .fetch_one(db.pool())
        .await?;
    Ok(found != 0)
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
            tags: vec!["zk", "gk"],
        }
    }

    async fn write(db: &Db, source: SourceId, entry: &EntryWrite<'_>) -> LemmaId {
        let mut conn = db.pool().acquire().await.unwrap();
        write_entry(&mut conn, source, entry, WriteMode::Replace)
            .await
            .unwrap()
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
        assert_eq!(d.tags, vec!["zk", "gk"], "考試標籤要能存進去也讀得回來");
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

    /// 查字典會在每次按鍵時執行，全表掃描是不能接受的。
    ///
    /// 這個測試存在的原因很具體：原本的 `LIKE 'x%' ESCAPE '\'` 看起來沒問題，
    /// 但 SQLite 只要看到 ESCAPE 就放棄索引，77 萬詞條要掃 1.5 秒。
    /// 純看程式碼看不出來，只有查詢計畫會說實話。
    #[tokio::test]
    async fn search_never_falls_back_to_a_full_scan() {
        let (db, source, profile) = setup().await;
        write(&db, source, &run_entry()).await;

        let plan: Vec<String> = sqlx::query(&format!("EXPLAIN QUERY PLAN {SEARCH_SQL}"))
            .bind("en")
            .bind("run")
            .bind("en")
            .bind("run")
            .bind("en")
            .bind("run")
            .bind(prefix_upper_bound("run"))
            .bind(10i64)
            .bind(profile)
            .fetch_all(db.pool())
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.get::<String, _>("detail"))
            .collect();

        let plan_text = plan.join("\n");

        // 正向斷言最能抓到迴歸：前綴比對必須是走索引的範圍查詢。
        // 改回 `LIKE 'x%'` 的話這一行就會消失。
        assert!(
            plan.iter()
                .any(|d| d.contains("idx_lemma_normalized") && d.contains("normalized>")),
            "前綴比對沒有走索引範圍查詢：\n{plan_text}"
        );
        // 完全相符與詞形相符也各自要有索引
        assert!(
            plan.iter()
                .any(|d| d.contains("idx_lemma_normalized") && d.contains("normalized=")),
            "完全相符沒有走索引：\n{plan_text}"
        );
        assert!(
            plan.iter()
                .any(|d| d.contains("surface_form") && d.starts_with("SEARCH")),
            "詞形比對沒有走索引：\n{plan_text}"
        );
        // CTE（matched / best / top 與它們的別名）本來就只能掃，
        // 但實體表出現在 SCAN 裡就是出事了
        assert!(
            !plan.iter().any(|d| d.starts_with("SCAN")
                && ["lemma", "surface_form", "sense", "card"]
                    .iter()
                    .any(|t| d.contains(t))),
            "有實體表被全表掃描：\n{plan_text}"
        );
    }

    /// 建立「同一個字被兩個來源分別收錄」的情境：
    /// ECDICT 不標詞性只給中文，Wiktionary 標 verb 並給英文定義。
    async fn two_sources_for_run(db: &Db, ecdict: SourceId) -> LemmaId {
        let wiktionary = upsert_source(
            db,
            NewSource {
                slug: "wiktionary-en",
                name: "Wiktionary",
                license: Some("CC BY-SA 4.0"),
                attribution: Some("Wiktionary contributors"),
                homepage: None,
                version: None,
            },
            t0(),
        )
        .await
        .unwrap();

        // ECDICT 風格：沒有詞性、有詞頻與中文翻譯
        let id = write(
            db,
            ecdict,
            &EntryWrite {
                lang: "en",
                headword: "run",
                pos: "",
                freq_rank: Some(300),
                senses: vec![NewSense {
                    gloss: "v. 跑",
                    gloss_lang: "zh-CN",
                    translation: Some("跑"),
                    ..Default::default()
                }],
                tags: vec!["zk"],
                ..Default::default()
            },
        )
        .await;

        // Wiktionary 風格：有詞性、沒有詞頻、英文定義加例句
        write(
            db,
            wiktionary,
            &EntryWrite {
                lang: "en",
                headword: "run",
                pos: "verb",
                freq_rank: None,
                senses: vec![NewSense {
                    gloss: "To move swiftly on foot",
                    gloss_lang: "en",
                    examples: vec![NewExample {
                        text: "She ran to the station.",
                        translation: None,
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await;

        id
    }

    /// 一個字在清單上只該出現一次，即使資料層拆成好幾筆詞條。
    #[tokio::test]
    async fn search_shows_each_word_once() {
        let (db, ecdict, profile) = setup().await;
        two_sources_for_run(&db, ecdict).await;

        let hits = search(&db, "en", "run", profile, 10).await.unwrap();
        assert_eq!(hits.len(), 1, "同一個字重複出現在結果裡：{hits:?}");
        assert_eq!(
            hits[0].translation.as_deref(),
            Some("跑"),
            "代表詞條應該是資料較完整（有詞頻）的那筆，預覽才有中文"
        );
        assert_eq!(hits[0].tags, vec!["zk"]);
    }

    /// 點開之後要看到全部：中文翻譯、英文定義、例句，並標明各自的詞性。
    #[tokio::test]
    async fn detail_merges_every_source_and_pos() {
        let (db, ecdict, profile) = setup().await;
        let id = two_sources_for_run(&db, ecdict).await;

        let d = detail(&db, id.0, profile).await.unwrap().unwrap();
        assert_eq!(d.text, "run");
        assert_eq!(d.senses.len(), 2, "兩個來源的釋義都要在同一頁");

        assert_eq!(d.senses[0].gloss, "v. 跑", "有詞頻的來源排前面");
        assert_eq!(d.senses[0].pos, "", "ECDICT 這筆沒有詞性");
        assert_eq!(d.senses[1].gloss, "To move swiftly on foot");
        assert_eq!(d.senses[1].pos, "verb", "要看得出這條屬於哪個詞性");
        assert_eq!(d.senses[1].examples[0].text, "She ran to the station.");
    }

    /// 從任何一筆詞條點進去，看到的都該是同一個合併結果。
    #[tokio::test]
    async fn detail_is_the_same_from_any_entry_of_the_word() {
        let (db, ecdict, profile) = setup().await;
        two_sources_for_run(&db, ecdict).await;

        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM lemma WHERE normalized = 'run'")
            .fetch_all(db.pool())
            .await
            .unwrap();
        assert_eq!(ids.len(), 2, "資料層本來就該分開存");

        let a = detail(&db, ids[0], profile).await.unwrap().unwrap();
        let b = detail(&db, ids[1], profile).await.unwrap().unwrap();
        assert_eq!(a, b);
    }

    /// 測驗題目的品質決定估計準不準。
    #[tokio::test]
    async fn placement_sampling_excludes_unsuitable_words() {
        let (db, source, _) = setup().await;

        // 每種都是實際抽樣時真的跑出來過的雜訊
        let candidates = [
            ("water", true),       // 正常的字
            ("Montgomery", false), // 專有名詞
            ("B", false),          // 單字母
            ("a lot of", false),   // 片語
            ("etc.", false),       // 縮寫
            ("the", false),        // 功能詞
        ];
        for (i, (word, _)) in candidates.iter().enumerate() {
            write(
                &db,
                source,
                &EntryWrite {
                    lang: "en",
                    headword: word,
                    pos: "",
                    freq_rank: Some(i as i64 + 1),
                    senses: vec![NewSense {
                        gloss: "意思",
                        gloss_lang: "zh-CN",
                        translation: Some("意思"),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )
            .await;
        }

        let bands = vec![wordforge_core::placement::FrequencyBand {
            start_rank: 1,
            end_rank: 100,
        }];
        let items = sample_for_placement(&db, "en", &bands, 50).await.unwrap();
        let picked: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();

        assert_eq!(
            picked,
            vec!["water"],
            "抽到了不適合當測驗題的詞：{picked:?}"
        );
    }

    /// 沒有翻譯的詞條不能當題目——答完要顯示意思讓使用者對照。
    #[tokio::test]
    async fn placement_sampling_requires_a_translation() {
        let (db, source, _) = setup().await;
        write(
            &db,
            source,
            &EntryWrite {
                lang: "en",
                headword: "obscure",
                pos: "",
                freq_rank: Some(10),
                senses: vec![NewSense {
                    gloss: "only an english definition",
                    gloss_lang: "en",
                    translation: None,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await;

        let bands = vec![wordforge_core::placement::FrequencyBand {
            start_rank: 1,
            end_rank: 100,
        }];
        assert!(
            sample_for_placement(&db, "en", &bands, 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 每一層各抽固定題數，且標上正確的層索引。
    #[tokio::test]
    async fn placement_sampling_covers_every_band() {
        let (db, source, _) = setup().await;
        for rank in [10, 20, 1_500, 1_600, 9_000] {
            write(
                &db,
                source,
                &EntryWrite {
                    lang: "en",
                    headword: &format!("word{rank}"),
                    pos: "",
                    freq_rank: Some(rank),
                    senses: vec![NewSense {
                        gloss: "意思",
                        gloss_lang: "zh-CN",
                        translation: Some("意思"),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )
            .await;
        }

        let bands = wordforge_core::placement::default_bands();
        let items = sample_for_placement(&db, "en", &bands, 1).await.unwrap();

        // 1~500 抽 1、1001~2000 抽 1、8001~16000 抽 1
        assert_eq!(items.len(), 3);
        let band_indexes: Vec<usize> = items.iter().map(|i| i.band_index).collect();
        assert_eq!(band_indexes, vec![0, 2, 5]);
    }

    #[tokio::test]
    async fn detail_returns_none_for_unknown_id() {
        let (db, _, profile) = setup().await;
        assert!(detail(&db, 999, profile).await.unwrap().is_none());
    }

    /// 這條測試存在的理由是它曾經是錯的：lemma 的鍵是 `(lang, text, pos)`，
    /// 但 Wiktionary 的 `cat` 有好幾個詞源各自一筆 `pos="noun"`。
    /// `write_entry` 每一筆都先 `DELETE FROM sense WHERE lemma_id=? AND source_id=?`，
    /// 所以最後處理的那個詞源（catapult、category 那些縮寫）
    /// 把「貓」的義項整組洗掉了。使用者在字典裡查 cat 只看得到一堆縮寫。
    #[tokio::test]
    async fn a_second_etymology_does_not_erase_the_first() {
        let (db, source, _) = setup().await;

        let animal = EntryWrite {
            lang: "en",
            headword: "cat",
            pos: "noun",
            senses: vec![NewSense {
                gloss: "A domesticated feline animal.",
                gloss_lang: "en",
                ..Default::default()
            }],
            ..Default::default()
        };
        let abbreviation = EntryWrite {
            lang: "en",
            headword: "cat",
            pos: "noun",
            senses: vec![NewSense {
                gloss: "Abbreviation of catapult.",
                gloss_lang: "en",
                ..Default::default()
            }],
            ..Default::default()
        };

        let mut conn = db.pool().acquire().await.unwrap();
        let mut seen = std::collections::HashSet::new();
        write_entry(&mut conn, source, &animal, WriteMode::Batch(&mut seen))
            .await
            .unwrap();
        let lemma = write_entry(
            &mut conn,
            source,
            &abbreviation,
            WriteMode::Batch(&mut seen),
        )
        .await
        .unwrap();
        drop(conn);

        let glosses: Vec<String> =
            sqlx::query_scalar("SELECT gloss FROM sense WHERE lemma_id = ? ORDER BY sort_order")
                .bind(lemma.0)
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert_eq!(
            glosses,
            vec![
                "A domesticated feline animal.".to_string(),
                "Abbreviation of catapult.".to_string(),
            ],
            "兩個詞源都要留著，而且照匯入順序排"
        );
    }

    /// `Replace` 仍然要是取代——單筆重寫（教材匯入）靠的就是這個。
    /// 兩個模式的差別是刻意的，不是其中一個沒改到。
    #[tokio::test]
    async fn replace_still_overwrites_what_the_same_source_wrote() {
        let (db, source, _) = setup().await;

        for gloss in ["舊的", "新的"] {
            write(
                &db,
                source,
                &EntryWrite {
                    lang: "en",
                    headword: "cat",
                    pos: "noun",
                    senses: vec![NewSense {
                        gloss,
                        gloss_lang: "en",
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )
            .await;
        }

        let glosses: Vec<String> = sqlx::query_scalar(
            "SELECT s.gloss FROM sense s JOIN lemma l ON l.id = s.lemma_id
              WHERE l.text = 'cat' ORDER BY s.sort_order",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(glosses, vec!["新的".to_string()]);
    }

    /// 同一個詞的每個詞源都會帶同一組 IPA。append 時不擋的話，
    /// 字典頁會列出四五個一模一樣的發音。
    #[tokio::test]
    async fn appending_does_not_pile_up_identical_pronunciations() {
        let (db, source, _) = setup().await;

        let entry = EntryWrite {
            lang: "en",
            headword: "cat",
            pos: "noun",
            senses: vec![NewSense {
                gloss: "g",
                gloss_lang: "en",
                ..Default::default()
            }],
            pronunciations: vec![NewPronunciation {
                accent: Some("uk"),
                ipa: Some("/kæt/"),
                ..Default::default()
            }],
            ..Default::default()
        };

        let mut conn = db.pool().acquire().await.unwrap();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..3 {
            write_entry(&mut conn, source, &entry, WriteMode::Batch(&mut seen))
                .await
                .unwrap();
        }
        drop(conn);

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pronunciation p JOIN lemma l ON l.id = p.lemma_id
              WHERE l.text = 'cat'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    /// 這條測試存在的理由是它曾經是錯的：`glossary` 用
    /// `GROUP BY l.normalized` 配 `MIN(l.text)`，而大寫字母的排序在小寫之前，
    /// 所以 `Straight`（姓氏）永遠贏過 `straight`（直的）。SQLite 的 bare column
    /// 規則讓後面兩個 correlated subquery 也跟著綁到那一列，
    /// 結果是點文章裡的 straight 得到「A surname.」而且完全沒有翻譯。
    /// 真實資料庫裡 bank→「A surname.」、cat→「Central Atlas Tamazight」
    /// 也都是同一個原因。
    #[tokio::test]
    async fn a_proper_noun_never_outranks_the_everyday_word() {
        let (db, source, _) = setup().await;

        // 大寫的姓氏條目：只有目標語言釋義，沒有母語翻譯——跟 Wiktionary 一樣
        write(
            &db,
            source,
            &EntryWrite {
                lang: "en",
                headword: "Straight",
                pos: "name",
                senses: vec![NewSense {
                    gloss: "A surname.",
                    gloss_lang: "en",
                    translation: None,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await;

        write(
            &db,
            source,
            &EntryWrite {
                lang: "en",
                headword: "straight",
                pos: "adj",
                senses: vec![NewSense {
                    gloss: "Without a bend or curve.",
                    gloss_lang: "en",
                    translation: Some("直的"),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await;

        let got = glossary(&db, "en", &["straight".to_string()])
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "straight");
        assert_eq!(got[0].translation.as_deref(), Some("直的"));
    }

    /// 翻譯與目標語言定義常常來自不同的字典（翻譯在 ECDICT、定義在
    /// Wiktionary），而它們是 lemma 表裡不同的兩列。兩邊都要取得到，
    /// 否則有 ECDICT 的人就看不到英文定義。
    #[tokio::test]
    async fn a_translation_and_a_definition_can_come_from_different_entries() {
        let (db, source, _) = setup().await;

        // 只有翻譯，沒有目標語言定義（ECDICT 的形狀：pos 是空的）
        write(
            &db,
            source,
            &EntryWrite {
                lang: "en",
                headword: "cat",
                pos: "",
                senses: vec![NewSense {
                    gloss: "n. 貓",
                    gloss_lang: "zh-TW",
                    translation: Some("n. 貓"),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await;

        // 只有目標語言定義，沒有翻譯（Wiktionary 的形狀）
        write(
            &db,
            source,
            &EntryWrite {
                lang: "en",
                headword: "cat",
                pos: "noun",
                senses: vec![NewSense {
                    gloss: "A domesticated feline animal.",
                    gloss_lang: "en",
                    translation: None,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .await;

        let got = glossary(&db, "en", &["cat".to_string()]).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].translation.as_deref(), Some("n. 貓"));
        assert_eq!(
            got[0].gloss.as_deref(),
            Some("A domesticated feline animal.")
        );
    }

    /// 沒有匯入雙語字典的人（例如學日文只有 Wiktionary）一樣要查得到東西。
    /// 排序的第一條規則整個失效時，後面的規則要接得住。
    #[tokio::test]
    async fn a_dictionary_without_translations_still_picks_the_common_word() {
        let (db, source, _) = setup().await;

        for (headword, pos, gloss) in [
            ("March", "name", "The third month."),
            ("march", "verb", "To walk with regular steps."),
        ] {
            write(
                &db,
                source,
                &EntryWrite {
                    lang: "en",
                    headword,
                    pos,
                    senses: vec![NewSense {
                        gloss,
                        gloss_lang: "en",
                        translation: None,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )
            .await;
        }

        let got = glossary(&db, "en", &["march".to_string()]).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "march");
        assert_eq!(got[0].gloss.as_deref(), Some("To walk with regular steps."));
    }
}
