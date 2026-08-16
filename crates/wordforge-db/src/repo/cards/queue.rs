//! 今天要發幾張卡。
//!
//! 每日上限存在 profile 的設定裡，「再學 10 個」的加碼只算今天——
//! 那個加碼曾經只存在單次回應裡，前端一重新取佇列就回到上限，
//! 按了等於沒按。
//!
//! 佇列空掉的原因要分得出來：學完了跟「整個牌組被分級測驗收起來」
//! 是兩件事，混在一起講會讓使用者以為自己已經學完。

use time::OffsetDateTime;
use wordforge_core::model::{Card, ProfileId};

use super::{NOT_BURIED, SELECT_CARD, row_to_card};
use crate::ts::{self, ParseTs};
use crate::{Db, Result};

/// 今天還可以引入幾張新卡。
///
/// 「今天引入的新卡」定義為 `review_log` 裡 `state = 'new'` 的紀錄——
/// 那是一張卡的第一次複習，之後再怎麼重複都不會再算一次。
pub async fn new_cards_introduced_today(
    db: &Db,
    profile_id: ProfileId,
    day_start: OffsetDateTime,
) -> Result<i64> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT r.card_id)
         FROM review_log r JOIN card c ON c.id = r.card_id
         WHERE c.profile_id = ? AND r.state = 'new' AND r.reviewed_at >= ?",
    )
    .bind(profile_id.0)
    .bind(ts::to_sql(day_start))
    .fetch_one(db.pool())
    .await?;
    Ok(n)
}

/// 今天該做的卡片佇列。
///
/// 這跟單純的「due <= now」不一樣，差別就是這個 App 能不能天天用下去：
///
/// 1. **學習中的卡最優先**：今天剛看過、幾分鐘後要再看一次的卡不能被排到後面，
///    否則當天根本記不起來。
/// 2. **接著是到期的複習卡**：這些是已經投資過的記憶，錯過就要重學。
/// 3. **最後才引入新卡，而且有每日上限**：一次把 1600 個字全設成到期，
///    開啟 App 看到「待複習 1600」只會讓人直接關掉。FSRS 的排程本來就
///    假設新卡是每天少量穩定引入的。
pub async fn daily_queue(
    db: &Db,
    profile_id: ProfileId,
    now: OffsetDateTime,
    day_start: OffsetDateTime,
    new_per_day: i64,
    max_reviews: i64,
) -> Result<Vec<Card>> {
    let now_sql = ts::to_sql(now);

    // 學習中 + 到期複習，先來後到
    let mut queue: Vec<Card> = sqlx::query(&format!(
        "{SELECT_CARD} WHERE profile_id = ? AND suspended = 0 AND {NOT_BURIED}
           AND state <> 'new' AND due <= ?
         ORDER BY due ASC LIMIT ?"
    ))
    .bind(profile_id.0)
    .bind(&now_sql)
    .bind(&now_sql)
    .bind(max_reviews.max(0))
    .fetch_all(db.pool())
    .await?
    .iter()
    .map(row_to_card)
    .collect::<Result<_>>()?;

    let introduced = new_cards_introduced_today(db, profile_id, day_start).await?;
    let remaining = (new_per_day - introduced).max(0);
    if remaining == 0 {
        return Ok(queue);
    }

    // 新卡依詞頻由常用到罕見引入，跟加入牌組時的順序一致
    let new_cards: Vec<Card> = sqlx::query(&format!(
        "{SELECT_CARD} AS c WHERE profile_id = ? AND suspended = 0 AND {NOT_BURIED}
           AND state = 'new' AND due <= ?
         ORDER BY (SELECT freq_rank IS NULL FROM lemma WHERE id = c.lemma_id),
                  (SELECT freq_rank FROM lemma WHERE id = c.lemma_id),
                  c.id
         LIMIT ?"
    ))
    .bind(profile_id.0)
    .bind(&now_sql)
    .bind(&now_sql)
    .bind(remaining)
    .fetch_all(db.pool())
    .await?
    .iter()
    .map(row_to_card)
    .collect::<Result<_>>()?;

    queue.extend(new_cards);
    Ok(queue)
}

/// 佇列的完整狀態。
///
/// 「沒有卡片可做」有好幾種原因，UI 必須分得出來，否則使用者只會看到
/// 「今天的份做完了」而不知道其實整個牌組被收起來了。
#[derive(Debug, Clone, PartialEq)]
pub struct QueueStatus {
    /// 現在到期的複習卡
    pub due_reviews: i64,
    /// 今天還能引入幾張新卡（受每日上限限制）
    pub new_today: i64,
    /// 牌組裡還有幾張沒學過的新卡（不受每日上限限制，不含被收起的）
    pub new_in_deck: i64,
    /// 被收起來的卡（多半來自分級測驗判定「太簡單」）
    pub suspended: i64,
    /// 之後還有卡片要複習時，最近的一張是什麼時候
    pub next_due: Option<OffsetDateTime>,
}

