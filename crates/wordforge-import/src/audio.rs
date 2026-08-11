//! 下載真人發音音檔。
//!
//! ## 為什麼只下載牌組裡的字
//!
//! 完整的 Wiktionary 音檔集有好幾 GB，但學習者實際會聽到的只有牌組裡那幾百個字。
//! 匯入時只把網址記進資料庫，需要時再針對那些字下載——300 個字大約 10 MB。
//!
//! ## 授權
//!
//! 音檔來自 Wikimedia Commons，多為 CC BY-SA 或 CC0，**逐檔標示**。
//! 每個檔案的授權存在 `pronunciation.audio_license`，UI 顯示時要帶上。
//!
//! Wikimedia 要求所有自動化請求帶有意義的 User-Agent 並註明聯絡方式，
//! 見 <https://meta.wikimedia.org/wiki/User-Agent_policy>。不遵守會被擋。

use std::path::{Path, PathBuf};

use futures::StreamExt;
use serde::Serialize;
use sqlx::Row;
use wordforge_db::Db;

use crate::{ImportError, Result};

/// Wikimedia 的政策要求 UA 能識別是誰、出了問題找得到人。
const USER_AGENT: &str = concat!(
    "Wordforge/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/guanwei6238/wordforge)"
);

/// 同時下載幾個檔案。
///
/// 對 Wikimedia 這種公共資源，開太多並行連線是不禮貌也沒必要的；
/// 4 條就足以在幾分鐘內抓完幾百個字。
const CONCURRENCY: usize = 4;

/// 單一檔案的大小上限。單字發音都是幾十 KB，
/// 超過這個數字代表抓錯東西了。
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AudioProgress {
    pub total: u64,
    pub downloaded: u64,
    pub failed: u64,
    pub skipped: u64,
}

/// 一筆待下載的音檔。
#[derive(Debug, Clone)]
struct Pending {
    pronunciation_id: i64,
    lemma_id: i64,
    url: String,
}

/// 音檔的存放檔名。
///
/// 用 id 而不是原始檔名：Commons 的檔名含空白、括號、非 ASCII 字元，
/// 直接拿來當路徑遲早出事。副檔名保留是為了讓播放器判斷格式。
fn file_name(p: &Pending) -> String {
    let ext = p
        .url
        .rsplit('.')
        .next()
        .filter(|e| matches!(*e, "ogg" | "mp3" | "oga" | "wav" | "opus"))
        .unwrap_or("ogg");
    format!("{}-{}.{ext}", p.lemma_id, p.pronunciation_id)
}

/// 找出牌組裡「有錄音網址、還沒下載」的發音。
async fn pending_for_deck(db: &Db, profile_id: i64, limit: i64) -> Result<Vec<Pending>> {
    let rows = sqlx::query(
        "SELECT p.id, p.lemma_id, p.audio_url
         FROM pronunciation p
         WHERE p.audio_url IS NOT NULL AND p.audio_path IS NULL
           AND EXISTS (SELECT 1 FROM card c
                       WHERE c.lemma_id = p.lemma_id AND c.profile_id = ?)
         -- 一個字可能有好幾種口音，先抓一個就能發音了
         GROUP BY p.lemma_id
         LIMIT ?",
    )
    .bind(profile_id)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| Pending {
            pronunciation_id: r.get("id"),
            lemma_id: r.get("lemma_id"),
            url: r.get("audio_url"),
        })
        .collect())
}

/// 幫牌組裡的字下載發音。
///
/// `dir` 是音檔目錄（通常是 app 資料目錄下的 `audio/`）。
/// 資料庫存的是相對於它的檔名，這樣整個資料夾搬走也不會壞。
pub async fn download_for_deck(
    db: &Db,
    profile_id: i64,
    dir: &Path,
    limit: i64,
    on_progress: impl Fn(AudioProgress) + Send + Sync,
) -> Result<AudioProgress> {
    std::fs::create_dir_all(dir)?;

    let pending = pending_for_deck(db, profile_id, limit).await?;
    let mut progress = AudioProgress {
        total: pending.len() as u64,
        ..Default::default()
    };
    if pending.is_empty() {
        return Ok(progress);
    }

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ImportError::Io(std::io::Error::other(e.to_string())))?;

    let results = futures::stream::iter(pending.into_iter().map(|p| {
        let client = client.clone();
        let dir = dir.to_path_buf();
        async move { (p.clone(), fetch_one(&client, &p, &dir).await) }
    }))
    .buffer_unordered(CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    for (pending, result) in results {
        match result {
            Ok(name) => {
                sqlx::query("UPDATE pronunciation SET audio_path = ? WHERE id = ?")
                    .bind(&name)
                    .bind(pending.pronunciation_id)
                    .execute(db.pool())
                    .await?;
                progress.downloaded += 1;
            }
            Err(e) => {
                // 單一檔案失敗不該中斷整批：Commons 上偶爾有連結失效的條目
                tracing::warn!(url = %pending.url, error = %e, "音檔下載失敗");
                progress.failed += 1;
            }
        }
        on_progress(progress);
    }

    Ok(progress)
}

