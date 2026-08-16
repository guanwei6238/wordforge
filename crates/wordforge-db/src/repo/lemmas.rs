//! 字典裡的詞條，以及「這個詞形算哪個字」。
//!
//! 這一組是匯入來的資料：整份刪掉重匯是正常操作，所以它跟學習歷史
//! （[`super::cards`]）刻意分開存。
//!
//! 詞形還原沒有語言知識可用（那要查表，而表就是使用者匯入的字典），
//! 所以這裡的做法一律是「查表 + 挑一個」，而挑哪一個是有講究的——
//! 見 [`family`] 與 [`base_form`] 的說明。

use wordforge_core::model::{LemmaId, ProfileId};

use crate::{Db, Result};

/// 要寫入的新詞條。
#[derive(Debug, Clone)]
pub struct NewLemma<'a> {
    pub lang: &'a str,
    pub text: &'a str,
    pub pos: &'a str,
    pub freq_rank: Option<i64>,
    pub cefr: Option<&'a str>,
}

/// 寫入詞條；已存在則補上缺少的詞頻與 CEFR 資訊。
///
/// `COALESCE(excluded.x, lemma.x)` 的用意：後匯入的來源若沒帶詞頻，
/// 不該把先前來源帶進來的詞頻洗掉。
pub async fn upsert(db: &Db, lemma: NewLemma<'_>) -> Result<LemmaId> {
    let normalized = wordforge_core::text::normalize(lemma.text);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO lemma (lang, text, normalized, pos, freq_rank, cefr)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (lang, text, pos) DO UPDATE SET
             freq_rank = COALESCE(excluded.freq_rank, lemma.freq_rank),
             cefr      = COALESCE(excluded.cefr, lemma.cefr)
         RETURNING id",
    )
    .bind(lemma.lang)
    .bind(lemma.text)
    .bind(&normalized)
    .bind(lemma.pos)
    .bind(lemma.freq_rank)
    .bind(lemma.cefr)
    .fetch_one(db.pool())
    .await?;

    Ok(LemmaId(id))
}

/// 登記一個表面形（`ran` → `run`），供詞形還原使用。
pub async fn add_surface_form(
    db: &Db,
    lang: &str,
    form: &str,
    lemma_id: LemmaId,
    tag: &str,
) -> Result<()> {
    let normalized = wordforge_core::text::normalize(form);
    sqlx::query(
        "INSERT INTO surface_form (lang, form, normalized, lemma_id, tag)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (lang, normalized, lemma_id, tag) DO NOTHING",
    )
    .bind(lang)
    .bind(form)
    .bind(&normalized)
    .bind(lemma_id.0)
    .bind(tag)
    .execute(db.pool())
    .await?;
    Ok(())
}

/// 由任意詞形找出對應的 lemma。先查本身，再查表面形對照表。
///
/// 一個詞形可能對到多個 lemma（`saw` = see 的過去式，也是「鋸子」）；
/// 這裡回傳詞頻最高的那個，需要精確消歧時交給 LLM 或上下文判斷。
/// 一個表面形可能對應到的**所有** lemma。
///
/// [`find_by_form`] 只回一個 id，而它挑的是「id 最小的那個」——
/// 也就是匯入順序最早的那個，實際上等於字母序。這對判斷
/// 「這個字他會不會」是錯的：`ran` 在字典裡自己也是一個詞條，
/// 而 `ran` < `run`，所以會回 `ran` 而不是 `run`。學習者明明學過
/// `run`，文章裡的 `ran` 卻被算成生字。`better`（該對到 `good`）、
/// `studied`（該對到 `study`）都有同樣的問題。
///
/// 挑「正確的那一個」需要真正的詞形還原，而那是有歧義的
/// （`saw` 可以是 see 的過去式，也可以是「鋸子」）。判斷懂不懂
/// 不需要解決這個歧義：整個家族有任何一個是他會的，就算他看得懂。
pub async fn family(db: &Db, lang: &str, form: &str) -> Result<Vec<LemmaId>> {
    let normalized = wordforge_core::text::normalize(form);
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM lemma
         WHERE lang = ? AND normalized = ?
         UNION
         SELECT l.id FROM lemma l
           JOIN surface_form s ON s.lemma_id = l.id
         WHERE s.lang = ? AND s.normalized = ?",
    )
    .bind(lang)
    .bind(&normalized)
    .bind(lang)
    .bind(&normalized)
    .fetch_all(db.pool())
    .await?;

    Ok(ids.into_iter().map(LemmaId).collect())
}

