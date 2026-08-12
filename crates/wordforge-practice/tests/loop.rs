//! 整條學習迴圈的測試：出題 → 作答 → 批改 → 不懂的字進牌組。
//!
//! LLM 用假的：真正要驗證的是編排邏輯（有沒有把 prompt 組對、
//! 回應解析得對不對、不懂的字有沒有真的變成卡片），
//! 而不是模型本身。用真模型反而讓測試不穩定又慢。

use std::sync::Mutex;

use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use wordforge_core::model::{LemmaId, ProfileId};
use wordforge_core::practice::ExerciseKind;
use wordforge_db::Db;
use wordforge_db::dict::{EntryWrite, NewSense, NewSource};
use wordforge_db::repo::{NewLemma, cards, lemmas, profiles};
use wordforge_llm::{ChatRequest, ChatResponse, LlmProvider};
use wordforge_practice::{GradeInput, PracticeEngine};

/// 依序回傳預設答案的假模型，同時記下收到的 prompt。
struct FakeLlm {
    responses: Mutex<Vec<String>>,
    seen: Mutex<Vec<String>>,
}

impl FakeLlm {
    fn new(responses: &[&str]) -> Self {
        Self {
            responses: Mutex::new(responses.iter().rev().map(|s| s.to_string()).collect()),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn last_prompt(&self) -> String {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }

    fn call_count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[async_trait]
impl LlmProvider for FakeLlm {
    async fn chat(&self, req: &ChatRequest) -> wordforge_llm::Result<ChatResponse> {
        self.seen
            .lock()
            .unwrap()
            .push(req.messages.iter().map(|m| m.content.clone()).collect());

        let text = self
            .responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| "{}".to_string());
        Ok(ChatResponse {
            text,
            input_tokens: None,
            output_tokens: None,
        })
    }

    fn model(&self) -> &str {
        "fake-model"
    }
}

fn t0() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
}

/// 依「這題要不要答對」組出作答的選項索引。
///
/// **不能寫死索引**：出題時會把正確答案的位置打散，寫死 `Some(0)` 的話
/// 測試驗到的是洗牌的結果，不是判分邏輯。`Some(true)` 是答對、
/// `Some(false)` 是隨便挑一個錯的、`None` 是沒作答。
fn answer(
    exercise: &wordforge_practice::ExerciseView,
    plan: &[Option<bool>],
) -> Vec<Option<usize>> {
    use wordforge_practice::payload::ExerciseBody;

    let items = match &exercise.body {
        ExerciseBody::Choices { items } | ExerciseBody::Cloze { items, .. } => items,
        ExerciseBody::Reading { questions, .. } => questions,
        ExerciseBody::Translation { .. } => panic!("翻譯題沒有選項可以挑"),
    };

    plan.iter()
        .zip(items)
        .map(|(want, item)| match want {
            Some(true) => Some(item.answer_index),
            Some(false) => Some((item.answer_index + 1) % item.options.len().max(1)),
            None => None,
        })
        .collect()
}

/// 建一個有字典、有牌組的資料庫。
async fn setup(words: &[&str]) -> (Db, i64) {
    setup_lang("en", words).await
}

/// 同上，但可以指定學的是哪種語言。
///
/// 「載入哪個語言的字典就能學哪個語言」是這個專案的設計目標，
/// 所以測試裡的語言必須是參數，不能是常數——否則寫死 `"en"` 的
/// 迴歸不會有任何測試抓得到。
async fn setup_lang(lang: &str, words: &[&str]) -> (Db, i64) {
    let db = Db::open_in_memory().await.unwrap();
    let profile = profiles::create(&db, "我", "zh-TW", lang, t0())
        .await
        .unwrap();

    let source = wordforge_db::dict::upsert_source(
        &db,
        NewSource {
            slug: "test",
            name: "測試字典",
            license: None,
            attribution: None,
            homepage: None,
            version: None,
        },
        t0(),
    )
    .await
    .unwrap();

    let mut conn = db.pool().acquire().await.unwrap();
    for (i, word) in words.iter().enumerate() {
        wordforge_db::dict::write_entry(
            &mut conn,
            source,
            &EntryWrite {
                lang,
                headword: word,
                pos: "",
                freq_rank: Some(i as i64 + 1),
                senses: vec![NewSense {
                    gloss: "意思",
                    gloss_lang: "zh-CN",
                    translation: Some("意思"),
                    ..Default::default()
                }],
                ..Default::default()
            },
            wordforge_db::dict::WriteMode::Replace,
        )
        .await
        .unwrap();
    }
    drop(conn);

    (db, profile.0)
}

/// 把某個字加進牌組並設成到期，讓它成為出題素材。
async fn put_in_deck(db: &Db, profile: i64, lemma_id: i64) {
    cards::ensure(
        db,
        ProfileId(profile),
        wordforge_core::model::LemmaId(lemma_id),
        wordforge_core::model::CardKind::Recognition,
        t0(),
    )
    .await
    .unwrap();
}

/// 把一個字加進牌組**並且真的複習過一次**。
///
/// 跟 `put_in_deck` 的差別只有 `reps` 與 `state`，但那個差別是真的：
/// 剛加進牌組、還沒看過的卡在排程上也算「今天到期」，可是那不是
/// 「學過的字」。克漏字只挖學過的字，所以測試的牌組也得長得像
/// 真的用過的牌組——填一張沒有複習紀錄的卡進去，測到的會是另一件事。
async fn study(db: &Db, profile: i64, lemma_id: i64) {
    let card = cards::ensure(
        db,
        ProfileId(profile),
        LemmaId(lemma_id),
        wordforge_core::model::CardKind::Recognition,
        t0(),
    )
    .await
    .unwrap();

    let (next, log) = wordforge_core::srs::Scheduler::default().review(
        &card,
        wordforge_core::model::Rating::Easy,
        t0(),
        None,
    );
    cards::record_review(db, &next, &log).await.unwrap();
}

/// 把一個字推到詞彙量之外，讓它真的算「不會」。
///
/// 覆蓋率驗收把「詞頻在估計詞彙量以內」的字算成會的（那才跟告訴模型的
/// 數字一致）。所以測試裡想要一個字被判定為生字，就得給它一個
/// 超出詞彙量的詞頻——`ubiquitous` 在現實中本來也是這樣。
async fn make_rare(db: &Db, lemma_id: i64, rank: i64) {
    sqlx::query("UPDATE lemma SET freq_rank = ? WHERE id = ?")
        .bind(rank)
        .bind(lemma_id)
        .execute(db.pool())
        .await
        .unwrap();
}

/// 設定分級測驗估的詞彙量，用來決定題型。
async fn set_vocabulary(db: &Db, profile: i64, n: i64) {
    sqlx::query(
        "UPDATE profile SET settings_json = json_object('estimated_vocabulary', ?) WHERE id = ?",
    )
    .bind(n)
    .bind(profile)
    .execute(db.pool())
    .await
    .unwrap();
}

// ---------------------------------------------------------------- 測試

/// 這是整套設計的核心：批改時發現「他不會這個字」，就把字排進複習。
#[tokio::test]
async fn unknown_words_from_grading_become_review_cards() {
    let (db, profile) = setup(&["park", "diligent", "weather"]).await;
    set_vocabulary(&db, profile, 300).await;
    put_in_deck(&db, profile, 1).await;

    let llm = FakeLlm::new(&[
        // 出題
        r#"{"items":[{"source":"我昨天去了公園","target_word":"park",
                      "reference":"I went to the park yesterday"}]}"#,
        // 批改：模型判斷他不會 diligent
        r#"{"score":50,
            "items":[{"index":1,"correct":false,"reference":"I went to the park yesterday"}],
            "corrections":[{"original":"I go to park","corrected":"I went to the park",
                            "grammar_point":"tense","severity":"major"}],
            "unknown_words":["diligent"]}"#,
    ]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::TranslationToTarget), t0())
        .await
        .unwrap();

    let feedback = engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec!["I go to park yesterday".into()],
                choices: vec![],
                marked_unknown: vec![],
            },
            t0(),
        )
        .await
        .unwrap();

    assert_eq!(feedback.unknown_words, vec!["diligent"]);
    assert_eq!(feedback.added_to_deck, vec!["diligent"], "應該建了卡片");

    // 真的在牌組裡，而且是張還沒學過的新卡
    let queue = cards::daily_queue(&db, ProfileId(profile), t0(), t0(), 50, 200)
        .await
        .unwrap();
    let words: Vec<i64> = queue.iter().map(|c| c.lemma_id.0).collect();
    assert!(words.contains(&2), "diligent 應該進了複習佇列：{words:?}");
}

/// 使用者自己點「這個字我不會」也要進牌組，而且不能跟模型的判斷重複。
#[tokio::test]
async fn words_marked_by_the_learner_are_merged_with_the_models() {
    let (db, profile) = setup(&["park", "diligent", "weather"]).await;
    set_vocabulary(&db, profile, 300).await;

    let llm = FakeLlm::new(&[
        r#"{"items":[{"source":"天氣很好"}]}"#,
        r#"{"score":80,"unknown_words":["diligent"]}"#,
    ]);
    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::TranslationToTarget), t0())
        .await
        .unwrap();

    let feedback = engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec!["The weather is good".into()],
                choices: vec![],
                // 模型沒抓到，但使用者自己知道不會
                marked_unknown: vec!["weather".into(), "Diligent".into()],
            },
            t0(),
        )
        .await
        .unwrap();

    let mut added = feedback.added_to_deck.clone();
    added.sort();
    assert_eq!(added, vec!["diligent", "weather"], "大小寫不同不該重複加");
}

/// 模型偶爾會回傳片語、拼錯的字、或根本不是單字的東西，那些不該變成卡片。
#[tokio::test]
async fn junk_words_are_not_turned_into_cards() {
    let (db, profile) = setup(&["park"]).await;
    set_vocabulary(&db, profile, 300).await;

    let llm = FakeLlm::new(&[
        r#"{"items":[{"source":"我去公園"}]}"#,
        r#"{"score":90,"unknown_words":["park","zzzznotaword","take a walk",""]}"#,
    ]);
    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::TranslationToTarget), t0())
        .await
        .unwrap();

    let feedback = engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec!["I go to the park".into()],
                choices: vec![],
                marked_unknown: vec![],
            },
            t0(),
        )
        .await
        .unwrap();

    assert_eq!(
        feedback.added_to_deck,
        vec!["park"],
        "字典裡查不到的不該建卡"
    );
}

