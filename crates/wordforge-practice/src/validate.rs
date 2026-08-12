//! 模型回應的驗收。
//!
//! ## 為什麼需要這一層
//!
//! 原本每個地方都寫 `filter_map(|q| serde_json::from_value(q).ok())`：
//! 解析失敗的題目**直接消失**。使用者拿到三題而不是四題，畫面上沒有任何
//! 異狀，log 也沒有一行——只有出題的人知道少了一題。壞掉的樣子看起來
//! 完全正常，正是最難查的那一類。
//!
//! ## serde 就是 schema
//!
//! 不另外寫一份 JSON Schema：`ChoiceItem` 的欄位定義**就是**契約，
//! `from_value` 的錯誤訊息已經是「哪個欄位缺了、型別是什麼」。再維護一份
//! 平行的 schema 只會有兩份互相漂移的真相——那正是這個專案踩過的坑
//! （前後端各一份模型清單）。
//!
//! 這裡補的是 serde **表達不了**的部分：跨欄位的關係。
//! `answer_index` 要落在 `options` 的範圍內、`option_notes` 要跟 `options`
//! 一樣長、克漏字的空格編號要跟題數對得上——這些是型別系統看不到的。
//!
//! ## 致命與瑕疵
//!
//! 這個模組只回報**致命**問題：資料沒辦法用。那種要帶著上一次的輸出
//! 回問一次（非交互式的後端不記得自己寫過什麼）。
//!
//! 逐選項解說缺漏屬於瑕疵——題目照樣做得完，只是少了「你選的那個為什麼
//! 不行」。那條路徑在 `engine::fill_option_notes`，補不到就算了，
//! 不會讓整份練習失敗。

use serde_json::Value;

use crate::payload::ChoiceItem;

/// 一處驗收沒過的地方。
///
/// `path` 用 JSON Pointer 風格（`/questions/2/answer_index`），因為那是
/// 模型看得懂、也指得回去的定位方式。說「第三題有問題」它還要自己數。
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub path: String,
    pub detail: String,
}

impl Problem {
    fn new(path: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}：{}", self.path, self.detail)
    }
}

/// 非空字串欄位。
fn need_text(value: &Value, field: &str) -> Option<Problem> {
    match value.get(field).and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => None,
        Some(_) => Some(Problem::new(
            format!("/{field}"),
            "是空字串。這個欄位一定要有內容。",
        )),
        None => Some(Problem::new(
            format!("/{field}"),
            "缺這個欄位，或者型別不是字串。",
        )),
    }
}

/// 一組選擇題。閱讀的 `questions`、文法與克漏字的 `items` 共用。
///
/// **題數少一題不算問題**：四題的閱讀測驗照樣做得完，為了湊到五題再燒一次
/// 完整的呼叫不划算。這裡只抓「這一題沒辦法作答」的情形。
pub fn check_choice_items(field: &str, value: &Value) -> Vec<Problem> {
    let Some(items) = value.get(field).and_then(|v| v.as_array()) else {
        return vec![Problem::new(
            format!("/{field}"),
            "缺這個欄位，或者它不是陣列。",
        )];
    };

    if items.is_empty() {
        return vec![Problem::new(format!("/{field}"), "一題都沒有。")];
    }
    let mut problems = Vec::new();
    for (i, raw) in items.iter().enumerate() {
        let at = format!("/{field}/{i}");

        // serde 的錯誤訊息本身就說得夠清楚（哪個欄位缺了、型別是什麼），
        // 直接轉給模型看，不要自己重寫一遍
        let item: ChoiceItem = match serde_json::from_value(raw.clone()) {
            Ok(item) => item,
            Err(e) => {
                problems.push(Problem::new(at, e.to_string()));
                continue;
            }
        };

        if item.options.len() < 2 {
            problems.push(Problem::new(
                format!("{at}/options"),
                format!("只有 {} 個選項，至少要兩個。", item.options.len()),
            ));
            continue;
        }
        // 跨欄位的關係，serde 表達不了：索引超出範圍的話這一題永遠答不對，
        // 而畫面上看起來完全正常
        if item.answer_index >= item.options.len() {
            problems.push(Problem::new(
                format!("{at}/answer_index"),
                format!(
                    "是 {}，但只有 {} 個選項（索引從 0 起算）。",
                    item.answer_index,
                    item.options.len()
                ),
            ));
        }
        if item.options.iter().any(|o| o.trim().is_empty()) {
            problems.push(Problem::new(format!("{at}/options"), "有空白的選項。"));
        }
    }
    problems
}

/// 閱讀測驗的回應。
pub fn check_reading(value: &Value) -> Vec<Problem> {
    let mut problems = Vec::new();
    problems.extend(need_text(value, "passage"));
    problems.extend(check_choice_items("questions", value));
    problems
}

