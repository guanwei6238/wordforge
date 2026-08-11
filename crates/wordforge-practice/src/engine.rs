//! 出題與批改的編排。

use time::OffsetDateTime;
use wordforge_core::model::{CardKind, LemmaId, ProfileId};
use wordforge_core::practice::{self, ExerciseKind, LearnerProfile};
use wordforge_db::Db;
use wordforge_db::exercises::{self, ExerciseId, NewExercise};
use wordforge_db::repo::{cards, lemmas};
use wordforge_llm::{LlmProvider, prompts};

use crate::payload::*;

#[derive(Debug, thiserror::Error)]
pub enum PracticeError {
    #[error(transparent)]
    Db(#[from] wordforge_db::DbError),

    #[error(transparent)]
    Llm(#[from] wordforge_llm::LlmError),

    #[error("模型回傳的內容看不懂：{0}")]
    BadResponse(String),

    #[error("找不到這份練習")]
    NotFound,

    #[error("詞彙量還不夠出這種題目")]
    NotEnoughVocabulary,
}

pub type Result<T> = std::result::Result<T, PracticeError>;

/// 「算是會了」的門檻（天）。與複習頁的統計一致。
const KNOWN_STABILITY_DAYS: f64 = 21.0;

/// 出題時給模型看的已知詞樣本數。
///
/// 不能把幾千個字全塞進 prompt——那會吃掉整個 context，
/// 而且模型並不需要完整清單才知道該用什麼難度的字。
const KNOWN_SAMPLE: i64 = 60;

/// 閱讀理解的目標覆蓋率。生詞控制在 4% 左右。
const READING_COVERAGE: f64 = 0.96;

/// 覆蓋率不合格時最多重試幾次。
///
/// 連續失敗代表目標詞選得太難，該退回去換一批更常用的字，
/// 而不是無限重試燒 token。
const COVERAGE_RETRIES: usize = 2;

pub struct PracticeEngine<'a> {
    db: &'a Db,
    llm: &'a dyn LlmProvider,
    pub target_lang: String,
    pub native_lang: String,
}

impl<'a> PracticeEngine<'a> {
    pub fn new(db: &'a Db, llm: &'a dyn LlmProvider) -> Self {
        Self {
            db,
            llm,
            target_lang: "English".into(),
            native_lang: "繁體中文".into(),
        }
    }

    // ------------------------------------------------------------ 學習者狀態

    /// 蒐集出題需要知道的一切。
    pub async fn learner_profile(&self, profile_id: i64) -> Result<LearnerProfile> {
        let pid = ProfileId(profile_id);

        // 分級測驗的估計優先：它反映使用者真正的程度，
        // 而卡片數只算得到「在這個 App 裡學過的」。
        let estimated: Option<i64> = sqlx_scalar_estimated(self.db, profile_id).await?;
        let mastered = cards::known_lemma_ids(self.db, pid, KNOWN_STABILITY_DAYS)
            .await?
            .len() as i64;

        let weak_grammar = exercises::weak_grammar_points(self.db, pid, 20, 5).await?;
        let recent_kinds = exercises::recent_kinds(self.db, pid, 6)
            .await?
            .iter()
            .filter_map(|k| parse_kind(k))
            .collect();

        Ok(LearnerProfile {
            vocabulary: estimated.unwrap_or(0).max(mastered),
            weak_grammar,
            recent_kinds,
        })
    }

    // ------------------------------------------------------------ 出題