/// 閱讀理解的覆蓋率要在本地驗收，太難就帶著超標的詞重寫。
#[tokio::test]
async fn a_passage_that_is_too_hard_gets_regenerated() {
    // 冠詞與介系詞也要在字典裡，否則會被算成生詞——
    // 實際使用時它們當然在，這裡是測試資料要對得上現實
    let (db, profile) = setup(&["the", "on", "cat", "sat", "mat", "ubiquitous", "paradigm"]).await;
    set_vocabulary(&db, profile, 3_000).await;

    // 前五個字都是「已掌握」
    for lemma in 1..=5 {
        put_in_deck(&db, profile, lemma).await;
    }
    // 後兩個要超出詞彙量才會被算成生字
    make_rare(&db, 6, 30_000).await;
    make_rare(&db, 7, 30_000).await;
    sqlx::query("UPDATE card SET state='review', stability=40.0")
        .execute(db.pool())
        .await
        .unwrap();

    let llm = FakeLlm::new(&[
        // 第一篇塞滿生詞，覆蓋率不合格
        r#"{"title":"Hard","passage":"ubiquitous paradigm ubiquitous paradigm",
            "questions":[],"new_words":[]}"#,
        // 重寫後只用已知詞
        r#"{"title":"Easy","passage":"The cat sat on the mat. The cat sat.",
            "questions":[{"question":"Where?","options":["mat","tree"],"option_notes":["note1","note2"],"answer_index":0}],
            "new_words":[]}"#,
    ]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap();

    assert_eq!(llm.call_count(), 2, "第一篇不合格應該要求重寫");
    assert!(
        llm.last_prompt().contains("ubiquitous"),
        "重寫時要指名超標的詞，只說「太難了」模型會換一批同樣難的字"
    );

    match &exercise.body {
        wordforge_practice::payload::ExerciseBody::Reading { title, .. } => {
            assert_eq!(title, "Easy");
        }
        other => panic!("題型不對：{other:?}"),
    }
    assert!(exercise.coverage.unwrap() > 0.9);
}

/// 選擇題在本地判分，不必浪費一次 LLM 呼叫。
#[tokio::test]
async fn grammar_choices_are_graded_locally() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    let llm = FakeLlm::new(&[r#"{"items":[
             {"prompt":"I ___ yesterday","options":["go","went"],"option_notes":["note1","note2"],"answer_index":1,
              "grammar_point":"tense","explanation":"過去式"},
             {"prompt":"She ___ a book","options":["read","reads"],"option_notes":["note1","note2"],"answer_index":1,
              "grammar_point":"agreement"}
           ]}"#]);
    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    let feedback = engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec![],
                choices: answer(&exercise, &[Some(true), Some(false)]),
                marked_unknown: vec![],
            },
            t0(),
        )
        .await
        .unwrap();

    assert_eq!(llm.call_count(), 1, "批改選擇題不該再呼叫模型");
    assert_eq!(feedback.score, Some(50.0));
    assert!(feedback.items[0].correct);
    assert!(!feedback.items[1].correct);
    assert_eq!(feedback.items[1].reference.as_deref(), Some("reads"));
}

/// 文法題答錯必須留下紀錄，否則做再多練習系統也學不到你哪裡不會。
///
/// 這條路徑原本是斷的：選擇題在本地判分，但只產生「對/錯」，
/// 沒有把 grammar_point 轉成 correction，於是弱點永遠是空的。
#[tokio::test]
async fn wrong_grammar_answers_become_recorded_weak_points() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    let llm = FakeLlm::new(&[r#"{"items":[
             {"prompt":"I ___ yesterday","options":["go","went"],"option_notes":["note1","note2"],"answer_index":1,
              "grammar_point":"tense","explanation":"過去式"},
             {"prompt":"She ___ a book","options":["read","reads"],"option_notes":["note1","note2"],"answer_index":1,
              "grammar_point":"subject-verb agreement"},
             {"prompt":"___ apple","options":["a","an"],"option_notes":["note1","note2"],"answer_index":1,
              "grammar_point":"articles"}
           ]}"#]);
    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    // 第一題對，後兩題錯
    let feedback = engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec![],
                choices: answer(&exercise, &[Some(true), Some(false), None]),
                marked_unknown: vec![],
            },
            t0(),
        )
        .await
        .unwrap();

    assert_eq!(feedback.corrections.len(), 2, "兩題錯就該有兩筆修正");
    let points: Vec<&str> = feedback
        .corrections
        .iter()
        .filter_map(|c| c.grammar_point.as_deref())
        .collect();
    assert!(points.contains(&"subject-verb agreement"));
    assert!(points.contains(&"articles"));
    assert!(!points.contains(&"tense"), "答對的題目不該被記成弱點");

    // 沒作答的那題要標示出來，不能假裝他選了什麼
    let unanswered = feedback
        .corrections
        .iter()
        .find(|c| c.grammar_point.as_deref() == Some("articles"))
        .unwrap();
    assert_eq!(unanswered.original, "（沒有作答）");

    // 真的累積進學習者狀態，下次出題才用得到。
    // 剛答錯的文法點是排「一分鐘後」再練，所以要讓時間往前走一點——
    // 實際使用時，看完批改再點下一題本來就不只一分鐘。
    let learner = engine
        .learner_profile(profile, t0() + Duration::minutes(5))
        .await
        .unwrap();
    assert!(learner.weak_grammar.contains(&"articles".to_string()));
    assert!(!learner.weak_grammar.contains(&"tense".to_string()));
}

/// 批改時要讓模型知道這個人的老毛病，它才分得出「又犯了」和「第一次」。
#[tokio::test]
async fn grading_tells_the_model_about_past_mistakes() {
    let (db, profile) = setup(&["park"]).await;
    set_vocabulary(&db, profile, 500).await;

    let llm = FakeLlm::new(&[
        // 第一次練習：錯了時態
        r#"{"items":[{"source":"我去了公園"}]}"#,
        r#"{"score":40,"corrections":[{"original":"I go","corrected":"I went",
                                       "grammar_point":"tense"}]}"#,
        // 第二次練習
        r#"{"items":[{"source":"她吃了蘋果"}]}"#,
        r#"{"score":50,"corrections":[]}"#,
    ]);
    let engine = PracticeEngine::new(&db, &llm);

    for answer in ["I go park", "She eat apple"] {
        let exercise = engine
            .generate(profile, Some(ExerciseKind::TranslationToTarget), t0())
            .await
            .unwrap();
        engine
            .grade(
                profile,
                &GradeInput {
                    exercise_id: exercise.exercise_id,
                    answers: vec![answer.into()],
                    choices: vec![],
                    marked_unknown: vec![],
                },
                t0(),
            )
            .await
            .unwrap();
    }

    // 第二次批改的 prompt 應該帶著第一次累積的 tense
    assert!(
        llm.last_prompt().contains("tense"),
        "批改時沒有帶上既有弱點：{}",
        llm.last_prompt()
    );
}

/// 詞彙量不夠就不該硬出閱讀測驗——看不懂九成只會變成查字典。
#[tokio::test]
async fn reading_is_refused_when_vocabulary_is_too_small() {
    let (db, profile) = setup(&["cat"]).await;
    set_vocabulary(&db, profile, 100).await;

    let llm = FakeLlm::new(&[]);
    let engine = PracticeEngine::new(&db, &llm);

    let err = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        wordforge_practice::PracticeError::NotEnoughVocabulary
    ));
    assert_eq!(llm.call_count(), 0, "不該白呼叫模型");
}

/// 沒指定題型時，系統依詞彙量自己選。
#[tokio::test]
async fn the_system_picks_a_kind_that_fits_the_learner() {
    let (db, profile) = setup(&["cat"]).await;
    set_vocabulary(&db, profile, 50).await;

    let llm = FakeLlm::new(&[r#"{"items":[{"source":"The cat is here"}]}"#]);
    let engine = PracticeEngine::new(&db, &llm);

    let exercise = engine.generate(profile, None, t0()).await.unwrap();
    assert_eq!(
        exercise.kind,
        ExerciseKind::TranslationToNative,
        "50 個字只能做英翻中"
    );
}

/// 批改結果要存起來，之後才累積得出文法弱點。
#[tokio::test]
async fn corrections_accumulate_into_weak_points() {
    let (db, profile) = setup(&["park"]).await;
    set_vocabulary(&db, profile, 500).await;

    let llm = FakeLlm::new(&[
        r#"{"items":[{"source":"我去了公園"}]}"#,
        r#"{"score":40,"corrections":[
             {"original":"I go","corrected":"I went","grammar_point":"tense"},
             {"original":"park","corrected":"the park","grammar_point":"articles"}
           ]}"#,
    ]);
    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::TranslationToTarget), t0())
        .await
        .unwrap();
    engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec!["I go park".into()],
                choices: vec![],
                marked_unknown: vec![],
            },
            t0(),
        )
        .await
        .unwrap();

    let learner = engine
        .learner_profile(profile, t0() + Duration::minutes(5))
        .await
        .unwrap();
    assert!(learner.weak_grammar.contains(&"tense".to_string()));
    assert!(learner.weak_grammar.contains(&"articles".to_string()));
}

