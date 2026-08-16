//! 從卡片反推「他會哪些字」。
//!
//! 出題、覆蓋率、生詞挑選全都靠這裡的答案，所以「算不算會」的定義
//! 要跟 prompt 說的一致——曾經 prompt 告訴模型「他掌握約 5200 個單字」，
//! 而這裡只認 stability ≥ 21 天的卡，使用者一張都不到門檻，
//! 覆蓋率永遠是 0%。

use time::OffsetDateTime;
use wordforge_core::model::{LemmaId, ProfileId};

use crate::ts;
use crate::{Db, Result};
use std::collections::HashSet;

/// 這位學習者「算是會了」的字——**嚴格定義**，用於統計顯示。
///
/// 定義：辨識卡已經畢業到長期複習，且 stability 達到門檻
/// （預設 21 天 ≈ 撐得過三週不複習）。
///
/// 這個定義刻意保守，因為「已掌握 N 字」是要給使用者看的成就數字，
/// 寧可低報。**不要拿它做 90% 法則的驗收**——見 [`known_vocabulary`]。
pub async fn known_lemma_ids(
    db: &Db,
    profile_id: ProfileId,
    min_stability: f64,
) -> Result<HashSet<LemmaId>> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT lemma_id FROM card
         WHERE profile_id = ? AND kind = 'recognition' AND state = 'review'
           AND stability >= ?",
    )
    .bind(profile_id.0)
    .bind(min_stability)
    .fetch_all(db.pool())
    .await?;

    Ok(ids.into_iter().map(LemmaId).collect())
}

/// 90% 法則驗收要用的「他看得懂的字」。
///
/// 這跟 [`known_lemma_ids`] 是兩個問題，混用會出大事：
///
/// - 「已掌握 N 字」是成就數字，要保守，只算真的背熟的
/// - 「這篇文章他看不看得懂」要算**全部看得懂的字**，包括從來沒進過
///   牌組、但分級測驗說他早就會的那幾千個
///
/// 混用的後果實測過：使用者背了三週、21 張卡進入複習但最高 stability
/// 只有 15.7，於是嚴格定義回傳 **0 個字**。覆蓋率永遠是 0%，
/// 每一篇文章都被判定太難，重試迴圈每次都跑滿三輪——一題 98 秒，
/// 而且驗收本身完全沒有作用（最後接受的只是「第三篇，不管它是什麼」）。
///
/// 而同一時間 prompt 裡跟模型說的是「他掌握約 5200 個單字」。
/// 出題端與驗收端對「他會什麼」的認知必須一致，否則驗收只是在空轉。
///
/// 所以這裡的定義跟 `known_sample` 的抽樣池對齊：
///
/// 1. 已經進入長期複習的卡（真的背過）
/// 2. 分級測驗判定太簡單而收起來的卡（測驗說他會）
/// 3. 詞頻落在估計詞彙量以內的字（推定會，跟告訴模型的數字同一個依據）
pub async fn known_vocabulary(
    db: &Db,
    profile_id: ProfileId,
    lang: &str,
    vocabulary: i64,
    min_stability: f64,
) -> Result<HashSet<LemmaId>> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT lemma_id FROM card
         WHERE profile_id = ?1 AND kind = 'recognition'
           AND ((state = 'review' AND stability >= ?2)
                OR (suspended = 1 AND reps = 0))
         UNION
         SELECT id FROM lemma
         WHERE lang = ?3 AND freq_rank IS NOT NULL AND freq_rank <= ?4",
    )
    .bind(profile_id.0)
    .bind(min_stability)
    .bind(lang)
    .bind(vocabulary.max(0))
    .fetch_all(db.pool())
    .await?;

    Ok(ids.into_iter().map(LemmaId).collect())
}

/// 學過但快忘掉的字——「不熟」的那批。
///
/// ## 為什麼要單獨挑這些
///
/// 閱讀文章原本只放兩種字：他很熟的（撐起覆蓋率）和全新的（要教的）。
/// 中間那批——學過、但記憶正在衰退——反而沒被用到，而那正是
/// 讀文章最有價值的地方：在上下文裡再遇到一次，比抽卡複習更接近
/// 真正的使用場景，也更容易記住。
///
/// ## 怎麼定義「不熟」
///
/// FSRS 的可提取性 R 是「現在能想起來的機率」：
///
/// ```text
/// R = (1 + FACTOR * 距上次複習天數 / stability) ^ DECAY     DECAY < 0
/// ```
///
/// R 越低越不熟。這裡**不用真的算 R**——`DECAY` 是負的，所以 R 對
/// `距上次複習天數 / stability` 單調遞減，直接照那個比值由大到小排
/// 就是同一個順序。省掉一個 SQLite 不一定有的 `pow()`。
///
/// 跟「今天到期」不一樣：到期只看有沒有跨過門檻，這裡看的是衰退到
/// 什麼程度。逾期三週的字和剛好今天到期的字，前者急迫得多。
pub async fn shaky_words(
    db: &Db,
    profile_id: ProfileId,
    lang: &str,
    now: OffsetDateTime,
    limit: i64,
) -> Result<Vec<String>> {
    let words: Vec<String> = sqlx::query_scalar(
        "SELECT l.text
         FROM card c JOIN lemma l ON l.id = c.lemma_id
         WHERE c.profile_id = ?1 AND c.suspended = 0
           AND l.lang = ?2
           AND c.state IN ('review', 'relearning')
           AND c.stability > 0 AND c.last_review IS NOT NULL
         ORDER BY (julianday(?3) - julianday(c.last_review)) / c.stability DESC
         LIMIT ?4",
    )
    .bind(profile_id.0)
    .bind(lang)
    .bind(ts::to_sql(now))
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    Ok(words)
}

