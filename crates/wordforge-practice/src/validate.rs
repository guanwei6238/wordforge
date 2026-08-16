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

/// 指定了文法點的練習：每一題都要考那個點。
///
/// prompt 已經講得夠死了（「每一題都填 `{point}`，不要填別的」），
/// 但那只是請求。使用者選了「冠詞」卻拿到一份摻著時態與介系詞的綜合
/// 練習時，畫面上看不出任何異狀——每一題都是合法的文法題，只是沒有
/// 一題在練他要練的東西，而且那些題目的對錯還會記到別的文法點的排程上。
///
/// 標籤走 `normalize_point`：模型寫 `Articles`、`article` 都算數，
/// 真的考成別的點才退回去。
pub fn check_grammar_focus(field: &str, value: &Value, point: &str) -> Vec<Problem> {
    let Some(items) = value.get(field).and_then(|v| v.as_array()) else {
        // 陣列本身的問題由 `check_choice_items` 報，這裡不重複講一次
        return Vec::new();
    };

    let wanted = [point.to_string()];
    items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let raw = item.get("grammar_point").and_then(|p| p.as_str());
            match raw {
                Some(raw)
                    if wordforge_core::grammar_points::normalize_point(&wanted, raw).is_some() =>
                {
                    None
                }
                Some(raw) => Some(Problem::new(
                    format!("/{field}/{i}/grammar_point"),
                    format!(
                        "是「{raw}」，但這份練習指定要練 `{point}`。\
                         這一題請改成考 `{point}`，標籤也填 `{point}`。"
                    ),
                )),
                None => Some(Problem::new(
                    format!("/{field}/{i}/grammar_point"),
                    format!("沒填。這份練習指定要練 `{point}`，每一題都要填這個標籤。"),
                )),
            }
        })
        .collect()
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

/// 這次翻譯練習要練的一個字，連同它在句子裡會長成的樣子。
///
/// `forms` 來自 `lemmas::forms`，是整個家族的正規化詞形。指派了 `run`
/// 而句子寫 `She ran` 時，字面比對認不出來——那是驗收要認得的，
/// 不是要退回去的。
#[derive(Debug, Clone)]
pub struct WordAssignment {
    /// 指派的字，原樣。錯誤訊息要拿它回去跟模型講。
    pub word: String,
    /// 這個字的家族詞形，已正規化
    pub forms: std::collections::HashSet<String>,
}

impl WordAssignment {
    /// 這個標籤指的是不是這個字。模型偶爾會回 `Borrow` 或 `borrowed`。
    fn is_me(&self, raw: &str) -> bool {
        let normalized = wordforge_core::text::normalize(raw);
        !normalized.is_empty()
            && (normalized == wordforge_core::text::normalize(&self.word)
                || self.forms.contains(&normalized))
    }
}

/// 翻譯題的驗收條件。
pub struct TranslationSpec<'a> {
    /// 這次指派的字。空的代表這次沒有指定用字（新使用者一個學過的字都沒有），
    /// 那時模型自由造句，只驗題目本身。
    pub assignments: &'a [WordAssignment],
    /// 練習方向是「母語 → 目標語」。決定該去哪一句裡找那個字：
    /// 這個方向的目標語句子是 `reference`，反過來是 `source`。
    pub to_target: bool,
    /// 目標語言代碼。片語要拼回去比對，而拼法跟語言有關。
    pub target_lang: &'a str,
}

