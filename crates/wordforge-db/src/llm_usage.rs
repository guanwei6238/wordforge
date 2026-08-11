//! LLM 用量紀錄。
//!
//! 存在的理由很單純：使用者看不到自己一天燒了多少，就沒辦法判斷
//! 「今天還能不能再練一輪」，也不知道哪一種題型特別貴。
//!
//! ## 為什麼字元數是必填、token 是選填
//!
//! 拿不拿得到 token 取決於後端。HTTP API 的回應裡就有 `usage`；
//! `claude -p --output-format text` 與 `codex exec` 的輸出只有內容，
//! 沒有任何用量資訊。
//!
//! 與其把字元數乘個係數謊報成 token，不如兩個都記、分開顯示：
//! 字元數每個後端都量得到，而且要回答「我今天用了多少」已經夠了。

use serde::Serialize;
use time::OffsetDateTime;

use crate::{Db, Result, ts};
use wordforge_core::model::ProfileId;

/// 一次呼叫要記的東西。
#[derive(Debug, Clone)]
pub struct NewCall<'a> {
    pub model: &'a str,
    /// 這次呼叫在做什麼：`generate` / `grade`
    pub purpose: &'a str,
    pub prompt_chars: i64,
    pub response_chars: i64,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// 失敗的呼叫也要記——重試會燒掉額度，看不到的話用量永遠對不上
    pub ok: bool,
}

pub async fn record(
    db: &Db,
    profile_id: ProfileId,
    call: NewCall<'_>,
    now: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO llm_call
             (profile_id, called_at, model, purpose, prompt_chars, response_chars,
              input_tokens, output_tokens, ok)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(profile_id.0)
    .bind(ts::to_sql(now))
    .bind(call.model)
    .bind(call.purpose)
    .bind(call.prompt_chars)
    .bind(call.response_chars)
    .bind(call.input_tokens)
    .bind(call.output_tokens)
    .bind(i64::from(call.ok))
    .execute(db.pool())
    .await?;
    Ok(())
}

/// 一段期間的用量小結。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageSummary {
    pub calls: i64,
    /// 失敗的次數。重試會燒額度，所以要單獨看得到。
    pub failed: i64,
    pub prompt_chars: i64,
    pub response_chars: i64,
    /// 後端有回報才有值。`None` 代表這段期間沒有任何一次呼叫回報過 token。
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// 有幾次呼叫回報了 token。跟 `calls` 不一樣就代表混用了不同後端。
    pub calls_with_tokens: i64,
}

/// `since` 之後的用量。
pub async fn summary(
    db: &Db,
    profile_id: ProfileId,
    since: OffsetDateTime,
) -> Result<UsageSummary> {
    let row: (i64, i64, i64, i64, Option<i64>, Option<i64>, i64) = sqlx::query_as(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN ok = 0 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(prompt_chars), 0),
                COALESCE(SUM(response_chars), 0),
                SUM(input_tokens),
                SUM(output_tokens),
                COALESCE(SUM(CASE WHEN input_tokens IS NOT NULL
                                    OR output_tokens IS NOT NULL
                                  THEN 1 ELSE 0 END), 0)
         FROM llm_call
         WHERE profile_id = ? AND called_at >= ?",
    )
    .bind(profile_id.0)
    .bind(ts::to_sql(since))
    .fetch_one(db.pool())
    .await?;

    Ok(UsageSummary {
        calls: row.0,
        failed: row.1,
        prompt_chars: row.2,
        response_chars: row.3,
        input_tokens: row.4,
        output_tokens: row.5,
        calls_with_tokens: row.6,
    })
}

