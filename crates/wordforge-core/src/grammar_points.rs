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

/// 英文的受控文法點清單：`(識別碼, 中文名稱)`。
///
/// 挑選標準是「英文學習者真的會犯、而且值得單獨練」的錯誤類型。
/// 切太細（現在完成進行式 vs 過去完成式）會讓每個標籤都只有一兩筆紀錄，
/// 排程失去意義；切太粗（grammar）則練不到重點。
///
/// **這份清單只適用英文。** 日文的助詞、法文的性數一致、西班牙文的
/// 虛擬式，都不在這裡面——那些語言需要各自的清單，
/// 見 [`points_for`] 對未支援語言的處理方式。
pub const ENGLISH_POINTS: &[(&str, &str)] = &[
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

/// 某個語言的受控文法點清單。
///
/// 沒有清單的語言回傳空陣列，而不是硬套英文的那一份——
/// 拿 `articles`、`gerund-infinitive` 去標日文的錯誤只會產生垃圾資料。
/// 這種情況下 [`normalize_point`] 會原樣保留模型給的標籤：
/// 沒有收斂保證，但至少能用，也不會把錯的分類強加上去。
pub fn points_for(lang: &str) -> &'static [(&'static str, &'static str)] {
    match language_key(lang) {
        Some("en") => ENGLISH_POINTS,
        _ => &[],
    }
}

/// 把各種寫法的語言標示收斂成代碼。
///
/// 同一個語言在系統裡有好幾種寫法：資料庫存 BCP 47 代碼（`en`、`en-US`），
/// prompt 用的是給模型看的名稱（`English`）。兩邊都會走到這裡，
/// 與其要求每個呼叫端自己轉換，不如在這裡認得寬一點。
fn language_key(lang: &str) -> Option<&'static str> {
    let l = lang.trim().to_lowercase();
    if l.starts_with("en") || l == "英文" || l == "英語" {
        return Some("en");
    }
    None
}

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

/// 把模型回傳的標籤正規化到該語言的受控清單。
///
/// 有清單的語言（目前只有英文）：認不出來的回傳 `None`——
/// 與其累積一堆各錯一次的垃圾標籤，不如丟掉。
///
/// 沒有清單的語言：原樣保留（小寫、去空白）。沒有收斂保證，
/// 但總比丟掉全部、或硬套英文分類好。
pub fn normalize_point(lang: &str, raw: &str) -> Option<String> {
    let points = points_for(lang);
    if points.is_empty() {
        let cleaned = raw.trim().to_lowercase();
        return (!cleaned.is_empty()).then_some(cleaned);
    }
    normalize_english(raw).map(str::to_string)
}

/// 英文專用的正規化。
fn normalize_english(raw: &str) -> Option<&'static str> {
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
    if let Some((id, _)) = ENGLISH_POINTS
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
    let mut candidates: Vec<&(&str, &str)> = ENGLISH_POINTS
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

/// 受控識別碼的中文名稱，供 UI 顯示。沒有清單的語言回傳 `None`。
pub fn display_name(lang: &str, id: &str) -> Option<&'static str> {
    points_for(lang)
        .iter()
        .find(|(point, _)| *point == id)
        .map(|(_, name)| *name)
}

