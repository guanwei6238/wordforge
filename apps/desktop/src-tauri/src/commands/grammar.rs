//! 文法點：定義（可匯入、可編輯）加上「你學到哪」。

use serde::Serialize;
use tauri::AppHandle;
use time::OffsetDateTime;
use wordforge_core::model::ProfileId;
use wordforge_db::repo::profiles;
use wordforge_practice::PracticeEngine;

use crate::commands::cards::scheduler_for;
use crate::commands::llm::settings_dir;
use crate::llm_settings::LlmSettings;
use crate::{AppState, CmdResult, CommandError};

/// 一個文法點：定義加上「你學到哪」。
///
/// 定義來自 `grammar_def`（可匯入、可編輯），掌握狀態來自 `grammar_point`
/// （FSRS 排程）。兩者在這裡才合起來——資料層刻意分開，
/// 因為刪掉一份教材不該抹掉學習歷史。
#[derive(Debug, Serialize)]
pub struct GrammarView {
    pub point: String,
    pub name: String,
    pub explanation: Option<String>,
    pub examples: Vec<wordforge_db::grammar::GrammarExample>,
    pub level: Option<String>,
    pub origin: String,
    /// 還沒開始學就是 `None`
    pub state: Option<String>,
    pub due: Option<String>,
    pub error_count: i64,
    pub correct_count: i64,
    /// 記憶穩定度（天）。撐得過三週不複習就算「會了」。
    pub stability: Option<f64>,
}

/// 這個語言的全部文法點，附上掌握狀態。
#[tauri::command]
pub async fn list_grammar(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<Vec<GrammarView>> {
    let now = OffsetDateTime::now_utc();
    let (_, target) = profiles::languages(&state.db, ProfileId(profile_id)).await?;

    // 第一次開這一頁時把種子寫進去，讓英文開箱有東西可學。
    // 沒有種子的語言仍然是空的——硬套英文的分類只會產生垃圾資料。
    wordforge_db::grammar::seed_defs(&state.db, &target, now).await?;

    let defs = wordforge_db::grammar::list_defs(&state.db, &target).await?;
    let learned = wordforge_db::grammar::all_points(&state.db, ProfileId(profile_id)).await?;

    Ok(defs
        .into_iter()
        .map(|d| {
            let status = learned.iter().find(|p| p.point == d.point);
            GrammarView {
                point: d.point,
                name: d.name,
                explanation: d.explanation,
                examples: d.examples,
                level: d.level,
                origin: d.origin,
                state: status.map(|p| p.state.clone()),
                due: status.map(|p| p.due.clone()),
                error_count: status.map(|p| p.error_count).unwrap_or(0),
                correct_count: status.map(|p| p.correct_count).unwrap_or(0),
                stability: status.and_then(|p| p.stability),
            }
        })
        .collect())
}

/// 新增或編輯一個文法點的定義。
#[tauri::command]
pub async fn save_grammar(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    def: wordforge_db::grammar::GrammarDef,
) -> CmdResult<()> {
    let (_, target) = profiles::languages(&state.db, ProfileId(profile_id)).await?;
    let def = wordforge_db::grammar::GrammarDef {
        // 語言一律由 profile 決定，不讓前端指定——傳錯的話那筆定義
        // 會消失在另一個語言底下，而畫面上只會顯示「存好了」
        lang: target,
        ..def
    };
    wordforge_db::grammar::upsert_def(&state.db, &def, OffsetDateTime::now_utc()).await?;
    Ok(())
}

/// 刪掉一個文法點的定義。**不動掌握狀態**。
#[tauri::command]
pub async fn delete_grammar(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    point: String,
) -> CmdResult<bool> {
    let (_, target) = profiles::languages(&state.db, ProfileId(profile_id)).await?;
    Ok(wordforge_db::grammar::delete_def(&state.db, &target, &point).await?)
}

/// 請模型講解一個文法點，結果存進資料庫。
#[tauri::command]
pub async fn explain_grammar(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    point: String,
) -> CmdResult<wordforge_db::grammar::GrammarDef> {
    let settings = LlmSettings::load(&settings_dir(&app)?);
    let llm = settings
        .build()?
        .ok_or_else(|| CommandError::new("還沒有設定 AI 後端，請先到設定頁選一個"))?;

    let engine = PracticeEngine::for_profile(&state.db, llm.as_ref(), profile_id).await?;
    Ok(engine
        .explain_grammar(profile_id, &point, OffsetDateTime::now_utc())
        .await?)
}

/// 把一個文法點標成「我會了」或「還要練」。
///
/// 走的是跟答題一樣的 FSRS 排程：`known` 為真等於答對一次（間隔拉遠），
/// 為假等於答錯一次（很快再出現）。這樣自評與實際作答會匯流到同一個
/// 排程狀態，不會變成兩套互相打架的進度。
#[tauri::command]
pub async fn set_grammar_known(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    point: String,
    known: bool,
) -> CmdResult<()> {
    let scheduler = scheduler_for(&state.db, profile_id).await?;
    wordforge_db::grammar::record(
        &state.db,
        ProfileId(profile_id),
        &point,
        known,
        &scheduler,
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(())
}

/// 匯入一份文法清單（JSON 陣列）。回傳寫進去幾筆。
///
/// 格式刻意簡單，因為沒有事實上的標準——查過的開源來源要嘛授權不明，
/// 要嘛是標註規範而不是教材。與其硬套某一家的格式，不如定一個好手寫、
/// 也好從別的格式轉過來的：
///
/// ```json
/// [{"point": "te-form", "name": "て形", "explanation": "…", "level": "N5",
///   "examples": [{"text": "食べて", "translation": "吃（て形）"}]}]
/// ```
///
/// 只有 `point` 與 `name` 是必要的。已存在的識別碼會更新，
/// 但**不會**把既有的講解洗掉——那是使用者生成或寫過的東西。
#[tauri::command]
pub async fn import_grammar(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    path: String,
) -> CmdResult<usize> {
    let (_, target) = profiles::languages(&state.db, ProfileId(profile_id)).await?;
    let text = std::fs::read_to_string(&path)?;

    let defs: Vec<wordforge_db::grammar::GrammarDef> = serde_json::from_str(&text)
        .map_err(|e| CommandError::new(format!("這個檔案讀不出來：{e}")))?;
    if defs.is_empty() {
        return Err(CommandError::new("檔案裡一筆定義都沒有"));
    }

    let now = OffsetDateTime::now_utc();
    let mut written = 0usize;
    for (i, mut def) in defs.into_iter().enumerate() {
        def.lang = target.clone();
        def.origin = "import".into();
        def.sort_order = i as i64;
        // 一筆壞掉不該讓整份匯入失敗——回報寫進去幾筆就好
        match wordforge_db::grammar::upsert_def(&state.db, &def, now).await {
            Ok(_) => written += 1,
            Err(e) => tracing::warn!(error = %e, point = def.point, "這筆文法定義跳過"),
        }
    }
    Ok(written)
}
