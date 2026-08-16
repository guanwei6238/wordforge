//! 牌組怎麼組起來：依標籤加字、暫停、擱置、自動補充。
//!
//! 這裡每一個寫入都要能重跑。`add_by_tag` 曾經把已經在複習的卡片
//! 打回新卡——重跑一次就把進度清掉，而畫面上完全看不出來。

use sqlx::Row;
use time::OffsetDateTime;
use wordforge_core::model::{CardId, CardKind, ProfileId};

use crate::ts;
use crate::{Db, Result};

/// [`add_by_tag`] 的參數。
#[derive(Debug, Clone)]
pub struct AddByTag<'a> {
    pub lang: &'a str,
    /// 考試範圍標籤，如 `zk`（國中會考）
    pub tag: &'a str,
    pub kinds: &'a [CardKind],
    /// 最多加入幾個字
    pub limit: i64,
    /// 排除 the / of / and 這類功能詞。除非有特別理由，都該是 `true`。
    pub skip_function_words: bool,
    /// 跳過比這個排名更常用的字。分級測驗的結果會填在這裡，
    /// 學過幾年英文的人不必從第一個字重背。
    pub min_freq_rank: i64,
    /// `limit` 的語意：
    /// - `false`：把這個範圍最常用的 `limit` 個字加進來（已有的會被跳過，
    ///   所以重複執行不會一直長）
    /// - `true`：加入 `limit` 個**還不在牌組裡**的字（補充用）
    pub skip_existing: bool,
}

/// 依標籤批次建卡，例如「把國中範圍的字全部加進牌組」。
///
/// 依詞頻由常用到罕見加入——一次加一千個字，先學到的當然該是常用的那些。
/// 已經在牌組裡的字不會被重置，回傳實際新增的張數。
///
/// `skip_function_words` 預設應該給 `true`：依詞頻排下來，最前面清一色是
/// `the`、`of`、`and`、`I`，做成單字卡學不到東西（理由見
/// [`wordforge_core::wordlist`]）。
pub async fn add_by_tag(
    db: &Db,
    profile_id: ProfileId,
    opts: AddByTag<'_>,
    now: OffsetDateTime,
) -> Result<u64> {
    let AddByTag {
        lang,
        tag,
        kinds,
        limit,
        skip_function_words,
        min_freq_rank,
        skip_existing,
    } = opts;
    // 標籤在資料庫裡存成 " zk gk "，前後補空白比對才不會讓 zk 誤中 zkk
    let pattern = format!("% {} %", tag.trim());
    let due = ts::to_sql(now);

    // 功能詞清單是編譯期常數，不是使用者輸入，直接內嵌成 SQL 字面值。
    // 用 bind 的話得動態組出上百個 `?`，反而更難讀。
    let exclusion = if skip_function_words {
        let list = wordforge_core::wordlist::function_words(lang);
        if list.is_empty() {
            String::new()
        } else {
            let quoted: Vec<String> = list.iter().map(|w| format!("'{w}'")).collect();
            format!("AND normalized NOT IN ({})", quoted.join(","))
        }
    } else {
        String::new()
    };

    let mut added = 0u64;
    let mut tx = db.pool().begin().await?;
    for kind in kinds {
        // 補充模式：先濾掉已經在牌組裡的字，LIMIT 才等於「真正新增幾個」
        let not_in_deck = if skip_existing {
            "AND NOT EXISTS (SELECT 1 FROM card c
                             WHERE c.lemma_id = lemma.id AND c.profile_id = ?3
                               AND c.kind = ?4)"
        } else {
            ""
        };
        // 包一層子查詢有兩個理由：ORDER BY + LIMIT 要作用在挑選而不是插入，
        // 以及 SQLite 的 INSERT...SELECT 接 ON CONFLICT 需要語法上不含糊。
        // 用編號參數而不是裸 `?`：`not_in_deck` 片段會插在中間，
        // 裸問號的順序會跟著條件有沒有出現而改變。
        let res = sqlx::query(&format!(
            "INSERT INTO card (profile_id, lemma_id, kind, state, due)
             SELECT ?3, pick.id, ?4, 'new', ?5
             FROM (
                 SELECT id FROM lemma
                 WHERE lang = ?1 AND ' ' || tags || ' ' LIKE ?2 {exclusion}
                   AND (freq_rank IS NULL OR freq_rank >= ?6)
                   {not_in_deck}
                 ORDER BY freq_rank IS NULL, freq_rank, id
                 LIMIT ?7
             ) AS pick
             WHERE true
             ON CONFLICT (profile_id, lemma_id, kind) DO NOTHING"
        ))
        .bind(lang)
        .bind(&pattern)
        .bind(profile_id.0)
        .bind(kind.as_str())
        .bind(&due)
        .bind(min_freq_rank)
        .bind(limit)
        .execute(&mut *tx)
        .await?;
        added += res.rows_affected();
    }
    tx.commit().await?;

    Ok(added)
}