/// 送給模型的詞彙資訊必須有上限，而且要涵蓋各個難度。
///
/// 這是整個設計的成本關鍵：把一萬個已知詞塞進 prompt 大約 15,000 token，
/// 每出一題就燒一次。真正保證難度的是本地覆蓋率驗收，不是這份樣本。
#[tokio::test]
async fn the_vocabulary_sample_is_bounded_and_spans_difficulties() {
    let db = Db::open_in_memory().await.unwrap();
    let profile = profiles::create(&db, "我", "zh-TW", "en", t0())
        .await
        .unwrap();

    // 一本兩千字的字典，詞頻 1..2000
    let source = wordforge_db::dict::upsert_source(
        &db,
        NewSource {
            slug: "test",
            name: "測試字典",
            license: None,
            attribution: None,
            homepage: None,
            version: None,
        },
        t0(),
    )
    .await
    .unwrap();
    let mut conn = db.pool().acquire().await.unwrap();
    for rank in 1..=2_000 {
        let word = format!("word{rank:05}");
        wordforge_db::dict::write_entry(
            &mut conn,
            source,
            &EntryWrite {
                lang: "en",
                headword: &word,
                pos: "",
                freq_rank: Some(rank),
                senses: vec![NewSense {
                    gloss: "意思",
                    gloss_lang: "zh-CN",
                    translation: Some("意思"),
                    ..Default::default()
                }],
                ..Default::default()
            },
            wordforge_db::dict::WriteMode::Replace,
        )
        .await
        .unwrap();
    }
    drop(conn);
    set_vocabulary(&db, profile.0, 2_000).await;

    let llm = FakeLlm::new(&[
        r#"{"items":[{"prompt":"x","options":["a","b"],"option_notes":["note1","note2"],"answer_index":0}]}"#,
    ]);
    let engine = PracticeEngine::new(&db, &llm);
    engine
        .generate(profile.0, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    // prompt 裡出現的樣本詞
    let prompt = llm.last_prompt();
    let sampled: Vec<i64> = (1..=2_000)
        .filter(|r| prompt.contains(&format!("word{r:05}")))
        .collect();

    assert!(
        !sampled.is_empty() && sampled.len() <= 60,
        "樣本數要有上限，實際 {} 個",
        sampled.len()
    );

    // 涵蓋各難度：不能全部擠在最常用的那一段
    let hardest = sampled.iter().max().copied().unwrap();
    let easiest = sampled.iter().min().copied().unwrap();
    assert!(easiest <= 250, "要有基礎詞，最簡單的是第 {easiest} 名");
    assert!(
        hardest > 1_000,
        "要有接近程度上緣的字，否則模型判斷不出難度；最難的只到第 {hardest} 名"
    );
}

/// 卡片很少但測驗說程度不錯時，樣本不能只有那幾張卡。
///
/// 實際情況：在 App 裡複習過的只有 21 個字，分級測驗卻估出 5200 字。
/// 只送那 21 個最常用的字，模型會以為這是初學者。
#[tokio::test]
async fn a_small_deck_does_not_make_the_learner_look_like_a_beginner() {
    let db = Db::open_in_memory().await.unwrap();
    let profile = profiles::create(&db, "我", "zh-TW", "en", t0())
        .await
        .unwrap();

    let source = wordforge_db::dict::upsert_source(
        &db,
        NewSource {
            slug: "test",
            name: "測試字典",
            license: None,
            attribution: None,
            homepage: None,
            version: None,
        },
        t0(),
    )
    .await
    .unwrap();
    let mut conn = db.pool().acquire().await.unwrap();
    for rank in 1..=1_000 {
        let word = format!("word{rank:05}");
        wordforge_db::dict::write_entry(
            &mut conn,
            source,
            &EntryWrite {
                lang: "en",
                headword: &word,
                pos: "",
                freq_rank: Some(rank),
                senses: vec![NewSense {
                    gloss: "意思",
                    gloss_lang: "zh-CN",
                    translation: Some("意思"),
                    ..Default::default()
                }],
                ..Default::default()
            },
            wordforge_db::dict::WriteMode::Replace,
        )
        .await
        .unwrap();
    }
    drop(conn);

    // 牌組裡只有三個最常用的字，但測驗說會 1000 個
    for lemma in 1..=3 {
        put_in_deck(&db, profile.0, lemma).await;
    }
    set_vocabulary(&db, profile.0, 1_000).await;

    let llm = FakeLlm::new(&[
        r#"{"items":[{"prompt":"x","options":["a","b"],"option_notes":["note1","note2"],"answer_index":0}]}"#,
    ]);
    let engine = PracticeEngine::new(&db, &llm);
    engine
        .generate(profile.0, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    let prompt = llm.last_prompt();
    let sampled: Vec<i64> = (1..=1_000)
        .filter(|r| prompt.contains(&format!("word{r:05}")))
        .collect();

    assert!(
        sampled.len() > 10,
        "樣本不該被牌組大小限制住，實際只有 {} 個",
        sampled.len()
    );
    assert!(
        sampled.iter().any(|r| *r > 500),
        "要抽到測驗推定他會的那些較難的字"
    );
}

/// 連續出題不該一直是同一個情境。
///
/// 不指定主題時模型永遠寫校園生活與天氣，十篇讀起來像同一篇。
#[tokio::test]
async fn consecutive_articles_use_different_topics() {
    let (db, profile) = setup(&["the", "cat", "sat"]).await;
    set_vocabulary(&db, profile, 3_000).await;
    for lemma in 1..=3 {
        put_in_deck(&db, profile, lemma).await;
    }
    sqlx::query("UPDATE card SET state='review', stability=40.0")
        .execute(db.pool())
        .await
        .unwrap();

    let article = r#"{"title":"T","passage":"The cat sat. The cat sat.",
                      "questions":[],"new_words":[]}"#;
    let llm = FakeLlm::new(&[article, article, article]);
    let engine = PracticeEngine::new(&db, &llm);

    let mut topics = Vec::new();
    for i in 0..3 {
        engine
            .generate(
                profile,
                Some(ExerciseKind::Reading),
                t0() + Duration::hours(i),
            )
            .await
            .unwrap();

        // 從 prompt 裡找出這次用了哪個主題
        let prompt = llm.last_prompt();
        let used = wordforge_core::practice::TOPICS
            .iter()
            .find(|t| prompt.contains(**t))
            .expect("prompt 裡沒有指定主題");
        topics.push(*used);
    }

    let unique: std::collections::HashSet<&&str> = topics.iter().collect();
    assert_eq!(unique.len(), 3, "三次應該是三個不同主題，實際：{topics:?}");
}

/// 覆蓋率不合格重寫時，模型必須看得到自己上一篇寫了什麼。
///
/// 重試訊息說「其餘內容盡量保留」，但 API 那邊我們沒把模型的回答
/// 加進 messages，CLI 更是每次全新行程——沒附上的話那句話無從執行。
#[tokio::test]
async fn the_retry_shows_the_model_its_previous_attempt() {
    let (db, profile) = setup(&["the", "on", "cat", "sat", "mat", "ubiquitous"]).await;
    set_vocabulary(&db, profile, 3_000).await;
    for lemma in 1..=5 {
        put_in_deck(&db, profile, lemma).await;
    }
    make_rare(&db, 6, 30_000).await;
    sqlx::query("UPDATE card SET state='review', stability=40.0")
        .execute(db.pool())
        .await
        .unwrap();

    let llm = FakeLlm::new(&[
        // 太難，會被打回
        r#"{"title":"Hard","passage":"Ubiquitous ubiquitous ubiquitous.",
            "questions":[],"new_words":[]}"#,
        r#"{"title":"Easy","passage":"The cat sat on the mat.",
            "questions":[],"new_words":[]}"#,
    ]);
    let engine = PracticeEngine::new(&db, &llm);
    engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap();

    let retry = llm.last_prompt();
    assert!(
        retry.contains("Ubiquitous ubiquitous ubiquitous."),
        "重試時沒把上一篇附上，模型無從「保留其餘內容」：{retry}"
    );
    assert!(retry.contains("ubiquitous"), "也要指名哪些詞超標");
}

/// 模型對同一個文法點的各種說法，必須收斂成一筆。
///
/// 沒有這一步的話，`tense`、`past tense`、`Verb Tense` 會變成三個
/// 各自排程的文法點，統計和練習都被稀釋。
#[tokio::test]
async fn grammar_labels_are_normalized_to_one_point() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    // 三題都在考時態，但模型每題寫成不同說法
    let llm = FakeLlm::new(&[r#"{"items":[
             {"prompt":"a","options":["x","y"],"option_notes":["note1","note2"],"answer_index":0,"grammar_point":"tense"},
             {"prompt":"b","options":["x","y"],"option_notes":["note1","note2"],"answer_index":0,"grammar_point":"Past Tense"},
             {"prompt":"c","options":["x","y"],"option_notes":["note1","note2"],"answer_index":0,"grammar_point":"verb_tense"}
           ]}"#]);
    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    // 三題全錯
    engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec![],
                choices: answer(&exercise, &[Some(false); 3]),
                marked_unknown: vec![],
            },
            t0(),
        )
        .await
        .unwrap();

    let points = wordforge_db::grammar::all_points(&db, ProfileId(profile))
        .await
        .unwrap();
    assert_eq!(points.len(), 1, "三種寫法應該收斂成一個：{points:?}");
    assert_eq!(points[0].point, "tense");
    assert_eq!(points[0].error_count, 3, "三次都要算在同一個文法點上");
}

/// 認不出來的標籤寧可丟掉，不要污染統計。
#[tokio::test]
async fn unrecognised_grammar_labels_are_dropped() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    let llm = FakeLlm::new(&[r#"{"items":[
             {"prompt":"a","options":["x","y"],"option_notes":["note1","note2"],"answer_index":0,"grammar_point":"這句怪怪的"},
             {"prompt":"b","options":["x","y"],"option_notes":["note1","note2"],"answer_index":0,"grammar_point":"articles"}
           ]}"#]);
    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec![],
                choices: answer(&exercise, &[Some(false); 2]),
                marked_unknown: vec![],
            },
            t0(),
        )
        .await
        .unwrap();

    let points = wordforge_db::grammar::all_points(&db, ProfileId(profile))
        .await
        .unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].point, "articles");
}

