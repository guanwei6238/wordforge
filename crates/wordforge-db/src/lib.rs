//! # wordforge-db
//!
//! 本地 SQLite 儲存層。整個 App 的資料就是一個 `.db` 檔案，
//! 使用者可以直接複製走、放進雲端硬碟同步、或用任何 SQLite 工具檢視。
//!
//! 這一層只負責存取，不放商業邏輯：排程算法在 `wordforge-core`。

pub mod repo;

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
