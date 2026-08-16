//! 情境主題：出題時輪換的場景清單，使用者可以自己加。

use time::OffsetDateTime;
use wordforge_core::model::ProfileId;
use wordforge_db::repo::profiles;

use crate::{AppState, CmdResult};

/// 這個語言的全部情境主題，含停用的。
///
/// 出題時只會用到啟用而且題型對得上的那些，但設定頁要全部看得到——
/// 停用的看不見的話，使用者會以為它被刪掉了。
#[tauri::command]
pub async fn list_topics(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
) -> CmdResult<Vec<wordforge_db::topics::Topic>> {
    let (_, target) = profiles::languages(&state.db, ProfileId(profile_id)).await?;
    // 第一次開這一頁時把種子寫進去，讓開箱就有題材可用
    wordforge_db::topics::seed(&state.db, &target, OffsetDateTime::now_utc()).await?;
    Ok(wordforge_db::topics::list(&state.db, &target).await?)
}

/// 新增或更新一個主題。
///
/// 有 `id` 就是編輯（會改到文字本身），沒有就是新增。分開處理是因為
/// 文字是唯一鍵：直接 upsert 一個新文字只會多長一筆，舊的還留著，
/// 使用者按編輯存檔後會看到兩個主題。
#[tauri::command]
pub async fn save_topic(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    topic: wordforge_db::topics::Topic,
) -> CmdResult<i64> {
    let (_, target) = profiles::languages(&state.db, ProfileId(profile_id)).await?;
    let now = OffsetDateTime::now_utc();

    if topic.id > 0 {
        wordforge_db::topics::rename(&state.db, topic.id, &topic.text, now).await?;
    }
    // 語言一律由 profile 決定，不讓前端指定——傳錯的話那筆主題會消失在
    // 另一個語言底下，而畫面上只會顯示「存好了」
    let topic = wordforge_db::topics::Topic {
        lang: target,
        ..topic
    };
    Ok(wordforge_db::topics::upsert(&state.db, &topic, now).await?)
}

#[tauri::command]
pub async fn delete_topic(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    id: i64,
) -> CmdResult<bool> {
    let (_, target) = profiles::languages(&state.db, ProfileId(profile_id)).await?;
    Ok(wordforge_db::topics::delete(&state.db, &target, id).await?)
}
