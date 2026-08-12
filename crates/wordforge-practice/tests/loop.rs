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
             {"prompt":"a","options":["x","y"],"answer_index":0,"grammar_point":"tense"},
             {"prompt":"b","options":["x","y"],"answer_index":0,"grammar_point":"Past Tense"},
             {"prompt":"c","options":["x","y"],"answer_index":0,"grammar_point":"verb_tense"}
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
                choices: vec![Some(1), Some(1), Some(1)],
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
             {"prompt":"a","options":["x","y"],"answer_index":0,"grammar_point":"這句怪怪的"},
             {"prompt":"b","options":["x","y"],"answer_index":0,"grammar_point":"articles"}
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
                choices: vec![Some(1), Some(1)],
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
            "questions":[{"question":"Q","options":["A","B"],"answer_index":0}]}"#;
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
}

/// 片語偵測不能只對有空格的語言成立。
#[tokio::test]
async fn phrases_are_detected_in_a_language_without_spaces() {
    let (db, profile) = setup_lang("ja", &["気にする", "勤勉"]).await;
    set_vocabulary(&db, profile, 2_000).await;

    let passage = r#"{"title":"T","passage":"勤勉な人は気にする。",
            "questions":[{"question":"Q","options":["A","B"],"answer_index":0}]}"#;
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
            "questions":[{"question":"Q","options":["A","B"],"answer_index":0}]}"#]);

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
            "questions":[{"question":"Q","options":["A","B"],"answer_index":0}]}"#;
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
        "questions":[{"question":"Q","options":["A","B"],"answer_index":0}]}"#;
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
        "questions":[{"question":"Q","options":["A","B"],"answer_index":0}]}"#;
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
            "questions":[{"question":"Q","options":["A","B"],"answer_index":0}]}"#,
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
        "questions":[{"question":"Q","options":["A","B"],"answer_index":0}]}"#;
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