/// 給 prompt 用的清單字串。
///
/// 直接列在 prompt 裡是最有效的手段：與其事後猜模型想說什麼，
/// 不如一開始就限制它只能從這些裡面挑。
///
/// 沒有清單的語言回傳 `None`，prompt 那邊會退回「請使用一致的術語」。
pub fn prompt_list(lang: &str) -> Option<String> {
    let points = points_for(lang);
    (!points.is_empty()).then(|| {
        points
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>()
            .join("、")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_ids_pass_through() {
        for (id, _) in ENGLISH_POINTS {
            assert_eq!(
                normalize_point("en", id).as_deref(),
                Some(*id),
                "{id} 應該原樣通過"
            );
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
            assert_eq!(
                normalize_point("en", raw).as_deref(),
                Some("tense"),
                "沒收斂：{raw}"
            );
        }
    }

    #[test]
    fn common_aliases_map_correctly() {
        assert_eq!(
            normalize_point("en", "article").as_deref(),
            Some("articles")
        );
        assert_eq!(
            normalize_point("en", "determiners").as_deref(),
            Some("articles")
        );
        assert_eq!(
            normalize_point("en", "subject-verb agreement").as_deref(),
            Some("subject-verb-agreement")
        );
        assert_eq!(
            normalize_point("en", "S-V Agreement").as_deref(),
            Some("subject-verb-agreement")
        );
        assert_eq!(
            normalize_point("en", "passive").as_deref(),
            Some("passive-voice")
        );
        assert_eq!(
            normalize_point("en", "gerund").as_deref(),
            Some("gerund-infinitive")
        );
        assert_eq!(
            normalize_point("en", "word order").as_deref(),
            Some("word-order")
        );
    }

    /// 較長的識別碼優先，否則 `subject-verb agreement` 會被 `agreement` 搶走。
    #[test]
    fn longer_matches_win() {
        assert_eq!(
            normalize_point("en", "error in subject-verb-agreement").as_deref(),
            Some("subject-verb-agreement")
        );
        // 同時命中兩個識別碼時取較長的那個。規則本身是任意的，
        // 但必須是確定的——否則同一句話兩次跑出不同結果。
        assert_eq!(
            normalize_point("en", "phrasal verbs and prepositions").as_deref(),
            Some("phrasal-verbs"),
        );
    }

    /// 認不出來的寧可丟掉，也不要累積一堆各錯一次的垃圾標籤。
    #[test]
    fn unknown_labels_are_rejected() {
        assert_eq!(normalize_point("en", ""), None);
        assert_eq!(normalize_point("en", "   "), None);
        assert_eq!(normalize_point("en", "這句話怪怪的"), None);
        assert_eq!(normalize_point("en", "style"), None);
    }

    #[test]
    fn every_point_has_a_chinese_name() {
        for (id, name) in ENGLISH_POINTS {
            assert!(!name.is_empty(), "{id} 沒有中文名稱");
            assert_eq!(display_name("en", id), Some(*name));
        }
        assert_eq!(display_name("en", "not-a-point"), None);
    }

    #[test]
    fn ids_are_unique_and_lowercase() {
        let mut seen = std::collections::HashSet::new();
        for (id, _) in ENGLISH_POINTS {
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
                ENGLISH_POINTS.iter().any(|(id, _)| id == target),
                "別名 {alias} 指向不存在的 {target}"
            );
        }
    }

    /// 同一個語言的各種寫法都要認得：資料庫存代碼、prompt 用名稱。
    #[test]
    fn language_is_recognised_by_code_or_name() {
        for lang in ["en", "en-US", "English", "english", "英文"] {
            assert!(!points_for(lang).is_empty(), "{lang} 應該對到英文的清單");
        }
        assert!(points_for("ja").is_empty());
        assert!(points_for("").is_empty());
    }

    /// 沒有清單的語言不能硬套英文那一份。
    ///
    /// 拿 articles、gerund-infinitive 去標日文的錯誤只會產生垃圾資料。
    #[test]
    fn unsupported_languages_keep_the_model_labels() {
        assert!(points_for("ja").is_empty());
        assert_eq!(prompt_list("ja"), None, "沒有清單就不該假裝有");

        // 原樣保留（正規化大小寫），而不是丟掉或硬塞英文分類
        assert_eq!(
            normalize_point("ja", "助詞の使い方"),
            Some("助詞の使い方".to_string())
        );
        assert_eq!(
            normalize_point("ja", "  Particles  "),
            Some("particles".to_string())
        );
        assert_eq!(normalize_point("ja", "  "), None);

        // 英文的分類不該外洩到其他語言
        assert_eq!(display_name("ja", "tense"), None);
    }

    #[test]
    fn prompt_list_contains_every_point() {
        let list = prompt_list("en").expect("英文要有清單");
        for (id, _) in ENGLISH_POINTS {
            assert!(list.contains(id), "prompt 清單漏了 {id}");
        }
    }
}
