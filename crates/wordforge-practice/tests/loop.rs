//! 整條學習迴圈的測試：出題 → 作答 → 批改 → 不懂的字進牌組。
//!
//! LLM 用假的：真正要驗證的是編排邏輯（有沒有把 prompt 組對、
//! 回應解析得對不對、不懂的字有沒有真的變成卡片），
//! 而不是模型本身。用真模型反而讓測試不穩定又慢。

use std::sync::Mutex;

use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use wordforge_core::model::ProfileId;
use wordforge_core::practice::ExerciseKind;
use wordforge_db::Db;
use wordforge_db::dict::{EntryWrite, NewSense, NewSource};
use wordforge_db::repo::{cards, profiles};
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

/// 建一個有字典、有牌組的資料庫。
async fn setup(words: &[&str]) -> (Db, i64) {
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
    for (i, word) in words.iter().enumerate() {
        wordforge_db::dict::write_entry(
            &mut conn,
            source,
            &EntryWrite {
                lang: "en",
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
            "questions":[{"question":"Where?","options":["mat","tree"],"answer_index":0}],
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
             {"prompt":"I ___ yesterday","options":["go","went"],"answer_index":1,
              "grammar_point":"tense","explanation":"過去式"},
             {"prompt":"She ___ a book","options":["read","reads"],"answer_index":1,
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
                choices: vec![Some(1), Some(0)],
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
             {"prompt":"I ___ yesterday","options":["go","went"],"answer_index":1,
              "grammar_point":"tense","explanation":"過去式"},
             {"prompt":"She ___ a book","options":["read","reads"],"answer_index":1,
              "grammar_point":"subject-verb agreement"},
             {"prompt":"___ apple","options":["a","an"],"answer_index":1,
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
                choices: vec![Some(1), Some(0), None],
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
        )
        .await
        .unwrap();
    }
    drop(conn);
    set_vocabulary(&db, profile.0, 2_000).await;

    let llm = FakeLlm::new(&[r#"{"items":[{"prompt":"x","options":["a","b"],"answer_index":0}]}"#]);
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

    let llm = FakeLlm::new(&[r#"{"items":[{"prompt":"x","options":["a","b"],"answer_index":0}]}"#]);
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
