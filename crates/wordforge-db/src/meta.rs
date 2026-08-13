//! `app_meta`：跨 profile 的小狀態，記「這個資料庫已經做過什麼」。
//!
//! 刻意做成 key-value 而不是一張有欄位的表：這裡放的是一次性的
//! 遷移標記（種子補到第幾版之類），每加一個就開一次 migration
//! 並不划算，而且它們之間沒有關係，查詢也永遠是照 key 直接取。

use crate::{Db, Result};

pub async fn get(db: &Db, key: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT value FROM app_meta WHERE key = ?")
            .bind(key)
            .fetch_optional(db.pool())
            .await?,
    )
}

/// 數字型的標記。存的是文字，讀不出數字時當成沒有——
/// 手動改壞了一個值不該讓 App 開不起來，重跑一次補齊就好。
pub async fn get_i64(db: &Db, key: &str) -> Result<Option<i64>> {
    Ok(get(db, key).await?.and_then(|v| v.trim().parse().ok()))
}

pub async fn set(db: &Db, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO app_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn set_i64(db: &Db, key: &str, value: i64) -> Result<()> {
    set(db, key, &value.to_string()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_value_survives_a_round_trip_and_overwrites() {
        let db = Db::open_in_memory().await.unwrap();
        assert_eq!(get(&db, "nope").await.unwrap(), None);

        set(&db, "k", "v1").await.unwrap();
        assert_eq!(get(&db, "k").await.unwrap().as_deref(), Some("v1"));

        set(&db, "k", "v2").await.unwrap();
        assert_eq!(
            get(&db, "k").await.unwrap().as_deref(),
            Some("v2"),
            "同一個 key 要覆蓋，不是長出第二筆"
        );
    }

    /// 值壞掉時當成沒有。這種標記用來決定「要不要補一次資料」，
    /// 讀不出來就再補一次——比讓 App 開不起來好。
    #[tokio::test]
    async fn a_broken_number_reads_as_missing() {
        let db = Db::open_in_memory().await.unwrap();
        set(&db, "n", "七").await.unwrap();
        assert_eq!(get_i64(&db, "n").await.unwrap(), None);

        set_i64(&db, "n", 2).await.unwrap();
        assert_eq!(get_i64(&db, "n").await.unwrap(), Some(2));
    }
}
