//! 十萬張卡的熱路徑量測。
//!
//! 這不是單元測試，是拿來回答「牌組很大的時候會不會卡」的。預設 `ignore`，
//! 因為它要建十萬列，跑一次好幾秒。
//!
//! **一定要用 release 跑**：
//!
//! ```bash
//! cargo test --release -p wordforge-db --test perf -- --ignored --nocapture
//! ```
//!
//! debug build 的數字沒有參考價值——同一組查詢在 debug 下是 200~360 ms，
//! release 下是 28~150 ms。使用者拿到的是 release。
//!
//! ## 這條測試抓到過什麼
//!
//! 加 `buried_until` 時把它放進了 `idx_card_due` 的第三欄。它是範圍條件，
//! 一進索引，SQLite 就沒辦法再用索引替 `due` 排序，退化成
//! `USE TEMP B-TREE FOR ORDER BY`——把所有到期的卡撈出來排一遍，
//! `LIMIT` 也救不了。開 App 的第一個查詢因此變成四倍慢。
//!
//! 被埋葬的卡是極少數，當成殘留條件逐列檢查便宜得多。
//!
//! ## 目前的數字（release、十萬張卡）
//!
//! | 查詢 | 耗時 | 在什麼時候跑 |
//! | --- | --- | --- |
//! | `daily_queue` | ~48 ms | 每次開 App |
//! | `queue_status` | ~29 ms | 每次開 App |
//! | `known_lemma_ids` | ~150 ms | 產生閱讀測驗時 |
//! | `bury` | <1 ms | 按 B 鍵 |
//!
//! `known_lemma_ids` 最慢是因為它本來就要回傳七萬多列——那是「他會哪些字」
//! 的完整集合，不是索引沒生效（計畫顯示走的是覆蓋索引）。它只在出題時跑一次，
//! 而那次呼叫本來就要等模型好幾秒。
use std::time::Instant;
use time::OffsetDateTime;
use wordforge_core::model::{CardKind, LemmaId, ProfileId};
use wordforge_db::Db;
use wordforge_db::repo::{cards, profiles};

#[tokio::test]
#[ignore = "要跑就用 release：cargo test --release -p wordforge-db --test perf -- --ignored --nocapture"]
async fn a_hundred_thousand_cards_stay_responsive() {
    let n: i64 = 100_000;
    let t0 = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let db = Db::open_in_memory().await.unwrap();
    let profile = profiles::create(&db, "我", "zh-TW", "en", t0)
        .await
        .unwrap();

    let build = Instant::now();
    let mut tx = db.pool().begin().await.unwrap();
    for i in 0..n {
        sqlx::query(
            "INSERT INTO lemma (id, lang, text, normalized, pos, freq_rank)
             VALUES (?, 'en', 'w' || ?, 'w' || ?, '', ?)",
        )
        .bind(i + 1)
        .bind(i)
        .bind(i)
        .bind(i + 1)
        .execute(&mut *tx)
        .await
        .unwrap();

        // 三成到期、七成還沒到，貼近真實牌組
        let due = if i % 10 < 3 {
            t0 - time::Duration::days(1)
        } else {
            t0 + time::Duration::days(30)
        };
        sqlx::query(
            "INSERT INTO card (profile_id, lemma_id, kind, state, due, stability, difficulty, reps)
             VALUES (?, ?, 'recognition', ?, ?, 30.0, 5.0, 3)",
        )
        .bind(profile.0)
        .bind(i + 1)
        .bind(if i % 4 == 0 { "new" } else { "review" })
        .bind(
            due.format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
        )
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    println!("建 {n} 張卡：{:?}", build.elapsed());

    let mut worst = std::time::Duration::ZERO;
    for (name, elapsed) in [
        ("daily_queue（開 App 第一個查詢）", {
            let t = Instant::now();
            let q = cards::daily_queue(&db, profile, t0, t0, 15, 200)
                .await
                .unwrap();
            assert!(!q.is_empty());
            t.elapsed()
        }),
        ("queue_status（空狀態判斷）", {
            let t = Instant::now();
            cards::queue_status(&db, profile, t0, t0, 15).await.unwrap();
            t.elapsed()
        }),
        ("known_lemma_ids（覆蓋率用）", {
            let t = Instant::now();
            let k = cards::known_lemma_ids(&db, profile, 21.0).await.unwrap();
            assert!(!k.is_empty());
            t.elapsed()
        }),
        ("bury 單張", {
            let t = Instant::now();
            cards::bury(
                &db,
                profile,
                wordforge_core::model::CardId(1),
                t0 + time::Duration::days(1),
            )
            .await
            .unwrap();
            t.elapsed()
        }),
    ] {
        println!("  {name}: {elapsed:?}");
        worst = worst.max(elapsed);
    }

    let _ = (LemmaId(1), CardKind::Recognition, ProfileId(1));
    assert!(
        worst < std::time::Duration::from_millis(200),
        "最慢的查詢 {worst:?} 超過 200 ms，使用者會感覺到卡（記得用 --release 跑）"
    );
}
