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

        // 翻譯的每一句各自排程：錯的明天再出現，對的就結束。
        // 這是「練到 100 分」的機制——不排的話，錯的那幾句只是靜靜地
        // 留在紀錄裡，而它們正是他還不會的東西。
        if let ExerciseBody::Translation { items, .. } = &body {
            let indices: Vec<usize> = (0..items.len()).collect();
            self.schedule_sentences(profile_id, input.exercise_id, &indices, &feedback, now)
                .await?;
            self.tag_sentences(profile_id, input.exercise_id, &feedback, &input.answers)
                .await?;
        }

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

    /// 只重寫沒全對的那幾題，其餘沿用上一次的批改。
    ///
    /// ## 為什麼不整份重來
    ///
    /// 翻譯題的批改是一次完整的模型呼叫。五題裡錯兩題，整份重送等於
    /// 為了那兩題燒掉一整次的額度與幾十秒；而且已經對的那三題會被
    /// 重新判一次——同一句話兩次批改的結果不一定一樣，使用者會看到
    /// 「我明明沒動它，怎麼變成錯的」。
    ///
    /// ## 分數改成本地算
    ///
    /// 合併過的這一份，模型從來沒有看過全貌，它給的分數不會是整份的
    /// 分數。所以這裡用「答對幾題 ÷ 總題數」自己算——這也讓「做到
    /// 100 分」有一個說得清楚的定義：每一題都對。
    ///
    /// `redone` 是 (題號, 新的作答)，題號從 0 起算。
    pub async fn regrade(
        &self,
        profile_id: i64,
        exercise_id: i64,
        redone: &[(usize, String)],
        now: OffsetDateTime,
    ) -> Result<Feedback> {
        let record = exercises::get(self.db, ExerciseId(exercise_id))
            .await?
            .ok_or(PracticeError::NotFound)?;
        let body: ExerciseBody = serde_json::from_str(&record.payload_json)
            .map_err(|e| PracticeError::BadResponse(e.to_string()))?;

        let ExerciseBody::Translation { to_target, items } = &body else {
            return Err(PracticeError::Unsupported(
                "只有翻譯題可以單獨重寫某幾題。選擇題請用「再做一次」整份重做——\
                 那個不必打模型，重做一次很便宜。"
                    .into(),
            ));
        };

        // 上一次的作答與批改是這次的底：沒重寫的題目原封不動留著
        let past = exercises::attempts(self.db, ExerciseId(exercise_id)).await?;
        let last = past.last().ok_or(PracticeError::Unsupported(
            "這份練習還沒有作答過，沒有東西可以重寫。".into(),
        ))?;
        let previous: GradeInput = serde_json::from_str(&last.answer_json)
            .map_err(|e| PracticeError::BadResponse(e.to_string()))?;
        let mut merged: Feedback = serde_json::from_str(&last.feedback_json)
            .map_err(|e| PracticeError::BadResponse(e.to_string()))?;

        // 題號超出範圍就當作沒這一題，不要 panic 也不要靜靜地改到別題
        let mut wanted: Vec<(usize, String)> = redone
            .iter()
            .filter(|(i, _)| *i < items.len())
            .cloned()
            .collect();
        if wanted.is_empty() {
            return Err(PracticeError::Unsupported("沒有指定要重寫哪一題。".into()));
        }

        // 一句每天只練一次。當天反覆重寫同一句刷到全對，看起來是 100 分，
        // 實際上只是背下了剛剛看到的參考答案——而參考答案就印在上面那一行。
        let mut locked = Vec::new();
        for (i, _) in &wanted {
            if sentences::practised_today(
                self.db,
                ProfileId(profile_id),
                exercise_id,
                *i as i64,
                now,
            )
            .await?
            {
                locked.push(*i + 1);
            }
        }
        wanted.retain(|(i, _)| !locked.contains(&(*i + 1)));
        if wanted.is_empty() {
            return Err(PracticeError::Unsupported(format!(
                "第 {} 題今天已經練過了。明天會再出現——隔一天再想一次才是複習，\
                 現在重寫只是抄上面的參考答案。",
                locked
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join("、")
            )));
        }
        let redone = wanted;

        let mut answers = previous.answers.clone();
        answers.resize(items.len(), String::new());
        for (i, text) in &redone {
            answers[*i] = text.clone();
        }

        // 只把重寫的那幾題送出去
        let pairs: Vec<(String, String)> = redone
            .iter()
            .map(|(i, text)| (items[*i].source.clone(), text.clone()))
            .collect();
        let weak_points =
            grammar::due_points(self.db, ProfileId(profile_id), now, GRAMMAR_BATCH).await?;
        let points = self.grammar_points(now).await?;
        let req = prompts::translation_feedback(
            self.target_name(),
            self.native_name(),
            *to_target,
            &pairs,
            &weak_points,
            &points,
        );
        let value = self.ask_json(profile_id, "grade", &req).await?;
        let fresh: Feedback =
            serde_json::from_value(value).map_err(|e| PracticeError::BadResponse(e.to_string()))?;

        // 回來的結果是**子集**的題號（第 1 題指的是重寫清單裡的第一題），
        // 要對回原本的題號。模型沒給 index 或給了範圍外的值時退回照順序貼——
        // 貼錯的話使用者會看到「我改的是第 4 題，講評卻掛在第 2 題」。
        merged.items.resize_with(items.len(), ItemResult::default);
        for (k, result) in fresh.items.iter().enumerate() {
            let within = result
                .index
                .checked_sub(1)
                .filter(|idx| *idx < redone.len())
                .unwrap_or(k);
            let Some((target, _)) = redone.get(within) else {
                continue;
            };
            merged.items[*target] = ItemResult {
                index: *target + 1,
                ..result.clone()
            };
        }

        // 修正建議換成這一次的：它們只講重寫的那幾題，舊的那些仍然
        // 留在上一次的紀錄裡，兩份對照著看才知道改掉了什麼
        merged.corrections = fresh.corrections;
        merged.unknown_words = fresh.unknown_words.clone();
        merged.taught_words.clear();

        let correct = merged.items.iter().filter(|r| r.correct).count();
        merged.score = Some(100.0 * correct as f64 / items.len() as f64);

        for word in &previous.marked_unknown {
            if !merged
                .unknown_words
                .iter()
                .any(|w| w.eq_ignore_ascii_case(word))
            {
                merged.unknown_words.push(word.clone());
            }
        }
        let to_add = merged.unknown_words.clone();
        merged.added_to_deck = self.add_unknown_words(profile_id, &to_add, now).await?;

        let input = GradeInput {
            exercise_id,
            answers,
            choices: Vec::new(),
            marked_unknown: previous.marked_unknown.clone(),
        };
        self.record_grammar_results(profile_id, &body, &input, &merged, now)
            .await?;

        // 只更新這次重寫的那幾句：沒動到的題目不該因為這次送出
        // 就被記成「今天練過了」，那會讓它們明天才回來。
        let touched: Vec<usize> = redone.iter().map(|(i, _)| *i).collect();
        self.schedule_sentences(profile_id, exercise_id, &touched, &merged, now)
            .await?;
        self.tag_sentences(profile_id, exercise_id, &merged, &input.answers)
            .await?;

        let answer_json = serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());
        let feedback_json = serde_json::to_string(&merged).unwrap_or_else(|_| "{}".into());
        exercises::record_attempt(
            self.db,
            ExerciseId(exercise_id),
            &answer_json,
            merged.score,
            &feedback_json,
            now,
        )
        .await?;

        Ok(merged)
    }

    /// 把「這一句錯在哪個文法點」記到句子上。
    ///
    /// 批改本來就會標文法點，而且那些標籤已經在驅動文法題的排程——
    /// 缺的只是對回句子。對不回去的就丟掉：把「你在冠詞上錯過」掛到
    /// 一句根本沒有冠詞問題的話上，畫面看不出來。
    async fn tag_sentences(
        &self,
        profile_id: i64,
        exercise_id: i64,
        feedback: &Feedback,
        answers: &[String],
    ) -> Result<()> {
        let points = self.grammar_points(OffsetDateTime::now_utc()).await?;
        let mut by_item: std::collections::HashMap<usize, Vec<String>> =
            std::collections::HashMap::new();

        for (item, raw) in attribute_corrections(&feedback.corrections, answers) {
            // 收斂到受控清單，理由跟排程那邊一樣：不收的話同一個點會散成
            // tense / past tense / Verb Tense 好幾個標籤
            let Some(point) = self.normalize_point(&points, &raw) else {
                continue;
            };
            by_item.entry(item).or_default().push(point);
        }

        for (item, points) in by_item {
            word_sentences::add_grammar_points(
                self.db,
                ProfileId(profile_id),
                exercise_id,
                item as i64,
                &points,
            )
            .await?;
        }
        Ok(())
    }

    /// 把舊練習答錯的句子補進複習排程。
    ///
    /// 排程是在**批改的當下**寫入的，所以句子複習這個功能上線之前做過的
    /// 練習一句都沒排進去。實測使用者的資料庫：10 份翻譯練習裡只有功能
    /// 上線後的那一份有排程，前面 9 份共 35 句答錯的永遠不會回來——
    /// 而那些正是最該回來的。
    ///
    /// 補得回來是因為逐題對錯還留在 `attempt.feedback_json` 裡。
    /// 用**當時的時間**排而不是現在：一句 8/12 做錯的排到 8/13，
    /// 早就過期，開起來就能練；用現在的時間會變成「明天再說」。
    ///
    /// 已經有排程的練習整份跳過：`miss` 是 `misses + 1`，重跑會讓
    /// 錯誤次數憑空多一次。
    ///
    /// 回傳補了幾句。
    pub async fn backfill_sentence_reviews(
        &self,
        profile_id: i64,
        now: OffsetDateTime,
    ) -> Result<u64> {
        let key = format!("sentence_review_backfill:{profile_id}");
        if wordforge_db::meta::get_i64(self.db, &key).await? == Some(REVIEW_BACKFILL_VERSION) {
            return Ok(0);
        }
        let pid = ProfileId(profile_id);

        let mut done = 0;
        let mut offset = 0i64;
        loop {
            let batch = exercises::recent(self.db, pid, 100, offset).await?;
            if batch.is_empty() {
                break;
            }
            offset += batch.len() as i64;

            for record in batch {
                // 只有翻譯題的句子是「一題」——閱讀與克漏字的句子對不回排程
                if !record.kind.starts_with("translation") {
                    continue;
                }
                if sentences::has_any(self.db, record.id).await? {
                    continue;
                }
                let Some(raw) = record.feedback_json.as_deref() else {
                    continue;
                };
                let Ok(feedback) = serde_json::from_str::<Feedback>(raw) else {
                    continue;
                };
                let when = wordforge_db::ts::from_sql(&record.created_at).unwrap_or(now);
                for (i, item) in feedback.items.iter().enumerate() {
                    // 寫對的不排：那一句練起來了，這正是 `pass` 的語意
                    if item.correct {
                        continue;
                    }
                    sentences::miss(self.db, pid, record.id, i as i64, when).await?;
                    word_sentences::mark_missed(self.db, pid, record.id, i as i64).await?;
                    done += 1;
                }
            }
        }

        wordforge_db::meta::set_i64(self.db, &key, REVIEW_BACKFILL_VERSION).await?;
        Ok(done)
    }

    /// 把翻譯的每一句排進（或移出）複習。
    ///
    /// 沒有逐題結果的那幾句**不動**：模型偶爾只回一個總分，那時候
    /// 說不出這一句對不對。猜「對」會讓還不會的句子從此消失，
    /// 猜「錯」會讓明明寫對的句子明天又冒出來——兩個都比不動更糟。
    async fn schedule_sentences(
        &self,
        profile_id: i64,
        exercise_id: i64,
        indices: &[usize],
        feedback: &Feedback,
        now: OffsetDateTime,
    ) -> Result<()> {
        let pid = ProfileId(profile_id);
        for &i in indices {
            let Some(result) = feedback.items.get(i) else {
                continue;
            };
            if result.correct {
                sentences::pass(self.db, pid, exercise_id, i as i64).await?;
            } else {
                sentences::miss(self.db, pid, exercise_id, i as i64, now).await?;
                // 次數同時累計到句子那一側：排程那張表在寫對之後整列會被刪掉，
                // 而「這句我錯過三次」正是練起來之後最值得留著的訊號
                word_sentences::mark_missed(self.db, pid, exercise_id, i as i64).await?;
            }
        }
        Ok(())
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