/// 換一份字典就該能學那個語言：語言必須從 profile 一路流到 prompt 與字典查詢。
///
/// 這條測試存在的理由是它曾經整條斷掉——`profile.target_lang` 有寫進去
/// 但沒有任何地方讀出來，每個查詢都硬編 `"en"`，於是日文 profile 拿到的
/// 是「請用 English 出題」加上一次查不到東西的英文字典查詢。
#[tokio::test]
async fn the_profile_language_drives_prompts_and_lookups() {
    let (db, profile) = setup_lang("ja", &["公園", "勤勉", "天気"]).await;
    set_vocabulary(&db, profile, 300).await;
    put_in_deck(&db, profile, 1).await;

    let llm = FakeLlm::new(&[
        r#"{"items":[{"source":"昨日は公園に行きました","target_word":"公園",
                      "reference":"I went to the park yesterday"}]}"#,
        r#"{"score":50,
            "items":[{"index":1,"correct":false}],
            "corrections":[],
            "unknown_words":["勤勉"]}"#,
    ]);

    let engine = PracticeEngine::for_profile(&db, &llm, profile)
        .await
        .unwrap();
    let exercise = engine
        .generate(profile, Some(ExerciseKind::TranslationToTarget), t0())
        .await
        .unwrap();

    let prompt = llm.last_prompt();
    assert!(
        prompt.contains("日本語"),
        "出題 prompt 沒提到目標語言：{prompt}"
    );
    assert!(
        !prompt.contains("English"),
        "出題 prompt 仍然在講英文：{prompt}"
    );

    engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec!["公園に行った".into()],
                choices: vec![],
                marked_unknown: vec![],
            },
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap();

    // 模型說「他不會勤勉」——那個字只存在於日文字典裡，
    // 查得到才代表字典查詢用的是 profile 的語言。
    let added: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM card c JOIN lemma l ON l.id = c.lemma_id
         WHERE c.profile_id = ? AND l.text = '勤勉'",
    )
    .bind(profile)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(added, 1, "日文的生字沒有被排進複習");
}

/// 閱讀解析的生字與片語由本地字典查，不是模型寫的。
///
/// 這條分開測是因為它是整份解析裡唯一「不可能出錯」的部分：
/// 模型可能亂講，但字典查得到就是查得到。
#[tokio::test]
async fn the_reading_glossary_comes_from_the_dictionary_not_the_model() {
    let (db, profile) = setup(&["search for", "diligent", "the", "key"]).await;
    set_vocabulary(&db, profile, 2_000).await;
    // diligent 要超出詞彙量才會被算成生字；其餘的都算會
    make_rare(&db, 2, 30_000).await;

    let passage = r#"{"title":"T","passage":"She had to search for the diligent key.",
            "questions":[{"question":"Q","options":["A","B"],"option_notes":["note1","note2"],"answer_index":0}]}"#;
    let llm = FakeLlm::new(&[
        passage,
        passage,
        passage,
        r#"{"score":0,"items":[{"index":1,"correct":false}],"unknown_words":[]}"#,
    ]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap();

    let feedback = engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec![],
                choices: vec![Some(1)],
                marked_unknown: vec![],
            },
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap();

    let phrase = feedback
        .glossary
        .iter()
        .find(|g| g.is_phrase)
        .expect("字典裡有 search for 這個條目，解析就該列出來");
    assert_eq!(phrase.text, "search for");

    assert!(
        feedback.glossary.iter().any(|g| g.text == "diligent"),
        "沒學過的字要附釋義：{:?}",
        feedback.glossary
    );
    assert!(
        !feedback.glossary.iter().any(|g| g.text == "the"),
        "虛詞不該出現在解析裡：{:?}",
        feedback.glossary
    );
    // 模型完全沒提供 glossary，這些全是本地查出來的
    assert!(!feedback.glossary.is_empty());

    // 解析時點文章裡的任何一個字都要查得到，所以已經會的字也要在裡面，
    // 但 is_unknown 要分得出來——UI 要挑「這篇的生字」時看的是那一欄。
    let known = feedback
        .glossary
        .iter()
        .find(|g| g.text == "key")
        .expect("已經會的實詞也要能查到釋義，否則點下去什麼都不會跳出來");
    assert!(!known.is_unknown, "學過的字不該被標成生字");
    let unknown = feedback
        .glossary
        .iter()
        .find(|g| g.text == "diligent")
        .unwrap();
    assert!(unknown.is_unknown);
}

/// 片語偵測不能只對有空格的語言成立。
#[tokio::test]
async fn phrases_are_detected_in_a_language_without_spaces() {
    let (db, profile) = setup_lang("ja", &["気にする", "勤勉"]).await;
    set_vocabulary(&db, profile, 2_000).await;

    let passage = r#"{"title":"T","passage":"勤勉な人は気にする。",
            "questions":[{"question":"Q","options":["A","B"],"option_notes":["note1","note2"],"answer_index":0}]}"#;
    let llm = FakeLlm::new(&[
        passage,
        passage,
        passage,
        r#"{"score":0,"items":[{"index":1,"correct":false}],"unknown_words":[]}"#,
    ]);

    let engine = PracticeEngine::for_profile(&db, &llm, profile)
        .await
        .unwrap();
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap();

    let feedback = engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec![],
                choices: vec![Some(1)],
                marked_unknown: vec![],
            },
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap();

    assert!(
        feedback
            .glossary
            .iter()
            .any(|g| g.text == "気にする" && g.is_phrase),
        "日文沒有空格，n-gram 要用空字串接回去才查得到：{:?}",
        feedback.glossary
    );
}

/// 生字要建在原形上，不然同一個字會散成好幾張卡各自排程。
#[tokio::test]
async fn an_inflection_from_grading_lands_on_the_base_form() {
    let (db, profile) = setup(&["study", "park"]).await;
    set_vocabulary(&db, profile, 300).await;
    put_in_deck(&db, profile, 2).await;

    // 變化形自己也是詞條，而且拼字排在原形前面——這是當初出錯的形狀
    let studied = lemmas::upsert(
        &db,
        NewLemma {
            lang: "en",
            text: "studied",
            pos: "",
            freq_rank: Some(15_971),
            cefr: None,
        },
    )
    .await
    .unwrap();
    lemmas::add_surface_form(&db, "en", "studied", LemmaId(1), "past")
        .await
        .unwrap();

    let llm = FakeLlm::new(&[
        r#"{"items":[{"source":"我去了公園","target_word":"park","reference":"I went to the park"}]}"#,
        r#"{"score":50,"items":[{"index":1,"correct":false}],
            "corrections":[],"unknown_words":["studied"]}"#,
    ]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::TranslationToTarget), t0())
        .await
        .unwrap();
    engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec!["I go to park".into()],
                choices: vec![],
                marked_unknown: vec![],
            },
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap();

    let on_base: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE profile_id = ? AND lemma_id = ?")
            .bind(profile)
            .bind(1_i64)
            .fetch_one(db.pool())
            .await
            .unwrap();
    let on_inflection: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM card WHERE profile_id = ? AND lemma_id = ?")
            .bind(profile)
            .bind(studied.0)
            .fetch_one(db.pool())
            .await
            .unwrap();

    assert_eq!(on_base, 1, "卡片該建在 study 上");
    assert_eq!(on_inflection, 0, "不該多開一張 studied 的卡");
}

/// 指定教材之後，模型看到的 prompt 裡必須真的有課本原文。
///
/// 這是「只考課本」整個功能的驗收點：資料表、切塊、檢索都做對了，
/// 但只要 engine 忘了把 excerpt 傳下去，使用者看到的還是自由發揮的題目。
#[tokio::test]
async fn a_chosen_material_constrains_what_the_model_sees() {
    let (db, profile) = setup(&["park", "reluctant"]).await;
    set_vocabulary(&db, profile, 300).await;
    put_in_deck(&db, profile, 2).await;

    let material = wordforge_db::material::create(
        &db,
        ProfileId(profile),
        wordforge_db::material::NewMaterial {
            title: "第三課",
            kind: "text",
            lang: "en",
            source_path: None,
            license_note: None,
        },
        t0(),
    )
    .await
    .unwrap();
    let chunks = wordforge_db::material::add_chunks(
        &db,
        material,
        &[
            "The weather was nice on Sunday.".into(),
            "Amy was reluctant to buy the fish at the market.".into(),
        ],
    )
    .await
    .unwrap();
    // reluctant 只出現在第二塊；檢索要挑中它而不是第一塊
    let reluctant = lemmas::base_form(&db, "en", "reluctant")
        .await
        .unwrap()
        .unwrap();
    wordforge_db::material::set_chunk_vocab(&db, &[(chunks[1], vec![(reluctant, 1)])])
        .await
        .unwrap();

    let llm = FakeLlm::new(&[
        r#"{"items":[{"source":"艾米不願意買魚","target_word":"reluctant",
                      "reference":"Amy was reluctant to buy the fish"}]}"#,
    ]);

    let engine = PracticeEngine::new(&db, &llm).with_material(Some(material.0));
    engine
        .generate(profile, Some(ExerciseKind::TranslationToTarget), t0())
        .await
        .unwrap();

    let prompt = llm.last_prompt();
    assert!(
        prompt.contains("Amy was reluctant to buy the fish at the market."),
        "課本原文沒有進 prompt：{prompt}"
    );
    assert!(prompt.contains("指定教材"), "沒有講清楚這是硬限制");
}

/// 沒指定教材時 prompt 裡不該出現空的教材段落。
#[tokio::test]
async fn no_material_means_no_material_section() {
    let (db, profile) = setup(&["park"]).await;
    set_vocabulary(&db, profile, 300).await;
    put_in_deck(&db, profile, 1).await;

    let llm = FakeLlm::new(&[
        r#"{"items":[{"source":"我去公園","target_word":"park","reference":"I go to the park"}]}"#,
    ]);
    let engine = PracticeEngine::new(&db, &llm);
    engine
        .generate(profile, Some(ExerciseKind::TranslationToTarget), t0())
        .await
        .unwrap();

    assert!(!llm.last_prompt().contains("指定教材"));
}