/// 把「其實早就會」的新卡收起來。
///
/// 分級測驗說使用者大概掌握了前 N 個常用字，但牌組裡可能已經排了一堆
/// 比 N 更常用的字。這些卡直接**暫停**而不是刪除——判斷可能不準，
/// 使用者之後想學隨時可以恢復，複習歷程也不會消失。
///
/// 只動從未複習過的卡，任何已經開始學的進度都保留。
pub async fn suspend_easy_new_cards(
    db: &Db,
    profile_id: ProfileId,
    lang: &str,
    below_rank: i64,
) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE card SET suspended = 1
         WHERE profile_id = ? AND suspended = 0 AND state = 'new' AND reps = 0
           AND lemma_id IN (
               SELECT id FROM lemma
               WHERE lang = ? AND freq_rank IS NOT NULL AND freq_rank < ?
           )",
    )
    .bind(profile_id.0)
    .bind(lang)
    .bind(below_rank)
    .execute(db.pool())
    .await?;
    Ok(res.rows_affected())
}

/// 牌組裡有幾張卡屬於別的語言。
///
/// 換目標語言時要拿這個數字警告使用者：舊卡不會自己消失，
/// 不講的話他明天打開 App 會看到一堆上一個語言的字混在複習裡。
pub async fn count_other_languages(db: &Db, profile_id: ProfileId, lang: &str) -> Result<i64> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM card c JOIN lemma l ON l.id = c.lemma_id
         WHERE c.profile_id = ? AND c.suspended = 0 AND l.lang <> ?",
    )
    .bind(profile_id.0)
    .bind(lang)
    .fetch_one(db.pool())
    .await?;
    Ok(n)
}

/// 把一張卡藏到明天。
///
/// 不動排程：埋葬的意思是「今天不想看到」，不是「我答錯了」。
/// 常見情境是同一個字的另一種卡型剛看過，或這題卡住想先跳過——
/// 兩種都不該影響 FSRS 對記憶強度的估計。
pub async fn bury(
    db: &Db,
    profile_id: ProfileId,
    card_id: CardId,
    until: OffsetDateTime,
) -> Result<bool> {
    let res = sqlx::query("UPDATE card SET buried_until = ? WHERE id = ? AND profile_id = ?")
        .bind(ts::to_sql(until))
        .bind(card_id.0)
        .bind(profile_id.0)
        .execute(db.pool())
        .await?;
    Ok(res.rows_affected() > 0)
}

/// 收起一張卡，要主動恢復才會回來。
pub async fn suspend(db: &Db, profile_id: ProfileId, card_id: CardId) -> Result<bool> {
    let res = sqlx::query("UPDATE card SET suspended = 1 WHERE id = ? AND profile_id = ?")
        .bind(card_id.0)
        .bind(profile_id.0)
        .execute(db.pool())
        .await?;
    Ok(res.rows_affected() > 0)
}

/// 把別的語言的卡片收起來。
///
/// 用 suspend 而不是刪除：使用者可能只是暫時換去學日文，
/// 半年後回來時那些英文卡的複習歷史還在，不必從頭學。
pub async fn suspend_other_languages(db: &Db, profile_id: ProfileId, lang: &str) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE card SET suspended = 1
         WHERE profile_id = ? AND suspended = 0
           AND lemma_id IN (SELECT id FROM lemma WHERE lang <> ?)",
    )
    .bind(profile_id.0)
    .bind(lang)
    .execute(db.pool())
    .await?;
    Ok(res.rows_affected())
}

/// 自動補充設定：牌組見底時要從哪個範圍補、補到剩幾張。
#[derive(Debug, Clone, PartialEq)]
pub struct AutoRefill<'a> {
    /// 從哪個範圍補（`cet4`、`gk`…）
    pub tag: &'a str,
    /// 牌組裡未學的新卡少於這個數量時就補到這個數量
    pub keep_ahead: i64,
    /// 跳過比這更常用的字（分級測驗的結果）
    pub min_freq_rank: i64,
}

