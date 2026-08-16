//! 查字典，以及從字典把字加進牌組。

use serde::Serialize;
use time::OffsetDateTime;
use wordforge_core::model::{CardKind, LemmaId, ProfileId};
use wordforge_db::dict::{DictStats, SearchHit, WordDetail};
use wordforge_db::repo::cards;

use crate::commands::placement::start_rank;
use crate::{AppState, CmdResult, CommandError};

#[tauri::command]
pub async fn search_words(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lang: String,
    query: String,
    limit: i64,
) -> CmdResult<Vec<SearchHit>> {
    Ok(wordforge_db::dict::search(&state.db, &lang, &query, profile_id, limit).await?)
}

#[tauri::command]
pub async fn word_detail(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lemma_id: i64,
) -> CmdResult<Option<WordDetail>> {
    Ok(wordforge_db::dict::detail(&state.db, lemma_id, profile_id).await?)
}

#[tauri::command]
pub async fn dictionary_stats(state: tauri::State<'_, AppState>) -> CmdResult<DictStats> {
    Ok(wordforge_db::dict::stats(&state.db).await?)
}

/// 把查到的字加進牌組。`kinds` 空著就只建立辨識卡。
#[tauri::command]
pub async fn add_lemma_to_deck(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lemma_id: i64,
    kinds: Vec<String>,
) -> CmdResult<()> {
    let now = OffsetDateTime::now_utc();
    let kinds: Vec<CardKind> = if kinds.is_empty() {
        vec![CardKind::Recognition]
    } else {
        kinds
            .iter()
            .map(|k| match k.as_str() {
                "recognition" => Ok(CardKind::Recognition),
                "recall" => Ok(CardKind::Recall),
                "listening" => Ok(CardKind::Listening),
                "spelling" => Ok(CardKind::Spelling),
                other => Err(CommandError::new(format!("未知的卡片類型：{other}"))),
            })
            .collect::<CmdResult<_>>()?
    };

    for kind in kinds {
        cards::ensure(
            &state.db,
            ProfileId(profile_id),
            LemmaId(lemma_id),
            kind,
            now,
        )
        .await?;
    }
    Ok(())
}

/// 一個標籤的字數與牌組進度。
#[derive(Debug, Serialize)]
pub struct TagSummary {
    pub tag: String,
    pub total: i64,
    pub in_deck: i64,
}

#[tauri::command]
pub async fn deck_tags(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lang: String,
) -> CmdResult<Vec<TagSummary>> {
    // 扣掉分級測驗判定「已經會了」的字，顯示的數字才是真正能加的
    let min_rank = start_rank(&state.db, profile_id).await?;
    let rows = cards::tag_summary(&state.db, ProfileId(profile_id), &lang, min_rank).await?;
    Ok(rows
        .into_iter()
        .map(|(tag, total, in_deck)| TagSummary {
            tag,
            total,
            in_deck,
        })
        .collect())
}

/// 依考試範圍批次加入單字，例如把國中會考範圍的字全部排進複習。
#[tauri::command]
pub async fn add_words_by_tag(
    state: tauri::State<'_, AppState>,
    profile_id: i64,
    lang: String,
    tag: String,
    limit: i64,
) -> CmdResult<u64> {
    Ok(cards::add_by_tag(
        &state.db,
        ProfileId(profile_id),
        cards::AddByTag {
            lang: &lang,
            tag: &tag,
            kinds: &[CardKind::Recognition],
            limit,
            // 功能詞不做成卡片，理由見 wordforge_core::wordlist
            skip_function_words: true,
            // 分級測驗說已經會的字就不要再排進來
            min_freq_rank: start_rank(&state.db, profile_id).await?,
            skip_existing: false,
        },
        OffsetDateTime::now_utc(),
    )
    .await?)
}

/// 匯入了哪些語言的字典。設定頁的目標語言選單用它，
/// 使用者才不會選到一個沒有字典的語言。
#[tauri::command]
pub async fn dictionary_languages(
    state: tauri::State<'_, AppState>,
) -> CmdResult<Vec<(String, i64)>> {
    Ok(wordforge_db::dict::languages(&state.db).await?)
}
