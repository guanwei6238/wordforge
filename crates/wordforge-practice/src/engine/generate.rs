//! 出題：四個題型各一條路。
//!
//! 共同的形狀是「組 prompt → 要 JSON → 本地驗收 → 存起來」，但每個題型
//! 驗的東西不一樣：閱讀與克漏字要實算覆蓋率（`measure_coverage`），
//! 翻譯要確認指派的字真的用上了，文法要確認考的是指定的那個點。
//!
//! prompt 講的話都只是請求，本地驗得到的就在這裡驗——這是這個專案
//! 反覆踩過的那個坑：只相信模型的話，壞掉的樣子看起來完全正常。

use super::*;

impl PracticeEngine<'_> {
    pub(super) async fn generate_translation(
        &self,
        profile_id: i64,
        kind: ExerciseKind,
        learner: &LearnerProfile,
        now: OffsetDateTime,
    ) -> Result<ExerciseView> {
        let count = practice::translation_count(learner.vocabulary);
        let words = self.translation_words(profile_id, count, now).await?;
        // 湊不出那麼多字就少出幾題。硬湊只能拿沒學過的字填，
        // 那等於要他寫出從沒見過的單字——寧可這次只練三題。
        // 一個字都沒有時保持原題數：那時模型是自由造句，不受單字限制。
        let count = if words.is_empty() {
            count
        } else {
            count.min(words.len())
        };

        let excerpt = self
            .material_excerpt(&words, now.unix_timestamp() as u64)
            .await?;

        // 主題輪換。跟閱讀同一個機制，只是翻譯一直沒接上——沒有主題時
        // 模型拿到一組日常單字（water、catch、sign）永遠寫出同一批場景。
        //
        // 只看翻譯自己的歷史：跟閱讀共用記憶的話，六個名額會被兩種題型
        // 分掉，兩邊都輪不完一輪就被沖掉了。
        // 指定教材時取材範圍由課本決定，主題輪換不該再插手（同 `generate_reading`）
        let topic = if excerpt.is_some() {
            None
        } else {
            self.choose_topic(profile_id, kind, TRANSLATION_KINDS, now)
                .await?
        };

        let mut req = prompts::translation_task(
            self.target_name(),
            self.native_name(),
            kind == ExerciseKind::TranslationToTarget,
            excerpt.as_deref(),
            topic.as_deref().unwrap_or(""),
            &words,
            count,
        );

        // 每個字的家族詞形先查好，驗收才問得出「這句有沒有真的練到它」。
        // 查在迴圈外：驗收本身要是同步的（`ask_valid_json` 每次重試都會
        // 呼叫它），而且同一批字重試時也不會變。
        let assignments = self
            .word_assignments(&words[..words.len().min(count)])
            .await?;
        let spec = crate::validate::TranslationSpec {
            assignments: &assignments,
            to_target: kind == ExerciseKind::TranslationToTarget,
            target_lang: &self.target_lang,
        };
        let value = self
            .ask_valid_json(profile_id, "generate", &mut req, |v| {
                crate::validate::check_translation(&spec, v)
            })
            .await?;

        let raw_items = value
            .get("items")
            .and_then(|i| i.as_array())
            .ok_or_else(|| PracticeError::BadResponse("回應裡沒有 items".into()))?;

        let items: Vec<TranslationItem> = raw_items
            .iter()
            .filter_map(|item| {
                Some(TranslationItem {
                    source: item.get("source")?.as_str()?.to_string(),
                    target_word: item
                        .get("target_word")
                        .and_then(|w| w.as_str())
                        .map(str::to_string),
                    reference: item
                        .get("reference")
                        .and_then(|r| r.as_str())
                        .map(str::to_string),
                })
            })
            .collect();

        if items.is_empty() {
            return Err(PracticeError::BadResponse("一題都沒產出來".into()));
        }

        let body = ExerciseBody::Translation {
            to_target: kind == ExerciseKind::TranslationToTarget,
            items,
        };
        // 主題一定要存回去，否則 `recent_topics` 永遠是空的、`pick_topic`
        // 永遠從同一個位置挑，輪換等於沒有開。空字串存成 NULL——
        // 存進去的話它會佔掉一個記憶名額，還會被當成「用過的主題」。
        self.store(profile_id, kind, body, words, None, topic.as_deref(), now)
            .await
    }

    pub(super) async fn generate_reading(
        &self,
        profile_id: i64,
        learner: &LearnerProfile,
        now: OffsetDateTime,
    ) -> Result<ExerciseView> {
        // 驗收要用的「他看得懂的字」，跟 prompt 裡告訴模型的是同一個依據。
        // 用嚴格的 known_lemma_ids 的話，剛開始學的人會拿到空集合，
        // 覆蓋率永遠 0%，重試迴圈每次跑滿——實測一題 98 秒而且驗收沒有作用。
        let known = cards::known_vocabulary(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            learner.vocabulary,
            KNOWN_STABILITY_DAYS,
        )
        .await?;
        let known_sample = self.known_sample(profile_id, learner.vocabulary).await?;
        let word_count = practice::reading_length(learner.vocabulary);

        // 覆蓋率目標由使用者設定。90% 是常被引用的數字，但多少最舒服
        // 因人而異——想讀順一點就調高，想每篇多學幾個字就調低。
        let target_coverage = profiles::study_settings(self.db, ProfileId(profile_id))
            .await?
            .reading_coverage;

        // 新詞數量由覆蓋率目標反推，不是拍腦袋的數字
        let budget = wordforge_core::coverage::new_word_budget(
            word_count,
            target_coverage,
            prompts::ReadingSpec::REPEATS_PER_NEW_WORD,
        );

        // 新詞必須是他還不會的字。拿到期的複習字來填的話，覆蓋率算起來
        // 都算會——實測 99%，整篇沒有東西可學。
        let candidates = lemmas::new_word_candidates(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            learner.vocabulary,
            NEW_WORD_REACH,
            NEW_WORD_POOL,
        )
        .await?;
        // 排掉最近幾篇教過的。生詞不會自動進牌組，所以沒有這一步的話
        // 每一篇都會拿到一模一樣的字——實測確認過。
        let recent =
            // 只看會注入生詞的題型。不限的話中間穿插的文法題與翻譯題
            // 會佔掉記憶名額，把閱讀的歷史沖掉。
            //
            // **克漏字也不算**。它的 target_words 是挖掉的複習字——那些他
            // 已經會了，本來就不在生詞候選池裡，排除它們沒有任何作用，
            // 卻會佔掉五個記憶名額。做五題克漏字之後，下一篇閱讀就會
            // 拿回六篇前的同一批生詞。
            exercises::recent_target_words(
                self.db,
                ProfileId(profile_id),
                &[ExerciseKind::Reading.as_str()],
                NEW_WORD_MEMORY,
            )
            .await?;
        let fresh: Vec<practice::NewWord> = candidates
            .iter()
            .filter(|c| !recent.iter().any(|r| r == &c.text))
            .cloned()
            .collect();

        // 候選被排光的話寧可重複也不能沒有生詞——沒有生詞的文章
        // 覆蓋率會衝到 99%，那就回到當初的問題了
        let pool = if fresh.is_empty() {
            &candidates
        } else {
            &fresh
        };

        let target_words: Vec<String> =
            practice::balance_by_pos(pool, practice::DESIRED_POS, budget)
                .into_iter()
                .map(|w| w.text)
                .collect();

        // 「順便複習」用的是**快忘掉的字**而不是「今天到期的字」。
        //
        // 到期只看有沒有跨過門檻；快忘掉看的是衰退到什麼程度。逾期三週的
        // 字和剛好今天到期的字，前者在文章裡再遇到一次的價值高得多。
        // 這些字他學過，所以不佔生詞預算，等於免費的強化。
        let review_words = cards::shaky_words(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            now,
            REVIEW_WORDS,
        )
        .await?;

        // 主題輪換：不指定的話模型永遠寫校園生活與天氣，
        // 十篇讀起來像同一篇。用時間戳當 seed，同一批候選也不會每次都給同一個。
        let topic = self
            .choose_topic(profile_id, ExerciseKind::Reading, PROSE_KINDS, now)
            .await?;

        // 指定教材時，取材範圍由課本決定，主題輪換就不該再插手
        // 教材檢索用複習字：課本裡本來就不會有他還沒學的生詞
        let excerpt = self
            .material_excerpt(&review_words, now.unix_timestamp() as u64)
            .await?;
        let topic = if excerpt.is_some() { None } else { topic };

        let spec = prompts::ReadingSpec {
            target_lang: self.target_name(),
            native_lang: self.native_name(),
            word_count,
            target_coverage,
            known_word_count: learner.vocabulary as usize,
            cefr: None,
            known_sample: &known_sample,
            target_words: &target_words,
            review_words: &review_words,
            topic: topic.as_deref(),
            material_excerpt: excerpt.as_deref(),
            question_count: 4,
        };

        let mut req = prompts::reading_comprehension(&spec);

        for attempt in 0..=COVERAGE_RETRIES {
            let value = self.ask_json(profile_id, "generate", &req).await?;
            let passage = value
                .get("passage")
                .and_then(|p| p.as_str())
                .unwrap_or_default()
                .to_string();

            if passage.trim().is_empty() {
                return Err(PracticeError::BadResponse("沒有產出文章".into()));
            }

            // Prompt 只能提高命中率，本地重算才是保證
            let coverage = self.measure_coverage(&passage, &known).await?;

            tracing::info!(
                attempt,
                coverage = coverage.ratio(),
                band = ?coverage.band(),
                "覆蓋率驗收"
            );

            // 只有「太難」才重寫。太簡單也不理想（這次學不到新字），
            // 但重試訊息講的是「把難字換掉」，拿去處理太簡單的文章只會更糟；
            // 而且對學習者來說，讀一篇太簡單的文章無害，讀不懂的才是災難。
            // 完全沒有已知詞資料時（還沒做分級測驗、牌組也是空的），
            // 覆蓋率必定是 0，重寫幾次都一樣。與其燒兩次額度，不如接受。
            let no_baseline = known.is_empty();
            if no_baseline {
                tracing::warn!("沒有任何已知詞資料，跳過覆蓋率驗收（建議先做分級測驗）");
            }

            // 「太難」要跟著設定走。原本用寫死的難度帶（<90% 才算太難），
            // 使用者把目標設成 98% 的話那個判斷完全不會生效。
            let too_hard = coverage.ratio() < target_coverage - COVERAGE_TOLERANCE;
            let acceptable = !too_hard || no_baseline || attempt == COVERAGE_RETRIES;
            if acceptable {
                let mut questions: Vec<ChoiceItem> = parse_choice_items(&value, "questions");

                // 補解說要在洗牌**之前**：重試回來的解說是照原本的選項順序寫的，
                // 洗完再補就會配到別的選項上
                self.fill_option_notes(
                    profile_id,
                    &mut req,
                    &mut questions,
                    "questions",
                    &value.to_string(),
                )
                .await?;
                shuffle_answers(&mut questions, shuffle_seed(now));

                let new_words: Vec<NewWord> = value
                    .get("new_words")
                    .and_then(|w| w.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|w| serde_json::from_value(w.clone()).ok())
                            .collect()
                    })
                    .unwrap_or_default();

                let body = ExerciseBody::Reading {
                    title: value
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("Reading")
                        .to_string(),
                    passage,
                    // 沒給就是沒給。硬塞一段空字串的話，UI 會出現一個
                    // 打開來是空白的「全文翻譯」，比沒有更糟。
                    translation: value
                        .get("translation")
                        .and_then(|t| t.as_str())
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .map(str::to_string),
                    new_words,
                    questions,
                };
                return self
                    .store(
                        profile_id,
                        ExerciseKind::Reading,
                        body,
                        target_words,
                        Some(coverage.ratio()),
                        // `as_deref` 而不是 `Some(topic)`：指定教材時沒有主題，
                        // 原本會存進一個空字串，而 `recent_topics` 只濾 NULL，
                        // 那個空主題會佔掉一個記憶名額
                        topic.as_deref(),
                        now,
                    )
                    .await;
            }

            // 帶著實際超標的詞重試，比單純說「太難了」有效得多
            let offenders: Vec<String> = coverage
                .unknown_types
                .iter()
                .filter(|(w, _)| !target_words.iter().any(|t| t.eq_ignore_ascii_case(w)))
                .take(10)
                .map(|(w, _)| w.clone())
                .collect();
            tracing::info!(
                ratio = coverage.ratio(),
                ?offenders,
                "覆蓋率不合格，要求重寫"
            );
            req.messages.push(prompts::coverage_retry(
                coverage.ratio(),
                target_coverage,
                &offenders,
                &passage,
            ));
        }

        Err(PracticeError::BadResponse("重試後覆蓋率仍不合格".into()))
    }

    /// 克漏字：用他已經會的字寫一篇短文，把今天該複習的字挖掉。
    ///
    /// 跟閱讀測驗相反——閱讀刻意放生詞考「看不看得懂」，
    /// 克漏字整篇都是已知詞，考的是「在情境裡想不想得起來那個字」。
    /// 所以這裡不做覆蓋率驗收（沒有生詞可驗），改成驗空格與題目對不對得上。
    pub(super) async fn generate_cloze(
        &self,
        profile_id: i64,
        learner: &LearnerProfile,
        now: OffsetDateTime,
    ) -> Result<ExerciseView> {
        // 挖「快忘掉的字」優先，補不夠再拿今天到期的。
        // 逾期三週的字在句子裡再想起來一次，價值比剛好今天到期的高。
        let mut blanks = cards::shaky_words(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            now,
            CLOZE_BLANKS,
        )
        .await?;
        // 補不夠時拿今天到期的，但**只拿學過的**。
        //
        // `due_words` 不看 reps：牌組裡剛加進去、他從來沒看過的新卡
        // 也算「到期」。拿那種字來挖空，考的就不是「想不想得起來」，
        // 而是「猜不猜得中」——四個選項裡他一個都沒學過。
        for word in self
            .studied_due_words(profile_id, CLOZE_BLANKS, now)
            .await?
        {
            if blanks.len() >= CLOZE_BLANKS as usize {
                break;
            }
            if !blanks.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
                blanks.push(word);
            }
        }

        if blanks.is_empty() {
            return Err(PracticeError::BadResponse(
                "牌組裡還沒有可以拿來挖空的字，先去複習頁學幾個字再回來".into(),
            ));
        }

        let known_sample = self.known_sample(profile_id, learner.vocabulary).await?;
        let excerpt = self
            .material_excerpt(&blanks, now.unix_timestamp() as u64)
            .await?;

        let topic = self
            .choose_topic(profile_id, ExerciseKind::Cloze, PROSE_KINDS, now)
            .await?;
        // 指定教材時取材範圍由課本決定，主題輪換就不該再插手
        let topic = if excerpt.is_some() { None } else { topic };

        let mut req = prompts::cloze_passage(&prompts::ClozeSpec {
            target_lang: self.target_name(),
            native_lang: self.native_name(),
            word_count: practice::reading_length(learner.vocabulary),
            known_word_count: learner.vocabulary as usize,
            known_sample: &known_sample,
            blank_words: &blanks,
            topic: topic.as_deref(),
            material_excerpt: excerpt.as_deref(),
        });
        let value = self
            .ask_valid_json(
                profile_id,
                "generate",
                &mut req,
                crate::validate::check_cloze,
            )
            .await?;

        let passage = value
            .get("passage")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string();

        let mut items: Vec<ChoiceItem> = parse_choice_items(&value, "items");

        // 空格與題目一定要對得上。模型會跳號、會亂序、會多給一題、
        // 會忘了挖空——不檢查的話使用者會看到「有題目卻沒有空格」，
        // 而且送出之後判分還是照跑，錯得無聲無息。
        //
        // 亂序（`[2, 5, 1, 3, …]`）是最常見的一種，而它其實不是壞資料：
        // 編號沒少也沒重複，只是沒照文章順序寫。那個在本地改得掉，
        // 所以重新編號而不是丟掉整份題目。
        if passage.trim().is_empty() || items.is_empty() {
            return Err(PracticeError::BadResponse("沒有產出挖空的文章".into()));
        }

        let (passage, order) = practice::renumber_blanks(&passage, items.len());
        if order.is_empty() {
            return Err(PracticeError::BadResponse(
                "文章裡一個空格都沒有，克漏字沒有東西可以作答".into(),
            ));
        }
        if order.len() != items.len() {
            tracing::warn!(
                blanks = order.len(),
                items = items.len(),
                "克漏字有題目沒有對應的空格，那些題目丟掉"
            );
        }
        // 依空格的出現順序重排；沒有空格的題目在這一步自然被丟掉
        items = order.iter().map(|&i| items[i].clone()).collect();

        // 補解說要在洗牌之前，否則補回來的解說會配到別的選項上
        self.fill_option_notes(
            profile_id,
            &mut req,
            &mut items,
            "items",
            &value.to_string(),
        )
        .await?;
        shuffle_answers(&mut items, shuffle_seed(now));

        let body = ExerciseBody::Cloze {
            title: value
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("Cloze")
                .to_string(),
            passage,
            translation: value
                .get("translation")
                .and_then(|t| t.as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string),
            items,
        };

        // target_words 存的是挖掉的複習字，不是新教的字——
        // 批改時 `injects_new_words` 因此不能把克漏字算進去。
        self.store(
            profile_id,
            ExerciseKind::Cloze,
            body,
            blanks,
            None,
            topic.as_deref(),
            now,
        )
        .await
    }

    pub(super) async fn generate_grammar(
        &self,
        profile_id: i64,
        learner: &LearnerProfile,
        now: OffsetDateTime,
    ) -> Result<ExerciseView> {
        // 使用者指定了要練哪個文法點就練那個；沒指定就用今天到期的弱點
        let wanted: Vec<String> = match &self.grammar_focus {
            Some(point) => vec![point.clone()],
            None => learner.weak_grammar.clone(),
        };
        let points = self.grammar_points(now).await?;

        // 指定的點要連定義一起帶給模型。識別碼是使用者可以自己取的
        // （`grammar_def` 開放編輯與匯入），`te-form` 這種只有作者看得懂
        // 的字串丟過去，模型只能猜它在考什麼。查不到定義時仍然照樣指定，
        // 只是名稱退回識別碼本身——那也還是比不指定好。
        let focus_def = match &self.grammar_focus {
            Some(point) => grammar::get_def(self.db, &self.target_lang, point).await?,
            None => None,
        };
        let focus = match &self.grammar_focus {
            Some(point) => prompts::DrillFocus::Point(prompts::PointBrief {
                point,
                name: focus_def
                    .as_ref()
                    .map_or(point.as_str(), |d| d.name.as_str()),
                explanation: focus_def.as_ref().and_then(|d| d.explanation.as_deref()),
            }),
            None => prompts::DrillFocus::Weak(&learner.weak_grammar),
        };

        let known_sample = self.known_sample(profile_id, learner.vocabulary).await?;
        let excerpt = self
            .material_excerpt(&wanted, now.unix_timestamp() as u64)
            .await?;

        let mut req = prompts::grammar_drill(
            self.target_name(),
            self.native_name(),
            &points,
            focus,
            &known_sample,
            GRAMMAR_BATCH as usize,
            excerpt.as_deref(),
        );
        // 指定了點就連「有沒有真的在練這個點」一起驗。prompt 講得再死
        // 也只是請求，而這件事本地驗得到：標籤對不上就把題目退回去重出。
        let focus_point = self.grammar_focus.clone();
        let value = self
            .ask_valid_json(profile_id, "generate", &mut req, |v| {
                let mut problems = crate::validate::check_choice_items("items", v);
                if let Some(point) = focus_point.as_deref() {
                    problems.extend(crate::validate::check_grammar_focus("items", v, point));
                }
                problems
            })
            .await?;

        let mut items: Vec<ChoiceItem> = parse_choice_items(&value, "items");

        // 驗收不當閘門，所以修不好的題目還是會在這裡被丟掉——差別是
        // 現在丟掉之前已經 log 過、也給過模型一次修正的機會。
        // 一題都不剩就真的沒有東西可以交出去了。
        if items.is_empty() {
            return Err(PracticeError::BadResponse("一題都沒產出來".into()));
        }

        // 補解說要在洗牌之前，否則補回來的解說會配到別的選項上
        self.fill_option_notes(
            profile_id,
            &mut req,
            &mut items,
            "items",
            &value.to_string(),
        )
        .await?;
        shuffle_answers(&mut items, shuffle_seed(now));

        self.store(
            profile_id,
            ExerciseKind::Grammar,
            ExerciseBody::Choices { items },
            Vec::new(),
            None,
            None,
            now,
        )
        .await
    }
}