/// 翻譯題的回應。
///
/// ## 為什麼要驗「有沒有用到那個字」
///
/// 這個題型的整個意義就是「拿今天該複習的字造句給他翻」。prompt 一直
/// 都有列出那些字，但列出來只是請求：實際跑起來模型常常寫出一句
/// 通順、自然、跟指定的字沒有關係的句子。畫面上看不出異狀——題目是
/// 好題目，只是那個字沒練到，而 `target_words` 還照樣記成「這次練了
/// 這些字」。使用者感覺到的是「怎麼每次都在練別的字」。
///
/// 這件事本地驗得到（見 CLAUDE.md：凡是能在本地驗的就不要只相信模型），
/// 所以就在這裡驗，驗不過帶著句子回問一次。
pub fn check_translation(spec: &TranslationSpec, value: &Value) -> Vec<Problem> {
    let Some(items) = value.get("items").and_then(|v| v.as_array()) else {
        return vec![Problem::new("/items", "缺這個欄位，或者它不是陣列。")];
    };
    if items.is_empty() {
        return vec![Problem::new("/items", "一題都沒有。")];
    }

    let mut problems = Vec::new();
    // 一個字被兩題用掉，就代表有另一個字整份都沒練到
    let mut used: Vec<&str> = Vec::new();

    for (i, item) in items.iter().enumerate() {
        let at = format!("/items/{i}");
        if let Some(p) = need_text(item, "source") {
            problems.push(Problem::new(format!("{at}{}", p.path), p.detail));
            continue;
        }
        if spec.assignments.is_empty() {
            continue;
        }

        let source = item.get("source").and_then(|s| s.as_str()).unwrap_or("");
        let reference = item.get("reference").and_then(|r| r.as_str()).unwrap_or("");
        let raw_word = item
            .get("target_word")
            .and_then(|w| w.as_str())
            .unwrap_or("")
            .trim();

        let wanted: Vec<&str> = spec.assignments.iter().map(|a| a.word.as_str()).collect();
        let Some(assigned) = spec.assignments.iter().find(|a| a.is_me(raw_word)) else {
            problems.push(Problem::new(
                format!("{at}/target_word"),
                if raw_word.is_empty() {
                    format!(
                        "沒填。每一題都要標明它練的是哪一個字，而且只能從這份清單挑：{}。",
                        wanted.join("、")
                    )
                } else {
                    format!(
                        "是「{raw_word}」，不在這次要練的清單裡：{}。\
                         這些字是照他的複習進度挑的，換成別的字這一題就白練了。",
                        wanted.join("、")
                    )
                },
            ));
            continue;
        };

        if used.contains(&assigned.word.as_str()) {
            problems.push(Problem::new(
                format!("{at}/target_word"),
                format!(
                    "「{}」前面已經有一題用過了。一題一個字剛好把清單用完，\
                     重複用等於有一個字整份都沒練到。",
                    assigned.word
                ),
            ));
            continue;
        }
        used.push(&assigned.word);

        // 目標語言那一句才驗得了：母語 → 目標語時，那個字要出現在
        // 參考答案裡（題目是母語，本來就不會有它）。
        let (field, sentence) = if spec.to_target {
            ("reference", reference)
        } else {
            ("source", source)
        };

        if sentence.trim().is_empty() {
            problems.push(Problem::new(
                format!("{at}/{field}"),
                format!(
                    "是空的。這一題要練「{}」，沒有{}就沒辦法確認這個字真的用得上。",
                    assigned.word,
                    if spec.to_target {
                        "參考答案"
                    } else {
                        "題目句子"
                    }
                ),
            ));
            continue;
        }

        if !wordforge_core::text::mentions_any(sentence, &assigned.forms, spec.target_lang) {
            problems.push(Problem::new(
                format!("{at}/{field}"),
                format!(
                    "沒有用到「{word}」（也沒有它的任何變化形）：「{sentence}」。\
                     這一題的目的就是練這個字，請重寫成自然會用到「{word}」的句子。",
                    word = assigned.word,
                ),
            ));
        }
    }

    problems
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

    fn tagged(point: &str) -> Value {
        json!({
            "question": "Q",
            "options": ["a", "b"],
            "answer_index": 0,
            "grammar_point": point,
        })
    }

    /// 這條測試存在的理由：使用者選了「冠詞」，模型出一份摻著時態的
    /// 綜合練習，畫面上完全看不出來——每一題都是合法的文法題，
    /// 只是沒有一題在練他要練的東西。
    #[test]
    fn a_drill_that_ignores_the_chosen_point_is_reported() {
        let v = json!({ "items": [tagged("articles"), tagged("tense")] });
        let problems = check_grammar_focus("items", &v, "articles");

        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].path, "/items/1/grammar_point");
        assert!(problems[0].detail.contains("tense"), "要講出它實際填了什麼");
    }

    /// 標籤的寫法不是重點，考的是不是那個點才是。`Articles`、`article`
    /// 都走 `normalize_point` 收得回來，為了大小寫再燒一次呼叫是浪費。
    #[test]
    fn a_differently_spelled_tag_still_counts_as_the_chosen_point() {
        let v = json!({ "items": [tagged("Articles"), tagged("article")] });
        assert_eq!(check_grammar_focus("items", &v, "articles"), Vec::new());
    }

    #[test]
    fn a_drill_item_without_a_tag_is_reported_when_a_point_was_chosen() {
        let v = json!({ "items": [item(4, 0)] });
        let problems = check_grammar_focus("items", &v, "articles");
        assert_eq!(problems[0].path, "/items/0/grammar_point");
    }

    /// 陣列本身壞掉是 `check_choice_items` 的職責，兩邊都報的話
    /// 同一件事會在回問訊息裡出現兩次。
    #[test]
    fn a_missing_items_array_is_left_to_the_other_check() {
        assert_eq!(
            check_grammar_focus("items", &json!({}), "articles"),
            Vec::new()
        );
    }

    /// 沒有指派用字時（新使用者一個學過的字都沒有）只驗題目本身。
    fn free_form() -> TranslationSpec<'static> {
        TranslationSpec {
            assignments: &[],
            to_target: true,
            target_lang: "en",
        }
    }

    fn assign(word: &str, forms: &[&str]) -> WordAssignment {
        WordAssignment {
            word: word.to_string(),
            forms: forms.iter().map(|f| f.to_string()).collect(),
        }
    }

    #[test]
    fn translation_items_need_a_source_sentence() {
        let ok = json!({ "items": [{"source": "我昨天去了公園"}] });
        assert_eq!(check_translation(&free_form(), &ok), Vec::new());

        let blank = json!({ "items": [{"source": "  "}] });
        assert_eq!(
            check_translation(&free_form(), &blank)[0].path,
            "/items/0/source"
        );

        assert_eq!(
            check_translation(&free_form(), &json!({}))[0].path,
            "/items"
        );
    }

    /// 這條測試存在的理由是它曾經沒有被驗過：prompt 列了要練的字，
    /// 模型回一句通順、自然、跟那個字沒有關係的句子。畫面上完全看不出來
    /// ——題目是好題目，只是那個字沒練到，而系統還把它記成「這次練了
    /// 這個字」。使用者感覺到的是「怎麼每次都在練別的字」。
    #[test]
    fn a_sentence_that_never_uses_the_assigned_word_is_reported() {
        let assignments = [assign("borrow", &["borrow", "borrowed", "borrowing"])];
        let spec = TranslationSpec {
            assignments: &assignments,
            to_target: true,
            target_lang: "en",
        };

        let missed = json!({ "items": [{
            "source": "我昨天去圖書館",
            "target_word": "borrow",
            "reference": "I went to the library yesterday.",
        }]});
        let problems = check_translation(&spec, &missed);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].path, "/items/0/reference");
        assert!(problems[0].detail.contains("borrow"), "要講出是哪個字");
        assert!(
            problems[0]
                .detail
                .contains("I went to the library yesterday."),
            "要把句子附回去，非交互式的後端不記得自己寫過什麼"
        );
    }

    /// 屈折形算練到了。`borrowed` 退回去重寫的話，模型下一次八成
    /// 寫出更生硬的句子，而使用者本來就練到了那個字。
    #[test]
    fn an_inflected_form_satisfies_the_assignment() {
        let assignments = [assign("borrow", &["borrow", "borrowed", "borrowing"])];
        let spec = TranslationSpec {
            assignments: &assignments,
            to_target: true,
            target_lang: "en",
        };
        let ok = json!({ "items": [{
            "source": "我昨天跟他借了一本書",
            "target_word": "borrow",
            "reference": "I borrowed a book from him yesterday.",
        }]});
        assert_eq!(check_translation(&spec, &ok), Vec::new());
    }

    /// 目標語 → 母語的方向，那個字在題目句子裡，不在參考答案裡。
    /// 兩個方向找錯邊的話，驗收會把每一題都退回去。
    #[test]
    fn the_direction_decides_which_sentence_carries_the_word() {
        let assignments = [assign("borrow", &["borrow", "borrowed"])];
        let spec = TranslationSpec {
            assignments: &assignments,
            to_target: false,
            target_lang: "en",
        };
        let ok = json!({ "items": [{
            "source": "I borrowed a book from him.",
            "target_word": "borrow",
            "reference": "我跟他借了一本書",
        }]});
        assert_eq!(check_translation(&spec, &ok), Vec::new());

        let wrong_side = json!({ "items": [{
            "source": "我跟他借了一本書",
            "target_word": "borrow",
            "reference": "I borrowed a book from him.",
        }]});
        assert_eq!(
            check_translation(&spec, &wrong_side)[0].path,
            "/items/0/source"
        );
    }

    /// 模型換一個字來出題，就是使用者說的「本來要練這個字，結果練到別的」。
    #[test]
    fn a_word_outside_the_assignment_list_is_reported() {
        let assignments = [assign("borrow", &["borrow"]), assign("return", &["return"])];
        let spec = TranslationSpec {
            assignments: &assignments,
            to_target: true,
            target_lang: "en",
        };
        let strayed = json!({ "items": [{
            "source": "我把書還了",
            "target_word": "lend",
            "reference": "I lent the book back.",
        }]});
        let problems = check_translation(&spec, &strayed);
        assert_eq!(problems[0].path, "/items/0/target_word");
        assert!(
            problems[0].detail.contains("borrow"),
            "要把可以挑的字列回去"
        );
    }

    /// 一題一個字剛好用完，重複用等於有一個字整份都沒練到。
    #[test]
    fn reusing_one_word_for_two_items_is_reported() {
        let assignments = [assign("borrow", &["borrow"]), assign("return", &["return"])];
        let spec = TranslationSpec {
            assignments: &assignments,
            to_target: true,
            target_lang: "en",
        };
        let repeated = json!({ "items": [
            {"source": "a", "target_word": "borrow", "reference": "I borrow it."},
            {"source": "b", "target_word": "borrow", "reference": "You borrow it."},
        ]});
        let problems = check_translation(&spec, &repeated);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].path, "/items/1/target_word");
    }

    /// 題數少一題仍然不算問題（既有的取捨：三題的練習照樣做得完），
    /// 所以「有字沒被用到」不報。
    #[test]
    fn a_short_translation_set_is_still_acceptable() {
        let assignments = [assign("borrow", &["borrow"]), assign("return", &["return"])];
        let spec = TranslationSpec {
            assignments: &assignments,
            to_target: true,
            target_lang: "en",
        };
        let short = json!({ "items": [
            {"source": "a", "target_word": "borrow", "reference": "I borrow it."},
        ]});
        assert_eq!(check_translation(&spec, &short), Vec::new());
    }
}