/// 完全沒有已知詞資料時，重試沒有意義——不能白燒兩次額度。
///
/// 這是實測發現的：使用者背了三週但沒有一張卡的 stability 達到 21 天，
/// 嚴格定義下已知詞是空集合，覆蓋率永遠 0%，每題都跑滿三輪重試，
/// 一題 98 秒而且驗收完全沒有作用。
#[tokio::test]
async fn no_known_words_means_no_pointless_retries() {
    let (db, profile) = setup(&["alpha", "beta"]).await;
    // 詞彙量夠高才出得了閱讀測驗，但字典裡沒有任何字落在那個範圍內，
    // 牌組也是空的——known_vocabulary 於是是空集合。
    // 這對應到「分級測驗做過了，但字典的詞頻表還沒匯入」。
    set_vocabulary(&db, profile, 2_000).await;
    make_rare(&db, 1, 30_000).await;
    make_rare(&db, 2, 30_000).await;

    let llm = FakeLlm::new(&[r#"{"title":"T","passage":"Alpha beta gamma delta.",
            "questions":[{"question":"Q","options":["A","B"],"option_notes":["note1","note2"],"answer_index":0}]}"#]);

    let engine = PracticeEngine::new(&db, &llm);
    let ex = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .expect("沒有基準時應該接受第一篇，而不是重試到放棄");

    assert_eq!(llm.call_count(), 1, "只該呼叫一次");
    assert_eq!(ex.coverage, Some(0.0), "覆蓋率照實記 0，不要假裝有驗過");
}

/// 有基準時，太難的文章還是要被打回——防呆不能把驗收整個關掉。
#[tokio::test]
async fn a_baseline_still_enforces_the_coverage_rule() {
    let (db, profile) = setup(&["the", "cat", "sat", "on", "mat", "ubiquitous"]).await;
    set_vocabulary(&db, profile, 3_000).await;
    make_rare(&db, 6, 30_000).await;

    let hard = r#"{"title":"H","passage":"Ubiquitous ubiquitous ubiquitous.",
            "questions":[{"question":"Q","options":["A","B"],"option_notes":["note1","note2"],"answer_index":0}]}"#;
    let llm = FakeLlm::new(&[hard, hard, hard]);

    let engine = PracticeEngine::new(&db, &llm);
    engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .ok();

    assert_eq!(llm.call_count(), 3, "有基準就該重試到上限");
}

/// 連續出兩篇文章，生詞不能一模一樣。
///
/// 生詞是照詞頻決定性挑出來的，而且**不會自動進牌組**——使用者讀完
/// 從上下文看懂了、沒標記任何字，那些字就留在候選池裡。沒有輪換的話
/// 第二篇會拿到完全相同的六個字。
#[tokio::test]
async fn consecutive_articles_teach_different_new_words() {
    let mut words: Vec<String> = vec!["the".into(), "cat".into(), "sat".into()];
    // 一批夠大的候選，讓輪換有東西可換
    for i in 0..60 {
        words.push(format!("newword{i}"));
    }
    let refs: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
    let (db, profile) = setup(&refs).await;
    set_vocabulary(&db, profile, 3_000).await;

    // 前三個當已知詞，其餘推到詞彙量之外當生詞候選
    for i in 4..=words.len() as i64 {
        make_rare(&db, i, 3_500 + i).await;
    }

    let passage = r#"{"title":"T","passage":"The cat sat.",
        "questions":[{"question":"Q","options":["A","B"],"option_notes":["note1","note2"],"answer_index":0}]}"#;
    let llm = FakeLlm::new(&[passage, passage, passage, passage, passage, passage]);
    let engine = PracticeEngine::new(&db, &llm);

    let first = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap()
        .target_words;
    let second = engine
        .generate(
            profile,
            Some(ExerciseKind::Reading),
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap()
        .target_words;

    assert!(!first.is_empty(), "第一篇要有生詞");
    let overlap: Vec<&String> = second.iter().filter(|w| first.contains(w)).collect();
    assert!(
        overlap.is_empty(),
        "第二篇又教了同樣的字：{overlap:?}\n第一篇 {first:?}\n第二篇 {second:?}"
    );
}

/// 候選被排光時寧可重複，也不能一個生詞都不給。
///
/// 沒有生詞的文章覆蓋率會衝到 99%，那就回到當初「整篇沒東西可學」的問題。
#[tokio::test]
async fn a_drained_pool_repeats_rather_than_giving_none() {
    let (db, profile) = setup(&["the", "cat", "sat", "solitary"]).await;
    set_vocabulary(&db, profile, 3_000).await;
    make_rare(&db, 4, 3_500).await;

    let passage = r#"{"title":"T","passage":"The cat sat.",
        "questions":[{"question":"Q","options":["A","B"],"option_notes":["note1","note2"],"answer_index":0}]}"#;
    let llm = FakeLlm::new(&[passage, passage, passage, passage, passage, passage]);
    let engine = PracticeEngine::new(&db, &llm);

    let first = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap()
        .target_words;
    let second = engine
        .generate(
            profile,
            Some(ExerciseKind::Reading),
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap()
        .target_words;

    assert_eq!(first, vec!["solitary"]);
    assert_eq!(
        second,
        vec!["solitary"],
        "只有一個候選時要重複給，不能給空的"
    );
}

/// 這篇教的生詞，讀完就該進牌組。
///
/// 不這樣做的話，使用者得自己一個一個點「我不會」才會被記錄，
/// 而人不會這樣做——從上下文看懂了就往下讀了。結果是那些字永遠留在
/// 候選池裡，只能靠五篇的記憶視窗擋著，六篇之後又拿到同一批。
#[tokio::test]
async fn words_taught_by_an_article_enter_the_deck() {
    let (db, profile) = setup(&["the", "cat", "sat", "solitary", "gaze"]).await;
    set_vocabulary(&db, profile, 3_000).await;
    make_rare(&db, 4, 3_500).await;
    make_rare(&db, 5, 3_600).await;

    let llm = FakeLlm::new(&[
        // 大部分是已知詞，覆蓋率才過得了驗收
        r#"{"title":"T","passage":"The cat sat. The cat sat. The cat sat solitary.",
            "questions":[{"question":"Q","options":["A","B"],"option_notes":["note1","note2"],"answer_index":0}]}"#,
        r#"{"score":100,"items":[{"index":1,"correct":true}],"unknown_words":[]}"#,
    ]);

    let engine = PracticeEngine::new(&db, &llm);
    let ex = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap();
    assert!(!ex.target_words.is_empty(), "這篇要有生詞");

    // 全部答對、一個字都沒標記——最常見的情況
    let feedback = engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: ex.exercise_id,
                answers: vec![],
                choices: vec![Some(0)],
                marked_unknown: vec![],
            },
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap();

    for word in &ex.target_words {
        assert!(
            feedback.taught_words.contains(word),
            "{word} 該被列為這篇教的字：{:?}",
            feedback.taught_words
        );
        let in_deck: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM card c JOIN lemma l ON l.id = c.lemma_id
             WHERE c.profile_id = ? AND l.text = ?",
        )
        .bind(profile)
        .bind(word)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(in_deck, 1, "{word} 沒有進牌組");
    }
}

/// 進了牌組之後就不該再被當成生詞——輪換從此是永久的，
/// 不再只靠五篇的記憶視窗。
#[tokio::test]
async fn a_word_already_taught_is_never_offered_as_new_again() {
    let (db, profile) = setup(&["the", "cat", "sat", "solitary", "gaze"]).await;
    set_vocabulary(&db, profile, 3_000).await;
    make_rare(&db, 4, 3_500).await;
    make_rare(&db, 5, 3_600).await;

    let passage = r#"{"title":"T","passage":"The cat sat. The cat sat. The cat sat.",
        "questions":[{"question":"Q","options":["A","B"],"option_notes":["note1","note2"],"answer_index":0}]}"#;
    let graded = r#"{"score":100,"items":[{"index":1,"correct":true}],"unknown_words":[]}"#;
    let llm = FakeLlm::new(&[passage, graded, passage, graded, passage]);
    let engine = PracticeEngine::new(&db, &llm);

    let first = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap();
    engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: first.exercise_id,
                answers: vec![],
                choices: vec![Some(0)],
                marked_unknown: vec![],
            },
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap();

    // 把出題歷史清掉，證明「不重複」靠的是牌組而不是記憶視窗
    sqlx::query("DELETE FROM exercise WHERE profile_id = ?")
        .bind(profile)
        .execute(db.pool())
        .await
        .unwrap();

    let second = engine
        .generate(
            profile,
            Some(ExerciseKind::Reading),
            t0() + Duration::hours(1),
        )
        .await
        .unwrap();

    for word in &second.target_words {
        assert!(
            !first.target_words.contains(word),
            "{word} 已經教過也進牌組了，不該再當生詞"
        );
    }
}

/// 模型出的題目，正確答案會集中在同一個位置——實際看過一整份都是
/// 第一個選項。使用者不用讀題就猜得到，那份練習就白做了。
///
/// 這件事只能在本地做：「請把答案分散」是驗收不了的請求。
#[tokio::test]
async fn the_correct_answer_does_not_always_sit_in_the_same_slot() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    // 每一份都是四題，而且模型每次都把答案放在第一個
    const ITEMS: &str = r#"{"items":[
             {"prompt":"a","options":["A1","A2","A3","A4"],"option_notes":["note1","note2","note3","note4"],"answer_index":0},
             {"prompt":"b","options":["B1","B2","B3","B4"],"option_notes":["note1","note2","note3","note4"],"answer_index":0},
             {"prompt":"c","options":["C1","C2","C3","C4"],"option_notes":["note1","note2","note3","note4"],"answer_index":0},
             {"prompt":"d","options":["D1","D2","D3","D4"],"option_notes":["note1","note2","note3","note4"],"answer_index":0}
           ]}"#;
    const ROUNDS: i64 = 10;

    let llm = FakeLlm::new(&[ITEMS; ROUNDS as usize]);
    let engine = PracticeEngine::new(&db, &llm);

    let mut seen = std::collections::HashSet::new();
    for round in 0..ROUNDS {
        // 出題時間不同，洗出來的順序就不同
        let exercise = engine
            .generate(
                profile,
                Some(ExerciseKind::Grammar),
                t0() + Duration::minutes(round),
            )
            .await
            .unwrap();

        let wordforge_practice::payload::ExerciseBody::Choices { items } = &exercise.body else {
            panic!("文法題該是選擇題");
        };

        for (i, item) in items.iter().enumerate() {
            seen.insert(item.answer_index);

            // 洗牌不能把答案本身弄丟：正確選項仍該是模型指定的那一個
            let expected = format!("{}1", ["A", "B", "C", "D"][i]);
            assert_eq!(
                item.options[item.answer_index],
                expected,
                "第 {} 題洗完之後指到了別的選項",
                i + 1
            );
            assert_eq!(item.options.len(), 4, "選項被弄丟了");
        }
    }

    assert_eq!(seen.len(), 4, "答案只落在 {seen:?} 這幾個位置，等於沒有洗");
}