    /// 產生一份練習。`kind` 給 `None` 就由系統依程度決定。
    pub async fn generate(
        &self,
        profile_id: i64,
        kind: Option<ExerciseKind>,
        now: OffsetDateTime,
    ) -> Result<ExerciseView> {
        let learner = self.learner_profile(profile_id).await?;
        let kind = kind.unwrap_or_else(|| practice::recommend_kind(&learner));

        if learner.vocabulary < kind.min_vocabulary() {
            return Err(PracticeError::NotEnoughVocabulary);
        }

        match kind {
            ExerciseKind::TranslationToTarget | ExerciseKind::TranslationToNative => {
                self.generate_translation(profile_id, kind, &learner, now)
                    .await
            }
            ExerciseKind::Reading | ExerciseKind::Cloze => {
                self.generate_reading(profile_id, &learner, now).await
            }
            ExerciseKind::Grammar => self.generate_grammar(profile_id, &learner, now).await,
        }
    }

    async fn generate_translation(
        &self,
        profile_id: i64,
        kind: ExerciseKind,
        learner: &LearnerProfile,
        now: OffsetDateTime,
    ) -> Result<ExerciseView> {
        // 用今天到期的字出題：翻譯的時候順便複習到了
        let due_words = self.due_words(profile_id, 8, now).await?;
        let count = practice::translation_count(learner.vocabulary);

        let req =
            prompts::translation_task(&self.target_lang, &self.native_lang, &due_words, count);
        let value = self.ask_json(&req).await?;

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
        self.store(profile_id, kind, body, due_words, None, now)
            .await
    }

