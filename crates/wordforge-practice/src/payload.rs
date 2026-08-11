//! 練習題目與批改結果的資料形狀。
//!
//! 這些型別同時是「LLM 要回傳的 JSON schema」與「前端拿到的資料」。
//! 兩者用同一份定義，prompt 裡寫的欄位名稱和這裡對不上時，
//! 解析就會失敗而不是默默給出空白畫面。

use serde::{Deserialize, Serialize};
use wordforge_core::practice::ExerciseKind;

/// 一題翻譯。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationItem {
    /// 要翻譯的句子
    pub source: String,
    /// 出題時刻意用上的字
    #[serde(default)]
    pub target_word: Option<String>,
    /// 參考答案。作答前不給前端看。
    #[serde(default)]
    pub reference: Option<String>,
}

/// 一題選擇題（閱讀理解與文法練習共用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceItem {
    #[serde(alias = "prompt")]
    pub question: String,
    pub options: Vec<String>,
    pub answer_index: usize,
    #[serde(default)]
    pub explanation: Option<String>,
    /// 文法題才有：這題在考哪個文法點
    #[serde(default)]
    pub grammar_point: Option<String>,
}

/// 閱讀理解裡預先標好的生詞。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewWord {
    pub word: String,
    #[serde(default)]
    pub gloss: Option<String>,
}

/// 題目內容。依題型而異，所以用 tagged enum——
/// 前端拿到 `kind` 就知道該渲染哪一種畫面。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExerciseBody {
    Translation {
        /// 中翻英還是英翻中
        to_target: bool,
        items: Vec<TranslationItem>,
    },
    Reading {
        title: String,
        passage: String,
        #[serde(default)]
        new_words: Vec<NewWord>,
        questions: Vec<ChoiceItem>,
    },
    Choices {
        items: Vec<ChoiceItem>,
    },
}

/// 送給前端的完整題目。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExerciseView {
    pub exercise_id: i64,
    pub kind: ExerciseKind,
    pub body: ExerciseBody,
    /// 這次刻意帶進來的目標詞
    pub target_words: Vec<String>,
    /// 產生當下的已知詞覆蓋率，只有閱讀類才有
    pub coverage: Option<f64>,
}

/// 使用者的作答。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradeInput {
    pub exercise_id: i64,
    /// 翻譯題：每題的文字答案
    #[serde(default)]
    pub answers: Vec<String>,
    /// 選擇題：每題選的選項索引，沒作答是 null
    #[serde(default)]
    pub choices: Vec<Option<usize>>,
    /// 使用者在閱讀時自己點出來「這個字我不會」
    #[serde(default)]
    pub marked_unknown: Vec<String>,
}

/// 單題的批改結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemResult {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub correct: bool,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

/// 一處需要修正的地方。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Correction {
    #[serde(default)]
    pub original: String,
    #[serde(default)]
    pub corrected: String,
    /// 用一致的英文術語（tense、articles…），會累積成文法弱點
    #[serde(default)]
    pub grammar_point: Option<String>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub explanation: Option<String>,
}

/// 批改結果。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Feedback {
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub items: Vec<ItemResult>,
    #[serde(default)]
    pub corrections: Vec<Correction>,
    /// LLM 判斷學習者不懂的字
    #[serde(default)]
    pub unknown_words: Vec<String>,
    /// 實際加進牌組的字。
    ///
    /// 跟 `unknown_words` 不一定相同：字典裡查不到的會被跳過，
    /// 已經在牌組裡的也不會重複加。UI 要顯示的是這一份。
    #[serde(default)]
    pub added_to_deck: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LLM 常常少給欄位，解析不能因此整個失敗。
    #[test]
    fn feedback_tolerates_missing_fields() {
        let f: Feedback = serde_json::from_str(r#"{"score": 80}"#).unwrap();
        assert_eq!(f.score, Some(80.0));
        assert!(f.corrections.is_empty());
        assert!(f.unknown_words.is_empty());

        let empty: Feedback = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Feedback::default());
    }

    #[test]
    fn feedback_parses_a_full_response() {
        let f: Feedback = serde_json::from_str(
            r#"{
                "score": 60,
                "items": [{"index": 1, "correct": false, "reference": "I went to the park"}],
                "corrections": [{"original": "I go", "corrected": "I went",
                                 "grammar_point": "tense", "severity": "major"}],
                "unknown_words": ["diligent", "park"]
            }"#,
        )
        .unwrap();

        assert_eq!(f.items[0].reference.as_deref(), Some("I went to the park"));
        assert_eq!(f.corrections[0].grammar_point.as_deref(), Some("tense"));
        assert_eq!(f.unknown_words, vec!["diligent", "park"]);
    }

    /// 出題的 prompt 用 `prompt` 當欄位名，閱讀用 `question`，兩種都要吃得下。
    #[test]
    fn choice_items_accept_both_field_names() {
        let a: ChoiceItem =
            serde_json::from_str(r#"{"question":"why?","options":["a","b"],"answer_index":0}"#)
                .unwrap();
        let b: ChoiceItem =
            serde_json::from_str(r#"{"prompt":"why?","options":["a","b"],"answer_index":0}"#)
                .unwrap();
        assert_eq!(a.question, b.question);
    }

    #[test]
    fn exercise_body_round_trips_through_json() {
        let body = ExerciseBody::Translation {
            to_target: true,
            items: vec![TranslationItem {
                source: "我昨天去公園".into(),
                target_word: Some("park".into()),
                reference: Some("I went to the park yesterday".into()),
            }],
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"kind\":\"translation\""));
        assert_eq!(serde_json::from_str::<ExerciseBody>(&json).unwrap(), body);
    }
}