/// 依用途拆開，回答「哪一種題型比較貴」。
pub async fn by_purpose(
    db: &Db,
    profile_id: ProfileId,
    since: OffsetDateTime,
) -> Result<Vec<(String, i64, i64)>> {
    Ok(sqlx::query_as(
        "SELECT purpose, COUNT(*), COALESCE(SUM(prompt_chars + response_chars), 0)
         FROM llm_call
         WHERE profile_id = ? AND called_at >= ?
         GROUP BY purpose
         ORDER BY SUM(prompt_chars + response_chars) DESC",
    )
    .bind(profile_id.0)
    .bind(ts::to_sql(since))
    .fetch_all(db.pool())
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::profiles;
    use time::Duration;

    fn t0() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    async fn setup() -> (Db, ProfileId) {
        let db = Db::open_in_memory().await.unwrap();
        let profile = profiles::create(&db, "我", "zh-TW", "en", t0())
            .await
            .unwrap();
        (db, profile)
    }

    fn call(purpose: &str, tokens: Option<(i64, i64)>) -> NewCall<'_> {
        NewCall {
            model: "sonnet",
            purpose,
            prompt_chars: 1_200,
            response_chars: 800,
            input_tokens: tokens.map(|t| t.0),
            output_tokens: tokens.map(|t| t.1),
            ok: true,
        }
    }

    #[tokio::test]
    async fn usage_adds_up() {
        let (db, profile) = setup().await;
        record(&db, profile, call("generate", None), t0())
            .await
            .unwrap();
        record(&db, profile, call("grade", None), t0())
            .await
            .unwrap();

        let s = summary(&db, profile, t0() - Duration::days(1))
            .await
            .unwrap();
        assert_eq!(s.calls, 2);
        assert_eq!(s.prompt_chars, 2_400);
        assert_eq!(s.response_chars, 1_600);
    }

    /// CLI 後端不回報 token，那時要誠實回 None 而不是 0。
    ///
    /// 0 看起來像「沒用到」，而實際上是「量不到」——兩件事差很多。
    #[tokio::test]
    async fn missing_token_counts_stay_missing() {
        let (db, profile) = setup().await;
        record(&db, profile, call("generate", None), t0())
            .await
            .unwrap();

        let s = summary(&db, profile, t0() - Duration::days(1))
            .await
            .unwrap();
        assert_eq!(s.input_tokens, None, "量不到就是 None，不是 0");
        assert_eq!(s.calls_with_tokens, 0);
        assert!(s.prompt_chars > 0, "但字元數每個後端都量得到");
    }

    /// 混用 CLI 與 API 時，要看得出 token 數只涵蓋一部分呼叫。
    #[tokio::test]
    async fn mixed_backends_report_how_many_calls_had_tokens() {
        let (db, profile) = setup().await;
        record(&db, profile, call("generate", None), t0())
            .await
            .unwrap();
        record(&db, profile, call("grade", Some((500, 300))), t0())
            .await
            .unwrap();

        let s = summary(&db, profile, t0() - Duration::days(1))
            .await
            .unwrap();
        assert_eq!(s.calls, 2);
        assert_eq!(s.calls_with_tokens, 1, "只有一次回報了 token");
        assert_eq!(s.input_tokens, Some(500));
    }

    /// 失敗的呼叫也燒額度，不能不算。
    #[tokio::test]
    async fn failed_calls_are_counted_separately() {
        let (db, profile) = setup().await;
        let mut failed = call("generate", None);
        failed.ok = false;
        record(&db, profile, failed, t0()).await.unwrap();

        let s = summary(&db, profile, t0() - Duration::days(1))
            .await
            .unwrap();
        assert_eq!(s.calls, 1);
        assert_eq!(s.failed, 1);
        assert!(s.prompt_chars > 0, "失敗的 prompt 一樣送出去了");
    }

    /// 期間之外的不算，否則「今天用了多少」永遠在漲。
    #[tokio::test]
    async fn only_calls_inside_the_window_count() {
        let (db, profile) = setup().await;
        record(
            &db,
            profile,
            call("generate", None),
            t0() - Duration::days(10),
        )
        .await
        .unwrap();
        record(&db, profile, call("generate", None), t0())
            .await
            .unwrap();

        let s = summary(&db, profile, t0() - Duration::days(1))
            .await
            .unwrap();
        assert_eq!(s.calls, 1);
    }

    #[tokio::test]
    async fn purposes_are_ranked_by_size() {
        let (db, profile) = setup().await;
        record(&db, profile, call("grade", None), t0())
            .await
            .unwrap();
        record(&db, profile, call("generate", None), t0())
            .await
            .unwrap();
        record(&db, profile, call("generate", None), t0())
            .await
            .unwrap();

        let rows = by_purpose(&db, profile, t0() - Duration::days(1))
            .await
            .unwrap();
        assert_eq!(rows[0].0, "generate", "用得最多的排前面");
        assert_eq!(rows[0].1, 2);
    }
}
