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
    /// 分級刻度上的位置。程式只認這個數字，`level` 只是顯示用的名稱。
    pub level_ordinal: Option<i64>,
    /// `point`（批改用的錯誤標籤）或 `pattern`（教學用的句型）
    pub kind: String,
    /// 怎麼在句子裡認出這個句型。空的表示**偵測不到**——
    /// 難度上限與「有沒有真的練到」對它都不生效，UI 要說得出這件事。
    pub detectors: Vec<wordforge_core::patterns::Detector>,
    pub origin: String,
    /// 還沒開始學就是 `None`
    pub state: Option<String>,
    pub due: Option<String>,
    pub error_count: i64,
    pub correct_count: i64,
    /// 記憶穩定度（天）。這是**證據**：排程算出來的。
    pub stability: Option<f64>,
    /// 使用者自己按過「我會了」。這是**主張**：他說的話。
    ///
    /// 跟 `stability` 分開露出來，因為兩者會不一致——標了「我會」
    /// 卻在練習裡錯三次是很正常的。不一致的時候由人決定要不要改，
    /// 不是由程式猜。
    pub known_at: Option<String>,
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
                level_ordinal: d.level_ordinal,
                kind: d.kind,
                detectors: d.detectors,
                origin: d.origin,
                state: status.map(|p| p.state.clone()),
                due: status.map(|p| p.due.clone()),
                error_count: status.map(|p| p.error_count).unwrap_or(0),
                correct_count: status.map(|p| p.correct_count).unwrap_or(0),
                stability: status.and_then(|p| p.stability),
                known_at: status.and_then(|p| p.known_at.clone()),
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

    // 手寫的偵測器要當場擋下來，**不是丟掉壞的那條就好**：
    // 使用者正看著那個對話框，可以馬上改；默默存一個抓不到東西的
    // regex，只會讓他以為難度上限開著，而它從來沒有作用過。
    //
    // 匯入走的是另一條路（丟掉壞的、其餘照收）——一份 80 條的檔案
    // 因為一條打錯字而整份失敗，那個取捨是相反的。
    let (_, rejected) = wordforge_practice::vet_detectors(&def.point, &def.detectors);
    if !rejected.is_empty() {
        return Err(CommandError::new(format!(
            "偵測規則沒通過自我檢查，這樣存進去它不會有任何作用：\n{}",
            rejected.join("\n")
        )));
    }

    wordforge_db::grammar::upsert_def(&state.db, &def, OffsetDateTime::now_utc()).await?;
    Ok(())
}

/// 一條偵測規則的試打結果。
#[derive(Debug, Serialize)]
pub struct DetectorCheck {
    /// 這條規則本身過不過（編譯 ＋ 控制組例句）
    pub ok: bool,
    /// 沒過的話，哪裡沒過
    pub problem: Option<String>,
    /// 試打的那一句有沒有被判定成這個句型。沒給句子就是 `None`。
    pub matched: Option<bool>,
    /// 命中的是哪一段。看得到抓到什麼，才知道規則是不是抓太寬。
    pub matched_text: Option<String>,
}

/// 試跑一條偵測規則。
///
/// ## 為什麼一定要繞到後端
///
/// 瀏覽器的 `RegExp` 跟 Rust 的 `regex` crate **不是同一種方言**：
/// JS 支援 lookahead `(?=)` 與反向參照，Rust 這邊完全不支援。
/// 在前端試過就存進去的話，會拿到一條「在編輯器裡看起來好好的、
/// 存進資料庫之後編譯失敗」的規則——而它失敗的樣子是安靜的：
/// 那個句型從此偵測不到，難度上限對它不生效。
///
/// 走的是跟存檔、匯入完全同一個函式（`compile_detector`），
/// 所以這裡說會過，存檔就一定會過。
#[tauri::command]
pub async fn check_detector(
    detector: wordforge_core::patterns::Detector,
    sentence: String,
) -> CmdResult<DetectorCheck> {
    let re = match wordforge_core::patterns::compile_detector(&detector) {
        Ok(re) => re,
        Err(problem) => {
            return Ok(DetectorCheck {
                ok: false,
                problem: Some(problem),
                matched: None,
                matched_text: None,
            });
        }
    };

    let trimmed = sentence.trim();
    let hit = (!trimmed.is_empty()).then(|| re.find(trimmed));

    Ok(DetectorCheck {
        ok: true,
        problem: None,
        matched: hit.map(|m| m.is_some()),
        matched_text: hit.flatten().map(|m| m.as_str().to_string()),
    })
}

