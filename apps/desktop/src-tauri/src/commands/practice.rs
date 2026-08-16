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

/// 今天該重練的句子。
///
/// 答錯的句子明天回來，答對的從此不再出現——「練到 100 分」就是
/// 這條清單清空。
#[tauri::command]
pub async fn due_sentences(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    limit: i64,
) -> CmdResult<Vec<DueSentenceView>> {
    let due = wordforge_db::sentences::due(
        &state.db,
        ProfileId(profile_id),
        OffsetDateTime::now_utc(),
        limit.clamp(1, 100),
    )
    .await?;

    // 同一份練習可能有好幾句到期，題目只讀一次
    let mut bodies: std::collections::HashMap<
        i64,
        (String, wordforge_practice::payload::ExerciseBody),
    > = std::collections::HashMap::new();
    let mut out = Vec::new();

    for item in due {
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

        out.push(DueSentenceView {
            exercise_id: item.exercise_id,
            item_index: item.item_index as usize,
            kind: kind.clone(),
            source: sentence.source.clone(),
            target_word: sentence.target_word.clone(),
            misses: item.misses,
        });
    }

    Ok(out)
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