/// 這條測試存在的理由是它曾經是錯的：不管中翻英還是英翻中，
/// 出題 prompt 都說「請出 N 個{母語}句子」，於是「英翻中」拿到的題目
/// 也是中文句子——那個題型等於不存在。
#[tokio::test]
async fn the_translation_direction_reaches_the_prompt() {
    let (db, profile) = setup(&["park"]).await;
    set_vocabulary(&db, profile, 1_000).await;
    put_in_deck(&db, profile, 1).await;

    let items = r#"{"items":[{"source":"S","target_word":"park","reference":"R"}]}"#;
    let llm = FakeLlm::new(&[items, items]);
    let engine = PracticeEngine::new(&db, &llm);

    engine
        .generate(profile, Some(ExerciseKind::TranslationToNative), t0())
        .await
        .unwrap();
    let to_native = llm.last_prompt();
    assert!(
        to_native.contains("English → 繁體中文"),
        "英翻中沒有把方向寫進 prompt：{to_native}"
    );
    assert!(
        to_native.contains("**English**句子"),
        "英翻中的題目句子該是英文：{to_native}"
    );

    engine
        .generate(
            profile,
            Some(ExerciseKind::TranslationToTarget),
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap();
    let to_target = llm.last_prompt();
    assert!(to_target.contains("繁體中文 → English"), "{to_target}");
    assert!(to_target.contains("**繁體中文**句子"), "{to_target}");
}

/// 這條測試存在的理由是它曾經是錯的：翻譯題用 `due_words`，那個查詢
/// 不看 `reps`。批改時 LLM 標出來的生詞會直接建成新卡，而新卡一建好
/// 就算「今天到期」——實測一個真實牌組，141 張到期的卡裡有 138 張
/// 從來沒看過。結果是中翻英要使用者寫出一個他從沒見過的單字。
#[tokio::test]
async fn translation_does_not_quiz_words_the_learner_has_never_studied() {
    let (db, profile) = setup(&[
        "alpha", "beta", "gamma", "delta", "epsilon", // 學過的
        "never1", "never2", "never3", "never4", // 只是躺在牌組裡
    ])
    .await;
    set_vocabulary(&db, profile, 1_000).await;

    for id in 1..=5 {
        study(&db, profile, id).await;
    }
    for id in 6..=9 {
        put_in_deck(&db, profile, id).await;
    }

    let items = r#"{"items":[{"source":"S","target_word":"alpha","reference":"R"}]}"#;
    let llm = FakeLlm::new(&[items]);
    let engine = PracticeEngine::new(&db, &llm);

    // 往後推，讓學過的字真的到期（`study` 給 Easy，下次複習排得很遠）
    engine
        .generate(
            profile,
            Some(ExerciseKind::TranslationToTarget),
            t0() + Duration::days(400),
        )
        .await
        .unwrap();

    let prompt = llm.last_prompt();
    for never in ["never1", "never2", "never3", "never4"] {
        assert!(
            !prompt.contains(never),
            "沒學過的字不該出現在翻譯題裡：{never}\n{prompt}"
        );
    }
}

/// 但牌組裡**只有**沒學過的字時，還是要出得了題。
///
/// 新使用者匯完字典、加了一批字就來練翻譯，這時候一個學過的字都沒有。
/// 給他沒學過的字，比一個字都給不出來好。
#[tokio::test]
async fn a_brand_new_deck_still_produces_a_translation_exercise() {
    let (db, profile) = setup(&["fresh1", "fresh2", "fresh3"]).await;
    set_vocabulary(&db, profile, 1_000).await;
    for id in 1..=3 {
        put_in_deck(&db, profile, id).await;
    }

    let items = r#"{"items":[{"source":"S","target_word":"fresh1","reference":"R"}]}"#;
    let llm = FakeLlm::new(&[items]);
    let engine = PracticeEngine::new(&db, &llm);

    engine
        .generate(profile, Some(ExerciseKind::TranslationToTarget), t0())
        .await
        .unwrap();

    let prompt = llm.last_prompt();
    assert!(
        ["fresh1", "fresh2", "fresh3"]
            .iter()
            .any(|w| prompt.contains(w)),
        "沒有可用的字時仍然要拿新卡頂上，不然這一頁對新使用者是空的：\n{prompt}"
    );
}

/// 解析要能給出全文翻譯；模型沒給的時候不能變成一段空白。
#[tokio::test]
async fn the_full_translation_is_kept_when_the_model_gives_one() {
    let (db, profile) = setup(&["the", "cat", "sat"]).await;
    set_vocabulary(&db, profile, 2_000).await;

    let with = r#"{"title":"T","passage":"The cat sat.","translation":"貓坐下了。",
        "questions":[{"question":"Q","options":["A","B"],"option_notes":["note1","note2"],"answer_index":0}]}"#;
    let llm = FakeLlm::new(&[with]);
    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap();
    let wordforge_practice::payload::ExerciseBody::Reading { translation, .. } = &exercise.body
    else {
        panic!("該是閱讀題");
    };
    assert_eq!(translation.as_deref(), Some("貓坐下了。"));

    // 只給空字串的話要當成沒給——UI 會出現一個打開來是空白的「全文翻譯」
    let blank = r#"{"title":"T","passage":"The cat sat.","translation":"   ",
        "questions":[{"question":"Q","options":["A","B"],"option_notes":["note1","note2"],"answer_index":0}]}"#;
    let llm = FakeLlm::new(&[blank]);
    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap();
    let wordforge_practice::payload::ExerciseBody::Reading { translation, .. } = &exercise.body
    else {
        panic!("該是閱讀題");
    };
    assert_eq!(translation.as_deref(), None);
}

/// 這條測試存在的理由是它曾經整條是假的：選「克漏字」時
/// `generate` 直接轉去 `generate_reading`，出來的是閱讀測驗，
/// 連存進資料庫的 kind 都寫成 reading——練習紀錄於是也跟著說謊。
#[tokio::test]
async fn choosing_cloze_actually_produces_a_cloze() {
    let (db, profile) = setup(&["borrow", "return", "weather"]).await;
    set_vocabulary(&db, profile, 1_000).await;
    for lemma in 1..=3 {
        study(&db, profile, lemma).await;
    }

    let llm = FakeLlm::new(&[r#"{"title":"A Rainy Day",
        "passage":"I had to {{1}} an umbrella because the {{2}} was bad.",
        "translation":"因為天氣很差，我得借一把傘。",
        "items":[
          {"options":["borrow","lend","buy","sell"],"option_notes":["note1","note2","note3","note4"],"answer_index":0,
           "explanation":"跟別人借用 borrow"},
          {"options":["weather","water","winter","wonder"],"option_notes":["note1","note2","note3","note4"],"answer_index":0,
           "explanation":"講的是天氣"}
        ]}"#]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Cloze), t0())
        .await
        .unwrap();

    assert_eq!(exercise.kind, ExerciseKind::Cloze, "存進去的題型不對");

    let wordforge_practice::payload::ExerciseBody::Cloze {
        passage,
        items,
        translation,
        ..
    } = &exercise.body
    else {
        panic!("選了克漏字卻拿到別的題型：{:?}", exercise.body);
    };
    assert_eq!(items.len(), 2);
    assert_eq!(translation.as_deref(), Some("因為天氣很差，我得借一把傘。"));
    assert_eq!(
        wordforge_core::practice::blank_numbers(passage),
        vec![1, 2],
        "文章裡要真的有挖空"
    );

    // 出題的 prompt 要說清楚挖的是哪些字
    let prompt = llm.last_prompt();
    assert!(prompt.contains("borrow"), "{prompt}");
    assert!(prompt.contains("只用學習者已經會的字"), "{prompt}");

    // 資料庫裡存的 kind 也要是 cloze，練習紀錄才不會說謊
    let kinds = wordforge_db::exercises::recent_kinds(&db, ProfileId(profile), 5)
        .await
        .unwrap();
    assert_eq!(kinds, vec!["cloze"]);
}

/// 克漏字在本地判分，一次模型都不用打；答錯的那格的正確答案要排回複習。
#[tokio::test]
async fn a_missed_blank_goes_back_into_the_deck() {
    let (db, profile) = setup(&["borrow", "return", "weather"]).await;
    set_vocabulary(&db, profile, 1_000).await;
    for lemma in 1..=3 {
        study(&db, profile, lemma).await;
    }

    let llm = FakeLlm::new(&[r#"{"title":"T",
        "passage":"I had to {{1}} an umbrella because the {{2}} was bad.",
        "items":[
          {"options":["borrow","lend","buy","sell"],"option_notes":["note1","note2","note3","note4"],"answer_index":0},
          {"options":["weather","water","winter","wonder"],"option_notes":["note1","note2","note3","note4"],"answer_index":0}
        ]}"#]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Cloze), t0())
        .await
        .unwrap();

    // 第一格對、第二格錯
    let feedback = engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec![],
                choices: answer(&exercise, &[Some(true), Some(false)]),
                marked_unknown: vec![],
            },
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap();

    assert_eq!(llm.call_count(), 1, "批改克漏字不該再呼叫模型");
    assert_eq!(feedback.score, Some(50.0));
    assert_eq!(
        feedback.unknown_words,
        vec!["weather"],
        "答錯的那格的正確答案該被當成「還沒真的會」"
    );
    assert!(
        feedback.taught_words.is_empty(),
        "挖空的是複習字，不是這篇新教的字"
    );
}

/// 模型會跳號、會多給一題。空格與題目對不上時要截到共同的長度，
/// 不然使用者會看到「有題目卻沒有空格」，而且判分照跑、錯得無聲無息。
#[tokio::test]
async fn extra_cloze_questions_without_a_blank_are_dropped() {
    let (db, profile) = setup(&["borrow", "return"]).await;
    set_vocabulary(&db, profile, 1_000).await;
    for lemma in 1..=2 {
        study(&db, profile, lemma).await;
    }

    let llm = FakeLlm::new(&[r#"{"title":"T","passage":"Please {{1}} it back.",
        "items":[
          {"options":["return","borrow"],"option_notes":["note1","note2"],"answer_index":0},
          {"options":["a","b"],"option_notes":["note1","note2"],"answer_index":0},
          {"options":["c","d"],"option_notes":["note1","note2"],"answer_index":0}
        ]}"#]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Cloze), t0())
        .await
        .unwrap();

    let wordforge_practice::payload::ExerciseBody::Cloze { items, .. } = &exercise.body else {
        panic!("該是克漏字");
    };
    assert_eq!(items.len(), 1, "只有一個空格，就只該留一題");
}

/// 克漏字不能把閱讀的生詞記憶沖掉。
///
/// 它的 target_words 是挖掉的**複習字**——那些他已經會了，本來就不在
/// 生詞候選池裡，排除它們沒有任何作用，卻會佔掉記憶名額。
/// 做幾題克漏字之後，下一篇閱讀就會拿回同一批生詞。
#[tokio::test]
async fn cloze_does_not_flush_the_reading_word_history() {
    // 候選池要夠大，第二篇才有別的字可挑——池子被掏空時會走
    // 「寧可重複也不能沒有生詞」那條退路，那樣就測不到記憶視窗了
    let mut words = vec!["the", "cat", "sat", "borrow"];
    let rare: Vec<String> = (0..12).map(|i| format!("rare{i:02}")).collect();
    words.extend(rare.iter().map(|s| s.as_str()));

    let (db, profile) = setup(&words).await;
    set_vocabulary(&db, profile, 3_000).await;
    for i in 0..12 {
        make_rare(&db, 5 + i, 3_500 + i).await;
    }
    study(&db, profile, 4).await;

    let passage = r#"{"title":"T","passage":"The cat sat. The cat sat. The cat sat.",
        "questions":[{"question":"Q","options":["A","B"],"option_notes":["note1","note2"],"answer_index":0}]}"#;
    let cloze = r#"{"title":"C","passage":"Please {{1}} it.",
        "items":[{"options":["borrow","lend"],"option_notes":["note1","note2"],"answer_index":0}]}"#;

    let llm = FakeLlm::new(&[passage, cloze, cloze, cloze, cloze, cloze, passage]);
    let engine = PracticeEngine::new(&db, &llm);

    let first = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap();

    // 中間穿插五題克漏字，剛好等於 NEW_WORD_MEMORY 的視窗
    for i in 1..=5 {
        engine
            .generate(
                profile,
                Some(ExerciseKind::Cloze),
                t0() + Duration::minutes(i),
            )
            .await
            .unwrap();
    }

    let second = engine
        .generate(
            profile,
            Some(ExerciseKind::Reading),
            t0() + Duration::minutes(10),
        )
        .await
        .unwrap();

    for word in &second.target_words {
        assert!(
            !first.target_words.contains(word),
            "{word} 上一篇才教過，克漏字把閱讀的歷史沖掉了"
        );
    }
}