/// 克漏字的回應。
///
/// 空格與題目對不上是這個題型專屬的致命問題：有題目卻沒有對應的空格時，
/// 那一題永遠答不到，而且判分照跑。
pub fn check_cloze(value: &Value) -> Vec<Problem> {
    let mut problems = Vec::new();
    problems.extend(need_text(value, "passage"));
    problems.extend(check_choice_items("items", value));

    // 編號亂序或對不上都**不**列進來：`renumber_blanks` 在本地就重排得掉，
    // 多出來的題目也截得掉。為了那個再燒一次完整的呼叫不划算。
    // 真正沒救的只有一種：一個空格都沒有，那就沒有東西可以作答。
    let passage = value.get("passage").and_then(|p| p.as_str()).unwrap_or("");
    if !passage.trim().is_empty() && wordforge_core::practice::blank_numbers(passage).is_empty() {
        problems.push(Problem::new(
            "/passage",
            "文章裡一個 {{n}} 空格都沒有。克漏字沒有空格就沒有東西可以作答。",
        ));
    }
    problems
}

/// 翻譯題的回應。
pub fn check_translation(value: &Value) -> Vec<Problem> {
    let Some(items) = value.get("items").and_then(|v| v.as_array()) else {
        return vec![Problem::new("/items", "缺這個欄位，或者它不是陣列。")];
    };
    if items.is_empty() {
        return vec![Problem::new("/items", "一題都沒有。")];
    }

    items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            need_text(item, "source").map(|p| Problem {
                path: format!("/items/{i}{}", p.path),
                detail: p.detail,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(options: usize, answer: usize) -> Value {
        json!({
            "question": "Q",
            "options": (0..options).map(|i| format!("opt{i}")).collect::<Vec<_>>(),
            "answer_index": answer,
        })
    }

    #[test]
    fn a_well_formed_set_has_no_problems() {
        let v = json!({ "items": [item(4, 0), item(4, 3)] });
        assert_eq!(check_choice_items("items", &v), Vec::new());
    }

    /// 這條測試存在的理由：原本解析失敗的題目會被 `filter_map(...ok())`
    /// 默默丟掉——使用者拿到三題而不是四題，畫面上沒有任何異狀。
    #[test]
    fn a_malformed_item_is_reported_not_dropped() {
        let v = json!({ "items": [item(4, 0), {"question": "壞掉的"}] });
        let problems = check_choice_items("items", &v);

        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].path, "/items/1");
        assert!(
            problems[0].detail.contains("options"),
            "serde 的訊息要說出缺哪個欄位：{}",
            problems[0].detail
        );
    }

    /// 索引超出範圍時那一題永遠答不對，而畫面上看起來完全正常。
    /// serde 表達不了這種跨欄位的關係。
    #[test]
    fn an_out_of_range_answer_index_is_caught() {
        let v = json!({ "items": [item(4, 9)] });
        let problems = check_choice_items("items", &v);

        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].path, "/items/0/answer_index");
        assert!(problems[0].detail.contains("是 9"), "要講出實際的值");
        assert!(problems[0].detail.contains("4 個選項"), "也要講出範圍");
    }

    /// 題數少一題不算問題：四題的閱讀測驗照樣做得完，
    /// 為了湊到五題再燒一次完整的呼叫不划算。
    #[test]
    fn a_short_question_set_is_not_a_problem() {
        let v = json!({ "items": [item(4, 0)] });
        assert_eq!(check_choice_items("items", &v), Vec::new());
    }

    /// 一次把所有問題講完。每來回一次就是一次完整的呼叫，
    /// 講一半下次還要再來一趟。
    #[test]
    fn every_problem_is_reported_not_just_the_first() {
        let v = json!({ "items": [item(4, 9), {"question": "壞掉的"}, item(1, 0)] });
        let problems = check_choice_items("items", &v);
        assert_eq!(problems.len(), 3, "{problems:?}");
    }

    #[test]
    fn a_missing_passage_is_fatal_for_reading() {
        let problems = check_reading(&json!({ "questions": [item(4, 0)] }));
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].path, "/passage");
    }

    /// 空格與題目對不上、或亂序，都在本地修得掉（`renumber_blanks` 重排、
    /// 多的題目截掉），不值得為它再燒一次完整的呼叫。
    /// 真正沒救的只有「一個空格都沒有」。
    #[test]
    fn only_a_blankless_passage_is_fatal_for_cloze() {
        let ok = json!({ "passage": "a {{1}} b {{2}}", "items": [item(2, 0), item(2, 0)] });
        assert_eq!(check_cloze(&ok), Vec::new());

        let short = json!({ "passage": "a {{1}}", "items": [item(2, 0), item(2, 0)] });
        assert_eq!(check_cloze(&short), Vec::new(), "對不上的部分本地截得掉");

        let none = json!({ "passage": "沒有空格", "items": [item(2, 0)] });
        assert!(
            check_cloze(&none)[0]
                .detail
                .contains("一個 {{n}} 空格都沒有")
        );
    }

    /// 亂序不算壞：`renumber_blanks` 在本地就改得掉，回問一次是浪費。
    #[test]
    fn out_of_order_blanks_are_not_a_problem() {
        let v = json!({ "passage": "a {{2}} b {{1}}", "items": [item(2, 0), item(2, 0)] });
        assert_eq!(check_cloze(&v), Vec::new());
    }

    #[test]
    fn translation_items_need_a_source_sentence() {
        let ok = json!({ "items": [{"source": "我昨天去了公園"}] });
        assert_eq!(check_translation(&ok), Vec::new());

        let blank = json!({ "items": [{"source": "  "}] });
        assert_eq!(check_translation(&blank)[0].path, "/items/0/source");

        assert_eq!(check_translation(&json!({}))[0].path, "/items");
    }
}
