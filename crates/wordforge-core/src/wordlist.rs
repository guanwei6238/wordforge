//! 不適合做成單字卡的詞。
//!
//! ## 為什麼需要這份清單
//!
//! 依詞頻加入單字時，最前面一定是 `the`、`of`、`and`、`I` 這些功能詞。
//! 它們確實最常出現，但做成單字卡是浪費時間：
//!
//! - 「the = 那」這種對應根本不成立，`the` 的用法要靠大量閱讀才會內化
//! - 代名詞、助動詞、be 動詞變化形同理，脫離句子就沒有意義
//! - 初學者的前一百張卡如果全是這些，會覺得「背了什麼都沒學到」
//!
//! 功能詞應該從**閱讀與對話**中習得，這正是 90% 法則那條路線要做的事；
//! 單字卡則留給有具體語意、查了字典就能記住的實詞。
//!
//! ## 這份清單刻意保守
//!
//! 只收「脫離句子就無法定義」的純語法詞。像 `in`、`on`、`under` 這類
//! 有空間語意的介系詞，以及 `have`、`go`、`make` 這種實義動詞，都**不**排除——
//! 它們對初學者是該背的字。

/// 英文的純語法功能詞。
///
/// 收錄範圍：冠詞、人稱／指示／關係代名詞、助動詞、be 與 do 的變化形、
/// 最基本的連接詞與否定詞。
const ENGLISH_FUNCTION_WORDS: &[&str] = &[
    // 冠詞
    "a",
    "an",
    "the", //
    // 人稱代名詞與所有格
    "i",
    "you",
    "he",
    "she",
    "it",
    "we",
    "they",
    "me",
    "him",
    "her",
    "us",
    "them",
    "my",
    "your",
    "his",
    "its",
    "our",
    "their",
    "mine",
    "yours",
    "hers",
    "ours",
    "theirs", //
    // 反身代名詞
    "myself",
    "yourself",
    "himself",
    "herself",
    "itself",
    "oneself",
    "ourselves",
    "yourselves",
    "themselves", //
    // 指示與關係代名詞
    "this",
    "that",
    "these",
    "those",
    "who",
    "whom",
    "whose",
    "which", //
    // be 動詞
    "be",
    "am",
    "is",
    "are",
    "was",
    "were",
    "been",
    "being", //
    // 助動詞 do
    "do",
    "does",
    "did",
    "done",
    "doing", //
    // 情態助動詞
    "will",
    "would",
    "shall",
    "should",
    "can",
    "could",
    "may",
    "might",
    "must",
    "ought", //
    // 基本連接詞
    "and",
    "or",
    "but",
    "nor",
    "if",
    "than",
    "as",
    "because",
    "so", //
    // 否定與存在
    "not",
    "no",
    "yes", //
    // 最抽象的介系詞（沒有具體空間或時間語意的那幾個）
    "of",
    "to", //
    // 縮寫與所有格記號常被切成獨立詞
    "s",
    "t",
    "re",
    "ve",
    "ll",
    "d",
    "m",
];

/// 這個字是否為不適合做成單字卡的功能詞。
///
/// 輸入應該是已經正規化過的詞（見 [`crate::text::normalize`]）。
/// 不認識的語言一律回傳 `false`——寧可多背幾張卡，也不要默默漏掉該學的字。
pub fn is_function_word(lang: &str, normalized_word: &str) -> bool {
    match lang {
        "en" => ENGLISH_FUNCTION_WORDS.contains(&normalized_word),
        _ => false,
    }
}

/// 某個語言的功能詞清單，供資料庫查詢直接使用。
pub fn function_words(lang: &str) -> &'static [&'static str] {
    match lang {
        "en" => ENGLISH_FUNCTION_WORDS,
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_pure_grammar_words() {
        for w in ["the", "of", "and", "i", "was", "would", "themselves"] {
            assert!(is_function_word("en", w), "{w} 應該被排除");
        }
    }

    /// 有具體語意的字必須留下來，它們正是初學者該背的。
    #[test]
    fn keeps_words_worth_a_flashcard() {
        for w in [
            "water", "book", "run", "happy", "in", "under", "have", "go", "make", "one",
        ] {
            assert!(!is_function_word("en", w), "{w} 不該被排除");
        }
    }

    /// 沒有清單的語言不要亂猜。
    #[test]
    fn unknown_languages_exclude_nothing() {
        assert!(!is_function_word("ja", "の"));
        assert!(!is_function_word("fr", "le"));
        assert!(function_words("ja").is_empty());
    }

    /// 清單本身必須是正規化過的小寫，否則比對永遠不會成立。
    #[test]
    fn list_is_normalized() {
        for w in ENGLISH_FUNCTION_WORDS {
            assert_eq!(*w, crate::text::normalize(w), "清單裡的 {w} 沒有正規化");
        }
    }

    #[test]
    fn list_has_no_duplicates() {
        let unique: std::collections::HashSet<_> = ENGLISH_FUNCTION_WORDS.iter().collect();
        assert_eq!(unique.len(), ENGLISH_FUNCTION_WORDS.len());
    }
}
