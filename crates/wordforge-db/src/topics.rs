//! 情境主題的存取。
//!
//! 出題時用來輪換題材。清單住在資料表而不是程式碼，理由跟 `grammar_def`
//! 一樣：寫死的十二個主題對準備多益的人、對醫生、對想練特定題材的人
//! 都不成立，而且他們改不了。
//!
//! 程式碼只提供種子（[`seed`]），其餘由使用者增刪改。

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{Db, Result, ts};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    #[serde(default)]
    pub id: i64,
    pub lang: String,
    /// 給模型看的主題描述，用母語寫。會直接進 prompt。
    pub text: String,
    /// 適用的題型。**空的表示全部題型都適用**，那是大多數。
    #[serde(default)]
    pub kinds: Vec<String>,
    /// seed（程式碼種子）/ import（匯入）/ manual（自己加）
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub sort_order: i64,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

fn row_to_topic(row: &sqlx::sqlite::SqliteRow) -> Topic {
    use sqlx::Row;
    let kinds: String = row.get("kinds_json");
    Topic {
        id: row.get("id"),
        lang: row.get("lang"),
        text: row.get("text"),
        // 題型壞掉時當成「全部適用」而不是「都不適用」——
        // 後者會讓這個主題從此不再出現，而畫面上完全看不出來
        kinds: serde_json::from_str(&kinds).unwrap_or_default(),
        origin: row.get("origin"),
        sort_order: row.get("sort_order"),
        enabled: row.get::<i64, _>("enabled") != 0,
    }
}

const SELECT_TOPIC: &str =
    "SELECT id, lang, text, kinds_json, origin, sort_order, enabled FROM topic";

/// 某個語言的全部主題，含停用的。設定頁用這個。
pub async fn list(db: &Db, lang: &str) -> Result<Vec<Topic>> {
    let rows = sqlx::query(&format!(
        "{SELECT_TOPIC} WHERE lang = ? ORDER BY sort_order, id"
    ))
    .bind(lang)
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(row_to_topic).collect())
}

/// 出題時可用的主題文字，已排除停用的、並照題型過濾。
///
/// `kinds` 是空的那些一律納入——那表示「什麼題型都適合」。
pub async fn usable(db: &Db, lang: &str, kind: &str) -> Result<Vec<String>> {
    Ok(list(db, lang)
        .await?
        .into_iter()
        .filter(|t| t.enabled)
        .filter(|t| t.kinds.is_empty() || t.kinds.iter().any(|k| k == kind))
        .map(|t| t.text)
        .collect())
}

/// 新增或更新一個主題。回傳它的 id。
///
/// `(lang, text)` 是鍵：同樣的文字重複加不會長出兩筆。用文字當鍵而不是
/// 只靠 id，是因為種子補齊要認得「這個主題已經有了」。
pub async fn upsert(db: &Db, topic: &Topic, now: OffsetDateTime) -> Result<i64> {
    let text = topic.text.trim();
    if text.is_empty() {
        return Err(crate::DbError::Invalid("主題不能是空的".into()));
    }

    let kinds = serde_json::to_string(&topic.kinds).unwrap_or_else(|_| "[]".into());
    let stamp = ts::to_sql(now);

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO topic (lang, text, kinds_json, origin, sort_order, enabled,
                            created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT (lang, text) DO UPDATE SET
             kinds_json = excluded.kinds_json,
             sort_order = excluded.sort_order,
             enabled    = excluded.enabled,
             updated_at = excluded.updated_at
         RETURNING id",
    )
    .bind(&topic.lang)
    .bind(text)
    .bind(&kinds)
    .bind(if topic.origin.is_empty() {
        "manual"
    } else {
        &topic.origin
    })
    .bind(topic.sort_order)
    .bind(i64::from(topic.enabled))
    .bind(&stamp)
    .fetch_one(db.pool())
    .await?;

    Ok(id)
}

/// 改一個主題的文字。
///
/// 分開一個函式而不是靠 [`upsert`]：文字是鍵，upsert 改不了它——
/// 傳新文字進去只會多長一筆，舊的還留著。這個坑不修的話，
/// 使用者按「編輯」存檔後會看到兩個主題。
pub async fn rename(db: &Db, id: i64, text: &str, now: OffsetDateTime) -> Result<bool> {
    let text = text.trim();
    if text.is_empty() {
        return Err(crate::DbError::Invalid("主題不能是空的".into()));
    }
    let affected = sqlx::query("UPDATE topic SET text = ?, updated_at = ? WHERE id = ?")
        .bind(text)
        .bind(ts::to_sql(now))
        .bind(id)
        .execute(db.pool())
        .await?
        .rows_affected();
    Ok(affected > 0)
}

pub async fn delete(db: &Db, lang: &str, id: i64) -> Result<bool> {
    let affected = sqlx::query("DELETE FROM topic WHERE id = ? AND lang = ?")
        .bind(id)
        .bind(lang)
        .execute(db.pool())
        .await?
        .rows_affected();
    Ok(affected > 0)
}

