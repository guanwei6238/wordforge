//! 拿真實資料庫的複本驗證種子補齊。
//!
//! `#[ignore]` 是因為它需要一個實際用過的資料庫，路徑由環境變數給。
//! 這條不會在 CI 跑，用途是改動種子清單之後手動確認：
//!
//! ```bash
//! WORDFORGE_TEST_DB=/path/to/copy.db cargo test -p wordforge-db \
//!     --test seed_upgrade_real -- --ignored --nocapture
//! ```
//!
//! **一定要用複本。** 這條測試會寫入。

use time::OffsetDateTime;
use wordforge_db::{Db, grammar};

#[tokio::test]
#[ignore = "需要真實資料庫的複本，路徑由 WORDFORGE_TEST_DB 指定"]
async fn an_existing_database_receives_the_new_points() {
    let Ok(path) = std::env::var("WORDFORGE_TEST_DB") else {
        panic!("請用 WORDFORGE_TEST_DB 指定資料庫複本的路徑");
    };

    let db = Db::open(&path).await.expect("開不起來");
    let before = grammar::list_defs(&db, "en").await.unwrap();
    println!(
        "補齊前：{} 筆，有等級的 {} 筆",
        before.len(),
        before.iter().filter(|d| d.level.is_some()).count()
    );

    let added = grammar::seed_defs(&db, "en", OffsetDateTime::now_utc())
        .await
        .unwrap();
    let after = grammar::list_defs(&db, "en").await.unwrap();
    println!("補齊 {added} 筆 → 現在 {} 筆", after.len());

    for d in &after {
        if !before.iter().any(|b| b.point == d.point) {
            println!(
                "  新增 {} [{}] {}",
                d.point,
                d.level.as_deref().unwrap_or("-"),
                d.name
            );
        }
    }

    assert!(added > 0, "既有資料庫沒有拿到新的點");
    assert_eq!(
        grammar::seed_defs(&db, "en", OffsetDateTime::now_utc())
            .await
            .unwrap(),
        0,
        "第二次不該再補一次"
    );
}