/// 這個字的整個家族長成的所有樣子，正規化後回傳。
///
/// 用途是「這個句子有沒有真的用到這個字」：出翻譯題時指派了 `run`，
/// 模型寫回來的句子是 `She ran to the station`，字面比對認不出來。
/// 拿家族的全部詞形去比才問得出答案。
///
/// 跟 [`family`] 的方向相反：那個是「詞形 → 有哪些 lemma」，
/// 這個是「詞形 → 這個家族的全部詞形」。
///
/// **查不到家族時回這個字本身**，不是空的。空的會讓呼叫端分不出
/// 「這個字沒有別的樣子」與「這個字典沒有詞形資料」，而兩者都要能
/// 比對——只匯了 ECDICT 的人整份字典沒有 `surface_form`，那時退化成
/// 字面比對仍然抓得到「模型換了一個完全不同的字」，只是抓不到
/// 屈折形。少驗一層比誤判成沒用到好。
pub async fn forms(db: &Db, lang: &str, word: &str) -> Result<Vec<String>> {
    let normalized = wordforge_core::text::normalize(word);
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    let forms: Vec<String> = sqlx::query_scalar(
        "WITH fam AS (
             SELECT id FROM lemma WHERE lang = ?1 AND normalized = ?2
             UNION
             SELECT l.id FROM lemma l
               JOIN surface_form s ON s.lemma_id = l.id
             WHERE s.lang = ?1 AND s.normalized = ?2
         )
         SELECT normalized FROM lemma WHERE id IN (SELECT id FROM fam)
         UNION
         SELECT normalized FROM surface_form WHERE lemma_id IN (SELECT id FROM fam)",
    )
    .bind(lang)
    .bind(&normalized)
    .fetch_all(db.pool())
    .await?;

    // 字典裡查不到也要認得自己：只匯詞頻表的人整個 `lemma` 表都沒有
    // 這個字，但句子裡原樣出現時仍然算練到了。
    Ok(if forms.is_empty() {
        vec![normalized]
    } else {
        forms
    })
}

/// 這個表面形應該歸到哪一個字底下——也就是要建卡的那個。
///
/// 挑家族裡詞頻排名最前面的。理由是詞頻表統計的是原形：
/// 任何一份詞頻表裡 `run` 都遠比 `ran` 常見，`study` 都遠比
/// `studied` 常見。實測 ECDICT + Wiktionary 的資料：
///
/// ```text
/// ran -> run       studied -> study    children -> child
/// better -> good   saw -> see          went -> go
/// ```
///
/// 已知的限制：同形異義詞會被併到比較常見的那個意思。
/// `left`（左）會歸到 `leave`，`saw`（鋸子）會歸到 `see`。
/// 要分開得看上下文，那需要真正的詞性標注。併錯的代價是
/// 一張卡標到鄰近的意思；不併的代價是同一個字散成好幾張卡，
/// 每張各自排程。前者比較容易發現也比較容易修。
pub async fn base_form(db: &Db, lang: &str, form: &str) -> Result<Option<LemmaId>> {
    let normalized = wordforge_core::text::normalize(form);
    if normalized.is_empty() {
        return Ok(None);
    }
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM (
             SELECT id, freq_rank FROM lemma
             WHERE lang = ?1 AND normalized = ?2
             UNION
             SELECT l.id, l.freq_rank FROM lemma l
               JOIN surface_form s ON s.lemma_id = l.id
             WHERE s.lang = ?1 AND s.normalized = ?2
         )
         ORDER BY freq_rank IS NULL, freq_rank, id
         LIMIT 1",
    )
    .bind(lang)
    .bind(&normalized)
    .fetch_optional(db.pool())
    .await?;

    Ok(id.map(LemmaId))
}

