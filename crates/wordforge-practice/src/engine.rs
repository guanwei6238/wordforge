//! 出題與批改的編排。

use time::OffsetDateTime;
use wordforge_core::model::{CardKind, LemmaId, ProfileId};
use wordforge_core::practice::{self, ExerciseKind, LearnerProfile};
use wordforge_db::Db;
use wordforge_db::dict;
use wordforge_db::exercises::{self, ExerciseId, NewExercise};
use wordforge_db::grammar;
use wordforge_db::llm_usage;
use wordforge_db::material::{self, MaterialId};
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
/// **不能把幾千個字全塞進 prompt**：一萬個字大約是 15,000 token，
/// 每出一題就燒掉這麼多，而且模型並不需要完整清單——
/// 它要的是「這個人的用字大概到什麼難度」。
///
/// 真正保證難度的是產生之後的本地覆蓋率驗收，不是這份樣本。
const KNOWN_SAMPLE: i64 = 60;

/// 樣本要涵蓋幾個難度層。
///
/// 只給最常用的字沒有意義：模型會看到 the / be / and，
/// 完全判斷不出這個人的程度上緣在哪，於是文章不是太簡單就是太難。
const SAMPLE_BANDS: i64 = 4;

/// 閱讀理解的目標覆蓋率。生詞控制在 4% 左右。
const READING_COVERAGE: f64 = 0.96;

/// 覆蓋率不合格時最多重試幾次。
///
/// 連續失敗代表目標詞選得太難，該退回去換一批更常用的字，
/// 而不是無限重試燒 token。
const COVERAGE_RETRIES: usize = 2;

/// 一次出題最多帶幾個文法點給模型。
///
/// 固定上限是重點：練習做得再多，prompt 也不會膨脹。
/// 沒到期的文法點就是「現在不需要練」，送過去只是浪費 token。
const GRAMMAR_BATCH: i64 = 5;

/// 記住最近幾個主題來避開。
///
/// 大約是主題池的一半：留一半可挑，才不會每次都在同幾個之間跳。
/// 片語最多看幾個詞。四個詞已經涵蓋 `in spite of`、`take care of` 這類，
/// 再長的多半是句子而不是詞條。
const MAX_PHRASE_LEN: usize = 4;

const TOPIC_MEMORY: i64 = 6;

pub struct PracticeEngine<'a> {
    db: &'a Db,
    llm: &'a dyn LlmProvider,
    /// 文法點跟單字用同一套 FSRS 排程
    scheduler: wordforge_core::srs::Scheduler,
    /// 目標語言的代碼（`en`、`ja`），用來查字典與挑文法點清單
    pub target_lang: String,
    /// 母語代碼，用來決定用什麼語言解釋
    pub native_lang: String,
    /// 只從這份教材出題。`None` 就是自由出題。
    ///
    /// 這是跟閱讀測驗相反的模式：閱讀測驗照程度當場生一篇，
    /// 這個把模型綁死在使用者的課本上。考試只考課本，
    /// 模型講到課本以外的東西就是干擾。
    pub material_id: Option<MaterialId>,
}

impl<'a> PracticeEngine<'a> {
    pub fn new(db: &'a Db, llm: &'a dyn LlmProvider) -> Self {
        Self {
            db,
            llm,
            scheduler: wordforge_core::srs::Scheduler::default(),
            target_lang: "en".into(),
            native_lang: "zh-TW".into(),
            material_id: None,
        }
    }

    /// 依 profile 設定的語言建立引擎。
    ///
    /// 「換一份字典就能學另一種語言」是這個專案的設計目標，
    /// 但那只有在語言真的從 profile 流下來時才成立——
    /// 先前每個地方都硬編 `"en"`，等於這個目標名存實亡。
    pub async fn for_profile(
        db: &'a Db,
        llm: &'a dyn LlmProvider,
        profile_id: i64,
    ) -> Result<Self> {
        let (native, target) =
            wordforge_db::repo::profiles::languages(db, ProfileId(profile_id)).await?;
        Ok(Self {
            db,
            llm,
            scheduler: wordforge_core::srs::Scheduler::default(),
            target_lang: target,
            native_lang: native,
            material_id: None,
        })
    }

    /// 出題只能取材自這份教材。
    pub fn with_material(mut self, material_id: Option<i64>) -> Self {
        self.material_id = material_id.map(MaterialId);
        self
    }

