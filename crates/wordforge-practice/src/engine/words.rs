//! 這次要練哪些字，以及素材從哪裡來。
//!
//! 選字是這個專案最容易寫錯的一塊：`ORDER BY due` 取前幾個完全是決定性的，
//! 而翻譯練習不動 FSRS 排程，於是連續兩份練習會拿到一模一樣的字。
//! 每個函式的說明都寫著它避開了哪一種重複。

use super::*;

impl PracticeEngine<'_> {
    /// 今天到期的字，**含從來沒看過的新卡**。
    ///
    /// 這是最後手段，不是預設。批改時 LLM 標出來的生詞會直接建成新卡，
    /// 而新卡一建好就算「到期」——所以這個查詢回的多半是他一次都沒學過的字。
    /// 拿那種字出題，中翻英等於要他寫出一個從沒見過的單字。
    ///
    /// 正常路徑用 [`Self::studied_due_words`]（多一個 `reps > 0`）。
    /// 這裡留著是為了新使用者：牌組裡全是新卡時，一個字都拿不到
    /// 比拿到沒學過的字更糟。
    pub(super) async fn due_words(
        &self,
        profile_id: i64,
        limit: i64,
        now: OffsetDateTime,
    ) -> Result<Vec<String>> {
        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id
             WHERE c.profile_id = ? AND c.suspended = 0 AND c.due <= ?
               AND l.lang = ?
             ORDER BY c.due LIMIT ?",
        )
        .bind(profile_id)
        .bind(format_ts(now))
        .bind(&self.target_lang)
        .bind(limit)
        .fetch_all(self.db.pool())
        .await
        .map_err(wordforge_db::DbError::from)?;
        Ok(words)
    }

    /// 翻譯題要用哪些字：一部分今天到期的，一部分從學過的字裡隨機抽。
    ///
    /// 配比由 [`practice::translation_mix`] 決定，那裡寫了為什麼不能全用
    /// 到期的字（順序固定 → 每次拿到同一批）。這裡處理的是取材與補位。
    ///
    /// ## 為什麼到期那半邊要看 `reps`
    ///
    /// 批改時 LLM 標出來的生詞會**直接建成新卡**，而新卡一建好就算到期。
    /// 練了幾次之後佇列裡絕大多數是他一次都沒學過的字（實測過一個牌組：
    /// 141 張到期的卡裡有 138 張 `reps = 0`）。而且它們的 `due` 全擠在
    /// 同一個時間點，`ORDER BY due` 在平手時順序固定——同一批字每次都贏。
    ///
    /// 拿沒學過的字出翻譯題，中翻英等於要他寫出一個從沒見過的單字。
    /// 克漏字早就擋掉了，這裡跟它用同一個查詢。
    ///
    /// ## 湊不齊的時候
    ///
    /// 學過的字先互相補：到期的不夠就多抽學會的，隨機抽的不夠就用剩下的
    /// 到期字。補完還是少於 `count` 就**回傳少於 count 個**——呼叫端會
    /// 把題數縮到實際拿得到的字數，不硬湊。少出兩題比拿沒學過的字出題好。
    ///
    /// 只有**一個學過的字都沒有**時才退到沒學過的到期字。那是新使用者
    /// 才會遇到的狀況（匯完字典、加了一批字就來練），這時候給空白比
    /// 給新卡更糟。
    ///
    /// ## 為什麼要排除最近用過的字
    ///
    /// 到期那半邊是 `ORDER BY due` 取前幾個，**完全決定性**。而翻譯練習
    /// 不動 FSRS 排程（`grade_translation` 不 review 任何卡片），所以做完
    /// 一份翻譯之後那些字的 `due` 沒有變——下一份拿到的是同一批。
    ///
    /// 實測一個牌組：到期且學過的字只有 7 個，出 5 題時到期那半邊固定
    /// 是同樣的 3 個。使用者看到的是「翻譯用的單字都一樣」。
    ///
    /// 而且 `due` 常常整批平手（同一次匯入、同一天複習的卡），平手時
    /// SQLite 的順序固定，字變多也不會自己輪換。
    ///
    /// 把要練的字配上它的家族詞形，交給驗收。
    ///
    /// 詞形要先查好：驗收在 `ask_valid_json` 的重試迴圈裡跑，那裡是同步的，
    /// 而同一批字重試幾次都是同一組詞形，查一次就夠。
    pub(super) async fn word_assignments(
        &self,
        words: &[String],
    ) -> Result<Vec<crate::validate::WordAssignment>> {
        let mut out = Vec::with_capacity(words.len());
        for word in words {
            let forms = lemmas::forms(self.db, &self.target_lang, word).await?;
            out.push(crate::validate::WordAssignment {
                word: word.clone(),
                forms: forms.into_iter().collect(),
            });
        }
        Ok(out)
    }

    /// 排除是**軟的**：排完沒東西剩就照用。少練一次不如重複練一次。
    pub(super) async fn translation_words(
        &self,
        profile_id: i64,
        count: usize,
        now: OffsetDateTime,
    ) -> Result<Vec<String>> {
        let (due_n, extra_n) = practice::translation_mix(count);

        // 只看翻譯自己的歷史。混進閱讀的話，那邊每篇教六個生詞，
        // 幾篇就把翻譯的記憶名額吃光了。
        let recent = exercises::recent_target_words(
            self.db,
            ProfileId(profile_id),
            TRANSLATION_KINDS,
            TRANSLATION_WORD_MEMORY,
        )
        .await?;
        let used_recently = |w: &String| recent.iter().any(|r| r.eq_ignore_ascii_case(w.as_str()));

        // 撈的量要**連排除掉的份一起算**。
        //
        // `LIMIT` 是在 SQL 裡截斷的，排除最近用過的字則發生在那之後——
        // 只撈 `count` 個的話，前幾個剛好都是上次用過的時候，剩下的
        // 不夠填到期那半邊的名額，於是又把上次的字拿回來。
        // 這個洞是測試抓到的：連兩次出題仍然重複拿到第一個字。
        let due_pool = self
            .studied_due_words(profile_id, count as i64 + TRANSLATION_WORD_MEMORY, now)
            .await?;

        // 沒用過的排前面，用過的仍留在池子裡當備胎——順序決定誰先被取用
        let (fresh_due, stale_due): (Vec<String>, Vec<String>) =
            due_pool.iter().cloned().partition(|w| !used_recently(w));
        let due_pool: Vec<String> = fresh_due.into_iter().chain(stale_due).collect();

        let mut words: Vec<String> = due_pool.iter().take(due_n).cloned().collect();

        // 隨機那半邊同樣先避開最近用過的：`sample_known_words` 的 exclude
        // 是硬排除，所以把 recent 一起傳進去，不夠再放寬。
        let mut avoid = words.clone();
        avoid.extend(recent.iter().cloned());
        let extra = cards::sample_known_words(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            &avoid,
            extra_n as i64,
        )
        .await?;
        words.extend(extra);

        let fill = |pool: Vec<String>, words: &mut Vec<String>| {
            for word in pool {
                if words.len() >= count {
                    break;
                }
                if !words.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
                    words.push(word);
                }
            }
        };

        // 隨機抽的不夠時，回頭用剩下的到期字補
        fill(due_pool, &mut words);

        // 到期的不夠時，再多抽一些學過的。
        // 這一輪的 exclude **只有 `words`**，不含 `recent`——上面避開最近用過的
        // 那層在這裡自動放寬。牌組小的時候排除完就沒東西可抽了，
        // 那時候重複出現一個練過的字，比硬縮題數好。
        if words.len() < count {
            let more = cards::sample_known_words(
                self.db,
                ProfileId(profile_id),
                &self.target_lang,
                &words,
                (count - words.len()) as i64,
            )
            .await?;
            words.extend(more);
        }

        // 最後手段：一個學過的字都沒有時，才拿沒學過的到期字。
        // 條件是 `is_empty` 而不是 `< count`——只差幾個字的時候要縮題數，
        // 不是拿新卡湊滿。
        if words.is_empty() {
            fill(
                self.due_words(profile_id, count as i64, now).await?,
                &mut words,
            );
        }

        Ok(words)
    }

    /// 這次要用哪個情境主題。
    ///
    /// 候選來自 `topic` 資料表（使用者可增刪改），照 `kind` 過濾——
    /// 「報導一則虛構的地方新聞」是體裁，對閱讀成立，出翻譯題就是歪的。
    ///
    /// `memory_kinds` 決定「最近用過」要看哪些題型的歷史。閱讀與克漏字
    /// 算同一組，翻譯的兩個方向算另一組：記憶名額只有幾個，混在一起的話
    /// 兩邊都輪不完一輪就被對方沖掉。
    ///
    /// 回傳 `None` 代表這次不指定主題——使用者可以把主題全部刪掉或停用，
    /// 那時候不該硬塞一個他刪掉的東西回去。
    pub(super) async fn choose_topic(
        &self,
        profile_id: i64,
        kind: ExerciseKind,
        memory_kinds: &[&str],
        now: OffsetDateTime,
    ) -> Result<Option<String>> {
        // 第一次用到時才把種子寫進去。放在這裡而不是開 App 時：
        // 出題本來就會等模型幾十秒，多一個查詢無所謂，而且不必在
        // 啟動路徑上多做一件會寫入的事。
        topics::seed(self.db, &self.target_lang, now).await?;

        let candidates = topics::usable(self.db, &self.target_lang, kind.as_str()).await?;
        let recent =
            exercises::recent_topics(self.db, ProfileId(profile_id), memory_kinds, TOPIC_MEMORY)
                .await?;

        Ok(practice::pick_topic(
            &candidates,
            &recent,
            now.unix_timestamp() as u64,
        ))
    }

    /// 今天到期、而且**至少複習過一次**的字。
    ///
    /// 跟 `due_words` 的差別只有 `reps > 0`，但那一個條件很重要：
    /// 牌組裡剛加進去、他從來沒看過的新卡在排程上也算「今天到期」。
    /// 克漏字拿那種字挖空，考的就不是「想不想得起來」而是「猜不猜得中」。
    pub(super) async fn studied_due_words(
        &self,
        profile_id: i64,
        limit: i64,
        now: OffsetDateTime,
    ) -> Result<Vec<String>> {
        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id
             WHERE c.profile_id = ? AND c.suspended = 0 AND c.due <= ?
               AND c.reps > 0 AND l.lang = ?
             ORDER BY c.due LIMIT ?",
        )
        .bind(profile_id)
        .bind(format_ts(now))
        .bind(&self.target_lang)
        .bind(limit)
        .fetch_all(self.db.pool())
        .await
        .map_err(wordforge_db::DbError::from)?;
        Ok(words)
    }

    /// 白名單裡有哪些字**沒有**出現在文章裡。
    ///
    /// 「出現」認的是整個詞族：這篇教 `establish`，文章寫 `established`
    /// 也算教到了。字面比對會把那種情況誤判成沒用到，然後要求模型多寫一次，
    /// 而它已經寫了。
    pub(super) async fn unused_targets(
        &self,
        passage: &str,
        targets: &[String],
    ) -> Result<Vec<String>> {
        let mut unused = Vec::new();
        for word in targets {
            let forms: std::collections::HashSet<String> =
                lemmas::forms(self.db, &self.target_lang, word)
                    .await?
                    .into_iter()
                    .collect();
            if !wordforge_core::text::mentions_any(passage, &forms, &self.target_lang) {
                unused.push(word.clone());
            }
        }
        Ok(unused)
    }

    /// 出題時給模型的**可用池**：這些字他都會，可以用、不必用。
    ///
    /// 兩段組成：
    ///
    /// 1. 學過但**還沒有例句**的字（上限 `NO_SENTENCE_SHARE`）——那些字複習時
    ///    只看得到釋義，讓模型有機會把它們寫進一句真的句子裡
    /// 2. 其餘從已知詞分帶隨機抽（[`known_sample`]），維持程度分布
    ///
    /// 第一段實際填幾個取「比例」與「真的有幾個」的小者：使用者的資料
    /// 現在只有 54 個沒例句的字，硬填會變成同一批重複。
    ///
    /// [`known_sample`]: Self::known_sample
    pub(super) async fn usable_pool(
        &self,
        profile_id: i64,
        vocabulary: i64,
        size: i64,
    ) -> Result<Vec<String>> {
        let want_fresh = ((size as f64) * NO_SENTENCE_SHARE) as i64;
        let mut pool = cards::words_without_sentences(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            want_fresh,
        )
        .await?;

        // 其餘用分帶抽樣補滿。多抽一些，因為要扣掉上面已經有的
        let sampled = self
            .known_sample(profile_id, vocabulary, size - pool.len() as i64)
            .await?;
        for word in sampled {
            if pool.len() as i64 >= size {
                break;
            }
            if !pool.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
                pool.push(word);
            }
        }
        Ok(pool)
    }

    /// 翻譯題的可用池：題目句子可以用哪些字。
    ///
    /// 跟文章類的差別是**證據等級**。文章要能寫得下去，所以可用池收進
    /// 「詞頻在估計詞彙量以內」的字——那是推定會，沒有證據。翻譯不行：
    /// 題目句子他要讀得懂才翻得出來，讀不懂的話那一題考的是別的東西。
    ///
    /// 所以順序是「真的學過的」優先（有卡片、已進入複習），不夠才用
    /// 推定的補。牌組小的時候補得多，那也是沒辦法的事——總不能因此
    /// 出不了題。
    pub(super) async fn sentence_pool(
        &self,
        profile_id: i64,
        vocabulary: i64,
        size: i64,
    ) -> Result<Vec<String>> {
        let want_fresh = ((size as f64) * NO_SENTENCE_SHARE) as i64;
        let mut pool = cards::words_without_sentences(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            want_fresh,
        )
        .await?;

        // 真的學過的字優先
        let studied = cards::sample_known_words(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            &pool,
            size - pool.len() as i64,
        )
        .await?;
        pool.extend(studied);

        // 還不夠才用推定會的補
        if (pool.len() as i64) < size {
            let sampled = self
                .known_sample(profile_id, vocabulary, size - pool.len() as i64)
                .await?;
            for word in sampled {
                if pool.len() as i64 >= size {
                    break;
                }
                if !pool.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
                    pool.push(word);
                }
            }
        }
        Ok(pool)
    }

    /// 已知詞的抽樣，讓模型感受用字範圍。
    ///
    /// 兩個要點：
    ///
    /// 1. **分層**。只給最常用的字，模型看到的是 the / be / and，
    ///    判斷不出程度上緣在哪。要涵蓋從最基礎到接近上限的各個難度。
    /// 2. **不能只看卡片**。在這個 App 裡複習過的字可能只有幾十個，
    ///    但分級測驗說他會五千個——只送那幾十個會讓模型嚴重低估程度。
    ///    所以把「詞頻在估計詞彙量以內」的字也納入抽樣池。
    pub(super) async fn known_sample(
        &self,
        profile_id: i64,
        vocabulary: i64,
        size: i64,
    ) -> Result<Vec<String>> {
        let per_band = (size / SAMPLE_BANDS).max(1);
        let upper = vocabulary.max(1);

        // 分層的邊界依估計詞彙量等比切開，程度低的人不會全部落在同一層
        let b1 = (upper / 8).max(1);
        let b2 = (upper / 4).max(b1 + 1);
        let b3 = (upper / 2).max(b2 + 1);

        let words: Vec<String> = sqlx::query_scalar(
            "WITH pool AS (
                 -- 確定會的：已經進入長期複習的卡片
                 SELECT l.text AS text, l.freq_rank AS freq_rank
                 FROM card c JOIN lemma l ON l.id = c.lemma_id
                 WHERE c.profile_id = ?1 AND c.state = 'review'
                   AND l.freq_rank IS NOT NULL
                 UNION
                 -- 推定會的：詞頻落在估計詞彙量以內的字。
                 -- 排除大寫開頭與過短的詞，那些多半是專有名詞與縮寫。
                 SELECT text, freq_rank FROM lemma
                 WHERE lang = ?7 AND freq_rank IS NOT NULL AND freq_rank <= ?2
                   AND text = lower(text) AND length(text) >= 3
                   AND text NOT LIKE '% %'
                   -- **排除牌組裡還沒學過的字**。詞頻低不代表他會——
                   -- 那些字是他自己排進來要學的，把它們寫進「他讀得懂的字」
                   -- 等於用他還不會的字出題。這條是測試抓到的。
                   AND NOT EXISTS (
                       SELECT 1 FROM card c
                       WHERE c.profile_id = ?1 AND c.lemma_id = lemma.id
                         AND c.state = 'new'
                   )
                   -- **排除專有名詞**。實測這個池子有 29.7% 是人名與地名
                   -- （charles、powell、raf…），而同一份 prompt 第 4 點
                   -- 正好寫著「不要使用專有名詞」——等於白燒三成的 token
                   -- 去給模型一份它不該用的清單。
                   --
                   -- 判斷方式跟 `new_word_candidates` 一致：詞性表裡出現
                   -- `name` 就整個排掉。維基詞典對專有名詞常常同時標上
                   -- 普通詞性（edward 是 name,noun），只排「只有 name」
                   -- 的話攔不住。
                   AND NOT EXISTS (
                       SELECT 1 FROM lemma p
                       WHERE p.lang = lemma.lang AND p.normalized = lemma.normalized
                         AND p.pos LIKE '%name%'
                   )
             ),
             banded AS (
                 SELECT text, freq_rank,
                     CASE WHEN freq_rank <= ?3 THEN 0
                          WHEN freq_rank <= ?4 THEN 1
                          WHEN freq_rank <= ?5 THEN 2
                          ELSE 3 END AS band
                 FROM pool
             ),
             picked AS (
                 SELECT text, freq_rank, band,
                        ROW_NUMBER() OVER (PARTITION BY band ORDER BY RANDOM()) AS rn
                 FROM banded
             )
             SELECT text FROM picked WHERE rn <= ?6 ORDER BY freq_rank",
        )
        .bind(profile_id)
        .bind(upper)
        .bind(b1)
        .bind(b2)
        .bind(b3)
        .bind(per_band)
        .bind(&self.target_lang)
        .fetch_all(self.db.pool())
        .await
        .map_err(wordforge_db::DbError::from)?;

        Ok(words)
    }

    /// 算出文章對這位學習者的實際覆蓋率。
    ///
    /// 「懂不懂」看的是整個詞形家族：學過 `run` 的人看到 `ran` 是懂的。
    /// 這件事必須做對，否則 90% 法則會系統性低估——文章明明剛好，
    /// 卻因為裡面有幾個變化形被判定太難而重寫。
    pub(super) async fn measure_coverage(
        &self,
        passage: &str,
        known: &std::collections::HashSet<LemmaId>,
    ) -> Result<wordforge_core::coverage::Coverage> {
        // 一次把文章裡的詞查完，避免在 async 閉包裡查資料庫
        let tokens = wordforge_core::text::tokenize(passage);
        let mut resolved: std::collections::HashMap<String, bool> =
            std::collections::HashMap::new();
        for token in &tokens {
            if resolved.contains_key(token) {
                continue;
            }
            let familiar = self.knows_token(token, known).await?;
            resolved.insert(token.clone(), familiar);
        }

        Ok(wordforge_core::coverage::analyze(passage, |w| {
            resolved.get(w).copied().unwrap_or(false)
        }))
    }

    /// 這個表面形學習者懂不懂：詞形家族裡有任何一個在已知集合裡就算懂。
    pub(super) async fn knows_token(
        &self,
        token: &str,
        known: &std::collections::HashSet<LemmaId>,
    ) -> Result<bool> {
        let family = lemmas::family(self.db, &self.target_lang, token).await?;
        Ok(family.iter().any(|id| known.contains(id)))
    }
}