/// 需要的話補充牌組，回傳實際加入的張數。
///
/// 「學完了就自己接上新的」是這個 App 該有的行為：使用者的目標是學語言，
/// 不是管理牌組。每次取佇列前檢查一次，成本只有一個 COUNT。
///
/// 補充一樣依詞頻由常用到罕見，也一樣跳過功能詞。
pub async fn refill_if_needed(
    db: &Db,
    profile_id: ProfileId,
    lang: &str,
    cfg: &AutoRefill<'_>,
    now: OffsetDateTime,
) -> Result<u64> {
    let waiting: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM card
         WHERE profile_id = ? AND suspended = 0 AND state = 'new'",
    )
    .bind(profile_id.0)
    .fetch_one(db.pool())
    .await?;

    if waiting >= cfg.keep_ahead {
        return Ok(0);
    }

    add_by_tag(
        db,
        profile_id,
        AddByTag {
            lang,
            tag: cfg.tag,
            kinds: &[CardKind::Recognition],
            // 只補差額，而且是「還不在牌組裡的」那些
            limit: cfg.keep_ahead - waiting,
            skip_function_words: true,
            min_freq_rank: cfg.min_freq_rank,
            skip_existing: true,
        },
        now,
    )
    .await
}

/// 恢復被收起來的卡，最常用的字優先。
///
/// 分級測驗的判斷可能不準，或者使用者就是想把那些字也複習一遍。
/// 卡片當初只是暫停沒有刪除，所以恢復後進度完好。
pub async fn unsuspend(db: &Db, profile_id: ProfileId, count: i64) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE card SET suspended = 0
         WHERE id IN (
             SELECT c.id FROM card c
               JOIN lemma l ON l.id = c.lemma_id
             WHERE c.profile_id = ? AND c.suspended = 1
             ORDER BY l.freq_rank IS NULL, l.freq_rank, c.id
             LIMIT ?
         )",
    )
    .bind(profile_id.0)
    .bind(count)
    .execute(db.pool())
    .await?;
    Ok(res.rows_affected())
}