async fn fetch_one(client: &reqwest::Client, p: &Pending, dir: &Path) -> Result<String> {
    let name = file_name(p);
    let path: PathBuf = dir.join(&name);

    let resp = client
        .get(&p.url)
        .send()
        .await
        .map_err(|e| ImportError::Io(std::io::Error::other(e.to_string())))?;

    if !resp.status().is_success() {
        return Err(ImportError::Io(std::io::Error::other(format!(
            "HTTP {}",
            resp.status()
        ))));
    }
    if let Some(len) = resp.content_length()
        && len > MAX_FILE_BYTES
    {
        return Err(ImportError::Io(std::io::Error::other(format!(
            "檔案過大：{len} bytes"
        ))));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ImportError::Io(std::io::Error::other(e.to_string())))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(ImportError::Io(std::io::Error::other("檔案過大")));
    }

    // 先寫暫存檔再改名：下載到一半被中斷時，不會留下半個檔案讓播放器讀到
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;

    Ok(name)
}

/// 牌組裡有多少字有錄音、已經下載幾個。
pub async fn audio_status(db: &Db, profile_id: i64) -> Result<(i64, i64)> {
    let available: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT p.lemma_id) FROM pronunciation p
         WHERE p.audio_url IS NOT NULL
           AND EXISTS (SELECT 1 FROM card c
                       WHERE c.lemma_id = p.lemma_id AND c.profile_id = ?)",
    )
    .bind(profile_id)
    .fetch_one(db.pool())
    .await?;

    let downloaded: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT p.lemma_id) FROM pronunciation p
         WHERE p.audio_path IS NOT NULL
           AND EXISTS (SELECT 1 FROM card c
                       WHERE c.lemma_id = p.lemma_id AND c.profile_id = ?)",
    )
    .bind(profile_id)
    .fetch_one(db.pool())
    .await?;

    Ok((available, downloaded))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(url: &str) -> Pending {
        Pending {
            pronunciation_id: 7,
            lemma_id: 42,
            url: url.into(),
        }
    }

    /// Commons 的檔名含空白、括號與非 ASCII，不能直接拿來當路徑。
    #[test]
    fn file_names_come_from_ids_not_from_the_url() {
        let p = pending("https://upload.wikimedia.org/…/En-uk-dictionary (RP).ogg");
        let name = file_name(&p);
        assert_eq!(name, "42-7.ogg");
        assert!(!name.contains(' '));
        assert!(!name.contains('/'));
    }

    #[test]
    fn keeps_known_audio_extensions() {
        assert!(file_name(&pending("https://x/a.mp3")).ends_with(".mp3"));
        assert!(file_name(&pending("https://x/a.opus")).ends_with(".opus"));
        // 認不得的副檔名不要照抄，避免 `..%2Fevil.sh` 這種東西進到路徑
        assert!(file_name(&pending("https://x/a.php?x=1")).ends_with(".ogg"));
        assert!(file_name(&pending("https://x/noext")).ends_with(".ogg"));
    }

    /// 路徑穿越：檔名一律由 id 組成，任何 URL 都不可能跳出目錄。
    #[test]
    fn cannot_escape_the_audio_directory() {
        let p = pending("https://x/../../../etc/passwd.ogg");
        let name = file_name(&p);
        assert_eq!(name, "42-7.ogg");
        assert!(!Path::new(&name).is_absolute());
        assert!(!name.contains(".."));
    }

    #[tokio::test]
    async fn nothing_to_download_is_not_an_error() {
        let db = Db::open_in_memory().await.unwrap();
        let dir = std::env::temp_dir().join(format!("wordforge-audio-{}", std::process::id()));
        let p = download_for_deck(&db, 1, &dir, 10, |_| {}).await.unwrap();
        assert_eq!(p.total, 0);
        assert_eq!(p.downloaded, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn status_reports_zero_on_an_empty_deck() {
        let db = Db::open_in_memory().await.unwrap();
        assert_eq!(audio_status(&db, 1).await.unwrap(), (0, 0));
    }
}