    /// 挑一段教材給模型看。
    ///
    /// 沒設教材就回 `None`，prompt 裡那一段整個消失——不會變成
    /// 「請參考以下教材：（空）」那種讓模型困惑的東西。
    ///
    /// 要練的字先還原成原形再去查，因為教材詞表存的是原形：
    /// 練 `go` 要找得到寫 `went` 的那一段。
    async fn material_excerpt(&self, target_words: &[String], seed: u64) -> Result<Option<String>> {
        let Some(id) = self.material_id else {
            return Ok(None);
        };

        let mut wanted = Vec::with_capacity(target_words.len());
        for word in target_words {
            if let Some(lemma) = lemmas::base_form(self.db, &self.target_lang, word).await? {
                wanted.push(lemma);
            }
        }

        Ok(material::pick_chunk(self.db, id, &wanted, seed).await?)
    }

    /// 給模型看的語言名稱。代碼對人類不友善，寫進 prompt 也不自然。
    fn target_name(&self) -> &str {
        display_language(&self.target_lang)
    }

    fn native_name(&self) -> &str {
        display_language(&self.native_lang)
    }

    // ------------------------------------------------------------ 學習者狀態

    /// 蒐集出題需要知道的一切。
    ///
    /// `now` 由呼叫端傳入而不是自己讀時鐘：文法點的到期判斷靠它，
    /// 內部偷讀 now_utc() 會讓測試根本測不到排程行為。
    pub async fn learner_profile(
        &self,
        profile_id: i64,
        now: OffsetDateTime,
    ) -> Result<LearnerProfile> {
        let pid = ProfileId(profile_id);

        // 分級測驗的估計優先：它反映使用者真正的程度，
        // 而卡片數只算得到「在這個 App 裡學過的」。
        let estimated: Option<i64> = sqlx_scalar_estimated(self.db, profile_id).await?;
        let mastered = cards::known_lemma_ids(self.db, pid, KNOWN_STABILITY_DAYS)
            .await?
            .len() as i64;

        // 只拿「今天到期」的文法點：練熟的不必再出，
        // 而且送給模型的數量固定，token 不會隨練習次數增加
        let weak_grammar = grammar::due_points(self.db, pid, now, GRAMMAR_BATCH).await?;
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
        let learner = self.learner_profile(profile_id, now).await?;
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

        let excerpt = self
            .material_excerpt(&due_words, now.unix_timestamp() as u64)
            .await?;
        let req = prompts::translation_task(
            self.target_name(),
            self.native_name(),
            excerpt.as_deref(),
            &due_words,
            count,
        );
        let value = self.ask_json(profile_id, "generate", &req).await?;

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
        self.store(profile_id, kind, body, due_words, None, None, now)
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
        let known_sample = self.known_sample(profile_id, learner.vocabulary).await?;
        let word_count = practice::reading_length(learner.vocabulary);
        let target_words = self.due_words(profile_id, 6, now).await?;

        // 主題輪換：不指定的話模型永遠寫校園生活與天氣，
        // 十篇讀起來像同一篇。用時間戳當 seed，同一批候選也不會每次都給同一個。
        let recent_topics =
            exercises::recent_topics(self.db, ProfileId(profile_id), TOPIC_MEMORY).await?;
        let topic = practice::pick_topic(&recent_topics, now.unix_timestamp() as u64);

        // 指定教材時，取材範圍由課本決定，主題輪換就不該再插手
        let excerpt = self
            .material_excerpt(&target_words, now.unix_timestamp() as u64)
            .await?;
        let topic = if excerpt.is_some() { "" } else { topic };

        let spec = prompts::ReadingSpec {
            target_lang: self.target_name(),
            native_lang: self.native_name(),
            word_count,
            target_coverage: READING_COVERAGE,
            known_word_count: learner.vocabulary as usize,
            cefr: None,
            known_sample: &known_sample,
            target_words: &target_words,
            topic: (!topic.is_empty()).then_some(topic),
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
                        Some(topic),
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
                &passage,
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
        let known_sample = self.known_sample(profile_id, learner.vocabulary).await?;
        let excerpt = self
            .material_excerpt(&learner.weak_grammar, now.unix_timestamp() as u64)
            .await?;
        let req = prompts::grammar_drill(
            self.target_name(),
            self.native_name(),
            &learner.weak_grammar,
            &known_sample,
            GRAMMAR_BATCH as usize,
            excerpt.as_deref(),
        );
        let value = self.ask_json(profile_id, "generate", &req).await?;

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

    async fn grade_translation(
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

        let req = prompts::translation_feedback(
            self.target_name(),
            self.native_name(),
            to_target,
            &pairs,
            weak_points,
        );
        let value = self.ask_json(profile_id, "grade", &req).await?;
        serde_json::from_value(value).map_err(|e| PracticeError::BadResponse(e.to_string()))
    }

    async fn grade_reading(
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

        // 分數可以在本地算，不必相信模型的算術
        let local = grade_choices(questions, input);
        feedback.score = local.score;

        // 解析裡的生字與片語由本地字典查，不佔 token 也不用等模型
        let known =
            cards::known_lemma_ids(self.db, ProfileId(profile_id), KNOWN_STABILITY_DAYS).await?;
        feedback.glossary = self.build_glossary(passage, &known).await?;
        Ok(feedback)
    }

    /// 從文章本身算出解析：哪些字你不會、哪裡有片語。
    ///
    /// 全部本地做，一次 LLM 都不呼叫。理由有三個：
    ///
    /// 1. **比模型準**。「他不會這個字」是拿文章去比對牌組算出來的，
    ///    不是模型猜的。90% 法則裡那不足 10%，這裡列的就是它本人。
    /// 2. **語言無關**。模型對小語種的解釋品質沒有保證，但字典是
    ///    使用者自己匯入的——查得到就是查得到。
    /// 3. **免費且即時**。不佔 token，也不用等 CLI 冷啟動。
    async fn build_glossary(
        &self,
        passage: &str,
        known: &std::collections::HashSet<LemmaId>,
    ) -> Result<Vec<GlossaryNote>> {
        let tokens = wordforge_core::text::tokenize(passage);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }

        // 單字：只留「不在已知集合裡」的。已經會的字不需要解釋。
        let mut unknown_words: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for token in &tokens {
            if !seen.insert(token.clone()) {
                continue;
            }
            if wordforge_core::wordlist::is_function_word(&self.target_lang, token) {
                continue;
            }
            if !self.knows_token(token, known).await? {
                unknown_words.push(token.clone());
            }
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
        let words = dict::glossary(self.db, &self.target_lang, &unknown_words).await?;

        let mut notes: Vec<GlossaryNote> = Vec::new();
        for e in phrases {
            notes.push(GlossaryNote {
                text: e.text,
                gloss: e.gloss,
                translation: e.translation,
                is_phrase: true,
                is_unknown: true,
            });
        }
        for e in words {
            notes.push(GlossaryNote {
                text: e.text,
                gloss: e.gloss,
                translation: e.translation,
                is_phrase: false,
                is_unknown: true,
            });
        }
        // 片語排前面：那才是查單字查不到的東西
        notes.sort_by(|a, b| b.is_phrase.cmp(&a.is_phrase).then(a.text.cmp(&b.text)));
        Ok(notes)
    }

    /// 把模型給的文法標籤收斂到該語言的受控清單。
    ///
    /// 模型即使被告知只能從清單挑，還是會偶爾寫成 `past tense` 或 `Articles`。
    /// 沒有這一步的話，同一個文法點會散成好幾個各自排程的標籤。
    fn normalize_point(&self, raw: &str) -> Option<String> {
        let normalized = wordforge_core::grammar_points::normalize_point(&self.target_lang, raw);
        if normalized.is_none() && !raw.trim().is_empty() {
            tracing::debug!(raw, "認不出來的文法標籤，略過");
        }
        normalized
    }

    /// 把這次的文法表現記進 FSRS。
    ///
    /// 答錯的縮短間隔、答對的拉遠。這是「練熟的文法點不再出現」的機制，
    /// 也是下次出題時 `due_points` 的資料來源。
    async fn record_grammar_results(
        &self,
        profile_id: i64,
        body: &ExerciseBody,
        input: &GradeInput,
        feedback: &Feedback,
        now: OffsetDateTime,
    ) -> Result<()> {
        let pid = ProfileId(profile_id);

        match body {
            // 選擇題知道每一題在考什麼，對錯都能記。
            // 這類題目的 corrections 是本地判分產生的，內容與 items 重複，
            // 兩邊都記會讓同一次錯誤算成兩次。
            ExerciseBody::Choices { items }
            | ExerciseBody::Reading {
                questions: items, ..
            } => {
                for (i, item) in items.iter().enumerate() {
                    let Some(point) = item
                        .grammar_point
                        .as_deref()
                        .and_then(|p| self.normalize_point(p))
                    else {
                        continue;
                    };
                    let correct =
                        input.choices.get(i).copied().flatten() == Some(item.answer_index);
                    grammar::record(self.db, pid, &point, correct, &self.scheduler, now).await?;
                }
            }

            // 翻譯題沒有標準答案可以比對，只能採信批改指出來的錯誤。
            // 這裡沒有「答對」的資訊——沒被指出來不代表用對了，
            // 可能只是那句話根本沒用到這個文法。
            ExerciseBody::Translation { .. } => {
                for correction in &feedback.corrections {
                    let Some(point) = correction
                        .grammar_point
                        .as_deref()
                        .and_then(|p| self.normalize_point(p))
                    else {
                        continue;
                    };
                    grammar::record(self.db, pid, &point, false, &self.scheduler, now).await?;
                }
            }
        }

        Ok(())
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

    // ------------------------------------------------------------ 小工具

    /// 送出一次請求並解析回應，順便把用量記下來。
    ///
    /// 記在這裡而不是各個呼叫點：這是整個 crate 唯一真正打模型的地方，
    /// 放在這裡就不會有「新加了一種題型但忘了記用量」的漏洞。
    ///
    /// 失敗的呼叫也記。prompt 一樣送出去了，額度一樣燒掉了——
    /// 只記成功的話，重試越多次帳面上反而越乾淨。
    async fn ask_json(
        &self,
        profile_id: i64,
        purpose: &str,
        req: &wordforge_llm::ChatRequest,
    ) -> Result<serde_json::Value> {
        let prompt_chars = req
            .system
            .as_deref()
            .map(|s| s.chars().count())
            .unwrap_or(0)
            + req
                .messages
                .iter()
                .map(|m| m.content.chars().count())
                .sum::<usize>();

        let result = self.llm.chat(req).await;

        let (response_chars, input_tokens, output_tokens, ok) = match &result {
            Ok(resp) => (
                resp.text.chars().count(),
                resp.input_tokens.map(|t| t as i64),
                resp.output_tokens.map(|t| t as i64),
                true,
            ),
            Err(_) => (0, None, None, false),
        };

        // 記用量失敗不該讓整次練習失敗——那是附帶的觀測，不是主線
        if let Err(e) = llm_usage::record(
            self.db,
            ProfileId(profile_id),
            llm_usage::NewCall {
                model: self.llm.model(),
                purpose,
                prompt_chars: prompt_chars as i64,
                response_chars: response_chars as i64,
                input_tokens,
                output_tokens,
                ok,
            },
            OffsetDateTime::now_utc(),
        )
        .await
        {
            tracing::warn!(error = %e, "用量沒記起來");
        }

        Ok(result?.json()?)
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
    ///
    /// 兩個要點：
    ///
    /// 1. **分層**。只給最常用的字，模型看到的是 the / be / and，
    ///    判斷不出程度上緣在哪。要涵蓋從最基礎到接近上限的各個難度。
    /// 2. **不能只看卡片**。在這個 App 裡複習過的字可能只有幾十個，
    ///    但分級測驗說他會五千個——只送那幾十個會讓模型嚴重低估程度。
    ///    所以把「詞頻在估計詞彙量以內」的字也納入抽樣池。
    async fn known_sample(&self, profile_id: i64, vocabulary: i64) -> Result<Vec<String>> {
        let per_band = (KNOWN_SAMPLE / SAMPLE_BANDS).max(1);
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
    async fn measure_coverage(
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
    async fn knows_token(
        &self,
        token: &str,
        known: &std::collections::HashSet<LemmaId>,
    ) -> Result<bool> {
        let family = lemmas::family(self.db, &self.target_lang, token).await?;
        Ok(family.iter().any(|id| known.contains(id)))
    }

    #[allow(clippy::too_many_arguments)]
    async fn store(
        &self,
        profile_id: i64,
        kind: ExerciseKind,
        body: ExerciseBody,
        target_words: Vec<String>,
        coverage: Option<f64>,
        topic: Option<&str>,
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
                topic,
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

/// 語言代碼 → 給模型看的名稱。
///
/// 只列常見的幾個，其餘原樣傳過去——模型認得 `ko`、`vi` 這類代碼，
/// 硬要維護一份完整對照表反而是負擔。
fn display_language(code: &str) -> &str {
    match code {
        "en" => "English",
        "ja" => "日本語",
        "ko" => "한국어",
        "fr" => "français",
        "de" => "Deutsch",
        "es" => "español",
        "zh-TW" | "zh-Hant" => "繁體中文",
        "zh-CN" | "zh-Hans" => "简体中文",
        other => other,
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