/// 這個語言的句型分級有哪幾格。設定頁的「我現在的程度」選單就是這個。
///
/// 選項來自資料，程式不預設任何分級體系——跟「設定頁的語言選單來自
/// 字典裡有什麼」是同一件事。
#[tauri::command]
pub async fn grammar_levels(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<Vec<wordforge_db::grammar::LevelOption>> {
    let (_, target) = profiles::languages(&state.db, ProfileId(profile_id)).await?;
    Ok(wordforge_db::grammar::level_options(&state.db, &target).await?)
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
/// 做兩件事：照樣送一次 FSRS 評分（會了＝答對、還要練＝答錯），
/// 並且記下這是**使用者自己說的**。
///
/// 兩件都要，因為它們是兩件不同的真話。只送評分的話，按一次
/// 「我會了」只把 stability 推到 3.17 天，而「已學會」的門檻是 21 天
/// ——按鈕看起來沒有反應，出題也不會知道他會這個句型。
#[tauri::command]
pub async fn set_grammar_known(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    point: String,
    known: bool,
) -> CmdResult<()> {
    let scheduler = scheduler_for(&state.db, profile_id).await?;
    wordforge_db::grammar::set_known(
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
) -> CmdResult<wordforge_practice::PatternReport> {
    let (_, target) = profiles::languages(&state.db, ProfileId(profile_id)).await?;
    let text = std::fs::read_to_string(&path)?;

    let defs: Vec<wordforge_db::grammar::GrammarDef> = serde_json::from_str(&text)
        .map_err(|e| CommandError::new(format!("這個檔案讀不出來：{e}")))?;
    if defs.is_empty() {
        return Err(CommandError::new("檔案裡一筆定義都沒有"));
    }

    let now = OffsetDateTime::now_utc();
    let mut report = wordforge_practice::PatternReport::default();
    for (i, mut def) in defs.into_iter().enumerate() {
        def.lang = target.clone();
        def.origin = "import".into();
        def.sort_order = i as i64;

        // **每一條偵測規則都當場實跑一次控制組。**
        //
        // 這一步是整個難度上限能不能成立的地方：一條打錯字的 regex
        // 什麼都比對不到，於是超綱檢查永遠通過，而畫面上完全正常——
        // 跟拿 `strings`（預設只掃 ASCII）去找中文字串一樣，
        // 方法自己壞了，結論卻很有信心。
        //
        // 壞的那條丟掉、其餘照收：一份 80 條的檔案不該因為一條打錯字
        // 而整份失敗。但**一定要說出丟了哪些**，不然使用者會以為
        // 難度上限開著，而它對那幾個句型從來沒有作用過。
        let (kept, rejected) = wordforge_practice::vet_detectors(&def.point, &def.detectors);
        def.detectors = kept;
        report.rejected.extend(rejected);
        if !def.detectors.is_empty() {
            report.detectable += 1;
        }

        // 一筆壞掉不該讓整份匯入失敗——回報寫進去幾筆就好
        match wordforge_db::grammar::upsert_def(&state.db, &def, now).await {
            Ok(_) => report.written += 1,
            Err(e) => tracing::warn!(error = %e, point = def.point, "這筆文法定義跳過"),
        }
    }
    Ok(report)
}