/// 把程式碼裡的種子寫進資料表；種子改版時補上缺的。
///
/// 版號控制與理由同 `grammar::seed_defs`：只補一次，否則使用者刪掉的
/// 主題每次開 App 都會回來。補齊只 INSERT 缺的文字，
/// **既有的一律不動**——使用者可能改過題型、調過順序、把它關掉了。
pub async fn seed(db: &Db, lang: &str, now: OffsetDateTime) -> Result<usize> {
    use wordforge_core::practice::{TOPIC_SEED_VERSION, TOPICS};

    let version_key = format!("topic_seed:{lang}");
    if crate::meta::get_i64(db, &version_key).await? == Some(TOPIC_SEED_VERSION) {
        return Ok(0);
    }

    let existing: Vec<String> = sqlx::query_scalar("SELECT text FROM topic WHERE lang = ?")
        .bind(lang)
        .fetch_all(db.pool())
        .await?;

    let mut written = 0usize;
    for (i, (text, kinds)) in TOPICS.iter().enumerate() {
        if existing.iter().any(|e| e == text) {
            continue;
        }
        upsert(
            db,
            &Topic {
                id: 0,
                lang: lang.to_string(),
                text: (*text).to_string(),
                kinds: kinds.iter().map(|k| (*k).to_string()).collect(),
                origin: "seed".into(),
                sort_order: i as i64,
                enabled: true,
            },
            now,
        )
        .await?;
        written += 1;
    }

    crate::meta::set_i64(db, &version_key, TOPIC_SEED_VERSION).await?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    async fn setup() -> Db {
        Db::open_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn seeding_fills_an_empty_database_once() {
        let db = setup().await;
        let first = seed(&db, "en", t0()).await.unwrap();
        assert!(first >= 10, "種子應該有十幾個，實際 {first}");
        assert_eq!(seed(&db, "en", t0()).await.unwrap(), 0, "不該種第二次");
    }

    /// 這條測試存在的理由：那份清單原本只給閱讀用，翻譯接上來之後
    /// 會拿到「報導一則虛構的地方新聞」當情境——那是體裁不是情境，
    /// 出翻譯題時整題是歪的。
    #[tokio::test]
    async fn article_genres_do_not_reach_translation() {
        let db = setup().await;
        seed(&db, "en", t0()).await.unwrap();

        let reading = usable(&db, "en", "reading").await.unwrap();
        let translation = usable(&db, "en", "translation_to_target").await.unwrap();

        assert!(
            reading.iter().any(|t| t.contains("新聞事件")),
            "閱讀該拿得到體裁類的題材"
        );
        assert!(
            !translation.iter().any(|t| t.contains("新聞事件")),
            "翻譯不該拿到體裁：{translation:?}"
        );
        assert!(
            translation.iter().any(|t| t.contains("旅行")),
            "一般情境兩邊都要有"
        );
    }

    /// 停用是「暫時不要出」，不是刪除：設定頁還看得到，出題撈不到。
    #[tokio::test]
    async fn a_disabled_topic_stays_visible_but_is_not_used() {
        let db = setup().await;
        seed(&db, "en", t0()).await.unwrap();

        let mut all = list(&db, "en").await.unwrap();
        let target = all[0].text.clone();
        all[0].enabled = false;
        upsert(&db, &all[0].clone(), t0()).await.unwrap();

        assert!(
            list(&db, "en")
                .await
                .unwrap()
                .iter()
                .any(|t| t.text == target),
            "停用的主題該還在設定頁上"
        );
        assert!(
            !usable(&db, "en", "reading")
                .await
                .unwrap()
                .contains(&target),
            "停用的主題不該被拿去出題"
        );
    }

    /// 這條測試存在的理由：文字是鍵，用 `upsert` 改文字只會多長一筆，
    /// 舊的還留著——使用者按編輯存檔後會看到兩個主題。
    #[tokio::test]
    async fn renaming_edits_in_place_instead_of_adding_a_second_row() {
        let db = setup().await;
        let id = upsert(
            &db,
            &Topic {
                id: 0,
                lang: "en".into(),
                text: "舊的".into(),
                kinds: Vec::new(),
                origin: "manual".into(),
                sort_order: 0,
                enabled: true,
            },
            t0(),
        )
        .await
        .unwrap();

        assert!(rename(&db, id, "新的", t0()).await.unwrap());

        let all = list(&db, "en").await.unwrap();
        assert_eq!(all.len(), 1, "改名長出第二筆了：{all:?}");
        assert_eq!(all[0].text, "新的");
    }

    /// 刪掉的主題不該在下次啟動時回來。
    #[tokio::test]
    async fn a_deleted_topic_does_not_come_back() {
        let db = setup().await;
        seed(&db, "en", t0()).await.unwrap();

        let victim = list(&db, "en").await.unwrap()[0].clone();
        assert!(delete(&db, "en", victim.id).await.unwrap());

        seed(&db, "en", t0()).await.unwrap();
        assert!(
            !list(&db, "en")
                .await
                .unwrap()
                .iter()
                .any(|t| t.text == victim.text),
            "刪掉的主題又被種回來了"
        );
    }

    /// 空白的主題進了資料表就會變成 prompt 裡的一段空字串。
    #[tokio::test]
    async fn a_blank_topic_is_rejected() {
        let db = setup().await;
        let blank = Topic {
            id: 0,
            lang: "en".into(),
            text: "   ".into(),
            kinds: Vec::new(),
            origin: "manual".into(),
            sort_order: 0,
            enabled: true,
        };
        assert!(upsert(&db, &blank, t0()).await.is_err());
    }
}
