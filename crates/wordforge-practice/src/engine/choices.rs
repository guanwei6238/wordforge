//! 選擇題在本地做得到的事：解析、洗牌、判分。
//!
//! 這些全部不碰資料庫也不碰模型，所以是純函式——出題與批改兩邊都用得到，
//! 而且測試不必準備任何東西。
//!
//! 洗牌那條特別重要：模型幾乎總是把正確答案放在第一個，不洗的話
//! 使用者按 A 就有八成的分數，這份練習就沒有在測任何東西。

use super::*;

/// 克漏字答錯的那幾格，正確答案是哪個字。
///
/// 這些字會被排回複習：挖空的本來就是他該複習的字，填不出來就是
/// 「還沒真的會」——比排程算出來的到期時間更直接的證據。
pub(super) fn missed_words(items: &[ChoiceItem], input: &GradeInput) -> Vec<String> {
    items
        .iter()
        .enumerate()
        .filter(|(i, item)| input.choices.get(*i).copied().flatten() != Some(item.answer_index))
        .filter_map(|(_, item)| item.options.get(item.answer_index).cloned())
        .collect()
}

/// 把驗收過的題目解析出來。
///
/// **一定要先驗收再叫這個**：這裡仍然會跳過解析不了的項目，但驗收
/// 已經保證不會有那種東西了。原本沒有驗收那一層，於是壞掉的題目
/// 直接消失——使用者拿到三題而不是四題，畫面上沒有任何異狀。
pub(super) fn parse_choice_items(value: &serde_json::Value, field: &str) -> Vec<ChoiceItem> {
    value
        .get(field)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// 哪幾題的逐選項解說沒生齊（題號 1 起算）。
///
/// 長度對不上也算缺：短一截的話最後幾個選項會沒有解說，
/// 長一截則代表模型自己也搞混了哪句配哪個選項——兩種都不能收。
pub(super) fn missing_option_notes(items: &[ChoiceItem]) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.option_notes.len() != item.options.len()
                || item.option_notes.iter().any(|n| n.trim().is_empty())
        })
        .map(|(i, _)| i + 1)
        .collect()
}

/// 洗牌的亂源。
///
/// 用奈秒而不是秒：連續出兩題常常落在同一秒內，秒級的 seed 會讓
/// 兩份題目的選項被搬到一模一樣的位置。`wordforge-core` 不碰時鐘，
/// 所以亂源在這一層決定。
pub(super) fn shuffle_seed(now: OffsetDateTime) -> u64 {
    now.unix_timestamp_nanos() as u64
}

/// 把一份選擇題的選項洗過，答案落在哪裡就是哪裡。
///
/// 模型出的題目，答案會集中在某幾個位置——實際看過一整份都是第一個選項。
/// 使用者不用讀題就猜得到，那份練習就白做了。
///
/// 這件事**只能在本地做**：叫模型「請把答案分散」是沒有辦法驗收的請求，
/// 而重排選項是我們自己就做得到的事，做完還測得出來。
pub(super) fn shuffle_answers(items: &mut [ChoiceItem], seed: u64) {
    for (i, item) in items.iter_mut().enumerate() {
        // 壞資料原樣放過：洗牌只會讓錯誤更難查
        if item.options.len() < 2 || item.answer_index >= item.options.len() {
            continue;
        }

        // 每一題各自的亂源。同一份裡共用一個 seed 的話，
        // 每題的選項會被搬到一模一樣的位置。
        let per_item = seed ^ (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let order = practice::shuffle_order(item.options.len(), per_item);

        item.options = practice::reorder(&item.options, &order);
        // 每個選項的解說跟選項是平行陣列，**一定要用同一個排列一起搬**。
        // 只搬其中一個的話每個選項會配到別人的解說，而那個畫面
        // 看起來完全合理，不會有人發現。
        if !item.option_notes.is_empty() {
            item.option_notes = practice::reorder(&item.option_notes, &order);
        }
        item.answer_index = order
            .iter()
            .position(|&k| k == item.answer_index)
            .unwrap_or(item.answer_index);
    }
}

/// 把模型寫的逐題講評掛回本地判分的結果上。
///
/// 對錯與參考答案一律用本地的（那是算出來的，不是猜的），
/// 只有 `comment` 採用模型的。
///
/// 對應靠模型自己給的 `index`（1 起算），不是陣列位置：它很常少回
/// 一題或換個順序，照位置貼的話第三題的講評會出現在第二題底下——
/// 而那個畫面看起來完全正常。`index` 整份都缺（全是預設的 0）時
/// 才退回照位置對，那是還有救的最後手段。
pub(super) fn align_comments(
    from_model: &[ItemResult],
    mut local: Vec<ItemResult>,
) -> Vec<ItemResult> {
    let numbered = from_model.iter().any(|r| r.index > 0);

    for (i, item) in local.iter_mut().enumerate() {
        let matched = if numbered {
            from_model.iter().find(|r| r.index == i + 1)
        } else {
            from_model.get(i)
        };
        if let Some(m) = matched
            && m.comment.as_deref().is_some_and(|c| !c.trim().is_empty())
        {
            item.comment = m.comment.clone();
        }
    }
    local
}

/// 選擇題可以在本地判分，不必浪費一次 LLM 呼叫。
///
/// 答錯的題目要轉成 `corrections`，文法弱點才累積得起來——
/// 這些紀錄正是下次出文法題的依據。少了這一步，
/// 做再多文法練習系統也學不到你哪裡不會。
pub(super) fn grade_choices(items: &[ChoiceItem], input: &GradeInput) -> Feedback {
    let mut results = Vec::with_capacity(items.len());
    let mut corrections = Vec::new();
    let mut correct_count = 0usize;

    for (i, item) in items.iter().enumerate() {
        let picked = input.choices.get(i).copied().flatten();
        let correct = picked == Some(item.answer_index);
        let answer = item.options.get(item.answer_index).cloned();

        if correct {
            correct_count += 1;
        } else if let Some(point) = item.grammar_point.as_ref().filter(|p| !p.trim().is_empty()) {
            corrections.push(Correction {
                original: picked
                    .and_then(|idx| item.options.get(idx).cloned())
                    .unwrap_or_else(|| "（沒有作答）".to_string()),
                corrected: answer.clone().unwrap_or_default(),
                grammar_point: Some(point.trim().to_string()),
                severity: Some("major".into()),
                explanation: item.explanation.clone(),
            });
        }

        results.push(ItemResult {
            index: i + 1,
            correct,
            reference: answer,
            comment: item.explanation.clone(),
        });
    }

    let score = if items.is_empty() {
        None
    } else {
        Some((correct_count as f64 / items.len() as f64) * 100.0)
    };

    Feedback {
        score,
        items: results,
        corrections,
        ..Default::default()
    }
}