/// 每個標籤有幾個字、其中幾個已經在牌組裡。
///
/// `min_freq_rank` 是分級測驗給的起點：比這更常用的字不列入計算，
/// 否則牌組頁顯示「國中 1603 字」但實際只能加 870 個，數字對不起來。
pub async fn tag_summary(
    db: &Db,
    profile_id: ProfileId,
    lang: &str,
    min_freq_rank: i64,
) -> Result<Vec<(String, i64, i64)>> {
    // 標籤是空白分隔的字串，SQLite 沒有 split，所以在 Rust 端展開。
    // 標籤種類只有十幾種，詞條數才是大的那一邊，撈回來的資料量不大。
    let rows = sqlx::query(
        "SELECT l.tags,
                COUNT(*) AS total,
                SUM(EXISTS (SELECT 1 FROM card c
                            WHERE c.lemma_id = l.id AND c.profile_id = ?)) AS in_deck
         FROM lemma l
         WHERE l.lang = ? AND l.tags <> ''
           AND (l.freq_rank IS NULL OR l.freq_rank >= ?)
         GROUP BY l.tags",
    )
    .bind(profile_id.0)
    .bind(lang)
    .bind(min_freq_rank)
    .fetch_all(db.pool())
    .await?;

    let mut totals: std::collections::BTreeMap<String, (i64, i64)> = Default::default();
    for row in rows {
        let tags: String = row.get("tags");
        let total: i64 = row.get("total");
        let in_deck: i64 = row.get::<Option<i64>, _>("in_deck").unwrap_or(0);
        for tag in tags.split_whitespace() {
            let e = totals.entry(tag.to_string()).or_insert((0, 0));
            e.0 += total;
            e.1 += in_deck;
        }
    }

    let mut out: Vec<(String, i64, i64)> = totals
        .into_iter()
        .map(|(tag, (total, in_deck))| (tag, total, in_deck))
        .collect();
    // 字多的標籤排前面，那通常是使用者真的會用的範圍
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use time::Duration;

    use crate::repo::fixture::*;
    use crate::repo::{cards, profiles};
    use wordforge_core::model::{CardKind, Rating};
    use wordforge_core::srs::Scheduler;

    /// 「把國中範圍的字加進牌組」是這個 App 最直接的用法之一。
    #[tokio::test]
    async fn add_by_tag_picks_frequent_words_first() {
        let (db, profile) = setup().await;

        // 三個國中字（詞頻不同）與一個高中字
        for (word, freq, tags) in [
            ("rare", 9000, " zk "),
            ("common", 100, " zk "),
            ("middle", 3000, " zk gk "),
            ("advanced", 50, " gk "),
        ] {
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ?)",
            )
            .bind(word)
            .bind(word)
            .bind(freq)
            .bind(tags)
            .execute(db.pool())
            .await
            .unwrap();
        }

        let added = cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 2,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();
        assert_eq!(added, 2);

        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id
             ORDER BY l.freq_rank",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            words,
            vec!["common", "middle"],
            "應該先加常用的字，而且高中字不該混進來"
        );
    }

    /// 重複執行不該把已經在複習的卡片打回新卡。
    #[tokio::test]
    async fn add_by_tag_never_resets_existing_cards() {
        let (db, profile) = setup().await;
        sqlx::query(
            "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
             VALUES ('en', 'apple', 'apple', '', 100, ' zk ')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE card SET state = 'review', stability = 30.0, reps = 5")
            .execute(db.pool())
            .await
            .unwrap();

        let added = cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        assert_eq!(added, 0, "已經在牌組裡的字不該重複加入");
        let (state, reps): (String, i64) = sqlx::query_as("SELECT state, reps FROM card")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(state, "review", "複習進度被重置了");
        assert_eq!(reps, 5);
    }

    /// 依詞頻加入時，最前面清一色是 the / of / and，
    /// 把它們做成單字卡是浪費使用者的時間。
    #[tokio::test]
    async fn add_by_tag_skips_function_words() {
        let (db, profile) = setup().await;
        for (word, freq) in [("the", 1), ("of", 2), ("water", 3), ("i", 4), ("book", 5)] {
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ' zk ')",
            )
            .bind(word)
            .bind(word)
            .bind(freq)
            .execute(db.pool())
            .await
            .unwrap();
        }

        cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: true,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id ORDER BY l.freq_rank",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(words, vec!["water", "book"], "功能詞不該進牌組");
    }

    /// 分級測驗說「你大概會前 2000 個字」，就不該再從第一個字開始排。
    #[tokio::test]
    async fn add_by_tag_can_skip_words_the_learner_already_knows() {
        let (db, profile) = setup().await;
        for (word, freq) in [("easy", 100), ("medium", 2500), ("hard", 9000)] {
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ' zk ')",
            )
            .bind(word)
            .bind(word)
            .bind(freq)
            .execute(db.pool())
            .await
            .unwrap();
        }

        cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 2_000,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id ORDER BY l.freq_rank",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(words, vec!["medium", "hard"], "太簡單的字不該再排進來");
    }

    /// 已經在牌組裡但其實早就會的新卡，應該能一次收起來。
    #[tokio::test]
    async fn easy_new_cards_can_be_suspended_in_bulk() {
        let (db, profile) = setup().await;
        for (word, freq) in [("easy", 100), ("hard", 9000)] {
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ' zk ')",
            )
            .bind(word)
            .bind(word)
            .bind(freq)
            .execute(db.pool())
            .await
            .unwrap();
        }
        cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        // 先讓 easy 有複習紀錄，確認有進度的卡不會被動到
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 10, 100)
            .await
            .unwrap();
        let easy = queue.iter().find(|c| c.lemma_id.0 == 1).unwrap();
        let (next, log) = Scheduler::default().review(easy, Rating::Good, t0(), None);
        cards::record_review(&db, &next, &log).await.unwrap();

        let suspended = cards::suspend_easy_new_cards(&db, profile, "en", 2_000)
            .await
            .unwrap();
        assert_eq!(suspended, 0, "已經開始學的卡不該被收起來");

        // 換一個乾淨的情境：沒複習過的簡單卡
        let (db2, profile2) = setup().await;
        sqlx::query(
            "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
             VALUES ('en', 'easy', 'easy', '', 100, ' zk ')",
        )
        .execute(db2.pool())
        .await
        .unwrap();
        cards::add_by_tag(
            &db2,
            profile2,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        let suspended = cards::suspend_easy_new_cards(&db2, profile2, "en", 2_000)
            .await
            .unwrap();
        assert_eq!(suspended, 1);
        let queue = cards::daily_queue(&db2, profile2, t0(), t0(), 10, 100)
            .await
            .unwrap();
        assert!(queue.is_empty(), "收起來的卡不該再出現在佇列裡");
    }

    /// 標籤比對必須精確：zk 不能命中 zkk。
    #[tokio::test]
    async fn tag_matching_is_exact() {
        let (db, profile) = setup().await;
        sqlx::query(
            "INSERT INTO lemma (lang, text, normalized, pos, tags)
             VALUES ('en', 'trap', 'trap', '', ' zkk ')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let added = cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 10,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();
        assert_eq!(added, 0);
    }

    #[tokio::test]
    async fn tag_summary_counts_words_and_deck_progress() {
        let (db, profile) = setup().await;
        for (word, tags) in [("a1", " zk gk "), ("a2", " zk "), ("a3", " gk ")] {
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', 1, ?)",
            )
            .bind(word)
            .bind(word)
            .bind(tags)
            .execute(db.pool())
            .await
            .unwrap();
        }
        cards::add_by_tag(
            &db,
            profile,
            cards::AddByTag {
                lang: "en",
                tag: "zk",
                kinds: &[CardKind::Recognition],
                limit: 1,
                skip_function_words: false,
                min_freq_rank: 0,
                skip_existing: false,
            },
            t0(),
        )
        .await
        .unwrap();

        let summary = cards::tag_summary(&db, profile, "en", 0).await.unwrap();
        let zk = summary.iter().find(|(t, ..)| t == "zk").unwrap();
        let gk = summary.iter().find(|(t, ..)| t == "gk").unwrap();

        assert_eq!(zk.1, 2, "zk 有兩個字");
        assert_eq!(zk.2, 1, "其中一個已加入牌組");
        assert_eq!(gk.1, 2);
        assert_eq!(gk.2, 1, "同一個字同時屬於 zk 與 gk，兩邊都要算到");
    }

    /// 學完了就該自己接上新的，不必使用者手動去牌組頁補。
    #[tokio::test]
    async fn refill_tops_up_the_deck_when_it_runs_low() {
        let (db, profile) = setup().await;
        // 字典裡有 100 個 cet4 的字
        for i in 1..=100 {
            let word = format!("w{i:04}");
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ' cet4 ')",
            )
            .bind(&word)
            .bind(&word)
            .bind(i)
            .execute(db.pool())
            .await
            .unwrap();
        }

        let cfg = cards::AutoRefill {
            tag: "cet4",
            keep_ahead: 20,
            min_freq_rank: 0,
        };

        // 牌組是空的 → 補到 20 張
        let added = cards::refill_if_needed(&db, profile, "en", &cfg, t0())
            .await
            .unwrap();
        assert_eq!(added, 20);

        // 還很滿 → 不動作
        let added = cards::refill_if_needed(&db, profile, "en", &cfg, t0())
            .await
            .unwrap();
        assert_eq!(added, 0, "牌組還夠的時候不該一直加");

        // 學掉 15 張之後剩 5 張 → 再補回 20
        let scheduler = Scheduler::default();
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 15, 100)
            .await
            .unwrap();
        for card in &queue {
            let (next, log) = scheduler.review(card, Rating::Easy, t0(), None);
            cards::record_review(&db, &next, &log).await.unwrap();
        }

        let added = cards::refill_if_needed(&db, profile, "en", &cfg, t0())
            .await
            .unwrap();
        assert_eq!(added, 15, "補回被學掉的那些");

        let status = cards::queue_status(&db, profile, t0(), t0(), 15)
            .await
            .unwrap();
        assert_eq!(status.new_in_deck, 20);
    }

    /// 補充也要尊重分級測驗的結果，別把已經會的字又塞回來。
    #[tokio::test]
    async fn refill_respects_the_placement_result() {
        let (db, profile) = setup().await;
        for i in 1..=50 {
            let word = format!("w{i:04}");
            sqlx::query(
                "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, tags)
                 VALUES ('en', ?, ?, '', ?, ' cet4 ')",
            )
            .bind(&word)
            .bind(&word)
            .bind(i)
            .execute(db.pool())
            .await
            .unwrap();
        }

        cards::refill_if_needed(
            &db,
            profile,
            "en",
            &cards::AutoRefill {
                tag: "cet4",
                keep_ahead: 10,
                min_freq_rank: 30,
            },
            t0(),
        )
        .await
        .unwrap();

        let min_rank: i64 = sqlx::query_scalar(
            "SELECT MIN(l.freq_rank) FROM card c JOIN lemma l ON l.id = c.lemma_id",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(min_rank, 30, "不該補進比起始詞頻更常用的字");
    }

    /// 被收起來的卡要能恢復，而且從最常用的開始。
    #[tokio::test]
    async fn unsuspend_brings_back_the_most_useful_words_first() {
        let (db, profile) = setup().await;
        seed_new_cards(&db, profile, 20).await;
        cards::suspend_easy_new_cards(&db, profile, "en", 100_000)
            .await
            .unwrap();
        assert_eq!(
            cards::queue_status(&db, profile, t0(), t0(), 15)
                .await
                .unwrap()
                .suspended,
            20
        );

        let restored = cards::unsuspend(&db, profile, 5).await.unwrap();
        assert_eq!(restored, 5);

        // 恢復的應該是詞頻 1~5（seed 依序給 freq_rank 1..n）
        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id
             WHERE c.suspended = 0 ORDER BY l.freq_rank",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(words, vec!["w0001", "w0002", "w0003", "w0004", "w0005"]);

        // 恢復後就能正常排進佇列
        let queue = cards::daily_queue(&db, profile, t0(), t0(), 15, 200)
            .await
            .unwrap();
        assert_eq!(queue.len(), 5);
    }

    /// 埋葬是「今天不想看到」，明天要自己回來。
    #[tokio::test]
    async fn a_buried_card_comes_back_tomorrow() {
        let (db, profile) = setup().await;
        let lemma = add_word(&db, "apple", 1).await;
        let card = cards::ensure(&db, profile, lemma, CardKind::Recognition, t0())
            .await
            .unwrap();

        let tomorrow = t0() + Duration::days(1);
        assert!(
            cards::bury(&db, profile, card.id.unwrap(), tomorrow)
                .await
                .unwrap()
        );

        let today = cards::daily_queue(&db, profile, t0(), t0(), 10, 10)
            .await
            .unwrap();
        assert!(today.is_empty(), "今天不該再出現");

        let later = cards::daily_queue(
            &db,
            profile,
            tomorrow + Duration::minutes(1),
            tomorrow,
            10,
            10,
        )
        .await
        .unwrap();
        assert_eq!(later.len(), 1, "明天要自己回來，不必使用者做任何事");
    }

    /// 埋葬不能動排程——那是「跳過」，不是「答錯」。
    #[tokio::test]
    async fn burying_does_not_touch_the_schedule() {
        let (db, profile) = setup().await;
        let lemma = add_word(&db, "apple", 1).await;
        let card = cards::ensure(&db, profile, lemma, CardKind::Recognition, t0())
            .await
            .unwrap();
        const SCHEDULE: &str = "SELECT due, state, reps FROM card WHERE id = ?";
        let id = card.id.unwrap();

        let before: (String, String, i64) = sqlx::query_as(SCHEDULE)
            .bind(id.0)
            .fetch_one(db.pool())
            .await
            .unwrap();

        cards::bury(&db, profile, id, t0() + Duration::days(1))
            .await
            .unwrap();

        let after: (String, String, i64) = sqlx::query_as(SCHEDULE)
            .bind(id.0)
            .fetch_one(db.pool())
            .await
            .unwrap();

        assert_eq!(after, before, "埋葬只是藏起來，排程一個欄位都不該動");
    }

    /// 暫停跟埋葬的差別就是「會不會自己回來」。
    #[tokio::test]
    async fn a_suspended_card_does_not_come_back_on_its_own() {
        let (db, profile) = setup().await;
        let lemma = add_word(&db, "apple", 1).await;
        let card = cards::ensure(&db, profile, lemma, CardKind::Recognition, t0())
            .await
            .unwrap();

        assert!(
            cards::suspend(&db, profile, card.id.unwrap())
                .await
                .unwrap()
        );

        let much_later = t0() + Duration::days(90);
        assert!(
            cards::daily_queue(&db, profile, much_later, much_later, 10, 10)
                .await
                .unwrap()
                .is_empty(),
            "九十天後也不該自己回來"
        );

        assert_eq!(cards::unsuspend(&db, profile, 10).await.unwrap(), 1);
        assert_eq!(
            cards::daily_queue(&db, profile, much_later, much_later, 10, 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// 別人的卡不能被埋葬或收起來。
    #[tokio::test]
    async fn burying_only_touches_your_own_cards() {
        let (db, profile) = setup().await;
        let other = profiles::create(&db, "他", "zh-TW", "en", t0())
            .await
            .unwrap();
        let lemma = add_word(&db, "apple", 1).await;
        let card = cards::ensure(&db, profile, lemma, CardKind::Recognition, t0())
            .await
            .unwrap();

        assert!(
            !cards::bury(&db, other, card.id.unwrap(), t0() + Duration::days(1))
                .await
                .unwrap()
        );
        assert!(!cards::suspend(&db, other, card.id.unwrap()).await.unwrap());
    }
}