/// 這條測試存在的理由是它真的發生過：模型回了
/// `numbers=[2, 5, 1, 3, 4, 7, 6, 8]`——八個編號一個沒少、一個沒重複，
/// 只是沒照文章順序寫。原本的檢查把它當成壞資料記了一筆 warning，
/// 而使用者看到的是第一個空格標著 2、第二個標著 5，右邊題目卻是 1、2、3…
///
/// 這個在本地改得掉，所以要改掉，不是丟掉整份題目。
#[tokio::test]
async fn blanks_written_out_of_order_are_renumbered_not_rejected() {
    let (db, profile) = setup(&["a1", "a2", "a3", "a4"]).await;
    set_vocabulary(&db, profile, 1_000).await;
    for lemma in 1..=4 {
        study(&db, profile, lemma).await;
    }

    // 空格照 2、4、1、3 的順序出現，選項用可辨識的字串標記原本的題號
    let llm = FakeLlm::new(&[r#"{"title":"T",
        "passage":"w {{2}} x {{4}} y {{1}} z {{3}}.",
        "items":[
          {"options":["one-right","one-wrong"],"option_notes":["note1","note2"],"answer_index":0},
          {"options":["two-right","two-wrong"],"option_notes":["note1","note2"],"answer_index":0},
          {"options":["three-right","three-wrong"],"option_notes":["note1","note2"],"answer_index":0},
          {"options":["four-right","four-wrong"],"option_notes":["note1","note2"],"answer_index":0}
        ]}"#]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Cloze), t0())
        .await
        .unwrap();

    let wordforge_practice::payload::ExerciseBody::Cloze { passage, items, .. } = &exercise.body
    else {
        panic!("該是克漏字");
    };

    // 文章裡的空格重新編成出現順序
    assert_eq!(
        wordforge_core::practice::blank_numbers(passage),
        vec![1, 2, 3, 4],
        "空格沒有照出現順序重新編號：{passage}"
    );
    assert_eq!(items.len(), 4, "一題都不該被丟掉");

    // 第 k 格對應的題目要是原本標著 k 的那一題
    let answered: Vec<&str> = items
        .iter()
        .map(|i| i.options[i.answer_index].as_str())
        .collect();
    assert_eq!(
        answered,
        vec!["two-right", "four-right", "one-right", "three-right"],
        "題目沒有跟著空格的順序重排"
    );
}

/// 每個選項各自的解說必須跟著它的選項一起被搬動。
///
/// 這條測試存在的理由是它壞掉的樣子看起來完全正常：洗牌只搬 options
/// 不搬 option_notes 的話，每個選項會配到別人的解說——畫面上依然是
/// 「你選了 X：某段說明」，只是那段說明講的是另一個選項。
#[tokio::test]
async fn each_option_keeps_its_own_note_through_the_shuffle() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    let llm = FakeLlm::new(&[r#"{"items":[
             {"prompt":"I ___ yesterday",
              "options":["went","go","gone","going"],
              "option_notes":["note-went","note-go","note-gone","note-going"],
              "answer_index":0,"grammar_point":"tense"}
           ]}"#]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    let wordforge_practice::payload::ExerciseBody::Choices { items } = &exercise.body else {
        panic!("該是選擇題");
    };
    let item = &items[0];

    assert_eq!(item.option_notes.len(), item.options.len());
    for (option, note) in item.options.iter().zip(&item.option_notes) {
        assert_eq!(
            note,
            &format!("note-{option}"),
            "「{option}」配到了別的選項的解說：{:?} / {:?}",
            item.options,
            item.option_notes
        );
    }

    // 洗過之後正確答案仍該指向 went
    assert_eq!(item.options[item.answer_index], "went");
}

/// 模型沒給 option_notes 時不能壞掉——舊的練習紀錄裡也沒有這個欄位。
#[tokio::test]
async fn missing_option_notes_are_tolerated() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    let llm =
        FakeLlm::new(&[r#"{"items":[{"prompt":"a","options":["x","y","z"],"answer_index":0}]}"#]);
    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    let wordforge_practice::payload::ExerciseBody::Choices { items } = &exercise.body else {
        panic!("該是選擇題");
    };
    assert!(items[0].option_notes.is_empty());
    assert_eq!(items[0].options.len(), 3, "沒有 notes 也要照常洗牌");
}

/// 模型寫的逐題講評要照它自己給的 index 對回題號。
///
/// 這條測試存在的理由是它壞掉的樣子看起來完全正常：照陣列位置貼的話，
/// 模型少回一題或換個順序，第三題的講評就會出現在第二題底下。
#[tokio::test]
async fn per_question_comments_follow_the_index_not_the_position() {
    let (db, profile) = setup(&["the", "cat", "sat"]).await;
    set_vocabulary(&db, profile, 2_000).await;

    let passage = r#"{"title":"T","passage":"The cat sat.",
        "questions":[
          {"question":"Q1","options":["a","b"],"option_notes":["note1","note2"],"answer_index":0},
          {"question":"Q2","options":["a","b"],"option_notes":["note1","note2"],"answer_index":0},
          {"question":"Q3","options":["a","b"],"option_notes":["note1","note2"],"answer_index":0}
        ]}"#;
    // 模型回的順序是 3、1，而且漏了第 2 題
    let graded = r#"{"score":0,"items":[
          {"index":3,"correct":false,"comment":"第三題的講評"},
          {"index":1,"correct":false,"comment":"第一題的講評"}
        ],"unknown_words":[]}"#;

    let llm = FakeLlm::new(&[passage, graded]);
    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Reading), t0())
        .await
        .unwrap();

    let feedback = engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec![],
                choices: answer(&exercise, &[Some(false); 3]),
                marked_unknown: vec![],
            },
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap();

    assert_eq!(feedback.items.len(), 3, "本地判分的三題都要在");
    assert_eq!(feedback.items[0].comment.as_deref(), Some("第一題的講評"));
    assert_eq!(
        feedback.items[1].comment.as_deref(),
        None,
        "模型沒講第二題，就不該把別題的講評貼上來"
    );
    assert_eq!(feedback.items[2].comment.as_deref(), Some("第三題的講評"));
}

/// 逐選項解說沒生齊時要補一次，而且**只收解說**。
///
/// 這條測試存在的理由是壞掉的方式很難看出來：重試回來的內容如果整份收下，
/// 模型很可能順手把題目重寫一遍——那樣答案就跟原本的題目對不上了，
/// 而使用者看不出來。只搬 option_notes，壞掉的重試最多是「還是沒有解說」。
#[tokio::test]
async fn missing_option_notes_are_filled_without_rewriting_the_questions() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    let llm = FakeLlm::new(&[
        // 第一次：兩題都沒有 option_notes
        r#"{"items":[
             {"prompt":"a","options":["a-right","a-wrong"],"answer_index":0},
             {"prompt":"b","options":["b-right","b-wrong"],"answer_index":0}
           ]}"#,
        // 補的時候順手把題目也重寫了——這些欄位必須被忽略
        r#"{"items":[
             {"prompt":"REWRITTEN","options":["x","y"],"answer_index":1,
              "option_notes":["a-right 的說明","a-wrong 的說明"]},
             {"prompt":"REWRITTEN","options":["x","y"],"answer_index":1,
              "option_notes":["b-right 的說明","b-wrong 的說明"]}
           ]}"#,
    ]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    assert_eq!(llm.call_count(), 2, "缺解說時該補一次");

    // 重試訊息要帶著上一次的結果：非交互式的後端不記得自己寫過什麼
    let retry = llm.last_prompt();
    assert!(retry.contains("a-right"), "沒有附上上一次的題目：{retry}");
    assert!(retry.contains("不要改動也不要重寫題目"), "{retry}");

    let wordforge_practice::payload::ExerciseBody::Choices { items } = &exercise.body else {
        panic!("該是選擇題");
    };

    for item in items {
        // 題目與選項一律沿用原本那份
        assert_ne!(item.question, "REWRITTEN", "重試把題目蓋掉了");
        assert!(
            item.options
                .iter()
                .all(|o| o.ends_with("-right") || o.ends_with("-wrong")),
            "選項被重試蓋掉了：{:?}",
            item.options
        );
        // 解說要收下，而且跟著它自己的選項
        assert_eq!(item.option_notes.len(), item.options.len());
        for (option, note) in item.options.iter().zip(&item.option_notes) {
            assert_eq!(note, &format!("{option} 的說明"), "解說配到別的選項了");
        }
    }
}

