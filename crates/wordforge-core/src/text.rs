//! 字串正規化與斷詞。
//!
//! 這裡只做「所有語言都成立」的最小共通處理。真正需要語言知識的部分
//! （英文的詞形還原、日文的分詞）屬於 `wordforge-dict`，因為那需要查表。

use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// 把詞正規化成可以拿來比對的鍵值：NFKC + 小寫 + 去除前後標點。
///
/// 保留內部的連字號與撇號（`well-known`、`don't` 是完整的詞）。
pub fn normalize(word: &str) -> String {
    word.nfkc()
        .flat_map(|c| c.to_lowercase())
        .collect::<String>()
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_string()
}

/// 把一段文本切成詞元，已正規化並濾掉純標點與純數字。
///
/// 用 Unicode 的 word boundary 規則，因此對拉丁語系、西里爾字母等都成立；
/// 中日韓沒有空格，需要另外接分詞器（見 `wordforge-dict`）。
pub fn tokenize(text: &str) -> Vec<String> {
    text.unicode_words()
        .map(normalize)
        .filter(|w| !w.is_empty() && w.chars().any(char::is_alphabetic))
        .collect()
}

/// 計算文本有幾個詞元（token）與幾個相異詞（type）。
pub fn token_type_counts(text: &str) -> (usize, usize) {
    let tokens = tokenize(text);
    let types: std::collections::HashSet<&String> = tokens.iter().collect();
    (tokens.len(), types.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lowercases_and_trims_punctuation() {
        assert_eq!(normalize("Hello,"), "hello");
        assert_eq!(normalize("\"Quoted\"!"), "quoted");
        assert_eq!(normalize("don't"), "don't");
        assert_eq!(normalize("well-known."), "well-known");
    }

    #[test]
    fn normalize_keeps_accents() {
        // 重音是拼字的一部分，不能剝掉，否則 résumé / resume 會混為一談
        assert_eq!(normalize("Café"), "café");
    }

    #[test]
    fn tokenize_drops_numbers_and_punctuation() {
        let t = tokenize("The cat sat on 3 mats -- really!");
        assert_eq!(t, vec!["the", "cat", "sat", "on", "mats", "really"]);
    }

    #[test]
    fn counts_tokens_and_types() {
        let (tokens, types) = token_type_counts("the cat and the hat");
        assert_eq!(tokens, 5);
        assert_eq!(types, 4);
    }
}
