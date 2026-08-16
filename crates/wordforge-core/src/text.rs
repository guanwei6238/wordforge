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

/// 這個語言的詞之間有沒有空格。
///
/// 決定多詞條目（片語）要怎麼從詞元拼回去查字典：`search for` 中間有空格，
/// 但日文的「気にする」沒有。判斷錯的話片語一個都查不到。
pub fn joins_with_space(lang: &str) -> bool {
    let key = lang.split(['-', '_']).next().unwrap_or(lang).to_lowercase();
    // 韓文有空格，所以不在這裡；泰文與中日文沒有
    !matches!(key.as_str(), "zh" | "ja" | "th" | "lo" | "my" | "km")
}

/// 產生所有 2..=max_n 長度的連續詞組，供片語查表用。
///
/// 這是「片語解釋」的基礎：文章裡出現 `search for`，字典裡剛好有這個
/// 多詞條目，那就值得單獨解釋一次——`search` 和 `for` 分開查都得不到
/// 「尋找」這個意思。
///
/// 對中日文還有一個附帶效果：`tokenize` 會把它們切成單字，
/// 這裡的 n-gram 拼回去再查字典，等於一個很粗的分詞器
/// （「公」「園」→「公園」查得到）。不是正規分詞，但比完全不做好。
pub fn ngrams(tokens: &[String], lang: &str, max_n: usize) -> Vec<String> {
    let sep = if joins_with_space(lang) { " " } else { "" };
    let mut out = Vec::new();
    for n in 2..=max_n {
        if tokens.len() < n {
            break;
        }
        for window in tokens.windows(n) {
            out.push(window.join(sep));
        }
    }
    out
}

/// 這段文字裡有沒有用到這組詞形之一。
///
/// `forms` 是同一個字的整個家族（`run` / `runs` / `ran` / `running`），
/// 已經正規化過——比對只認詞形，因為「有沒有練到 run」不能靠字面比對：
/// 題目句子寫的是 `ran`，而學習者要練的是 `run`。
///
/// 多詞條目（`search for`、「気にする」）比對 n-gram：只比單一詞元的話，
/// 片語永遠對不上，而字典裡有 69 萬個多詞條目。
pub fn mentions_any(text: &str, forms: &std::collections::HashSet<String>, lang: &str) -> bool {
    if forms.is_empty() {
        return false;
    }
    let tokens = tokenize(text);
    if tokens.iter().any(|t| forms.contains(t)) {
        return true;
    }

    // n-gram 只展開到「最長的那個詞形真的有多長」為止。整份展開到固定深度
    // 是白做的：家族裡全是單字時，一個 n-gram 都不需要。
    let max_n = forms
        .iter()
        .map(|f| form_length(f, lang))
        .max()
        .unwrap_or(1);
    if max_n < 2 {
        return false;
    }
    ngrams(&tokens, lang, max_n)
        .iter()
        .any(|gram| forms.contains(gram))
}

/// 一個詞形佔幾個詞元。跟 [`ngrams`] 的拼法要一致，否則長度算錯，
/// n-gram 就展不到那個片語真正需要的深度。
fn form_length(form: &str, lang: &str) -> usize {
    if joins_with_space(lang) {
        form.split_whitespace().count().max(1)
    } else {
        form.chars().count().max(1)
    }
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

    /// 片語靠空格拼回去，但那個假設對中日文不成立。
    #[test]
    fn phrase_joining_follows_the_language() {
        assert!(joins_with_space("en"));
        assert!(joins_with_space("en-US"));
        assert!(joins_with_space("ko"), "韓文有分寫");
        assert!(!joins_with_space("ja"));
        assert!(!joins_with_space("zh-TW"));
    }

    #[test]
    fn ngrams_cover_every_window() {
        let tokens: Vec<String> = ["search", "for", "the", "key"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let grams = ngrams(&tokens, "en", 3);
        assert!(grams.contains(&"search for".to_string()));
        assert!(grams.contains(&"search for the".to_string()));
        assert!(grams.contains(&"for the key".to_string()));
        assert!(
            !grams.contains(&"search".to_string()),
            "單字不算片語，那條路徑另外處理"
        );
    }

    /// 中日文沒有空格，拼錯的話片語一個都查不到。
    #[test]
    fn japanese_ngrams_have_no_spaces() {
        let tokens: Vec<String> = ["気", "に", "する"].iter().map(|s| s.to_string()).collect();
        let grams = ngrams(&tokens, "ja", 3);
        assert!(grams.contains(&"気にする".to_string()), "{grams:?}");
    }

    #[test]
    fn ngrams_stop_at_the_token_count() {
        let tokens = vec!["one".to_string()];
        assert!(ngrams(&tokens, "en", 4).is_empty(), "一個詞組不成片語");
    }

    #[test]
    fn tokenize_drops_numbers_and_punctuation() {
        let t = tokenize("The cat sat on 3 mats -- really!");
        assert_eq!(t, vec!["the", "cat", "sat", "on", "mats", "really"]);
    }

    fn forms(list: &[&str]) -> std::collections::HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// 「這一題有沒有練到 run」不能靠字面比對：句子裡寫的是 `ran`。
    #[test]
    fn an_inflected_form_counts_as_a_mention() {
        let family = forms(&["run", "runs", "ran", "running"]);
        assert!(mentions_any("She ran to the station.", &family, "en"));
        assert!(!mentions_any("She walked to the station.", &family, "en"));
    }

    /// 標點與大小寫不該讓比對失敗——句尾的那個字最常是要練的字。
    #[test]
    fn punctuation_and_case_do_not_hide_a_mention() {
        assert!(mentions_any("Did you RUN?", &forms(&["run"]), "en"));
    }

    /// 片語只比單一詞元的話永遠對不上，而字典裡有 69 萬個多詞條目。
    #[test]
    fn a_multiword_entry_is_matched_across_tokens() {
        assert!(mentions_any(
            "I will search for the key.",
            &forms(&["search for"]),
            "en"
        ));
        assert!(!mentions_any(
            "I will search the room.",
            &forms(&["search for"]),
            "en"
        ));
    }

    /// 中日文的片語沒有空格，拼法跟 `ngrams` 一致才對得上。
    ///
    /// 順帶驗到一件不能用直覺假設的事：`tokenize` 對日文是**逐字**切的
    /// （`昨日は公園に…` → `昨` `日` `は` `公` `園` …），所以連
    /// 「公園」這種兩個字的普通名詞都得靠 n-gram 才比得到。
    #[test]
    fn a_japanese_phrase_is_matched_without_spaces() {
        assert!(mentions_any("それは気にする", &forms(&["気にする"]), "ja"));
        assert!(mentions_any(
            "昨日は公園に行きました",
            &forms(&["公園"]),
            "ja"
        ));
        assert!(!mentions_any(
            "昨日は海に行きました",
            &forms(&["公園"]),
            "ja"
        ));
    }

    /// 查不到詞形時回 `false`，呼叫端才有辦法分辨「沒用到」與「驗不了」——
    /// 把驗不了當成沒用到會讓只匯詞頻表的人每一題都被退回去重出。
    #[test]
    fn an_empty_family_never_matches() {
        assert!(!mentions_any("anything at all", &forms(&[]), "en"));
    }

    #[test]
    fn counts_tokens_and_types() {
        let (tokens, types) = token_type_counts("the cat and the hat");
        assert_eq!(tokens, 5);
        assert_eq!(types, 4);
    }
}