    async fn generate_reading(
        &self,
        profile_id: i64,
        learner: &LearnerProfile,
        now: OffsetDateTime,
    ) -> Result<ExerciseView> {
        let known =
            cards::known_lemma_ids(self.db, ProfileId(profile_id), KNOWN_STABILITY_DAYS).await?;
        let known_sample = self.known_sample(profile_id).await?;
        let word_count = practice::reading_length(learner.vocabulary);
        let target_words = self.due_words(profile_id, 6, now).await?;

        let spec = prompts::ReadingSpec {
            target_lang: &self.target_lang,
            native_lang: &self.native_lang,
            word_count,
            target_coverage: READING_COVERAGE,
            known_word_count: learner.vocabulary as usize,
            cefr: None,
            known_sample: &known_sample,
            target_words: &target_words,
            topic: None,
            material_excerpt: None,
            question_count: 4,
        };

        let mut req = prompts::reading_comprehension(&spec);

        for attempt in 0..=COVERAGE_RETRIES {
            let value = self.ask_json(&req).await?;
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

            // 只有「太難」才重寫。太簡單也不理想（這次學不到新字），
            // 但重試訊息講的是「把難字換掉」，拿去處理太簡單的文章只會更糟；
            // 而且對學習者來說，讀一篇太簡單的文章無害，讀不懂的才是災難。
            let too_hard = coverage.band() == wordforge_core::coverage::CoverageBand::TooHard;
            let acceptable = !too_hard || attempt == COVERAGE_RETRIES;
            if acceptable {
                let questions: Vec<ChoiceItem> = value
                    .get("questions")
                    .and_then(|q| q.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|q| serde_json::from_value(q.clone()).ok())
                            .collect()
                    })
                    .unwrap_or_default();

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
                READING_COVERAGE,
                &offenders,
            ));
        }

        Err(PracticeError::BadResponse("重試後覆蓋率仍不合格".into()))
    }

    async fn generate_grammar(
        &self,
        profile_id: i64,
        learner: &LearnerProfile,
        now: OffsetDateTime,
    ) -> Result<ExerciseView> {
        let known_sample = self.known_sample(profile_id).await?;
        let req = prompts::grammar_drill(
            &self.target_lang,
            &self.native_lang,
            &learner.weak_grammar,
            &known_sample,
            5,
            None,
        );
        let value = self.ask_json(&req).await?;

        let items: Vec<ChoiceItem> = value
            .get("items")
            .and_then(|i| i.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|i| serde_json::from_value(i.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();

        if items.is_empty() {
            return Err(PracticeError::BadResponse("一題都沒產出來".into()));
        }

        self.store(
            profile_id,
            ExerciseKind::Grammar,
            ExerciseBody::Choices { items },
            Vec::new(),
            None,
            now,
        )
        .await
    }

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
            exercises::weak_grammar_points(self.db, ProfileId(profile_id), 20, 5).await?;

        let mut feedback = match &body {
            ExerciseBody::Translation { to_target, items } => {
                self.grade_translation(*to_target, items, input, &weak_points)
                    .await?
            }
            ExerciseBody::Reading {
                passage, questions, ..
            } => self.grade_reading(passage, questions, input).await?,
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

        feedback.added_to_deck = self
            .add_unknown_words(profile_id, &feedback.unknown_words, now)
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

    async fn grade_translation(
        &self,
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

        let req = prompts::translation_feedback(
            &self.target_lang,
            &self.native_lang,
            to_target,
            &pairs,
            weak_points,
        );
        let value = self.ask_json(&req).await?;
        serde_json::from_value(value).map_err(|e| PracticeError::BadResponse(e.to_string()))
    }

    async fn grade_reading(
        &self,
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
            prompts::reading_feedback(&self.target_lang, &self.native_lang, passage, &triples);
        let value = self.ask_json(&req).await?;
        let mut feedback: Feedback =
            serde_json::from_value(value).map_err(|e| PracticeError::BadResponse(e.to_string()))?;

        // 分數可以在本地算，不必相信模型的算術
        let local = grade_choices(questions, input);
        feedback.score = local.score;
        Ok(feedback)
    }

    /// 把不懂的字加進牌組。
    ///
    /// 只加字典裡查得到的：模型偶爾會回傳片語、拼錯的字、或根本不是單字的東西，
    /// 那些不該變成卡片。回傳實際加進去的字。
    async fn add_unknown_words(
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
            let Some(lemma_id) = lemmas::find_by_form(self.db, "en", &normalized).await? else {
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

    // ------------------------------------------------------------ 小工具

    async fn ask_json(&self, req: &wordforge_llm::ChatRequest) -> Result<serde_json::Value> {
        let resp = self.llm.chat(req).await?;
        Ok(resp.json()?)
    }

    /// 今天到期或即將學到的字，拿來當出題素材。
    async fn due_words(
        &self,
        profile_id: i64,
        limit: i64,
        now: OffsetDateTime,
    ) -> Result<Vec<String>> {
        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id
             WHERE c.profile_id = ? AND c.suspended = 0 AND c.due <= ?
             ORDER BY c.due LIMIT ?",
        )
        .bind(profile_id)
        .bind(format_ts(now))
        .bind(limit)
        .fetch_all(self.db.pool())
        .await
        .map_err(wordforge_db::DbError::from)?;
        Ok(words)
    }

    /// 已知詞的抽樣，讓模型感受用字範圍。
    async fn known_sample(&self, profile_id: i64) -> Result<Vec<String>> {
        let words: Vec<String> = sqlx::query_scalar(
            "SELECT l.text FROM card c JOIN lemma l ON l.id = c.lemma_id
             WHERE c.profile_id = ? AND c.state = 'review'
             ORDER BY l.freq_rank IS NULL, l.freq_rank LIMIT ?",
        )
        .bind(profile_id)
        .bind(KNOWN_SAMPLE)
        .fetch_all(self.db.pool())
        .await
        .map_err(wordforge_db::DbError::from)?;
        Ok(words)
    }

    /// 算出文章對這位學習者的實際覆蓋率。
    async fn measure_coverage(
        &self,
        passage: &str,
        known: &std::collections::HashSet<LemmaId>,
    ) -> Result<wordforge_core::coverage::Coverage> {
        // 一次把文章裡的詞查完，避免在 async 閉包裡查資料庫
        let tokens = wordforge_core::text::tokenize(passage);
        let mut lookup = std::collections::HashMap::new();
        for token in &tokens {
            if lookup.contains_key(token) {
                continue;
            }
            let id = lemmas::find_by_form(self.db, "en", token).await?;
            lookup.insert(token.clone(), id);
        }

        Ok(wordforge_core::coverage::analyze(passage, known, |w| {
            lookup.get(w).copied().flatten()
        }))
    }

    async fn store(
        &self,
        profile_id: i64,
        kind: ExerciseKind,
        body: ExerciseBody,
        target_words: Vec<String>,
        coverage: Option<f64>,
        now: OffsetDateTime,
    ) -> Result<ExerciseView> {
        let payload_json =
            serde_json::to_string(&body).map_err(|e| PracticeError::BadResponse(e.to_string()))?;

        let id = exercises::create(
            self.db,
            NewExercise {
                profile_id: ProfileId(profile_id),
                kind: kind.as_str(),
                payload_json: &payload_json,
                target_words: &target_words,
                coverage,
                model: Some(self.llm.model()),
                material_id: None,
            },
            now,
        )
        .await?;

        Ok(ExerciseView {
            exercise_id: id.0,
            kind,
            body,
            target_words,
            coverage,
        })
    }
}

/// 選擇題可以在本地判分，不必浪費一次 LLM 呼叫。
///
/// 答錯的題目要轉成 `corrections`，文法弱點才累積得起來——
/// 這些紀錄正是下次出文法題的依據。少了這一步，
/// 做再多文法練習系統也學不到你哪裡不會。
fn grade_choices(items: &[ChoiceItem], input: &GradeInput) -> Feedback {
    let mut results = Vec::with_capacity(items.len());
    let mut corrections = Vec::new();
    let mut correct_count = 0usize;

    for (i, item) in items.iter().enumerate() {
        let picked = input.choices.get(i).copied().flatten();
        let correct = picked == Some(item.answer_index);
        let answer = item.options.get(item.answer_index).cloned();

        if correct {
            correct_count += 1;
        } else if let Some(point) = item.grammar_point.as_ref().filter(|p| !p.trim().is_empty()) {
            corrections.push(Correction {
                original: picked
                    .and_then(|idx| item.options.get(idx).cloned())
                    .unwrap_or_else(|| "（沒有作答）".to_string()),
                corrected: answer.clone().unwrap_or_default(),
                grammar_point: Some(point.trim().to_string()),
                severity: Some("major".into()),
                explanation: item.explanation.clone(),
            });
        }

        results.push(ItemResult {
            index: i + 1,
            correct,
            reference: answer,
            comment: item.explanation.clone(),
        });
    }

    let score = if items.is_empty() {
        None
    } else {
        Some((correct_count as f64 / items.len() as f64) * 100.0)
    };

    Feedback {
        score,
        items: results,
        corrections,
        ..Default::default()
    }
}

fn parse_kind(s: &str) -> Option<ExerciseKind> {
    Some(match s {
        "translation_to_target" => ExerciseKind::TranslationToTarget,
        "translation_to_native" => ExerciseKind::TranslationToNative,
        "cloze" => ExerciseKind::Cloze,
        "reading" => ExerciseKind::Reading,
        "grammar" => ExerciseKind::Grammar,
        _ => return None,
    })
}

/// 與資料庫其他地方一致的時間格式。
fn format_ts(dt: OffsetDateTime) -> String {
    dt.to_offset(time::UtcOffset::UTC)
        .format(&time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z"
        ))
        .unwrap_or_default()
}

/// 讀出分級測驗估的詞彙量。
async fn sqlx_scalar_estimated(db: &Db, profile_id: i64) -> Result<Option<i64>> {
    let v: Option<i64> = sqlx::query_scalar(
        "SELECT CAST(json_extract(settings_json, '$.estimated_vocabulary') AS INTEGER)
         FROM profile WHERE id = ? AND json_valid(settings_json)",
    )
    .bind(profile_id)
    .fetch_optional(db.pool())
    .await
    .map_err(wordforge_db::DbError::from)?
    .flatten();
    Ok(v)
}