pub async fn queue_status(
    db: &Db,
    profile_id: ProfileId,
    now: OffsetDateTime,
    day_start: OffsetDateTime,
    new_per_day: i64,
) -> Result<QueueStatus> {
    let now_sql = ts::to_sql(now);

    let due_reviews: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM card
         WHERE profile_id = ? AND suspended = 0 AND state <> 'new' AND due <= ?",
    )
    .bind(profile_id.0)
    .bind(&now_sql)
    .fetch_one(db.pool())
    .await?;

    let new_in_deck: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM card
         WHERE profile_id = ? AND suspended = 0 AND state = 'new'",
    )
    .bind(profile_id.0)
    .fetch_one(db.pool())
    .await?;

    let suspended: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE profile_id = ? AND suspended = 1")
            .bind(profile_id.0)
            .fetch_one(db.pool())
            .await?;

    let introduced = new_cards_introduced_today(db, profile_id, day_start).await?;
    let new_today = (new_per_day - introduced).max(0).min(new_in_deck);

    let next_due: Option<String> = sqlx::query_scalar(
        "SELECT MIN(due) FROM card
         WHERE profile_id = ? AND suspended = 0 AND state <> 'new' AND due > ?",
    )
    .bind(profile_id.0)
    .bind(&now_sql)
    .fetch_one(db.pool())
    .await?;

    Ok(QueueStatus {
        due_reviews,
        new_today,
        new_in_deck,
        suspended,
        next_due: next_due.map(|s| s.parse_ts("card.due")).transpose()?,
    })
}

/// 今天的待辦數量：到期複習幾張、還能引入幾張新卡。
pub async fn daily_counts(
    db: &Db,
    profile_id: ProfileId,
    now: OffsetDateTime,
    day_start: OffsetDateTime,
    new_per_day: i64,
) -> Result<(i64, i64)> {
    let reviews: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM card
         WHERE profile_id = ? AND suspended = 0 AND state <> 'new' AND due <= ?",
    )
    .bind(profile_id.0)
    .bind(ts::to_sql(now))
    .fetch_one(db.pool())
    .await?;

    let waiting: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM card
         WHERE profile_id = ? AND suspended = 0 AND state = 'new'",
    )
    .bind(profile_id.0)
    .fetch_one(db.pool())
    .await?;

    let introduced = new_cards_introduced_today(db, profile_id, day_start).await?;
    let new_today = (new_per_day - introduced).max(0).min(waiting);

    Ok((reviews, new_today))
}

#[cfg(test)]
mod tests {
    use time::Duration;

    use crate::repo::cards;
    use crate::repo::fixture::*;
    use wordforge_core::model::{CardState, Rating};
    use wordforge_core::srs::Scheduler;

