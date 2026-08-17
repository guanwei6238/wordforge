//! AI 練習：出題、批改、練習紀錄。

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use time::OffsetDateTime;
use wordforge_core::model::ProfileId;
use wordforge_db::repo::lemmas;
use wordforge_practice::{ExerciseView, Feedback, GradeInput, PracticeEngine};

use crate::commands::llm::settings_dir;
use crate::llm_settings::LlmSettings;
use crate::{AppState, CmdResult, CommandError};

/// 目前能不能出題，以及會出什麼樣的題。
#[derive(Debug, Serialize)]
pub struct PracticeStatus {
    pub llm_ready: bool,
    pub vocabulary: i64,
    pub weak_grammar: Vec<String>,
    /// 依現在的程度，系統會出哪種題
    pub recommended: String,
    /// 每種題型需要的最低詞彙量，讓 UI 說得出「再多學 N 個字就能做閱讀測驗」
    pub requirements: Vec<(String, i64)>,
}

#[tauri::command]
pub async fn practice_status(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<PracticeStatus> {
    use wordforge_core::practice::ExerciseKind::*;

    let settings = LlmSettings::load(&settings_dir(&app)?);
    let llm_ready = settings.build().map(|p| p.is_some()).unwrap_or(false);

    // 沒有 LLM 也要能顯示程度，讓使用者知道設定完會拿到什麼
    let dummy = wordforge_llm::CliLlm::new(wordforge_llm::CliConfig::claude_code())
        .map_err(|e| CommandError::new(e.to_string()))?;
    let engine = PracticeEngine::for_profile(&state.db, &dummy, profile_id).await?;
    let learner = engine
        .learner_profile(profile_id, OffsetDateTime::now_utc())
        .await?;

    Ok(PracticeStatus {
        llm_ready,
        vocabulary: learner.vocabulary,
        weak_grammar: learner.weak_grammar.clone(),
        recommended: wordforge_core::practice::recommend_kind(&learner)
            .as_str()
            .to_string(),
        requirements: [
            TranslationToNative,
            TranslationToTarget,
            Cloze,
            Grammar,
            Reading,
        ]
        .into_iter()
        .map(|k| (k.as_str().to_string(), k.min_vocabulary()))
        .collect(),
    })
}

#[tauri::command]
pub async fn generate_exercise(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    kind: Option<String>,
    // 指定教材時，模型只能從那本書取材
    material_id: Option<i64>,
    // 文法題只練這一個點。None 就用今天到期的弱點（「隨機出目前會的」）
    grammar_point: Option<String>,
) -> CmdResult<ExerciseView> {
    let settings = LlmSettings::load(&settings_dir(&app)?);
    let llm = settings
        .build()?
        .ok_or_else(|| CommandError::new("還沒有設定 AI 後端，請先到設定頁選一個"))?;

    let kind = match kind.as_deref() {
        None | Some("") | Some("auto") => None,
        Some(k) => Some(parse_exercise_kind(k)?),
    };

    let engine = PracticeEngine::for_profile(&state.db, llm.as_ref(), profile_id)
        .await?
        .with_material(material_id)
        .with_grammar_focus(grammar_point);
    Ok(engine
        .generate(profile_id, kind, OffsetDateTime::now_utc())
        .await?)
}

#[tauri::command]
pub async fn grade_exercise(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    input: GradeInput,
) -> CmdResult<Feedback> {
    let settings = LlmSettings::load(&settings_dir(&app)?);
    let llm = settings
        .build()?
        .ok_or_else(|| CommandError::new("還沒有設定 AI 後端"))?;

    let engine = PracticeEngine::for_profile(&state.db, llm.as_ref(), profile_id).await?;
    Ok(engine
        .grade(profile_id, &input, OffsetDateTime::now_utc())
        .await?)
}

/// 練習紀錄的一列。
///
/// 不把整份 payload 送過來：清單上要顯示的只有「什麼時候做了什麼、幾分」，
/// 一次帶二十篇文章進 WebView 只是白費頻寬。要重做時再用
/// `load_exercise` 取完整內容。
#[derive(Debug, Serialize)]
pub struct ExerciseSummary {
    pub exercise_id: i64,
    pub kind: String,
    pub created_at: String,
    pub coverage: Option<f64>,
    /// 做過才有分數。`None` 代表出了題但沒作答（例如出到一半關掉）。
    pub score: Option<f64>,
    /// 做過幾次
    pub attempts: i64,
    /// 最後一次還有幾題沒全對。`None` 是沒作答過，或那次批改沒有逐題結果
    /// （模型偶爾只給總分）——那時候說不出「還有幾題」，就不要瞎猜。
    pub pending: Option<i64>,
    /// 閱讀題用文章標題，其他題型用第一題的開頭——清單上要認得出是哪一份
    pub title: String,
}

/// 從題目內容擠出一個看得懂的標題。
pub fn summarize(payload_json: &str) -> String {
    let Ok(body) = serde_json::from_str::<wordforge_practice::payload::ExerciseBody>(payload_json)
    else {
        return "（內容讀不出來）".into();
    };

    use wordforge_practice::payload::ExerciseBody::*;
    let raw = match &body {
        Reading { title, .. } | Cloze { title, .. } => title.clone(),
        Choices { items } => items
            .first()
            .map(|i| i.question.clone())
            .unwrap_or_default(),
        Translation { items, .. } => items.first().map(|i| i.source.clone()).unwrap_or_default(),
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "（沒有標題）".into();
    }
    // 太長的一行會把清單撐開。用字元數而不是位元組——中文一個字三個位元組，
    // 照位元組切會切在半個字中間。
    let cut: String = trimmed.chars().take(40).collect();
    if trimmed.chars().count() > 40 {
        format!("{cut}…")
    } else {
        cut
    }
}

/// 一頁的練習紀錄。
///
/// 帶著 `total` 一起回傳：少了它，UI 只知道「這頁有幾筆」，
/// 說不出「第 2 頁 / 共 7 頁」，也不知道最後一頁是不是到了。
#[derive(Debug, Serialize)]
pub struct ExercisePage {
    pub items: Vec<ExerciseSummary>,
    pub total: i64,
}

/// 做過的練習，新的在前。`offset` 用來翻頁。
#[tauri::command]
pub async fn list_exercises(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    limit: i64,
    offset: i64,
) -> CmdResult<ExercisePage> {
    let pid = ProfileId(profile_id);
    let records =
        wordforge_db::exercises::recent(&state.db, pid, limit.clamp(1, 200), offset.max(0)).await?;

    Ok(ExercisePage {
        items: records
            .into_iter()
            .map(|r| {
                let feedback: Option<Feedback> = r
                    .feedback_json
                    .as_deref()
                    .and_then(|j| serde_json::from_str(j).ok());
                ExerciseSummary {
                    exercise_id: r.id,
                    kind: r.kind,
                    created_at: r.created_at,
                    coverage: r.coverage,
                    score: feedback.as_ref().and_then(|f| f.score),
                    attempts: r.attempt_count,
                    pending: feedback.as_ref().and_then(|f| {
                        (!f.items.is_empty())
                            .then(|| f.items.iter().filter(|i| !i.correct).count() as i64)
                    }),
                    title: summarize(&r.payload_json),
                }
            })
            .collect(),
        total: wordforge_db::exercises::count(&state.db, pid).await?,
    })
}

/// 一頁的「你做過的句子」。
///
/// 帶 `total`：常練的字會累積到十幾句，只給「還有更多」的話
/// 使用者不知道翻不翻得完。
#[derive(Debug, Serialize)]
pub struct SentencePage {
    pub items: Vec<wordforge_db::word_sentences::WordSentence>,
    pub total: i64,
}

/// 這個字我在哪幾句話裡用過。
///
/// 資料在出題時就連好了（`engine::link_sentences`）。第一次呼叫會順便
/// 把既有練習補一次——這個功能對老使用者本來會是空的，而他做過的
/// 每一份練習裡都有句子。補寫靠 `app_meta` 的版號只跑一次。
#[tauri::command]
pub async fn word_sentences(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lemma_id: i64,
    limit: i64,
    offset: i64,
) -> CmdResult<SentencePage> {
    // 補寫不需要模型，但 engine 要一個 provider 才建得起來
    let dummy = wordforge_llm::CliLlm::new(wordforge_llm::CliConfig::claude_code())
        .map_err(|e| CommandError::new(e.to_string()))?;
    let engine = PracticeEngine::for_profile(&state.db, &dummy, profile_id).await?;
    match engine
        .backfill_sentences(profile_id, OffsetDateTime::now_utc())
        .await
    {
        Ok(n) if n > 0 => tracing::info!(exercises = n, "補寫了舊練習的句子連結"),
        Ok(_) => {}
        // 補寫失敗不該讓「看句子」整條路斷掉，已經連好的仍然讀得到
        Err(e) => tracing::warn!(error = %e, "補寫句子連結失敗"),
    }

    // 讀取要走跟寫入**同一條正規化**：句子是用 `base_form` 存的
    // （練 `ran` 存在 `run` 底下），而這裡拿到的是使用者正在看的那個詞條。
    // 字典裡 `ran` 自己也是四個獨立的詞條，直接拿它的 id 查會一句都沒有。
    let id = wordforge_core::model::LemmaId(lemma_id);
    let (_, lang) =
        wordforge_db::repo::profiles::languages(&state.db, ProfileId(profile_id)).await?;
    let mut family = match lemmas::text_of(&state.db, id).await? {
        Some(text) => lemmas::family(&state.db, &lang, &text).await?,
        None => Vec::new(),
    };
    if !family.contains(&id) {
        family.push(id);
    }

    Ok(SentencePage {
        items: wordforge_db::word_sentences::for_lemmas(
            &state.db,
            ProfileId(profile_id),
            &family,
            limit.clamp(1, 20),
            offset.max(0),
        )
        .await?,
        total: wordforge_db::word_sentences::count_for_lemmas(
            &state.db,
            ProfileId(profile_id),
            &family,
        )
        .await?,
    })
}

/// 今天要重練的一句翻譯。
///
/// **刻意不帶參考答案，也不帶你上次寫的東西**：這一句今天要的是
/// 「隔一天之後你自己想得出來嗎」。把上次的作答擺在旁邊，複習就退化成
/// 抄寫——而昨天那句話本來就是錯的。
#[derive(Debug, Serialize)]
pub struct DueSentenceView {
    pub exercise_id: i64,
    pub item_index: usize,
    /// 翻譯方向，決定 UI 該說「翻成英文」還是「翻成中文」
    pub kind: String,
    /// 要翻譯的句子
    pub source: String,
    /// 這一句刻意要練的字
    pub target_word: Option<String>,
    /// 錯過幾次。連錯三次的句子值得多看一眼。
    pub misses: i64,
}

/// 今天要練的句子，這一輪的份量加上總數。
///
/// 總數要跟著出來：畫面一次只出幾句，只回那幾句的話 UI 說不出
/// 「還有幾句」，使用者不知道自己在半路還是最後一輪。
#[derive(Debug, Serialize)]
pub struct DueSentencePage {
    pub items: Vec<DueSentenceView>,
    /// 今天總共還有幾句要練，不是這一輪有幾句
    pub total: i64,
}

/// 今天該重練的句子。
///
/// 答錯的句子明天回來，答對的從此不再出現——「練到 100 分」就是
/// 這條清單清空。
///
/// 第一次呼叫會順便把舊練習答錯的句子補進排程：排程是在批改的當下
/// 寫入的，所以這個功能上線之前做過的練習一句都沒排進去。
#[tauri::command]
pub async fn due_sentences(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    limit: i64,
) -> CmdResult<DueSentencePage> {
    // 補寫不需要模型，但 engine 要一個 provider 才建得起來
    if let Ok(dummy) = wordforge_llm::CliLlm::new(wordforge_llm::CliConfig::claude_code()) {
        match PracticeEngine::for_profile(&state.db, &dummy, profile_id).await {
            Ok(engine) => match engine
                .backfill_sentence_reviews(profile_id, OffsetDateTime::now_utc())
                .await
            {
                Ok(n) if n > 0 => tracing::info!(sentences = n, "補寫了舊練習的句子排程"),
                Ok(_) => {}
                // 補寫失敗不該讓「今天要練哪幾句」整條路斷掉
                Err(e) => tracing::warn!(error = %e, "補寫句子排程失敗"),
            },
            Err(e) => tracing::warn!(error = %e, "補寫句子排程失敗"),
        }
    }

    let now = OffsetDateTime::now_utc();
    let limit = limit.clamp(1, 100);
    let total = wordforge_db::sentences::due_count(&state.db, ProfileId(profile_id), now).await?;

    // 多撈幾句當緩衝。下面的迴圈會跳過兩種句子：練習內容讀不出來的，
    // 以及**跟這一輪方向不同的**。剛好撈 `limit` 句的話，最前面那幾句
    // 一被跳過就整頁空白，但總數還說有幾句要練，使用者看到的是一個
    // 清不掉的數字。
    const SKIPPABLE: i64 = 30;
    let due =
        wordforge_db::sentences::due(&state.db, ProfileId(profile_id), now, limit + SKIPPABLE)
            .await?;

    // 同一份練習可能有好幾句到期，題目只讀一次
    let mut bodies: std::collections::HashMap<
        i64,
        (String, wordforge_practice::payload::ExerciseBody),
    > = std::collections::HashMap::new();
    let mut out = Vec::new();
    // 這一輪的翻譯方向。中翻英與英翻中同時到期是常態，但批改的 prompt
    // 開頭就寫著這一份的方向，混在一起送等於告訴模型一件錯的事——
    // 分開送就變成兩次模型呼叫，而一輪只想打一次。
    // 另一個方向不會被漏掉：這一輪送完，下一輪自然輪到它。
    let mut direction: Option<String> = None;

    for item in due {
        if out.len() as i64 >= limit {
            break;
        }
        if let std::collections::hash_map::Entry::Vacant(slot) = bodies.entry(item.exercise_id) {
            let Some(record) = wordforge_db::exercises::get(
                &state.db,
                wordforge_db::exercises::ExerciseId(item.exercise_id),
            )
            .await?
            else {
                continue;
            };
            let Ok(body) = serde_json::from_str(&record.payload_json) else {
                continue;
            };
            slot.insert((record.kind, body));
        }
        let Some((kind, body)) = bodies.get(&item.exercise_id) else {
            continue;
        };
        let wordforge_practice::payload::ExerciseBody::Translation { items, .. } = body else {
            continue;
        };
        let Some(sentence) = items.get(item.item_index as usize) else {
            continue;
        };
        match &direction {
            Some(chosen) if chosen != kind => continue,
            Some(_) => {}
            None => direction = Some(kind.clone()),
        }

        out.push(DueSentenceView {
            exercise_id: item.exercise_id,
            item_index: item.item_index as usize,
            kind: kind.clone(),
            source: sentence.source.clone(),
            target_word: sentence.target_word.clone(),
            misses: item.misses,
        });
    }

    Ok(DueSentencePage { items: out, total })
}

/// 今天不寫這一句，明天再出現。回傳有沒有真的推遲到。
///
/// **不打模型**：沒有作答就沒有東西可以批改。這也是它跟「送出」
/// 最大的差別——跳過是即時的，不必等 CLI 冷啟動。
#[tauri::command]
pub async fn skip_sentence(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    exercise_id: i64,
    item_index: i64,
) -> CmdResult<bool> {
    Ok(wordforge_db::sentences::skip(
        &state.db,
        ProfileId(profile_id),
        exercise_id,
        item_index,
        OffsetDateTime::now_utc(),
    )
    .await?)
}

/// 送去批改的一句複習。
#[derive(Debug, Deserialize)]
pub struct DueAnswerInput {
    pub exercise_id: i64,
    pub item_index: usize,
    pub answer: String,
}

/// 一句複習的批改結果。
#[derive(Debug, Serialize)]
pub struct DueSentenceResultView {
    pub exercise_id: i64,
    pub item_index: usize,
    pub correct: bool,
    /// 口語說法。只在它跟正式說法不一樣時才有值。
    pub reference: Option<String>,
    pub reference_formal: Option<String>,
    pub comment: Option<String>,
    /// 逐處修正：你寫的哪一段、該改成什麼、為什麼。
    /// `comment` 只是一句摘要，這一份才說得出該怎麼改。
    pub corrections: Vec<wordforge_practice::payload::Correction>,
}

/// 批改這一輪的複習句子。**一次模型呼叫**，而且不寫進練習紀錄。
///
/// 跟 `regrade_items` 的分工：那個是「重寫某一份練習裡的某幾題」，
/// 會合併回那份練習、重算分數、往 `attempt` 寫一筆；這個是複習，
/// 紀錄寫進 `sentence_attempt`，練習紀錄完全不動。
#[tauri::command]
pub async fn grade_due_sentences(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    items: Vec<DueAnswerInput>,
) -> CmdResult<Vec<DueSentenceResultView>> {
    let settings = LlmSettings::load(&settings_dir(&app)?);
    let llm = settings
        .build()?
        .ok_or_else(|| CommandError::new("還沒有設定 AI 後端"))?;

    let answers: Vec<wordforge_practice::DueAnswer> = items
        .into_iter()
        .map(|i| wordforge_practice::DueAnswer {
            exercise_id: i.exercise_id,
            item_index: i.item_index,
            answer: i.answer,
        })
        .collect();

    let engine = PracticeEngine::for_profile(&state.db, llm.as_ref(), profile_id).await?;
    Ok(engine
        .grade_due_sentences(profile_id, &answers, OffsetDateTime::now_utc())
        .await?
        .into_iter()
        .map(|r| DueSentenceResultView {
            exercise_id: r.exercise_id,
            item_index: r.item_index,
            correct: r.correct,
            reference: r.reference,
            reference_formal: r.reference_formal,
            comment: r.comment,
            corrections: r.corrections,
        })
        .collect())
}

/// 複習紀錄的一列：那句題目、你寫了什麼、對不對。
#[derive(Debug, Serialize)]
pub struct SentenceAttemptView {
    pub id: i64,
    pub exercise_id: i64,
    pub item_index: usize,
    /// 題目那一句。存在練習的 payload 裡，這裡讀出來給 UI 用。
    pub source: String,
    pub kind: String,
    pub answer: String,
    pub correct: bool,
    pub reference: Option<String>,
    pub reference_formal: Option<String>,
    pub comment: Option<String>,
    /// 逐處修正。舊的紀錄（0020 之前）沒有這一欄，那時是空陣列。
    pub corrections: Vec<wordforge_practice::payload::Correction>,
    pub created_at: String,
}

/// 一頁複習紀錄。
#[derive(Debug, Serialize)]
pub struct SentenceAttemptPage {
    pub items: Vec<SentenceAttemptView>,
    pub total: i64,
}

/// 複習過的句子，新的在前。
///
/// 跟練習紀錄分開的兩張表、兩個查詢：一個是「這份題目做過幾次」，
/// 另一個是「今天複習了哪幾句」。
#[tauri::command]
pub async fn list_sentence_attempts(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    limit: i64,
    offset: i64,
) -> CmdResult<SentenceAttemptPage> {
    let pid = ProfileId(profile_id);
    let rows =
        wordforge_db::sentences::attempts(&state.db, pid, limit.clamp(1, 200), offset.max(0))
            .await?;

    // 題目本文在練習的 payload 裡。同一份練習只讀一次——一頁十筆
    // 常常來自同一兩份練習。
    let mut bodies: std::collections::HashMap<
        i64,
        Option<(String, wordforge_practice::payload::ExerciseBody)>,
    > = std::collections::HashMap::new();
    let mut items = Vec::with_capacity(rows.len());

    for row in rows {
        if let std::collections::hash_map::Entry::Vacant(slot) = bodies.entry(row.exercise_id) {
            let record = wordforge_db::exercises::get(
                &state.db,
                wordforge_db::exercises::ExerciseId(row.exercise_id),
            )
            .await?;
            slot.insert(record.and_then(|r| {
                serde_json::from_str(&r.payload_json)
                    .ok()
                    .map(|body| (r.kind, body))
            }));
        }

        // 題目讀不出來時仍然要列出這一筆：使用者寫過的東西不該因為
        // 那份練習壞掉就整列消失，只是題目那一欄說不出來。
        let (kind, source) = match bodies.get(&row.exercise_id).and_then(|b| b.as_ref()) {
            Some((kind, wordforge_practice::payload::ExerciseBody::Translation { items, .. })) => (
                kind.clone(),
                items
                    .get(row.item_index as usize)
                    .map(|i| i.source.clone())
                    .unwrap_or_default(),
            ),
            _ => (String::new(), String::new()),
        };

        items.push(SentenceAttemptView {
            id: row.id,
            exercise_id: row.exercise_id,
            item_index: row.item_index as usize,
            source,
            kind,
            answer: row.answer,
            correct: row.correct,
            reference: row.reference,
            reference_formal: row.reference_formal,
            comment: row.comment,
            // 解析不出來就當作沒有修正：一筆壞掉的紀錄不該讓整頁看不了
            corrections: serde_json::from_str(&row.corrections_json).unwrap_or_default(),
            created_at: row.created_at,
        });
    }

    Ok(SentenceAttemptPage {
        items,
        total: wordforge_db::sentences::attempt_count(&state.db, pid).await?,
    })
}

/// 重寫一題：第幾題（從 0 起算）、新的作答。
#[derive(Debug, Deserialize)]
pub struct RedoneItem {
    pub index: usize,
    pub answer: String,
}

/// 只重寫沒全對的那幾題，其餘沿用上一次的批改。
///
/// 分數會用「答對幾題 ÷ 總題數」重算——合併過的那一份模型沒看過全貌，
/// 它給的分數不會是整份的分數。
#[tauri::command]
pub async fn regrade_items(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    exercise_id: i64,
    items: Vec<RedoneItem>,
) -> CmdResult<Feedback> {
    let settings = LlmSettings::load(&settings_dir(&app)?);
    let llm = settings
        .build()?
        .ok_or_else(|| CommandError::new("還沒有設定 AI 後端"))?;

    let engine = PracticeEngine::for_profile(&state.db, llm.as_ref(), profile_id).await?;
    let redone: Vec<(usize, String)> = items.into_iter().map(|i| (i.index, i.answer)).collect();
    Ok(engine
        .regrade(profile_id, exercise_id, &redone, OffsetDateTime::now_utc())
        .await?)
}

/// 一次作答的完整內容：你當時寫了什麼、模型當時怎麼講。
///
/// 兩份 JSON 在這裡就解析好，不要丟原字串給前端再 parse 一次——
/// 那等於把「欄位長什麼樣」這件事在兩邊各寫一份，而這個專案已經
/// 為了前後端各一份模型清單踩過那個坑。
///
/// 解析失敗時給 `None` 而不是讓整個查詢失敗：一筆壞掉的紀錄不該
/// 讓「看不看得到過去的作答」整條路斷掉。
#[derive(Debug, Serialize)]
pub struct AttemptView {
    pub attempt_id: i64,
    pub created_at: String,
    pub score: Option<f64>,
    pub answer: Option<GradeInput>,
    pub feedback: Option<Feedback>,
}

/// 一份練習做過幾次，舊的在前。
#[tauri::command]
pub async fn list_attempts(
    state: tauri::State<'_, AppState>,
    exercise_id: i64,
) -> CmdResult<Vec<AttemptView>> {
    let rows = wordforge_db::exercises::attempts(
        &state.db,
        wordforge_db::exercises::ExerciseId(exercise_id),
    )
    .await?;

    Ok(rows
        .into_iter()
        .map(|a| AttemptView {
            attempt_id: a.id,
            created_at: a.created_at,
            score: a.score,
            answer: serde_json::from_str(&a.answer_json).ok(),
            feedback: serde_json::from_str(&a.feedback_json).ok(),
        })
        .collect())
}

/// 刪掉單獨一次作答，練習本身留著。
#[tauri::command]
pub async fn delete_attempt(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    attempt_id: i64,
) -> CmdResult<bool> {
    Ok(
        wordforge_db::exercises::delete_attempt(&state.db, ProfileId(profile_id), attempt_id)
            .await?,
    )
}

/// 刪掉一份練習紀錄，連同它的作答。回傳有沒有真的刪到。
#[tauri::command]
pub async fn delete_exercise(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    exercise_id: i64,
) -> CmdResult<bool> {
    Ok(wordforge_db::exercises::delete(
        &state.db,
        ProfileId(profile_id),
        wordforge_db::exercises::ExerciseId(exercise_id),
    )
    .await?)
}

/// 取回一份做過的練習，原封不動地再做一次。
///
/// 不重新出題：重做的價值就在於「同一份題目，這次答得比較好嗎」。
/// 送出之後照常走批改，`attempt` 會多一筆，舊的那筆留著。
#[tauri::command]
pub async fn load_exercise(
    state: tauri::State<'_, AppState>,
    exercise_id: i64,
) -> CmdResult<ExerciseView> {
    let record =
        wordforge_db::exercises::get(&state.db, wordforge_db::exercises::ExerciseId(exercise_id))
            .await?
            .ok_or_else(|| CommandError::new("找不到這份練習"))?;

    let body: wordforge_practice::payload::ExerciseBody =
        serde_json::from_str(&record.payload_json)
            .map_err(|e| CommandError::new(format!("這份練習的內容讀不出來：{e}")))?;

    Ok(ExerciseView {
        exercise_id: record.id,
        kind: parse_exercise_kind(&record.kind)?,
        body,
        target_words: record.target_words,
        coverage: record.coverage,
    })
}

pub fn parse_exercise_kind(s: &str) -> CmdResult<wordforge_core::practice::ExerciseKind> {
    use wordforge_core::practice::ExerciseKind::*;
    Ok(match s {
        "translation_to_target" => TranslationToTarget,
        "translation_to_native" => TranslationToNative,
        "cloze" => Cloze,
        "reading" => Reading,
        "grammar" => Grammar,
        other => return Err(CommandError::new(format!("未知的題型：{other}"))),
    })
}
