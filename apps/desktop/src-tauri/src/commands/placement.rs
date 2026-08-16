//! 分級測驗：問幾個字估出詞彙量，然後把已經會的字收起來。

use serde::Serialize;
use wordforge_core::model::ProfileId;
use wordforge_core::placement::{self, PlacementAnswer, PlacementResult};
use wordforge_db::Db;
use wordforge_db::dict::PlacementItem;
use wordforge_db::repo::cards;

use crate::{AppState, CmdResult};

/// 每個詞頻層抽幾題。七層共 35 題，大約三分鐘。
pub const PLACEMENT_ITEMS_PER_BAND: i64 = 5;

#[tauri::command]
pub async fn placement_items(
    state: tauri::State<'_, AppState>,
    lang: String,
) -> CmdResult<Vec<PlacementItem>> {
    Ok(wordforge_db::dict::sample_for_placement(
        &state.db,
        &lang,
        &placement::default_bands(),
        PLACEMENT_ITEMS_PER_BAND,
    )
    .await?)
}

/// 收下測驗結果：估計詞彙量、記住起始詞頻，並把牌組裡太簡單的新卡收起來。
#[derive(Debug, Serialize)]
pub struct PlacementOutcome {
    #[serde(flatten)]
    pub result: PlacementResult,
    /// 被收起來的「早就會了」的卡片數
    pub suspended_cards: u64,
}

#[tauri::command]
pub async fn submit_placement(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lang: String,
    answers: Vec<PlacementAnswer>,
) -> CmdResult<PlacementOutcome> {
    let result = placement::estimate(&placement::default_bands(), &answers);

    // 起始詞頻存進 profile，之後加入新字都會從這裡開始
    sqlx::query(
        "UPDATE profile
         SET settings_json = json_set(
                 CASE WHEN json_valid(settings_json) THEN settings_json ELSE '{}' END,
                 '$.start_rank', ?,
                 '$.estimated_vocabulary', ?)
         WHERE id = ?",
    )
    .bind(result.start_rank)
    .bind(result.estimated_vocabulary)
    .bind(profile_id)
    .execute(state.db.pool())
    .await?;

    let suspended =
        cards::suspend_easy_new_cards(&state.db, ProfileId(profile_id), &lang, result.start_rank)
            .await?;

    Ok(PlacementOutcome {
        result,
        suspended_cards: suspended,
    })
}

/// 讀出 profile 設定裡的起始詞頻；沒做過測驗就是 0（從頭開始）。
pub async fn start_rank(db: &Db, profile_id: i64) -> CmdResult<i64> {
    let rank: Option<i64> = sqlx::query_scalar(
        "SELECT CAST(json_extract(settings_json, '$.start_rank') AS INTEGER)
         FROM profile WHERE id = ? AND json_valid(settings_json)",
    )
    .bind(profile_id)
    .fetch_optional(db.pool())
    .await?
    .flatten();
    Ok(rank.unwrap_or(0))
}