/// 挑「剛好在他程度上緣」的生詞，給閱讀理解當新詞白名單。
///
/// ## 為什麼需要這個
///
/// 原本文章的新詞是拿「今天到期的複習字」去填。那些字他已經在學了，
/// 覆蓋率算起來都算會——實測產出的文章覆蓋率 99%，遠高於目標的 96%，
/// 也就是**整篇沒有任何新東西可學**。90% 法則的重點是那不足 10%，
/// 沒有生詞的話這條規則就只是個好看的數字。
///
/// ## 挑哪些
///
/// 詞頻落在「估計詞彙量」到「估計詞彙量 × `reach`」之間：比他會的
/// 再難一點，但還在會再遇到的常用範圍內。挑太罕見的字沒有學習價值。
///
/// 排掉的：
///
/// - 已經在牌組裡的（不管什麼狀態）——正在學的不算「新」
/// - 專有名詞（`name`）——`Romania`、`CH` 學了沒用
/// - 虛詞——那些該從閱讀中自然吸收，不該當生詞教
/// - 有空格的多詞條目——白名單要的是單字
///
/// 詞性從**同名的其他詞條**取：ECDICT 的 `pos` 是空的，詞性資訊
/// 在 Wiktionary 那批。實測這個區間 99% 的字對得起來。
pub async fn new_word_candidates(
    db: &Db,
    profile_id: ProfileId,
    lang: &str,
    vocabulary: i64,
    reach: f64,
    limit: i64,
) -> Result<Vec<wordforge_core::practice::NewWord>> {
    let upper = ((vocabulary as f64) * reach.max(1.0)) as i64;

    let rows: Vec<(i64, String, i64, Option<String>)> = sqlx::query_as(
        "SELECT l.id, l.text, l.freq_rank,
                (SELECT GROUP_CONCAT(DISTINCT p.pos) FROM lemma p
                  WHERE p.lang = l.lang AND p.normalized = l.normalized AND p.pos <> '')
         FROM lemma l
         WHERE l.lang = ?1
           AND l.freq_rank > ?2 AND l.freq_rank <= ?3
           AND l.text = lower(l.text)
           AND length(l.text) >= 3
           AND l.text NOT LIKE '% %'
           AND NOT EXISTS (
               SELECT 1 FROM card c
               WHERE c.profile_id = ?4 AND c.lemma_id = l.id
           )
           -- 變化形不算「新字」：教 `established` 沒有意義，
           -- 該教的是 `establish`。實測不擋的話 supporting / visiting
           -- 這類會佔掉一半的名額。
           AND NOT EXISTS (
               SELECT 1 FROM surface_form sf
               JOIN lemma base ON base.id = sf.lemma_id
               WHERE sf.lang = l.lang AND sf.normalized = l.normalized
                 AND base.normalized <> l.normalized
           )
         ORDER BY l.freq_rank
         LIMIT ?5",
    )
    .bind(lang)
    .bind(vocabulary.max(0))
    .bind(upper)
    .bind(profile_id.0)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .filter(|(_, text, _, _)| !wordforge_core::wordlist::is_function_word(lang, text))
        .map(|(id, text, freq_rank, pos)| {
            let pos: Vec<String> = pos
                .unwrap_or_default()
                .split(',')
                .filter(|p| !p.is_empty())
                .map(|p| p.to_string())
                .collect();
            wordforge_core::practice::NewWord {
                lemma_id: id,
                text,
                pos,
                freq_rank,
            }
        })
        // 詞性表裡出現 `name` 就整個排掉。
        //
        // 原本只排除「只有 name」的字，結果 `gould`（姓氏，詞性被標成
        // adj,noun,name）混進了學習者的生詞清單。維基詞典對專有名詞
        // 常常同時標上普通詞性，所以「只有 name」這條線攔不住。
        //
        // 代價是 `comet`（noun,name）這種好字也會被丟掉。可以接受：
        // 候選池有幾百個字而一篇只要幾個，寧可少幾個好字，
        // 也不要讓學習者背一個姓氏。
        .filter(|w| !w.pos.iter().any(|p| p == "name"))
        // 查不到詞性的**不排除**。
        //
        // 詞性來自 Wiktionary；只匯入 ECDICT 的人整份字典的 pos 都是
        // 空的。要求有詞性的話那些人會一個生詞都拿不到，文章覆蓋率
        // 衝回 99%，等於這個功能對他們不存在——而「只要有字典就能學」
        // 是這個專案的前提。
        //
        // 詞性配比是加分項，有生詞才是必要的。沒有詞性的候選會落到
        // balance_by_pos 的補滿路徑，照詞頻挑。
        .collect())
}

