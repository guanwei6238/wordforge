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
    /// 題目。克漏字沒有題幹（題目就是那個空格），所以可以是空的。
    #[serde(alias = "prompt", default)]
    pub question: String,
    pub options: Vec<String>,
    /// 每個選項一句話：對的那個為什麼對、錯的那些錯在哪。
    /// 與 `options` 平行，洗牌時必須一起搬。
    ///
    /// 這是「針對你的作答說明」的來源。選擇題在本地判分，模型從頭到尾
    /// 沒看過你選了什麼，所以答案感知的解說只能在**出題時**先備好——
    /// 每個選項各寫一句，判分時挑你按的那一句出來。不必多打一次模型，
    /// 而且重做同一份題目時解說還在。
    #[serde(default)]
    pub option_notes: Vec<String>,
    pub answer_index: usize,
    #[serde(default)]
    pub explanation: Option<String>,
    /// 文法題才有：這題在考哪個文法點
    #[serde(default)]
    pub grammar_point: Option<String>,
    /// easy / medium / hard。一整份同一個難度的話，做完不知道自己讀懂了沒有，
    /// 所以出題時會指定分配；模型沒給就是 `None`，UI 不顯示徽章。
    #[serde(default)]
    pub difficulty: Option<String>,
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
        /// 整篇的母語翻譯。作答前不顯示，解析時才展開——
        /// 逐字翻譯的清單太吵，整篇對照才是讀完之後真正想看的東西。
        #[serde(default)]
        translation: Option<String>,
        #[serde(default)]
        new_words: Vec<NewWord>,
        questions: Vec<ChoiceItem>,
    },
    /// 克漏字：一篇用已知詞寫的短文，把該複習的字挖掉。
    ///
    /// 跟 `Reading` 分開而不是共用：兩者考的東西相反——閱讀考「看不看得懂」，
    /// 克漏字考「想不想得起來」，所以文章的用字原則、要不要放生詞、
    /// 怎麼批改都不一樣。共用一個型別只會讓每個分支都在問「這是哪一種」。
    Cloze {
        title: String,
        /// 挖好空格的短文，空格是 `{{1}}`、`{{2}}`
        passage: String,
        #[serde(default)]
        translation: Option<String>,
        /// 第 k 題對應 `{{k}}` 那一格
        items: Vec<ChoiceItem>,
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

/// 閱讀解析裡的一條字詞說明。
///
/// 這些**不是模型產生的**，是拿文章去查你自己匯入的字典得到的。
/// 這件事對多語言很重要：模型對小語種的解釋品質沒有保證，
/// 但字典是使用者自己選的，查得到就是查得到。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GlossaryNote {
    /// 正規化後的比對鍵（小寫、去標點）。
    ///
    /// 前端要拿使用者點到的字去對這一欄，不能對 `text`——`text` 是字典
    /// 收錄的原形，大小寫不一定跟文章裡的一樣，多詞條目也是。
    pub term: String,
    /// 字典收錄的原形，顯示用
    pub text: String,
    pub gloss: Option<String>,
    pub translation: Option<String>,
    /// 多詞片語。單看每個字查不出這個意思，所以要單獨列。
    pub is_phrase: bool,
    /// 這個字不在你的已知詞裡——也就是 90% 法則裡那不足 10% 的部分
    pub is_unknown: bool,
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
    /// 文章裡的生字與片語，由本地字典查出來，不是模型寫的。
    #[serde(default)]
    pub glossary: Vec<GlossaryNote>,
    /// 這篇刻意要教的新字。跟 `unknown_words` 分開：那些是你答錯時
    /// 露出來的，這些是系統本來就打算教你的。兩者都會進牌組。
    #[serde(default)]
    pub taught_words: Vec<String>,
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
