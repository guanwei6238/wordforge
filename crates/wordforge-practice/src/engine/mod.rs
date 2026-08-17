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
use wordforge_db::sentences;
use wordforge_db::topics;
use wordforge_db::word_sentences;
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

    /// 這件事在這個題型上不成立。訊息要說得出「那該怎麼辦」——
    /// 「不支援」對使用者等於沒說。
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, PracticeError>;

/// 「算是會了」的門檻（天）。與複習頁的統計一致。
const KNOWN_STABILITY_DAYS: f64 = 21.0;

/// 可用池：文章類（閱讀、克漏字）給幾個字。
///
/// 「可用池」是**可以用、不必用**的那一層，跟硬性池（一定要挑幾個出來用）
/// 分開。它的作用是讓模型不必為了湊字而硬塞——手上的選擇夠多，
/// 寫不順的那幾個就跳過。
///
/// 330 這個數字是**量出來的**：實測使用者做過的四篇文章，一篇平均用到
/// 164 個不同的字。給一倍的餘裕，模型才有得挑。原本固定 60 個等於
/// 七成的用字要它自己想，那份樣本只夠讓它感受程度。
///
/// 池子大小跟牌組脫鉤是硬性條件：牌組長到一萬字時把候選全丟進 prompt，
/// 每次出題都在燒 token，模型的注意力也會被長清單稀釋。
const PROSE_WORD_POOL: i64 = 330;

/// 可用池：句子類（翻譯）給幾個字。
///
/// 同樣是量出來的：一份三到五題的翻譯練習平均用到 47 個不同的字。
/// 一句話能自然容納的字數跟一篇 300 詞的文章差一個量級，所以池子分兩種，
/// 不是同一個倍數套到底。
const SENTENCE_WORD_POOL: i64 = 95;

/// 可用池裡有多少比例是「學過但句子還很少的字」。
///
/// 那些字複習時幾乎只看得到釋義，印象最薄，所以優先讓模型有機會用上。
/// 實際填進去的數量取 `min(池子 × 這個比例, 真的有幾個)`——
/// 使用者的資料現在只有 54 個一句都沒有的字，硬填會變成重複同一批。
const NO_SENTENCE_SHARE: f64 = 0.2;

/// 練到幾句以上就不必再優先了。
///
/// 「還沒有例句」（0 句）只是這件事的極端；一個字只練過一句跟完全沒練過
/// 差別不大，都還沒到「在不同情境裡看過」。門檻放 3，名額由句數少的先拿，
/// 所以 0 句的永遠排在 1 句的前面——把門檻拉高不會排擠到真正最缺的那批。
const FEW_SENTENCES: i64 = 3;

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

/// 硬性池比「真的要用幾個」大多少。
///
/// 硬性池是「必須從這裡挑」的那一層：閱讀的生詞、克漏字要挖的字、
/// 翻譯每題要練的字。給剛好的數量等於逼模型把每一個都塞進去，而有些字
/// 在那個情境下就是寫不自然——ex12 那篇克漏字為了用掉 `fall`，
/// 寫出「In fall, the room was quiet」。
///
/// 文章類給兩倍：一篇 300 詞挑得動。翻譯只給 1.6 倍，因為**池子越大，
/// 模型越會只挑好寫的字**，而難寫的往往正是最該練的；一句話的容量小，
/// 這個偏差會更明顯。
const PROSE_CHOICE_FACTOR: usize = 2;

/// 翻譯的硬性池：滿額題數的幾倍（取整）。
const SENTENCE_CHOICE_FACTOR: f64 = 1.6;

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

/// 補寫舊練習的句子排程的版號。**改動補寫規則就要加一。**
const REVIEW_BACKFILL_VERSION: i64 = 1;

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

/// 翻譯選字要避開最近幾份翻譯用過的字。
///
/// 比 [`NEW_WORD_MEMORY`] 長，因為每份只有三到五個字——五份才二十個字左右，
/// 而牌組裡「學過的字」本來就比生詞候選池小得多（實測一個牌組只有 43 個）。
/// 太短的話輪不出效果，太長的話小牌組會被排除到沒東西可用，
/// 每次都得走放寬那條路。
const TRANSLATION_WORD_MEMORY: i64 = 8;

const TOPIC_MEMORY: i64 = 6;

/// 會寫成短文的題型。主題輪換時這兩種算同一組——同一個主題的文章
/// 接著一份同主題的克漏字，讀起來就是同一篇。
const PROSE_KINDS: &[&str] = &[ExerciseKind::Reading.as_str(), ExerciseKind::Cloze.as_str()];

/// 翻譯的兩個方向算同一組：中翻英和英翻中撞題材時，
/// 使用者看到的仍然是「又是這個情境」。
const TRANSLATION_KINDS: &[&str] = &[
    ExerciseKind::TranslationToTarget.as_str(),
    ExerciseKind::TranslationToNative.as_str(),
];

/// 一篇克漏字**至少**挖幾格。
///
/// 「剛好八格」是把 `CLOZE_BLANKS` 當成等號在用，模型只好把每個字都塞進去。
/// 改成下限之後，某個字這一篇實在放不進去就可以少挖一格——但再少就不行了：
/// 300 詞的文章挖五個洞已經接近「順便填空」，考不到「想不想得起來」。
const CLOZE_MIN_BLANKS: usize = 6;

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

mod choices;
mod generate;
mod grade;

pub use grade::{DueAnswer, DueSentenceResult};

mod links;
mod points;
mod words;

use choices::{
    align_comments, grade_choices, missed_words, missing_option_notes, parse_choice_items,
    shuffle_answers, shuffle_seed,
};
use links::checked_sentences;
use points::attribute_corrections;

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

        // 把這次的句子連回單字。連不上就跳過——這是加分項，
        // 不該讓一份出好的練習因此失敗。
        if let Err(e) = self.link_sentences(profile_id, id.0, &body, now).await {
            tracing::warn!(error = %e, "句子連結沒建起來");
        }

        Ok(ExerciseView {
            exercise_id: id.0,
            kind,
            body,
            target_words,
            coverage,
        })
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
