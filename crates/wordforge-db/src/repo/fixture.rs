//! 測試共用的起手式。
//!
//! 三個子模組的測試都要一個空資料庫加一個 profile，重複寫三份
//! 只會讓它們慢慢長歪。

use time::OffsetDateTime;
use wordforge_core::model::{LemmaId, ProfileId};

use crate::Db;
use crate::repo::{NewLemma, cards, lemmas, profiles};
use wordforge_core::model::CardKind;

/// 空資料庫加一個 profile，時間固定在 [`t0`]。
pub async fn setup() -> (Db, ProfileId) {
    let db = Db::open_in_memory().await.unwrap();
    let profile = profiles::create(&db, "我", "zh-TW", "en", t0())
        .await
        .unwrap();
    (db, profile)
}

/// 所有測試共用的固定時刻。真實時鐘會讓排程的斷言時好時壞。
pub fn t0() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

/// 一個詞條，附詞頻——牌組的加入順序看的就是詞頻。
pub async fn add_word(db: &Db, text: &str, freq: i64) -> LemmaId {
    lemmas::upsert(
        db,
        NewLemma {
            lang: "en",
            text,
            pos: "noun",
            freq_rank: Some(freq),
            cefr: None,
        },
    )
    .await
    .unwrap()
}

/// 建立 n 張新卡，詞頻由 1 開始遞增。
pub async fn seed_new_cards(db: &Db, profile: ProfileId, n: i64) {
    for i in 1..=n {
        let word = format!("w{i:04}");
        sqlx::query(
            "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
             VALUES ('en', ?, ?, '', ?, ' zk ')",
        )
        .bind(&word)
        .bind(&word)
        .bind(i)
        .execute(db.pool())
        .await
        .unwrap();
    }
    cards::add_by_tag(
        db,
        profile,
        cards::AddByTag {
            lang: "en",
            tag: "zk",
            kinds: &[CardKind::Recognition],
            limit: n,
            skip_function_words: false,
            min_freq_rank: 0,
            skip_existing: false,
        },
        t0(),
    )
    .await
    .unwrap();
}