/// 學過、但**做過的句子還很少**的字，句子最少的排前面。
///
/// 「句子」指的是使用者自己做過的（`word_sentence`），不是字典收錄的例句。
/// 一個字複習了幾次卻從來沒在真的句子裡用到，印象是最薄的——出題時
/// 優先把這些字放進可用池，讓模型有機會用上。
///
/// `max_sentences` 是門檻：已經練過那麼多句的字就不必再優先了，
/// 名額該讓給還沒練到的。
///
/// 排序是「句數少的優先，同組隨機」。**同組隨機是必要的**：光按句數排
/// 的話同一批 0 句的字每次都以同樣順序出現，模型跳過的那幾個就永遠輪不到。
pub async fn words_with_few_sentences(
    db: &Db,
    profile_id: ProfileId,
    lang: &str,
    max_sentences: i64,
    limit: i64,
) -> Result<Vec<String>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let words: Vec<String> = sqlx::query_scalar(
        "SELECT l.text
         FROM card c
           JOIN lemma l ON l.id = c.lemma_id
           LEFT JOIN word_sentence w
             ON w.lemma_id = c.lemma_id AND w.profile_id = c.profile_id
         WHERE c.profile_id = ?1 AND c.suspended = 0
           AND l.lang = ?2 AND c.state = 'review'
         GROUP BY c.lemma_id
         HAVING COUNT(w.id) < ?3
         ORDER BY COUNT(w.id), RANDOM()
         LIMIT ?4",
    )
    .bind(profile_id.0)
    .bind(lang)
    .bind(max_sentences)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;
    Ok(words)
}

/// 從「已經學會的字」裡隨機抽幾個。
///
/// 跟 [`shaky_words`] 和 `due_words` 都不一樣：那兩個都是**有順序的**
/// （最可能忘掉的優先、最早到期的優先），所以短時間內連出幾份題目
/// 會拿到同一批字。這個函數存在的唯一理由就是打破那個重複。
///
/// `exclude` 用來避開這次已經選過的字，比對不分大小寫——
/// 卡片存的是字典裡的原始拼寫，兩個來源的大小寫不一定一致。
///
/// 只取 `state = 'review'`：學習中的卡本來就會頻繁出現在複習佇列裡，
/// 再抽到這裡等於重複考同一批。
pub async fn sample_known_words(
    db: &Db,
    profile_id: ProfileId,
    lang: &str,
    exclude: &[String],
    limit: i64,
) -> Result<Vec<String>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }

    // ORDER BY RANDOM() 會掃過符合條件的卡。牌組是使用者自己的規模
    // （數百到數千張），不是字典那 224 萬列，所以掃得起。
    let words: Vec<String> = sqlx::query_scalar(
        "SELECT l.text
         FROM card c JOIN lemma l ON l.id = c.lemma_id
         WHERE c.profile_id = ?1 AND c.suspended = 0
           AND l.lang = ?2 AND c.state = 'review'
         GROUP BY l.text
         ORDER BY RANDOM()
         LIMIT ?3",
    )
    .bind(profile_id.0)
    .bind(lang)
    // 排除是在 Rust 這邊做的，所以要多撈一些才夠扣
    .bind(limit + exclude.len() as i64)
    .fetch_all(db.pool())
    .await?;

    Ok(words
        .into_iter()
        .filter(|w| !exclude.iter().any(|e| e.eq_ignore_ascii_case(w)))
        .take(limit as usize)
        .collect())
}

#[cfg(test)]
mod tests {
    use time::Duration;

    use crate::repo::cards;
    use crate::repo::fixture::*;
    use crate::ts;
    use wordforge_core::model::{CardKind, Rating};
    use wordforge_core::srs::Scheduler;

