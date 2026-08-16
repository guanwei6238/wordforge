//! # wordforge-db
//!
//! 本地 SQLite 儲存層。整個 App 的資料就是一個 `.db` 檔案，
//! 使用者可以直接複製走、放進雲端硬碟同步、或用任何 SQLite 工具檢視。
//!
//! 這一層只負責存取，不放商業邏輯：排程算法在 `wordforge-core`。

pub mod dict;
pub mod exercises;
pub mod grammar;
pub mod llm_usage;
pub mod material;
pub mod meta;
pub mod repo;
pub mod sentences;
pub mod topics;
pub(crate) mod ts;
pub mod word_sentences;

use std::path::Path;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("資料庫操作失敗：{0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("資料庫 migration 失敗：{0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("欄位 {field} 的值無法解析：{value}")]
    Decode { field: &'static str, value: String },

    #[error("找不到 {entity} (id = {id})")]
    NotFound { entity: &'static str, id: i64 },

    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, DbError>;

/// 資料庫連線池。Clone 成本很低，可以隨意傳遞。
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// 開啟（必要時建立）指定路徑的資料庫並套用所有 migration。
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // WAL 讓讀寫不互相阻塞：使用者在複習時，背景匯入字典不會卡住 UI
            .journal_mode(SqliteJournalMode::Wal)
            // 桌面單機情境下 NORMAL 已足夠，且比 FULL 快非常多
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5));

        Self::connect(opts, 4).await
    }

    /// 開啟一個純記憶體資料庫，主要給測試使用。
    pub async fn open_in_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .shared_cache(true)
            .foreign_keys(true);

        // 記憶體資料庫只存在於連線內，連線數必須是 1，否則第二條連線看不到資料表
        Self::connect(opts, 1).await
    }

    async fn connect(opts: SqliteConnectOptions, max_connections: u32) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(opts)
            .await?;

        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    /// 套用 `migrations/` 底下所有尚未執行的 migration。
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 回收空間並更新查詢計劃統計。適合在匯入大量字典資料後呼叫。
    pub async fn optimize(&self) -> Result<()> {
        sqlx::query("PRAGMA optimize").execute(&self.pool).await?;
        sqlx::query("VACUUM").execute(&self.pool).await?;
        Ok(())
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_apply_cleanly() {
        let db = Db::open_in_memory().await.expect("開啟記憶體資料庫");

        let tables: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .fetch_all(db.pool())
                .await
                .expect("列出資料表");
        let names: Vec<&str> = tables.iter().map(|(n,)| n.as_str()).collect();

        for expected in [
            "card",
            "lemma",
            "material",
            "profile",
            "review_log",
            "sense",
            "surface_form",
        ] {
            assert!(
                names.contains(&expected),
                "缺少資料表 {expected}：{names:?}"
            );
        }
    }

    /// 每個會觸發 CASCADE / SET NULL 的外鍵，子表欄位都必須有索引。
    ///
    /// 沒有索引時，刪除父表一列就要把子表整張掃過一次。這在開發期完全看不出來
    /// （子表是空的），但重新匯入一份大字典時會讓程式看起來像當掉——
    /// 實際發生過：七分鐘讀掉 604 GB、資料庫一筆都沒寫進去。
    #[tokio::test]
    async fn every_cascading_foreign_key_is_indexed() {
        use sqlx::Row;

        let db = Db::open_in_memory().await.unwrap();
        let pool = db.pool();

        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations'",
        )
        .fetch_all(pool)
        .await
        .unwrap();

        let mut missing = Vec::new();
        for table in &tables {
            let fks = sqlx::query(&format!("PRAGMA foreign_key_list('{table}')"))
                .fetch_all(pool)
                .await
                .unwrap();

            for fk in fks {
                let on_delete: String = fk.get("on_delete");
                // NO ACTION / RESTRICT 不會去掃子表，不需要索引
                if !matches!(on_delete.as_str(), "CASCADE" | "SET NULL") {
                    continue;
                }
                let column: String = fk.get("from");

                // 只要有任何索引以這個欄位開頭就夠了（含 UNIQUE 產生的自動索引）
                let indexes = sqlx::query(&format!("PRAGMA index_list('{table}')"))
                    .fetch_all(pool)
                    .await
                    .unwrap();
                let mut covered = false;
                for idx in indexes {
                    let name: String = idx.get("name");
                    let first: Option<String> = sqlx::query_scalar(&format!(
                        "SELECT name FROM pragma_index_info('{name}') WHERE seqno = 0"
                    ))
                    .fetch_optional(pool)
                    .await
                    .unwrap()
                    .flatten();
                    if first.as_deref() == Some(column.as_str()) {
                        covered = true;
                        break;
                    }
                }
                if !covered {
                    missing.push(format!("{table}.{column} (ON DELETE {on_delete})"));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "這些外鍵欄位沒有索引，刪除父列時會全表掃描：\n  {}",
            missing.join("\n  ")
        );
    }

    #[tokio::test]
    async fn foreign_keys_are_enforced() {
        let db = Db::open_in_memory().await.unwrap();
        // profile 999 不存在，外鍵應該擋下來
        let res = sqlx::query(
            "INSERT INTO material (profile_id, title, kind, lang, created_at)
             VALUES (999, 't', 'article', 'en', '2026-01-01T00:00:00Z')",
        )
        .execute(db.pool())
        .await;
        assert!(res.is_err(), "外鍵約束沒有生效");
    }
}
