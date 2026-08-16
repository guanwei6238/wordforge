//! 批改，以及批改的產物。
//!
//! 選擇題在本地判分（模型從頭到尾沒看過使用者選了什麼），翻譯題交給模型。
//! 兩條路最後都會匯到同一件事：把不會的字排進複習——「從錯誤裡看出他其實
//! 不會這個字」本來就是老師的工作，不該要使用者自己判斷。
//!
//! 生字解釋（`glossary`）刻意**不是模型產生的**，是拿文章去查使用者自己
//! 匯入的字典。模型對小語種的解釋沒有保證，字典查得到就是查得到。

use super::*;

impl PracticeEngine<'_> {
    // ------------------------------------------------------------ 批改

    /// 批改作答，並把「他不會的字」排進複習。
    pub async fn grade(
        &self,
        profile_id: i64,
        input: &GradeInput,
        now: OffsetDateTime,
    ) -> Result<Feedback> {
        let record = exercises::get(self.db, ExerciseId(input.exercise_id))
            .await?
            .ok_or(PracticeError::NotFound)?;
        let body: ExerciseBody = serde_json::from_str(&record.payload_json)
            .map_err(|e| PracticeError::BadResponse(e.to_string()))?;

        // 批改時讓模型看到「這個人最近常犯什麼」，它才判斷得出
        // 這次是同一個老毛病還是新問題
        let weak_points =
            grammar::due_points(self.db, ProfileId(profile_id), now, GRAMMAR_BATCH).await?;

        let mut feedback = match &body {
            ExerciseBody::Translation { to_target, items } => {
                self.grade_translation(profile_id, *to_target, items, input, &weak_points)
                    .await?
            }
            ExerciseBody::Reading {
                passage, questions, ..
            } => {
                self.grade_reading(profile_id, passage, questions, input)
                    .await?
            }
            // 克漏字在本地判分就夠了：答案是選出來的，對錯沒有模糊空間。
            // 答錯的那幾個字要排回複習——挖空的本來就是他該複習的字，
            // 填不出來就是「還沒真的會」，比到期時間更直接的證據。
            ExerciseBody::Cloze { items, passage, .. } => {
                let mut fb = grade_choices(items, input);
                fb.unknown_words = missed_words(items, input);
                // 解析時點文章裡的字要查得到意思，跟閱讀一樣。
                // 這一份完全來自本地字典，不多打一次模型。
                fb.glossary = self.passage_glossary(profile_id, passage).await?;
                fb
            }
            ExerciseBody::Choices { items } => grade_choices(items, input),
        };

        // 使用者自己點出來的字，跟模型判斷的合併
        for word in &input.marked_unknown {
            if !feedback
                .unknown_words
                .iter()
                .any(|w| w.eq_ignore_ascii_case(word))
            {
                feedback.unknown_words.push(word.clone());
            }
        }

        // 這篇刻意教的生詞也一起排進複習。
        //
        // 那些字是系統自己挑的——「你程度上緣、還沒學過」——然後放進
        // 一篇有上下文的文章讓他讀。讀完就是這個字的第一次接觸，
        // 該進牌組了。
        //
        // 不這樣做的話使用者得自己一個一個點「我不會」才會被記錄，
        // 而人不會這樣做：從上下文看懂了就往下讀了。結果是那些字
        // 永遠留在候選池裡，下一篇又拿到同一批。
        //
        // 進了牌組還有一個好處：`new_word_candidates` 排除牌組裡的字，
        // 所以輪換從此是永久的，五篇的記憶視窗只是還沒作答時的備援。
        // 只有閱讀類的 `target_words` 是「刻意挑的生詞」。翻譯題存的是
        // 到期的複習字——那些他已經在學了，當成新教的字加進牌組是錯的。
        let injects_new_words = matches!(body, ExerciseBody::Reading { .. });
        let taught: Vec<String> = record
            .target_words
            .iter()
            .filter(|_| injects_new_words)
            .filter(|w| {
                !feedback
                    .unknown_words
                    .iter()
                    .any(|u| u.eq_ignore_ascii_case(w))
            })
            .cloned()
            .collect();

        let mut to_add = feedback.unknown_words.clone();
        to_add.extend(taught.iter().cloned());

        feedback.added_to_deck = self.add_unknown_words(profile_id, &to_add, now).await?;
        // UI 要分得出「你不會所以幫你加」與「這篇教的，順便加」
        feedback.taught_words = taught;

        self.record_grammar_results(profile_id, &body, input, &feedback, now)
            .await?;

        let answer_json = serde_json::to_string(input).unwrap_or_else(|_| "{}".into());
        let feedback_json = serde_json::to_string(&feedback).unwrap_or_else(|_| "{}".into());
        exercises::record_attempt(
            self.db,
            ExerciseId(input.exercise_id),
            &answer_json,
            feedback.score,
            &feedback_json,
            now,
        )
        .await?;

        Ok(feedback)
    }

    pub(super) async fn grade_translation(
        &self,
        profile_id: i64,
        to_target: bool,
        items: &[TranslationItem],
        input: &GradeInput,
        weak_points: &[String],
    ) -> Result<Feedback> {
        let pairs: Vec<(String, String)> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                (
                    item.source.clone(),
                    input.answers.get(i).cloned().unwrap_or_default(),
                )
            })
            .collect();

        let points = self.grammar_points(OffsetDateTime::now_utc()).await?;
        let req = prompts::translation_feedback(
            self.target_name(),
            self.native_name(),
            to_target,
            &pairs,
            weak_points,
            &points,
        );
        let value = self.ask_json(profile_id, "grade", &req).await?;
        serde_json::from_value(value).map_err(|e| PracticeError::BadResponse(e.to_string()))
    }

    pub(super) async fn grade_reading(
        &self,
        profile_id: i64,
        passage: &str,
        questions: &[ChoiceItem],
        input: &GradeInput,
    ) -> Result<Feedback> {
        let triples: Vec<(String, String, String)> = questions
            .iter()
            .enumerate()
            .map(|(i, q)| {
                let picked = input
                    .choices
                    .get(i)
                    .copied()
                    .flatten()
                    .and_then(|idx| q.options.get(idx).cloned())
                    .unwrap_or_default();
                let correct = q.options.get(q.answer_index).cloned().unwrap_or_default();
                (q.question.clone(), picked, correct)
            })
            .collect();

        let req =
            prompts::reading_feedback(self.target_name(), self.native_name(), passage, &triples);
        let value = self.ask_json(profile_id, "grade", &req).await?;
        let mut feedback: Feedback =
            serde_json::from_value(value).map_err(|e| PracticeError::BadResponse(e.to_string()))?;

        // 分數與對錯都在本地算，不必相信模型的算術。
        // 講評則要照它自己給的 index 對回題號——照陣列位置貼的話，
        // 模型少回一題或換個順序，第三題的講評就會出現在第二題底下，
        // 而那個畫面看起來完全正常，沒有人會發現。
        let local = grade_choices(questions, input);
        feedback.items = align_comments(&feedback.items, local.items);
        feedback.score = local.score;

        feedback.glossary = self.passage_glossary(profile_id, passage).await?;
        Ok(feedback)
    }

    /// 一篇文章的解析：每個字的意思、哪些他不會、哪裡有片語。
    ///
    /// 閱讀與克漏字共用。全部本地做——查的是使用者自己匯入的字典，
    /// 不佔 token，也不用等 CLI 冷啟動。
    ///
    /// 「生字」的定義要跟出題時一致，否則會把他早就會的字全列出來。
    pub(super) async fn passage_glossary(
        &self,
        profile_id: i64,
        passage: &str,
    ) -> Result<Vec<GlossaryNote>> {
        let learner = self
            .learner_profile(profile_id, OffsetDateTime::now_utc())
            .await?;
        let known = cards::known_vocabulary(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            learner.vocabulary,
            KNOWN_STABILITY_DAYS,
        )
        .await?;
        self.build_glossary(passage, &known).await
    }

    /// 從文章本身算出解析：每個字的意思、哪些你不會、哪裡有片語。
    ///
    /// 全部本地做，一次 LLM 都不呼叫。理由有三個：
    ///
    /// 1. **比模型準**。「他不會這個字」是拿文章去比對牌組算出來的，
    ///    不是模型猜的。90% 法則裡那不足 10%，這裡列的就是它本人。
    /// 2. **語言無關**。模型對小語種的解釋品質沒有保證，但字典是
    ///    使用者自己匯入的——查得到就是查得到。
    /// 3. **免費且即時**。不佔 token，也不用等 CLI 冷啟動。
    ///
    /// ## 為什麼連「已經會的字」也查
    ///
    /// 解析階段點文章裡的任何一個字都要看得到翻譯——只查生字的話，
    /// 點到一個系統以為你會、你其實忘了的字，會什麼都不跳出來，
    /// 那個互動就顯得壞掉了。`is_unknown` 仍然分得出兩者，
    /// UI 要挑出「這篇的生字」時看那個欄位就好。
    pub(super) async fn build_glossary(
        &self,
        passage: &str,
        known: &std::collections::HashSet<LemmaId>,
    ) -> Result<Vec<GlossaryNote>> {
        let tokens = wordforge_core::text::tokenize(passage);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }

        // 實詞全查。虛詞（the、of）跳過——查得到但沒有人需要看。
        let mut content_words: Vec<String> = Vec::new();
        let mut unknown: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut seen = std::collections::HashSet::new();
        for token in &tokens {
            if !seen.insert(token.clone()) {
                continue;
            }
            if wordforge_core::wordlist::is_function_word(&self.target_lang, token) {
                continue;
            }
            if !self.knows_token(token, known).await? {
                unknown.insert(token.clone());
            }
            content_words.push(token.clone());
        }

        // 片語：字典裡真的有這個多詞條目才算。整組都是虛詞的排掉
        // （`to be`、`there is` 這種在字典裡也查得到，但列出來只是雜訊）。
        let candidates: Vec<String> =
            wordforge_core::text::ngrams(&tokens, &self.target_lang, MAX_PHRASE_LEN)
                .into_iter()
                .filter(|p| {
                    !p.split_whitespace()
                        .all(|w| wordforge_core::wordlist::is_function_word(&self.target_lang, w))
                })
                .collect();

        let phrases = dict::glossary(self.db, &self.target_lang, &candidates).await?;
        let words = dict::glossary(self.db, &self.target_lang, &content_words).await?;

        let mut notes: Vec<GlossaryNote> = Vec::new();
        for e in phrases {
            notes.push(GlossaryNote {
                term: e.term,
                text: e.text,
                gloss: e.gloss,
                translation: e.translation,
                is_phrase: true,
                is_unknown: true,
            });
        }
        for e in words {
            let is_unknown = unknown.contains(&e.term);
            notes.push(GlossaryNote {
                term: e.term,
                text: e.text,
                gloss: e.gloss,
                translation: e.translation,
                is_phrase: false,
                is_unknown,
            });
        }
        // 片語排前面：那才是查單字查不到的東西
        notes.sort_by(|a, b| b.is_phrase.cmp(&a.is_phrase).then(a.text.cmp(&b.text)));
        Ok(notes)
    }

    /// 把不懂的字加進牌組。
    ///
    /// 只加字典裡查得到的：模型偶爾會回傳片語、拼錯的字、或根本不是單字的東西，
    /// 那些不該變成卡片。回傳實際加進去的字。
    pub(super) async fn add_unknown_words(
        &self,
        profile_id: i64,
        words: &[String],
        now: OffsetDateTime,
    ) -> Result<Vec<String>> {
        let mut added = Vec::new();
        for word in words {
            let normalized = wordforge_core::text::normalize(word);
            if normalized.is_empty() {
                continue;
            }
            // 建在原形上：答錯 `studied` 該去複習 `study`，
            // 不是多開一張 `studied` 的卡跟既有的 `study` 各自排程
            let Some(lemma_id) = lemmas::base_form(self.db, &self.target_lang, &normalized).await?
            else {
                tracing::debug!(word, "字典裡查不到，不建卡");
                continue;
            };

            let card = cards::ensure(
                self.db,
                ProfileId(profile_id),
                lemma_id,
                CardKind::Recognition,
                now,
            )
            .await?;

            // 已經在複習的字不用再提醒一次
            if card.reps == 0 {
                added.push(word.clone());
            }
        }
        Ok(added)
    }
}
