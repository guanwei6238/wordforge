//! 文法點的受控詞彙表。
//!
//! ## 為什麼不能讓模型自由命名
//!
//! 批改 prompt 原本只說「請使用一致的英文術語」，然後給一個範例 `tense`。
//! 實際跑起來會拿到 `tense`、`past tense`、`verb tense`、`Tenses`、
//! `verb-tense`——它們是同一件事，卻各自變成獨立的文法點，
//! 各有各的 FSRS 排程。結果是：
//!
//! - 「最常錯的文法點」被稀釋成五個各錯一次的標籤
//! - 每個標籤各自從頭開始排程，練了也不會收斂
//! - 換一個模型（sonnet → codex）用詞又換一套
//!
//! 所以這裡定義一份固定清單：prompt 明確列出可選項，
//! 收到的標籤再正規化一次。模型不聽話時我們仍然接得住。

/// 受控的文法點清單：`(識別碼, 中文名稱)`。
///
/// 挑選標準是「英文學習者真的會犯、而且值得單獨練」的錯誤類型。
/// 切太細（現在完成進行式 vs 過去完成式）會讓每個標籤都只有一兩筆紀錄，
/// 排程失去意義；切太粗（grammar）則練不到重點。
pub const GRAMMAR_POINTS: &[(&str, &str)] = &[
    ("tense", "時態"),
    ("aspect", "動貌（進行 / 完成）"),
    ("subject-verb-agreement", "主詞動詞一致"),
    ("articles", "冠詞 a / an / the"),
    ("plural", "單複數"),
    ("countable-uncountable", "可數與不可數"),
    ("prepositions", "介系詞"),
    ("pronouns", "代名詞"),
    ("possessives", "所有格"),
    ("word-order", "語序"),
    ("question-formation", "疑問句"),
    ("negation", "否定句"),
    ("modals", "情態助動詞"),
    ("conditionals", "條件句"),
    ("passive-voice", "被動語態"),
    ("gerund-infinitive", "動名詞與不定詞"),
    ("relative-clauses", "關係子句"),
    ("conjunctions", "連接詞"),
    ("comparatives", "比較級與最高級"),
    ("adverb-placement", "副詞位置"),
    ("phrasal-verbs", "片語動詞"),
    ("collocation", "搭配詞"),
    ("word-choice", "用字選擇"),
    ("punctuation", "標點"),
    ("capitalization", "大小寫"),
    ("spelling", "拼字"),
];

/// 別名：模型實際會吐出來的各種說法 → 受控識別碼。
///
/// 這份表是「已知模型會這樣寫」的清單，不是窮舉。
/// 沒收錄的會走後面的關鍵字比對。
const ALIASES: &[(&str, &str)] = &[
    ("verb tense", "tense"),
    ("past tense", "tense"),
    ("present tense", "tense"),
    ("future tense", "tense"),
    ("tenses", "tense"),
    ("verb form", "tense"),
    ("perfect", "aspect"),
    ("progressive", "aspect"),
    ("continuous", "aspect"),
    ("agreement", "subject-verb-agreement"),
    ("sv agreement", "subject-verb-agreement"),
    ("s-v agreement", "subject-verb-agreement"),
    ("concord", "subject-verb-agreement"),
    ("article", "articles"),
    ("determiners", "articles"),
    ("determiner", "articles"),
    ("plurals", "plural"),
    ("number", "plural"),
    ("singular plural", "plural"),
    ("countability", "countable-uncountable"),
    ("preposition", "prepositions"),
    ("pronoun", "pronouns"),
    ("possessive", "possessives"),
    ("genitive", "possessives"),
    ("word order", "word-order"),
    ("syntax", "word-order"),
    ("questions", "question-formation"),
    ("interrogative", "question-formation"),
    ("negatives", "negation"),
    ("modal verbs", "modals"),
    ("modal", "modals"),
    ("conditional", "conditionals"),
    ("if clauses", "conditionals"),
    ("passive", "passive-voice"),
    ("voice", "passive-voice"),
    ("gerund", "gerund-infinitive"),
    ("infinitive", "gerund-infinitive"),
    ("verb patterns", "gerund-infinitive"),
    ("relative clause", "relative-clauses"),
    ("relative pronouns", "relative-clauses"),
    ("conjunction", "conjunctions"),
    ("linking words", "conjunctions"),
    ("comparative", "comparatives"),
    ("superlative", "comparatives"),
    ("comparison", "comparatives"),
    ("adverbs", "adverb-placement"),
    ("adverb", "adverb-placement"),
    ("phrasal verb", "phrasal-verbs"),
    ("collocations", "collocation"),
    ("vocabulary choice", "word-choice"),
    ("word usage", "word-choice"),
    ("diction", "word-choice"),
    ("lexical choice", "word-choice"),
];