pub async fn find_by_form(db: &Db, lang: &str, form: &str) -> Result<Option<LemmaId>> {
    let normalized = wordforge_core::text::normalize(form);
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM lemma
         WHERE lang = ? AND normalized = ?
         UNION
         SELECT l.id FROM lemma l
           JOIN surface_form s ON s.lemma_id = l.id
         WHERE s.lang = ? AND s.normalized = ?
         ORDER BY 1
         LIMIT 1",
    )
    .bind(lang)
    .bind(&normalized)
    .bind(lang)
    .bind(&normalized)
    .fetch_optional(db.pool())
    .await?;

    Ok(id.map(LemmaId))
}

#[cfg(test)]
mod tests {
    use crate::repo::fixture::*;
    use crate::repo::{NewLemma, lemmas};

    #[tokio::test]
    async fn upsert_lemma_is_idempotent_and_backfills() {
        let (db, _) = setup().await;
        let first = add_word(&db, "apple", 500).await;

        // 第二次匯入沒帶詞頻，不該把既有詞頻洗掉
        let second = lemmas::upsert(
            &db,
            NewLemma {
                lang: "en",
                text: "apple",
                pos: "noun",
                freq_rank: None,
                cefr: Some("A1"),
            },
        )
        .await
        .unwrap();

        assert_eq!(first, second, "同一個字不該產生兩筆 lemma");
        let (freq, cefr): (Option<i64>, Option<String>) =
            sqlx::query_as("SELECT freq_rank, cefr FROM lemma WHERE id = ?")
                .bind(first.0)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(freq, Some(500));
        assert_eq!(cefr.as_deref(), Some("A1"));
    }

    #[tokio::test]
    async fn find_by_form_resolves_inflections() {
        let (db, _) = setup().await;
        let run = add_word(&db, "run", 300).await;
        lemmas::add_surface_form(&db, "en", "Running", run, "gerund")
            .await
            .unwrap();

        assert_eq!(
            lemmas::find_by_form(&db, "en", "run").await.unwrap(),
            Some(run)
        );
        // 大小寫與標點都該被正規化掉
        assert_eq!(
            lemmas::find_by_form(&db, "en", "running,").await.unwrap(),
            Some(run)
        );
        assert_eq!(
            lemmas::find_by_form(&db, "en", "nonexistent")
                .await
                .unwrap(),
            None
        );
    }

    /// 出翻譯題時指派了 `run`，模型寫回來的句子是 `She ran`——
    /// 要問「這句有沒有練到 run」就得先拿得到整個家族的樣子。
    #[tokio::test]
    async fn a_words_family_yields_every_form_it_wears() {
        let (db, _) = setup().await;
        let run = add_word(&db, "run", 300).await;
        lemmas::add_surface_form(&db, "en", "ran", run, "past")
            .await
            .unwrap();
        lemmas::add_surface_form(&db, "en", "Running", run, "gerund")
            .await
            .unwrap();

        let forms = lemmas::forms(&db, "en", "run").await.unwrap();
        for want in ["run", "ran", "running"] {
            assert!(forms.contains(&want.to_string()), "{forms:?} 少了 {want}");
        }

        // 從屈折形問也要回到同一個家族，否則指派的字換個寫法就驗不了
        let from_inflection = lemmas::forms(&db, "en", "Ran.").await.unwrap();
        assert!(
            from_inflection.contains(&"run".to_string()),
            "{from_inflection:?}"
        );
    }

    /// 字典裡沒有詞形資料（只匯 ECDICT 就是這樣）時不能回空的：
    /// 空的會讓呼叫端把「驗不了」當成「沒用到」，每一題都退回去重出。
    #[tokio::test]
    async fn an_unknown_word_still_answers_with_itself() {
        let (db, _) = setup().await;
        assert_eq!(
            lemmas::forms(&db, "en", "Zzzz!").await.unwrap(),
            vec!["zzzz".to_string()]
        );
        assert!(lemmas::forms(&db, "en", "  ").await.unwrap().is_empty());
    }

    /// 設定頁的目標語言選單就是這份清單。
    #[tokio::test]
    async fn dictionary_languages_are_listed_by_size() {
        let (db, _) = setup().await;
        add_word(&db, "apple", 1).await;
        add_word(&db, "banana", 2).await;
        lemmas::upsert(
            &db,
            NewLemma {
                lang: "ja",
                text: "林檎",
                pos: "noun",
                freq_rank: Some(1),
                cefr: None,
            },
        )
        .await
        .unwrap();

        let langs = crate::dict::languages(&db).await.unwrap();
        assert_eq!(
            langs,
            vec![("en".to_string(), 2), ("ja".to_string(), 1)],
            "詞條多的排前面，使用者最可能要的排第一個"
        );
    }