    #[tokio::test]
    async fn known_words_require_graduated_stable_cards() {
        let (db, profile) = setup().await;
        let word = add_word(&db, "apple", 500).await;
        let card = cards::ensure(&db, profile, word, CardKind::Recognition, t0())
            .await
            .unwrap();

        // 只學了一次、還在 learning：不算會
        let scheduler = Scheduler::default();
        let (after_again, log) = scheduler.review(&card, Rating::Again, t0(), None);
        cards::record_review(&db, &after_again, &log).await.unwrap();
        assert!(
            cards::known_lemma_ids(&db, profile, 21.0)
                .await
                .unwrap()
                .is_empty()
        );

        // 手動拉到高 stability 的 review 狀態：才算會
        sqlx::query("UPDATE card SET state = 'review', stability = 40.0 WHERE id = ?")
            .bind(card.id.unwrap().0)
            .execute(db.pool())
            .await
            .unwrap();
        let known = cards::known_lemma_ids(&db, profile, 21.0).await.unwrap();
        assert!(known.contains(&word));

        // 門檻拉高到 50 天就不算了
        assert!(
            cards::known_lemma_ids(&db, profile, 50.0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 「不熟」看的是衰退到什麼程度，不是有沒有到期。
    ///
    /// 逾期三週的字跟剛好今天到期的字，前者在文章裡再遇到一次的價值高得多。
    #[tokio::test]
    async fn shaky_words_rank_by_how_far_memory_has_decayed() {
        let (db, profile) = setup().await;

        // 三個字，stability 相同但距上次複習差很多
        for (i, days_ago) in [("fresh", 1.0), ("fading", 10.0), ("almost_gone", 60.0)]
            .into_iter()
            .enumerate()
        {
            let (text, ago) = days_ago;
            let lemma = add_word(&db, text, i as i64 + 1).await;
            let card = cards::ensure(&db, profile, lemma, CardKind::Recognition, t0())
                .await
                .unwrap();
            sqlx::query(
                "UPDATE card SET state='review', stability=20.0, difficulty=5.0,
                                 reps=3, last_review=? WHERE id=?",
            )
            .bind(ts::to_sql(
                t0() - Duration::seconds((ago * 86_400.0) as i64),
            ))
            .bind(card.id.unwrap().0)
            .execute(db.pool())
            .await
            .unwrap();
        }

        let shaky = cards::shaky_words(&db, profile, "en", t0(), 10)
            .await
            .unwrap();
        assert_eq!(
            shaky,
            vec!["almost_gone", "fading", "fresh"],
            "最快忘掉的要排最前面"
        );
    }

    /// 這個函數存在的理由是「不重複」，所以測的就是它會不會重複。
    ///
    /// 翻譯題原本全部用 `ORDER BY due` 的字，一天連出幾份會拿到一模一樣的
    /// 單字。這裡驗兩件事：抽到的字會變，而且 `exclude` 真的排除得掉。
    #[tokio::test]
    async fn sampling_known_words_does_not_keep_returning_the_same_ones() {
        let (db, profile) = setup().await;

        for i in 0..20 {
            let lemma = add_word(&db, &format!("word{i}"), i + 1).await;
            let card = cards::ensure(&db, profile, lemma, CardKind::Recognition, t0())
                .await
                .unwrap();
            sqlx::query("UPDATE card SET state='review', stability=50.0, reps=4 WHERE id=?")
                .bind(card.id.unwrap().0)
                .execute(db.pool())
                .await
                .unwrap();
        }

        // 抽很多次，只要不是每次都同一組就達到目的了
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20 {
            let got = cards::sample_known_words(&db, profile, "en", &[], 3)
                .await
                .unwrap();
            assert_eq!(got.len(), 3);
            seen.insert(got);
        }
        assert!(
            seen.len() > 1,
            "20 次抽樣全都一樣，那就跟 ORDER BY due 沒兩樣了"
        );

        // exclude 不分大小寫：卡片存的是字典裡的原始拼寫
        let excluded: Vec<String> = (0..18).map(|i| format!("WORD{i}")).collect();
        let got = cards::sample_known_words(&db, profile, "en", &excluded, 5)
            .await
            .unwrap();
        let mut got = got;
        got.sort();
        assert_eq!(got, vec!["word18", "word19"], "只剩沒被排除的兩個");
    }

    /// 還在學的卡本來就會頻繁出現在複習佇列裡，再抽到翻譯題等於重複考。
    #[tokio::test]
    async fn sampling_known_words_skips_cards_still_being_learned() {
        let (db, profile) = setup().await;

        let learned = add_word(&db, "settled", 1).await;
        let card = cards::ensure(&db, profile, learned, CardKind::Recognition, t0())
            .await
            .unwrap();
        sqlx::query("UPDATE card SET state='review', stability=50.0, reps=4 WHERE id=?")
            .bind(card.id.unwrap().0)
            .execute(db.pool())
            .await
            .unwrap();

        // 新卡與學習中的卡都不該被抽到
        let fresh = add_word(&db, "brandnew", 2).await;
        cards::ensure(&db, profile, fresh, CardKind::Recognition, t0())
            .await
            .unwrap();
        let learning = add_word(&db, "halfway", 3).await;
        let c = cards::ensure(&db, profile, learning, CardKind::Recognition, t0())
            .await
            .unwrap();
        sqlx::query("UPDATE card SET state='learning', reps=1 WHERE id=?")
            .bind(c.id.unwrap().0)
            .execute(db.pool())
            .await
            .unwrap();

        let got = cards::sample_known_words(&db, profile, "en", &[], 10)
            .await
            .unwrap();
        assert_eq!(got, vec!["settled"]);
    }

    /// stability 高的字撐得比較久，同樣隔了三十天也沒那麼急。
    #[tokio::test]
    async fn a_stronger_memory_is_less_shaky_at_the_same_age() {
        let (db, profile) = setup().await;

        for (i, (text, stability)) in [("weak", 5.0), ("strong", 200.0)].into_iter().enumerate() {
            let lemma = add_word(&db, text, i as i64 + 1).await;
            let card = cards::ensure(&db, profile, lemma, CardKind::Recognition, t0())
                .await
                .unwrap();
            sqlx::query(
                "UPDATE card SET state='review', stability=?, difficulty=5.0,
                                 reps=3, last_review=? WHERE id=?",
            )
            .bind(stability)
            .bind(ts::to_sql(t0() - Duration::days(30)))
            .bind(card.id.unwrap().0)
            .execute(db.pool())
            .await
            .unwrap();
        }

        let shaky = cards::shaky_words(&db, profile, "en", t0(), 10)
            .await
            .unwrap();
        assert_eq!(shaky, vec!["weak", "strong"]);
    }

    /// 句子做得少的字要排在前面，做滿門檻的字要整個排除掉。
    ///
    /// 這條測試的重點是**排序**而不只是過濾：一開始只做了「完全沒有例句」
    /// 那個極端，但一個字只練過一句跟從沒練過差別不大，都還沒到
    /// 「在不同情境裡看過」。
    #[tokio::test]
    async fn the_words_with_the_fewest_sentences_come_first() {
        let (db, profile) = setup().await;
        let never = add_word(&db, "reluctant", 100).await;
        let once = add_word(&db, "borrow", 200).await;
        let plenty = add_word(&db, "settle", 300).await;
        for lemma in [never, once, plenty] {
            let card = cards::ensure(&db, profile, lemma, CardKind::Recognition, t0())
                .await
                .unwrap();
            let scheduler = wordforge_core::srs::Scheduler::default();
            let (next, log) = scheduler.review(&card, Rating::Easy, t0(), None);
            cards::record_review(&db, &next, &log).await.unwrap();
        }

        let exercise = crate::exercises::create(
            &db,
            crate::exercises::NewExercise {
                profile_id: profile,
                kind: "translation_to_target",
                payload_json: "{}",
                target_words: &[],
                coverage: None,
                model: None,
                material_id: None,
                topic: None,
            },
            t0(),
        )
        .await
        .unwrap();
        // 句子內容要不一樣：`word_sentence` 的唯一鍵含 `text`，
        // 三句寫同一句話會被折成一句，這條測試就驗不到門檻了。
        let sentence = |lemma: wordforge_core::LemmaId, index: i64| {
            let db = &db;
            let text = format!("A sentence number {index}.");
            async move {
                crate::word_sentences::record(
                    db,
                    crate::word_sentences::NewSentence {
                        profile_id: profile,
                        lemma_id: lemma,
                        exercise_id: exercise.0,
                        text: &text,
                        translation: None,
                        origin: "translation",
                        item_index: Some(index),
                    },
                    t0(),
                )
                .await
                .unwrap();
            }
        };
        sentence(once, 0).await;
        for index in 1..4 {
            sentence(plenty, index).await;
        }

        let got = cards::words_with_few_sentences(&db, profile, "en", 3, 10)
            .await
            .unwrap();
        assert_eq!(
            got,
            vec!["reluctant".to_string(), "borrow".to_string()],
            "一句都沒有的要排在只有一句的前面，做滿 3 句的不該再佔名額"
        );
    }

    /// 沒學過的新卡不算「不熟」——那是「不會」，屬於生詞白名單那條路。
    #[tokio::test]
    async fn brand_new_cards_are_not_shaky_words() {
        let (db, profile) = setup().await;
        let lemma = add_word(&db, "unseen", 1).await;
        cards::ensure(&db, profile, lemma, CardKind::Recognition, t0())
            .await
            .unwrap();

        assert!(
            cards::shaky_words(&db, profile, "en", t0(), 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