/// 補回來的解說長度還是對不上時不能收——配錯的解說比沒有解說更糟，
/// 因為畫面上看起來完全合理。
#[tokio::test]
async fn a_still_mismatched_retry_is_rejected_rather_than_misaligned() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    let llm = FakeLlm::new(&[
        r#"{"items":[{"prompt":"a","options":["x","y","z"],"answer_index":0}]}"#,
        // 三個選項卻只回兩句
        r#"{"items":[{"option_notes":["只有兩句","第二句"]}]}"#,
    ]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    let wordforge_practice::payload::ExerciseBody::Choices { items } = &exercise.body else {
        panic!("該是選擇題");
    };
    assert!(
        items[0].option_notes.is_empty(),
        "長度對不上還收下來了：{:?}",
        items[0].option_notes
    );
    assert_eq!(items[0].options.len(), 3, "題目本身不該受影響");
}

/// 解說生齊了就不該多打一次模型。
#[tokio::test]
async fn complete_option_notes_cost_no_extra_call() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    let llm = FakeLlm::new(&[r#"{"items":[
         {"prompt":"a","options":["x","y"],"answer_index":0,
          "option_notes":["x 的說明","y 的說明"]}
       ]}"#]);

    let engine = PracticeEngine::new(&db, &llm);
    engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    assert_eq!(llm.call_count(), 1, "解說已經齊了還多打了一次");
}

/// 克漏字的解析也要能點字查意思，跟閱讀一樣。
#[tokio::test]
async fn a_cloze_analysis_can_look_words_up_too() {
    let (db, profile) = setup(&["borrow", "umbrella", "the"]).await;
    set_vocabulary(&db, profile, 1_000).await;
    for lemma in 1..=3 {
        study(&db, profile, lemma).await;
    }

    let llm = FakeLlm::new(&[r#"{"title":"T","passage":"I had to {{1}} the umbrella.",
        "items":[{"options":["borrow","lend"],"answer_index":0,
                  "option_notes":["跟別人借用","借出去，方向相反"]}]}"#]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Cloze), t0())
        .await
        .unwrap();

    let feedback = engine
        .grade(
            profile,
            &GradeInput {
                exercise_id: exercise.exercise_id,
                answers: vec![],
                choices: answer(&exercise, &[Some(true)]),
                marked_unknown: vec![],
            },
            t0() + Duration::minutes(1),
        )
        .await
        .unwrap();

    assert_eq!(llm.call_count(), 1, "解析是本地查的，不該再打模型");
    assert!(
        feedback.glossary.iter().any(|g| g.text == "umbrella"),
        "文章裡的字要查得到釋義：{:?}",
        feedback.glossary
    );
    assert!(
        !feedback.glossary.iter().any(|g| g.text == "the"),
        "虛詞不該出現在解析裡"
    );
}

/// 格式壞掉時要把模型自己的輸出串回輸入，指出哪裡錯，再問一次。
///
/// 這條測試存在的理由是原本沒有這一層：`filter_map(...ok())` 把解析不了的
/// 題目**默默丟掉**，使用者拿到三題而不是四題，畫面上沒有任何異狀。
#[tokio::test]
async fn a_malformed_response_is_sent_back_with_the_problems() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    let llm = FakeLlm::new(&[
        // 第一題的 answer_index 超出範圍，第二題根本缺 options
        r#"{"items":[
             {"prompt":"a","options":["x","y"],"answer_index":9,
              "option_notes":["n1","n2"]},
             {"prompt":"b"}
           ]}"#,
        // 修好之後的版本
        r#"{"items":[
             {"prompt":"a","options":["x","y"],"answer_index":0,
              "option_notes":["n1","n2"]},
             {"prompt":"b","options":["p","q"],"answer_index":1,
              "option_notes":["n3","n4"]}
           ]}"#,
    ]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    assert_eq!(llm.call_count(), 2, "格式壞掉就該回問一次");

    // 重試訊息要帶著它自己的輸出——非交互式的後端不記得寫過什麼
    let retry = llm.last_prompt();
    assert!(
        retry.contains(r#""answer_index":9"#),
        "沒有把上一次的輸出串回去：{retry}"
    );
    assert!(
        retry.contains("/items/0/answer_index"),
        "沒有用 JSON Pointer 指出哪裡錯：{retry}"
    );
    assert!(retry.contains("/items/1"), "第二題的問題也要講：{retry}");

    let wordforge_practice::payload::ExerciseBody::Choices { items } = &exercise.body else {
        panic!("該是選擇題");
    };
    assert_eq!(items.len(), 2, "修好之後兩題都要在");
    for item in items {
        assert!(item.answer_index < item.options.len());
    }
}

/// 修不好的時候不能整份不給——三題的練習仍然做得完，
/// 整份不給只是把「少一題」換成「什麼都沒有」。
#[tokio::test]
async fn an_unfixable_response_still_yields_the_usable_questions() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    // 兩次都是一好一壞
    const BROKEN: &str = r#"{"items":[
         {"prompt":"a","options":["x","y"],"answer_index":0,"option_notes":["n1","n2"]},
         {"prompt":"b"}
       ]}"#;
    let llm = FakeLlm::new(&[BROKEN, BROKEN]);

    let engine = PracticeEngine::new(&db, &llm);
    let exercise = engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    assert_eq!(llm.call_count(), 2, "只重問一次，不無限重試");

    let wordforge_practice::payload::ExerciseBody::Choices { items } = &exercise.body else {
        panic!("該是選擇題");
    };
    assert_eq!(items.len(), 1, "壞掉的那題丟掉，好的那題照樣給");
}

/// 一題都不能用時才真的失敗——那時候沒有東西可以交出去。
#[tokio::test]
async fn a_completely_unusable_response_is_an_error() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    let llm = FakeLlm::new(&[r#"{"items":[]}"#, r#"{"items":[]}"#]);
    let engine = PracticeEngine::new(&db, &llm);

    let err = engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("一題都沒產出來"),
        "錯誤訊息要說得出發生什麼事：{err}"
    );
}

/// 針對性練習：使用者指定要練哪個文法點時，出題就只練那一個。
///
/// 「隨機出目前會的」與「針對性練習」的差別全在這裡——前者讓 FSRS
/// 排程決定要練什麼，後者由使用者自己挑。
#[tokio::test]
async fn a_chosen_grammar_point_is_the_one_that_gets_drilled() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    const ITEMS: &str = r#"{"items":[{"prompt":"a","options":["x","y"],
        "option_notes":["n1","n2"],"answer_index":0,"grammar_point":"conditionals"}]}"#;
    let llm = FakeLlm::new(&[ITEMS, ITEMS]);

    // 指定：只練條件句
    let focused = PracticeEngine::new(&db, &llm).with_grammar_focus(Some("conditionals".into()));
    focused
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();
    let prompt = llm.last_prompt();
    assert!(
        prompt.contains("conditionals"),
        "指定的文法點沒有進 prompt：{prompt}"
    );

    // 不指定：沒有弱點紀錄時退回基礎綜合練習
    let random = PracticeEngine::new(&db, &llm);
    random
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();
    assert!(
        llm.last_prompt().contains("基礎綜合練習"),
        "沒指定又沒有弱點時該退回綜合練習"
    );
}

/// 文法點的受控清單來自資料庫，不是寫死的常數。
///
/// 這條測試存在的理由是它曾經是寫死的：學日文的人拿到空清單，
/// 而且想加一個自己常錯的點沒有地方加。
#[tokio::test]
async fn the_controlled_list_comes_from_the_database() {
    let (db, profile) = setup(&["go"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    // 使用者自己加一個內建種子沒有的文法點
    wordforge_db::grammar::upsert_def(
        &db,
        &wordforge_db::grammar::GrammarDef {
            id: 0,
            lang: "en".into(),
            point: "tag-questions".into(),
            name: "附加問句".into(),
            explanation: None,
            examples: Vec::new(),
            level: None,
            sort_order: 99,
            origin: "manual".into(),
        },
        t0(),
    )
    .await
    .unwrap();

    let llm = FakeLlm::new(&[r#"{"items":[{"prompt":"a","options":["x","y"],
        "option_notes":["n1","n2"],"answer_index":0}]}"#]);
    let engine = PracticeEngine::new(&db, &llm);
    engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    let prompt = llm.last_prompt();
    assert!(
        prompt.contains("tag-questions"),
        "使用者自己加的文法點沒有進 prompt 的受控清單：{prompt}"
    );
    assert!(prompt.contains("tense"), "種子的項目也要在");
}

/// 沒有種子的語言開箱是空的，prompt 退回「請自己保持一致」。
/// 硬套英文的 articles、gerund-infinitive 去標日文只會產生垃圾資料。
#[tokio::test]
async fn a_language_without_grammar_definitions_falls_back_gracefully() {
    let (db, profile) = setup_lang("ja", &["行く"]).await;
    set_vocabulary(&db, profile, 1_000).await;

    let llm = FakeLlm::new(&[r#"{"items":[{"prompt":"a","options":["x","y"],
        "option_notes":["n1","n2"],"answer_index":0}]}"#]);
    // 用 for_profile 讓語言從 profile 流下來，跟實際使用一致
    let engine = PracticeEngine::for_profile(&db, &llm, profile)
        .await
        .unwrap();
    engine
        .generate(profile, Some(ExerciseKind::Grammar), t0())
        .await
        .unwrap();

    let prompt = llm.last_prompt();
    assert!(
        prompt.contains("請用一致的英文術語命名"),
        "沒有清單時該退回「請自己保持一致」：{prompt}"
    );
    assert!(
        !prompt.contains("gerund-infinitive"),
        "英文的分類外洩到日文了：{prompt}"
    );
}
