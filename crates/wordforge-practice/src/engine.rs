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
use wordforge_db::repo::{cards, lemmas, profiles};
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
/// 實際覆蓋率比目標低多少才算「太難、要重寫」。
///
/// 需要一段寬容：模型不可能剛好命中目標，而且每差一次就多燒一次呼叫。
/// 六個百分點大約是一個難度帶的寬度——目標 96% 時低於 90% 才重寫，
/// 跟原本寫死的行為一致。
const COVERAGE_TOLERANCE: f64 = 0.06;

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

/// 生詞從「估計詞彙量」到「估計詞彙量 × 這個倍數」之間挑。
///
/// 1.5 倍是「再難一點但還會再遇到」的範圍。挑更罕見的字學了用不到，
/// 而且會讓文章讀起來像在背 GRE 單字書。
const NEW_WORD_REACH: f64 = 1.5;

/// 先撈幾個候選再做詞性平衡。要夠多才湊得齊各種詞性，
/// 但這是一次有索引的查詢，多撈幾百個不影響。
const NEW_WORD_POOL: i64 = 400;

/// 一篇文章帶進幾個「快忘掉」的字。
///
/// 這些不佔生詞預算，但塞太多會把文章綁死——模型得同時滿足新詞白名單
/// 與這一批，句子會開始像清單。六個是能自然寫進一篇短文的量。
const REVIEW_WORDS: i64 = 6;

/// 挑生詞時要避開最近幾篇教過的字。
///
/// 五篇乘上每篇六到十四個字，大約是 30～70 個字的記憶。夠讓輪換看得出來，
/// 又不會久到把整個候選池鎖死——而且隔幾篇再遇到同一個字是好事，
/// 那就是間隔重複。
const NEW_WORD_MEMORY: i64 = 5;

const TOPIC_MEMORY: i64 = 6;

/// 一篇克漏字挖幾格。
///
/// 比閱讀的生詞多一點：這些字他已經會，挖八格不會讓文章變成謎語。
/// 再多的話一篇短文會被打成蜂窩，前後文的線索反而不夠推出答案。
const CLOZE_BLANKS: i64 = 8;

/// 格式不合格時最多重問幾次。
///
/// 連兩次寫不出合格的 JSON 代表這個模型就是做不到，再問只是燒額度——
/// 而且每一次都是一趟完整的呼叫，使用者已經在等了。
const FORMAT_RETRIES: usize = 1;