    /// 一次加一千個字，第一天不該全部湧出來。
    #[tokio::test]
    async fn daily_queue_caps_new_cards() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 50).await;

        let queue = cards::daily_queue(&db, profile, t0(), t0(), 15, 200)
            .await
            .unwrap();

        assert_eq!(queue.len(), 15, "每日新卡上限沒生效");
        // 依詞頻由常用到罕見引入
        let ids: Vec<i64> = queue.iter().map(|c| c.lemma_id.0).collect();
        assert_eq!(ids, (1..=15).collect::<Vec<_>>());
    }

    /// 今天已經引入過的新卡要計入額度，否則關掉再開就能無限刷新。
    #[tokio::test]
    async fn introduced_new_cards_count_against_the_daily_limit() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 50).await;
        let scheduler = Scheduler::default();

        // 學掉 5 張
        let first = cards::daily_queue(&db, profile, t0(), t0(), 15, 200)
            .await
            .unwrap();
        for card in first.iter().take(5) {
            let (next, log) = scheduler.review(card, Rating::Good, t0(), None);
            cards::record_review(&db, &next, &log).await.unwrap();
        }

        assert_eq!(
            cards::new_cards_introduced_today(&db, profile, t0())
                .await
                .unwrap(),
            5
        );

        // 稍後再開，只剩 10 張新卡額度
        let later = t0() + Duration::minutes(30);
        let queue = cards::daily_queue(&db, profile, later, t0(), 15, 200)
            .await
            .unwrap();
        let new_count = queue.iter().filter(|c| c.state == CardState::New).count();
        assert_eq!(new_count, 10, "已引入的新卡沒有計入額度");

        // 隔天額度重置
        let tomorrow = t0() + Duration::days(1);
        let queue = cards::daily_queue(&db, profile, tomorrow, tomorrow, 15, 200)
            .await
            .unwrap();
        let new_count = queue.iter().filter(|c| c.state == CardState::New).count();
        assert_eq!(new_count, 15, "隔天額度應該重置");
    }

    /// 幾分鐘後要再看一次的卡，必須排在新卡前面。
    #[tokio::test]
    async fn learning_cards_come_before_new_ones() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 10).await;
        let scheduler = Scheduler::default();

        // 第一張按 Again，10 分鐘內要再出現
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 5, 200)
            .await
            .unwrap();
        let (next, log) = scheduler.review(&queue[0], Rating::Again, t0(), None);
        cards::record_review(&db, &next, &log).await.unwrap();

        let later = t0() + Duration::minutes(5);
        let queue = cards::daily_queue(&db, profile, later, t0(), 5, 200)
            .await
            .unwrap();

        assert_eq!(queue[0].state, CardState::Learning, "學習中的卡要排最前面");
        assert_eq!(queue[0].lemma_id, next.lemma_id);
    }

    #[tokio::test]
    async fn daily_counts_report_what_is_left_today() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 30).await;

        let (reviews, new_today) = cards::daily_counts(&db, profile, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(reviews, 0, "還沒有任何卡進入複習階段");
        assert_eq!(new_today, 15);

        // 牌組裡只剩 3 張新卡時，不該顯示 15
        let (db2, profile2) = setup().await;
        seed_new_cards(&db2, profile2, 3).await;
        let (_, new_today) = cards::daily_counts(&db2, profile2, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(new_today, 3);
    }

    /// 佇列空掉的原因必須分得出來。
    ///
    /// 實際踩過：分級測驗把整個牌組收起來之後，UI 只說「今天的份做完了」，
    /// 使用者以為是自己學完了，其實是 296 張卡全被暫停。
    #[tokio::test]
    async fn queue_status_tells_apart_the_reasons_for_an_empty_queue() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 30).await;

        // 一、什麼都還沒做：有新卡可學
        let s = cards::queue_status(&db, profile, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(s.new_today, 15);
        assert_eq!(s.new_in_deck, 30);
        assert_eq!(s.suspended, 0);
        assert_eq!(s.next_due, None);

        // 二、今天的額度用完，但牌組裡還有字 → 這才是「做完了」
        let scheduler = Scheduler::default();
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 15, 200)
            .await
            .unwrap();
        for card in &queue {
            let (next, log) = scheduler.review(card, Rating::Easy, t0(), None);
            cards::record_review(&db, &next, &log).await.unwrap();
        }
        let s = cards::queue_status(&db, profile, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(s.new_today, 0, "今天的額度用完了");
        assert_eq!(s.new_in_deck, 15, "但牌組裡還有 15 個字在排隊");
        assert!(s.next_due.is_some(), "已學的卡有下次到期時間");

        // 三、剩下的新卡全被收起來 → 不是「做完了」，是沒東西可做
        cards::suspend_easy_new_cards(&db, profile, "en", 100_000)
            .await
            .unwrap();
        let s = cards::queue_status(&db, profile, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(s.new_in_deck, 0);
        assert_eq!(s.suspended, 15);
    }

    /// 超出每日上限繼續學：今天已引入的數量要算進去，不能重頭來過。
    #[tokio::test]
    async fn studying_more_extends_todays_quota() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 50).await;
        let scheduler = Scheduler::default();

        let first = cards::daily_queue(&db, profile, t0(), t0(), 15, 200)
            .await
            .unwrap();
        for card in &first {
            let (next, log) = scheduler.review(card, Rating::Easy, t0(), None);
            cards::record_review(&db, &next, &log).await.unwrap();
        }

        let introduced = cards::new_cards_introduced_today(&db, profile, t0())
            .await
            .unwrap();
        assert_eq!(introduced, 15);

        // 「再學 10 個」＝ 上限提高到 15 + 10
        let more = cards::daily_queue(&db, profile, t0(), t0(), introduced + 10, 200)
            .await
            .unwrap();
        let new_ones = more.iter().filter(|c| c.state == CardState::New).count();
        assert_eq!(new_ones, 10, "應該剛好多給 10 張，不是重新給 25 張");
    }
}
