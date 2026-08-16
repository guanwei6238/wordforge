//! AI 練習：出題、批改、練習紀錄。

use serde::Serialize;
use tauri::AppHandle;
use time::OffsetDateTime;
use wordforge_core::model::ProfileId;
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
            .map(|r| ExerciseSummary {
                exercise_id: r.id,
                kind: r.kind,
                created_at: r.created_at,
                coverage: r.coverage,
                score: r
                    .feedback_json
                    .as_deref()
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
                    .and_then(|v| v.get("score").and_then(|s| s.as_f64())),
                title: summarize(&r.payload_json),
            })
            .collect(),
        total: wordforge_db::exercises::count(&state.db, pid).await?,
    })
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