/// 講解一個文法點時附幾個例句。
///
/// 四個夠涵蓋不同人稱、時態、肯定與否定；再多就變成例句表，
/// 而使用者是來理解一個規則的，不是來背句子的。
const GRAMMAR_EXAMPLES: usize = 4;

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
    /// 只練這一個文法點。`None` 就用今天到期的弱點。
    ///
    /// 「隨機出目前會的」與「針對性練習」的差別就在這裡：前者讓排程
    /// 決定，後者由使用者指定。
    pub grammar_focus: Option<String>,
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
            grammar_focus: None,
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
            grammar_focus: None,
        })
    }

    /// 出題只能取材自這份教材。
    pub fn with_material(mut self, material_id: Option<i64>) -> Self {
        self.material_id = material_id.map(MaterialId);
        self
    }

    /// 文法題只練這一個點。
    pub fn with_grammar_focus(mut self, point: Option<String>) -> Self {
        self.grammar_focus = point.filter(|p| !p.trim().is_empty());
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
            ExerciseKind::Reading => self.generate_reading(profile_id, &learner, now).await,
            ExerciseKind::Cloze => self.generate_cloze(profile_id, &learner, now).await,
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
        let mut req = prompts::translation_task(
            self.target_name(),
            self.native_name(),
            kind == ExerciseKind::TranslationToTarget,
            excerpt.as_deref(),
            &words,
            count,
        );
        let value = self
            .ask_valid_json(
                profile_id,
                "generate",
                &mut req,
                crate::validate::check_translation,
            )
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
        self.store(profile_id, kind, body, words, None, None, now)
            .await
    }

    async fn generate_reading(
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
        let recent_topics =
            exercises::recent_topics(self.db, ProfileId(profile_id), TOPIC_MEMORY).await?;
        let topic = practice::pick_topic(&recent_topics, now.unix_timestamp() as u64);

        // 指定教材時，取材範圍由課本決定，主題輪換就不該再插手
        // 教材檢索用複習字：課本裡本來就不會有他還沒學的生詞
        let excerpt = self
            .material_excerpt(&review_words, now.unix_timestamp() as u64)
            .await?;
        let topic = if excerpt.is_some() { "" } else { topic };

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
    async fn generate_cloze(
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

        let recent_topics =
            exercises::recent_topics(self.db, ProfileId(profile_id), TOPIC_MEMORY).await?;
        let topic = practice::pick_topic(&recent_topics, now.unix_timestamp() as u64);
        // 指定教材時取材範圍由課本決定，主題輪換就不該再插手
        let topic = if excerpt.is_some() { "" } else { topic };

        let mut req = prompts::cloze_passage(&prompts::ClozeSpec {
            target_lang: self.target_name(),
            native_lang: self.native_name(),
            word_count: practice::reading_length(learner.vocabulary),
            known_word_count: learner.vocabulary as usize,
            known_sample: &known_sample,
            blank_words: &blanks,
            topic: (!topic.is_empty()).then_some(topic),
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
            Some(topic),
            now,
        )
        .await
    }

    async fn generate_grammar(
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

        let known_sample = self.known_sample(profile_id, learner.vocabulary).await?;
        let excerpt = self
            .material_excerpt(&wanted, now.unix_timestamp() as u64)
            .await?;

        let mut req = prompts::grammar_drill(
            self.target_name(),
            self.native_name(),
            &points,
            &wanted,
            &known_sample,
            GRAMMAR_BATCH as usize,
            excerpt.as_deref(),
        );
        let value = self
            .ask_valid_json(profile_id, "generate", &mut req, |v| {
                crate::validate::check_choice_items("items", v)
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

    /// 請模型講解一個文法點，並把結果存進 `grammar_def`。
    ///
    /// ## 為什麼要存起來
    ///
    /// 沒有可以直接匯入的開源文法書，所以講解一開始是空的。生成一次就
    /// 存下來：之後開這一頁不必再等模型，也不會每看一次燒一次額度。
    /// 存下來還有一個好處——使用者可以自己編輯，模型講得不好就改掉。
    pub async fn explain_grammar(
        &self,
        profile_id: i64,
        point: &str,
        now: OffsetDateTime,
    ) -> Result<wordforge_db::grammar::GrammarDef> {
        let Some(mut def) = grammar::get_def(self.db, &self.target_lang, point).await? else {
            return Err(PracticeError::NotFound);
        };

        let learner = self.learner_profile(profile_id, now).await?;
        let req = prompts::grammar_explanation(
            self.target_name(),
            self.native_name(),
            &def.point,
            &def.name,
            learner.vocabulary as usize,
            GRAMMAR_EXAMPLES,
        );

        let value = self.ask_json(profile_id, "explain", &req).await?;

        let explanation = value
            .get("explanation")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| PracticeError::BadResponse("沒有產出講解".into()))?;

        def.explanation = Some(explanation.to_string());
        def.examples = value
            .get("examples")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| serde_json::from_value(e.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();

        grammar::upsert_def(self.db, &def, now).await?;
        Ok(def)
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
    async fn passage_glossary(&self, profile_id: i64, passage: &str) -> Result<Vec<GlossaryNote>> {
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
    async fn build_glossary(
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

    /// 把模型給的文法標籤收斂到該語言的受控清單。
    ///
    /// 模型即使被告知只能從清單挑，還是會偶爾寫成 `past tense` 或 `Articles`。
    /// 沒有這一步的話，同一個文法點會散成好幾個各自排程的標籤。
    fn normalize_point(&self, points: &[String], raw: &str) -> Option<String> {
        let normalized = wordforge_core::grammar_points::normalize_point(points, raw);
        if normalized.is_none() && !raw.trim().is_empty() {
            tracing::debug!(raw, "認不出來的文法標籤，略過");
        }
        normalized
    }

    /// 這個語言目前的受控文法點清單。
    ///
    /// 從 `grammar_def` 讀，不是寫死的常數——「匯入什麼就能學什麼」
    /// 對文法跟對字典是同一個承諾。第一次讀到空的就把種子寫進去，
    /// 讓英文開箱有東西可用；沒有種子的語言仍然是空的，
    /// 那時 prompt 會退回「請自己保持一致」。
    async fn grammar_points(&self, now: OffsetDateTime) -> Result<Vec<String>> {
        grammar::seed_defs(self.db, &self.target_lang, now).await?;
        Ok(grammar::list_points(self.db, &self.target_lang).await?)
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
        let points = self.grammar_points(now).await?;

        match body {
            // 選擇題知道每一題在考什麼，對錯都能記。
            // 這類題目的 corrections 是本地判分產生的，內容與 items 重複，
            // 兩邊都記會讓同一次錯誤算成兩次。
            ExerciseBody::Choices { items }
            | ExerciseBody::Cloze { items, .. }
            | ExerciseBody::Reading {
                questions: items, ..
            } => {
                for (i, item) in items.iter().enumerate() {
                    let Some(point) = item
                        .grammar_point
                        .as_deref()
                        .and_then(|p| self.normalize_point(&points, p))
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
                        .and_then(|p| self.normalize_point(&points, p))
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

        let started = std::time::Instant::now();
        let result = self.llm.chat(req).await;
        let elapsed = started.elapsed();

        // 出一題要多久、時間花在哪，使用者是感覺得到的。沒有這行的話
        // 「太慢了」只能靠猜——是模型慢、是重試、還是本地查詢慢？
        tracing::info!(
            purpose,
            prompt_chars,
            elapsed_ms = elapsed.as_millis() as u64,
            ok = result.is_ok(),
            "LLM 呼叫完成"
        );

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

    /// 今天到期的字，**含從來沒看過的新卡**。
    ///
    /// 這是最後手段，不是預設。批改時 LLM 標出來的生詞會直接建成新卡，
    /// 而新卡一建好就算「到期」——所以這個查詢回的多半是他一次都沒學過的字。
    /// 拿那種字出題，中翻英等於要他寫出一個從沒見過的單字。
    ///
    /// 正常路徑用 [`Self::studied_due_words`]（多一個 `reps > 0`）。
    /// 這裡留著是為了新使用者：牌組裡全是新卡時，一個字都拿不到
    /// 比拿到沒學過的字更糟。
    async fn due_words(
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
    async fn translation_words(
        &self,
        profile_id: i64,
        count: usize,
        now: OffsetDateTime,
    ) -> Result<Vec<String>> {
        let (due_n, extra_n) = practice::translation_mix(count);

        // 多撈一點，讓補位有東西可用
        let due_pool = self
            .studied_due_words(profile_id, count as i64, now)
            .await?;
        let mut words: Vec<String> = due_pool.iter().take(due_n).cloned().collect();

        let extra = cards::sample_known_words(
            self.db,
            ProfileId(profile_id),
            &self.target_lang,
            &words,
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

        // 到期的不夠時，再多抽一些學過的
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

    /// 送出請求並驗收格式，沒過就把它的輸出串回輸入、指出哪裡錯，再問一次。
    ///
    /// ## 為什麼一定要串回去
    ///
    /// 非交互式的後端**完全不記得自己上一次寫了什麼**：CLI 每次都是全新的
    /// 行程，API 那邊我們也沒把它的回答加進 messages。只說「第三題的
    /// answer_index 超出範圍」它根本不知道第三題是什麼，只能重寫一份不相干的。
    ///
    /// ## 為什麼要驗
    ///
    /// 原本每個地方都寫 `filter_map(|q| from_value(q).ok())`——解析失敗的題目
    /// **直接消失**。使用者拿到三題而不是四題，畫面上沒有任何異狀，
    /// log 也沒有一行。那種壞法看起來完全正常，是最難查的一類。
    ///
    /// 重試次數壓在兩次：連兩次寫不出合格的 JSON 代表這個模型就是做不到，
    /// 再問只是燒額度，而且使用者已經等很久了。
    async fn ask_valid_json<F>(
        &self,
        profile_id: i64,
        purpose: &str,
        req: &mut wordforge_llm::ChatRequest,
        check: F,
    ) -> Result<serde_json::Value>
    where
        F: Fn(&serde_json::Value) -> Vec<crate::validate::Problem>,
    {
        let mut latest = None;

        for attempt in 0..=FORMAT_RETRIES {
            let value = self.ask_json(profile_id, purpose, req).await?;
            let problems: Vec<String> = check(&value).iter().map(|p| p.to_string()).collect();

            if problems.is_empty() {
                if attempt > 0 {
                    tracing::info!(attempt, "格式修正成功");
                }
                return Ok(value);
            }

            tracing::warn!(attempt, ?problems, "回應沒通過格式檢查");
            if attempt < FORMAT_RETRIES {
                req.messages
                    .push(prompts::format_retry(&problems, &value.to_string()));
            }
            latest = Some(value);
        }

        // 修不好就把最後一版交出去，讓呼叫端拿能用的部分。
        //
        // **不在這裡失敗**：驗收的目的是給模型一次修正的機會，不是閘門。
        // 四題壞了一題的話，三題的練習仍然做得完；整份不給只是把「少一題」
        // 換成「什麼都沒有」。真的一題都不能用時，呼叫端自己會報錯。
        tracing::warn!("格式仍不合格，改用能用的部分");
        Ok(latest.expect("迴圈至少跑一次"))
    }

    /// 逐選項解說沒生齊就補一次。
    ///
    /// ## 為什麼要在本地驗
    ///
    /// 「每個選項各寫一句」是 prompt 裡的請求，模型只會**大致**遵守——
    /// 而這一項驗得起來：`option_notes.len() == options.len()`，一行程式。
    /// 凡是能在本地驗的就不要只相信模型，這跟覆蓋率驗收是同一個原則。
    ///
    /// ## 為什麼只收解說、不收題目
    ///
    /// 重試回來的內容只拿 `option_notes`，題目、選項、答案一律沿用原本那份。
    /// 模型很可能順手把題目重寫一遍——那樣答案就跟文章對不上了，
    /// 而且使用者看不出來。只搬那一個欄位，壞掉的重試最多是「還是沒有解說」。
    ///
    /// 只補一次。連兩次寫不出來代表這個模型就是不寫，再問只是燒額度；
    /// 缺解說是體驗差一級，不是壞掉。
    async fn fill_option_notes(
        &self,
        profile_id: i64,
        req: &mut wordforge_llm::ChatRequest,
        items: &mut [ChoiceItem],
        field: &str,
        previous: &str,
    ) -> Result<()> {
        let missing = missing_option_notes(items);
        if missing.is_empty() {
            return Ok(());
        }
        tracing::info!(?missing, field, "逐選項解說沒生齊，要求補上");

        req.messages.push(prompts::option_notes_retry(
            self.native_name(),
            field,
            &missing,
            previous,
        ));

        // 補解說失敗不該讓整份練習失敗——題目本身是好的，
        // 少的只是「你選的那個為什麼不行」那一句
        let value = match self.ask_json(profile_id, "generate", req).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "補解說的呼叫失敗，維持沒有解說的版本");
                return Ok(());
            }
        };

        let filled: Vec<Vec<String>> = value
            .get(field)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|item| {
                        item.get("option_notes")
                            .and_then(|n| n.as_array())
                            .map(|n| {
                                n.iter()
                                    .filter_map(|s| s.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .unwrap_or_default();

        for (item, notes) in items.iter_mut().zip(filled) {
            // 長度還是對不上就不要收——配錯的解說比沒有解說更糟，
            // 因為畫面上看起來完全合理
            if notes.len() == item.options.len() && notes.iter().all(|n| !n.trim().is_empty()) {
                item.option_notes = notes;
            }
        }

        let still_missing = missing_option_notes(items);
        if !still_missing.is_empty() {
            tracing::warn!(?still_missing, "補完之後還是缺，不再重試");
        }
        Ok(())
    }

    /// 今天到期、而且**至少複習過一次**的字。
    ///
    /// 跟 `due_words` 的差別只有 `reps > 0`，但那一個條件很重要：
    /// 牌組裡剛加進去、他從來沒看過的新卡在排程上也算「今天到期」。
    /// 克漏字拿那種字挖空，考的就不是「想不想得起來」而是「猜不猜得中」。
    async fn studied_due_words(
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

/// 克漏字答錯的那幾格，正確答案是哪個字。
///
/// 這些字會被排回複習：挖空的本來就是他該複習的字，填不出來就是
/// 「還沒真的會」——比排程算出來的到期時間更直接的證據。
fn missed_words(items: &[ChoiceItem], input: &GradeInput) -> Vec<String> {
    items
        .iter()
        .enumerate()
        .filter(|(i, item)| input.choices.get(*i).copied().flatten() != Some(item.answer_index))
        .filter_map(|(_, item)| item.options.get(item.answer_index).cloned())
        .collect()
}

/// 把驗收過的題目解析出來。
///
/// **一定要先驗收再叫這個**：這裡仍然會跳過解析不了的項目，但驗收
/// 已經保證不會有那種東西了。原本沒有驗收那一層，於是壞掉的題目
/// 直接消失——使用者拿到三題而不是四題，畫面上沒有任何異狀。
fn parse_choice_items(value: &serde_json::Value, field: &str) -> Vec<ChoiceItem> {
    value
        .get(field)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// 哪幾題的逐選項解說沒生齊（題號 1 起算）。
///
/// 長度對不上也算缺：短一截的話最後幾個選項會沒有解說，
/// 長一截則代表模型自己也搞混了哪句配哪個選項——兩種都不能收。
fn missing_option_notes(items: &[ChoiceItem]) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.option_notes.len() != item.options.len()
                || item.option_notes.iter().any(|n| n.trim().is_empty())
        })
        .map(|(i, _)| i + 1)
        .collect()
}

/// 洗牌的亂源。
///
/// 用奈秒而不是秒：連續出兩題常常落在同一秒內，秒級的 seed 會讓
/// 兩份題目的選項被搬到一模一樣的位置。`wordforge-core` 不碰時鐘，
/// 所以亂源在這一層決定。
fn shuffle_seed(now: OffsetDateTime) -> u64 {
    now.unix_timestamp_nanos() as u64
}

/// 把一份選擇題的選項洗過，答案落在哪裡就是哪裡。
///
/// 模型出的題目，答案會集中在某幾個位置——實際看過一整份都是第一個選項。
/// 使用者不用讀題就猜得到，那份練習就白做了。
///
/// 這件事**只能在本地做**：叫模型「請把答案分散」是沒有辦法驗收的請求，
/// 而重排選項是我們自己就做得到的事，做完還測得出來。
fn shuffle_answers(items: &mut [ChoiceItem], seed: u64) {
    for (i, item) in items.iter_mut().enumerate() {
        // 壞資料原樣放過：洗牌只會讓錯誤更難查
        if item.options.len() < 2 || item.answer_index >= item.options.len() {
            continue;
        }

        // 每一題各自的亂源。同一份裡共用一個 seed 的話，
        // 每題的選項會被搬到一模一樣的位置。
        let per_item = seed ^ (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let order = practice::shuffle_order(item.options.len(), per_item);

        item.options = practice::reorder(&item.options, &order);
        // 每個選項的解說跟選項是平行陣列，**一定要用同一個排列一起搬**。
        // 只搬其中一個的話每個選項會配到別人的解說，而那個畫面
        // 看起來完全合理，不會有人發現。
        if !item.option_notes.is_empty() {
            item.option_notes = practice::reorder(&item.option_notes, &order);
        }
        item.answer_index = order
            .iter()
            .position(|&k| k == item.answer_index)
            .unwrap_or(item.answer_index);
    }
}

/// 把模型寫的逐題講評掛回本地判分的結果上。
///
/// 對錯與參考答案一律用本地的（那是算出來的，不是猜的），
/// 只有 `comment` 採用模型的。
///
/// 對應靠模型自己給的 `index`（1 起算），不是陣列位置：它很常少回
/// 一題或換個順序，照位置貼的話第三題的講評會出現在第二題底下——
/// 而那個畫面看起來完全正常。`index` 整份都缺（全是預設的 0）時
/// 才退回照位置對，那是還有救的最後手段。
fn align_comments(from_model: &[ItemResult], mut local: Vec<ItemResult>) -> Vec<ItemResult> {
    let numbered = from_model.iter().any(|r| r.index > 0);

    for (i, item) in local.iter_mut().enumerate() {
        let matched = if numbered {
            from_model.iter().find(|r| r.index == i + 1)
        } else {
            from_model.get(i)
        };
        if let Some(m) = matched
            && m.comment.as_deref().is_some_and(|c| !c.trim().is_empty())
        {
            item.comment = m.comment.clone();
        }
    }
    local
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