/// 把模型回傳的標籤正規化到受控清單。
///
/// 認不出來的回傳 `None`——與其累積一堆各錯一次的垃圾標籤，
/// 不如丟掉。清單夠完整的話這種情況很少見。
pub fn normalize_point(raw: &str) -> Option<&'static str> {
    let cleaned: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c == '_' || c == '/' { ' ' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }

    // 連字號與空白視為同一種分隔，比對時統一成空白
    let spaced = cleaned.replace('-', " ");
    let spaced = spaced.split_whitespace().collect::<Vec<_>>().join(" ");

    // 完全命中受控清單
    if let Some((id, _)) = GRAMMAR_POINTS
        .iter()
        .find(|(id, _)| id.replace('-', " ") == spaced)
    {
        return Some(id);
    }

    // 已知的別名
    if let Some((_, id)) = ALIASES.iter().find(|(alias, _)| *alias == spaced) {
        return Some(id);
    }

    // 最後一招：包含某個識別碼的關鍵字。
    // `past perfect tense` 這種組合說法靠這裡接住。
    // 用最長的識別碼優先，避免 `tense` 搶走 `subject-verb-agreement`。
    let mut candidates: Vec<&(&str, &str)> = GRAMMAR_POINTS
        .iter()
        .filter(|(id, _)| spaced.contains(&id.replace('-', " ")))
        .collect();
    candidates.sort_by_key(|(id, _)| std::cmp::Reverse(id.len()));
    if let Some((id, _)) = candidates.first() {
        return Some(id);
    }

    // 別名的關鍵字比對，同樣長的優先
    let mut alias_hits: Vec<&(&str, &str)> = ALIASES
        .iter()
        .filter(|(alias, _)| spaced.contains(alias))
        .collect();
    alias_hits.sort_by_key(|(alias, _)| std::cmp::Reverse(alias.len()));
    alias_hits.first().map(|(_, id)| *id)
}

/// 受控識別碼的中文名稱，供 UI 顯示。
pub fn display_name(id: &str) -> Option<&'static str> {
    GRAMMAR_POINTS
        .iter()
        .find(|(point, _)| *point == id)
        .map(|(_, name)| *name)
}

/// 給 prompt 用的清單字串。
///
/// 直接列在 prompt 裡是最有效的手段：與其事後猜模型想說什麼，
/// 不如一開始就限制它只能從這些裡面挑。
pub fn prompt_list() -> String {
    GRAMMAR_POINTS
        .iter()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>()
        .join("、")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_ids_pass_through() {
        for (id, _) in GRAMMAR_POINTS {
            assert_eq!(normalize_point(id), Some(*id), "{id} 應該原樣通過");
        }
    }

    /// 這些是模型實際會吐出來的各種寫法，全部要收斂到同一個識別碼。
    #[test]
    fn the_many_ways_a_model_says_tense() {
        for raw in [
            "tense",
            "Tense",
            "TENSE",
            "  tense  ",
            "tenses",
            "verb tense",
            "past tense",
            "Past Tense",
            "past-tense",
            "verb_tense",
            "past perfect tense",
        ] {
            assert_eq!(normalize_point(raw), Some("tense"), "沒收斂：{raw}");
        }
    }

    #[test]
    fn common_aliases_map_correctly() {
        assert_eq!(normalize_point("article"), Some("articles"));
        assert_eq!(normalize_point("determiners"), Some("articles"));
        assert_eq!(
            normalize_point("subject-verb agreement"),
            Some("subject-verb-agreement")
        );
        assert_eq!(
            normalize_point("S-V Agreement"),
            Some("subject-verb-agreement")
        );
        assert_eq!(normalize_point("passive"), Some("passive-voice"));
        assert_eq!(normalize_point("gerund"), Some("gerund-infinitive"));
        assert_eq!(normalize_point("word order"), Some("word-order"));
    }

    /// 較長的識別碼優先，否則 `subject-verb agreement` 會被 `agreement` 搶走。
    #[test]
    fn longer_matches_win() {
        assert_eq!(
            normalize_point("error in subject-verb-agreement"),
            Some("subject-verb-agreement")
        );
        // 同時命中兩個識別碼時取較長的那個。規則本身是任意的，
        // 但必須是確定的——否則同一句話兩次跑出不同結果。
        assert_eq!(
            normalize_point("phrasal verbs and prepositions"),
            Some("phrasal-verbs"),
        );
    }

    /// 認不出來的寧可丟掉，也不要累積一堆各錯一次的垃圾標籤。
    #[test]
    fn unknown_labels_are_rejected() {
        assert_eq!(normalize_point(""), None);
        assert_eq!(normalize_point("   "), None);
        assert_eq!(normalize_point("這句話怪怪的"), None);
        assert_eq!(normalize_point("style"), None);
    }

    #[test]
    fn every_point_has_a_chinese_name() {
        for (id, name) in GRAMMAR_POINTS {
            assert!(!name.is_empty(), "{id} 沒有中文名稱");
            assert_eq!(display_name(id), Some(*name));
        }
        assert_eq!(display_name("not-a-point"), None);
    }

    #[test]
    fn ids_are_unique_and_lowercase() {
        let mut seen = std::collections::HashSet::new();
        for (id, _) in GRAMMAR_POINTS {
            assert!(seen.insert(*id), "重複的識別碼：{id}");
            assert_eq!(*id, id.to_lowercase(), "識別碼要小寫：{id}");
            assert!(!id.contains(' '), "識別碼用連字號不用空白：{id}");
        }
    }

    /// 別名不能指向不存在的識別碼。
    #[test]
    fn aliases_point_at_real_ids() {
        for (alias, target) in ALIASES {
            assert!(
                GRAMMAR_POINTS.iter().any(|(id, _)| id == target),
                "別名 {alias} 指向不存在的 {target}"
            );
        }
    }

    #[test]
    fn prompt_list_contains_every_point() {
        let list = prompt_list();
        for (id, _) in GRAMMAR_POINTS {
            assert!(list.contains(id), "prompt 清單漏了 {id}");
        }
    }
}