    /// 學過 run 的人看到 ran 是懂的——90% 法則靠這件事成立。
    ///
    /// 這條測試存在的理由是它曾經是錯的：`find_by_form` 挑「id 最小的」，
    /// 而 `ran` 自己在字典裡也是一個詞條，且 `ran` < `run`，
    /// 所以查 `ran` 會回到 `ran` 而不是 `run`，學過的字被算成生字。
    #[tokio::test]
    async fn an_inflection_resolves_to_the_whole_family() {
        let (db, _) = setup().await;
        let run = add_word(&db, "run", 100).await;
        // 變化形自己也是詞條，而且拼字排在原形前面——這正是當初踩到的情況
        let ran_entry = add_word(&db, "ran", 900).await;
        lemmas::add_surface_form(&db, "en", "ran", run, "past")
            .await
            .unwrap();

        let family = lemmas::family(&db, "en", "ran").await.unwrap();
        assert!(family.contains(&run), "沒有把 ran 對回 run：{family:?}");
        assert!(family.contains(&ran_entry), "ran 自己那個詞條也該在家族裡");

        // 反過來：原形查得到自己
        assert!(
            lemmas::family(&db, "en", "run")
                .await
                .unwrap()
                .contains(&run)
        );
    }

    /// 大小寫與標點不能影響詞形比對。
    #[tokio::test]
    async fn family_lookup_normalizes_the_form() {
        let (db, _) = setup().await;
        let study = add_word(&db, "study", 100).await;
        lemmas::add_surface_form(&db, "en", "studied", study, "past")
            .await
            .unwrap();

        assert!(
            lemmas::family(&db, "en", "Studied,")
                .await
                .unwrap()
                .contains(&study),
            "文章裡的字會帶大寫與標點"
        );
        assert!(lemmas::family(&db, "en", "  ").await.unwrap().is_empty());
    }

    /// 別的語言的同拼字不能混進來。
    #[tokio::test]
    async fn family_lookup_stays_within_one_language() {
        let (db, _) = setup().await;
        let english = add_word(&db, "die", 100).await;
        let german = lemmas::upsert(
            &db,
            NewLemma {
                lang: "de",
                text: "die",
                pos: "article",
                freq_rank: Some(1),
                cefr: None,
            },
        )
        .await
        .unwrap();

        let family = lemmas::family(&db, "en", "die").await.unwrap();
        assert!(family.contains(&english));
        assert!(!family.contains(&german), "德文的 die 不該算進英文");
    }

    /// 從文章裡撿到的生字要建在原形上，不是建在變化形上。
    ///
    /// 這條測試存在的理由：`find_by_form` 挑 id 最小的，而變化形自己
    /// 也是詞條，所以答錯 `studied` 會建出一張 `studied` 的卡，
    /// 跟既有的 `study` 各自排程，變成同一個字要背兩次。
    #[tokio::test]
    async fn an_inflection_gets_a_card_on_its_base_form() {
        let (db, _) = setup().await;
        // 原形詞頻高，變化形詞頻低——每一份詞頻表都是這樣
        let study = add_word(&db, "study", 240).await;
        let studied = add_word(&db, "studied", 15_971).await;
        lemmas::add_surface_form(&db, "en", "studied", study, "past")
            .await
            .unwrap();

        assert_eq!(
            lemmas::base_form(&db, "en", "studied").await.unwrap(),
            Some(study),
            "應該歸到 study"
        );
        assert_ne!(
            lemmas::base_form(&db, "en", "studied").await.unwrap(),
            Some(studied)
        );

        // 原形查自己還是自己
        assert_eq!(
            lemmas::base_form(&db, "en", "study").await.unwrap(),
            Some(study)
        );
    }

    /// 沒有詞頻資料時也要給答案，不能回 None。
    #[tokio::test]
    async fn base_form_falls_back_when_there_is_no_frequency_data() {
        let (db, _) = setup().await;
        let lemma = lemmas::upsert(
            &db,
            NewLemma {
                lang: "en",
                text: "zzz",
                pos: "noun",
                freq_rank: None,
                cefr: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            lemmas::base_form(&db, "en", "ZZZ.").await.unwrap(),
            Some(lemma)
        );
        assert_eq!(lemmas::base_form(&db, "en", "  ").await.unwrap(), None);
    }
}
