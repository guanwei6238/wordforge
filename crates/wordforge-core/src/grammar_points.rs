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
//! 所以清單是受控的：prompt 明確列出可選項，收到的標籤再正規化一次。
//! 模型不聽話時我們仍然接得住。
//!
//! ## 清單本身不住在這裡
//!
//! **真正的清單存在資料庫的 `grammar_def` 表**，使用者可以匯入、可以編輯。
//! 這個模組只提供兩樣東西：英文那份**種子**（第一次啟動時寫進資料庫），
//! 以及**正規化的演算法**——後者要拿清單當參數，因為 `wordforge-core`
//! 不碰 I/O。
//!
//! 寫死一份清單曾經是這裡的做法，結果是學日文的人拿到空的，
//! 而且想加一個自己常錯的點沒有地方加。

/// 英文文法點的**種子**：`(識別碼, 中文名稱)`。
///
/// 第一次啟動時寫進 `grammar_def`，之後使用者就能編輯、刪除、增加。
/// 這裡不再是唯一的真相——只是「開箱就有東西可以學」的起點。
///
/// 挑選標準是「英文學習者真的會犯、而且值得單獨練」的錯誤類型。
/// 切太細（現在完成進行式 vs 過去完成式）會讓每個標籤都只有一兩筆紀錄，
/// 排程失去意義；切太粗（grammar）則練不到重點。
///
/// ## 排列方式
///
/// 照一般文法教材的順序分組（名詞與限定詞 → 動詞 → 情態與語氣 →
/// 句型 → 子句 → 非限定動詞 → 修飾與比較 → 搭配 → 書寫規範），
/// 陣列順序就是 `sort_order`。原本的排法沒有依據，文法頁讀起來
/// 像一份隨手列的清單，看不出先學什麼。
///
/// `level` 是 CEFR 等級。這個欄位資料表一直都有，只是從來沒填過——
/// 填了之後才有辦法說「這個點你現在還用不到」。等級是**大致**的：
/// 同一個點在不同教材會落在相鄰的級別，這裡取常見的那個。
///
/// ## 這份清單同時服務兩件事
///
/// 它既是**批改時的錯誤標籤**，也是**文法頁的學習主題**。兩者要的
/// 粒度不一樣：學習想要細（分詞構句該獨立成一課），標籤想要粗
/// （細了就每個標籤各錯一次，FSRS 排不動）。
///
/// 這一版往「學習」偏了一點——多出來的十四個點多半是 B1 以上，
/// 初學者本來就不太會踩到，所以對標籤稀釋的影響有限。真的稀釋得
/// 太嚴重的話，要退的是這裡而不是排程。
///
/// **這份種子只有英文。** 日文的助詞、法文的性數一致、西班牙文的虛擬式
/// 都不在這裡——那些語言請匯入或自己編輯，見 [`seed_for`]。
pub const ENGLISH_POINTS: &[(&str, &str, &str)] = &[
    // 名詞與限定詞
    ("articles", "冠詞 a / an / the", "A1"),
    ("plural", "名詞單複數", "A1"),
    ("countable-uncountable", "可數與不可數", "A2"),
    ("quantifiers", "數量詞 some / any / much / many", "A2"),
    ("pronouns", "代名詞", "A1"),
    ("possessives", "所有格", "A1"),
    // 動詞：時態與動貌
    ("subject-verb-agreement", "主詞動詞一致", "A1"),
    ("there-be", "there is / there are", "A1"),
    ("tense", "時態", "A1"),
    ("future-forms", "未來的表達 will / be going to", "A2"),
    ("used-to", "used to / would 表過去習慣", "A2"),
    ("aspect", "動貌（進行 / 完成）", "B1"),
    // 情態與語氣
    ("modals", "情態助動詞", "A2"),
    ("conditionals", "條件句", "B1"),
    ("subjunctive-wish", "假設語氣與 wish", "B2"),
    ("causative", "使役 have / get something done", "B2"),
    // 句型
    ("word-order", "語序", "A1"),
    ("question-formation", "疑問句", "A1"),
    ("negation", "否定句", "A1"),
    ("question-tags", "附加問句", "A2"),
    ("passive-voice", "被動語態", "B1"),
    ("inversion", "倒裝", "C1"),
    // 子句與連接
    ("conjunctions", "連接詞", "A2"),
    ("relative-clauses", "關係子句", "B1"),
    ("adverbial-clauses", "副詞子句（時間、原因、讓步）", "B1"),
    ("reported-speech", "間接引語", "B1"),
    ("noun-clauses", "名詞子句 that / whether / wh-", "B2"),
    ("participle-clauses", "分詞構句", "C1"),
    // 非限定動詞
    ("gerund-infinitive", "動名詞與不定詞", "B1"),
    // 修飾與比較
    ("comparatives", "比較級與最高級", "A2"),
    ("adjective-order", "形容詞排序", "B1"),
    ("adverb-placement", "副詞位置", "B1"),
    ("degree-result", "程度與結果 so / such / too / enough", "B1"),
    // 搭配
    ("prepositions", "介系詞", "A2"),
    ("phrasal-verbs", "片語動詞", "B1"),
    ("collocation", "搭配詞", "B2"),
    ("word-choice", "用字選擇", "B1"),
    // 書寫規範
    ("capitalization", "大小寫", "A1"),
    ("spelling", "拼字", "A1"),
    ("punctuation", "標點", "A2"),
];

/// 種子清單的版本。**改動 [`ENGLISH_POINTS`] 就要加一。**
///
/// `grammar_def` 只在第一次啟動時填，所以清單改了之後，早就用過的
/// 資料庫永遠看不到新的點——這個版號讓補齊只跑一次。
/// 只跑一次是重點：每次啟動都補的話，使用者刪掉的點會一直復活。
pub const SEED_VERSION: i64 = 2;

/// 某個語言的種子清單，第一次啟動時用來填 `grammar_def`。
///
/// 沒有種子的語言回傳空陣列，而**不是**硬套英文那一份——拿
/// `articles`、`gerund-infinitive` 去標日文的錯誤只會產生垃圾資料。
/// 那些語言開箱是空的，由使用者匯入或自己加。
pub fn seed_for(lang: &str) -> &'static [(&'static str, &'static str, &'static str)] {
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
    // 以下對應 SEED_VERSION 2 新增的點。沒有別名的話，模型照自己的
    // 習慣寫（"indirect speech"、"tag questions"）就會被丟掉——
    // normalize_point 認不出來時是回 None，那個錯誤不會有人發現。
    ("quantifier", "quantifiers"),
    ("determiners of quantity", "quantifiers"),
    ("much many", "quantifiers"),
    ("some any", "quantifiers"),
    ("there is", "there-be"),
    ("there are", "there-be"),
    ("existential there", "there-be"),
    ("future", "future-forms"),
    ("future tense forms", "future-forms"),
    ("will be going to", "future-forms"),
    ("past habits", "used-to"),
    ("used to", "used-to"),
    ("subjunctive", "subjunctive-wish"),
    ("wish", "subjunctive-wish"),
    ("unreal past", "subjunctive-wish"),
    ("causatives", "causative"),
    ("have something done", "causative"),
    ("tag questions", "question-tags"),
    ("tag question", "question-tags"),
    ("inverted word order", "inversion"),
    ("reported speech", "reported-speech"),
    ("indirect speech", "reported-speech"),
    ("reported statements", "reported-speech"),
    ("backshift", "reported-speech"),
    ("noun clause", "noun-clauses"),
    ("that clauses", "noun-clauses"),
    ("nominal clauses", "noun-clauses"),
    ("adverbial clause", "adverbial-clauses"),
    ("subordinate clauses", "adverbial-clauses"),
    ("subordination", "adverbial-clauses"),
    ("participle clause", "participle-clauses"),
    ("participial phrases", "participle-clauses"),
    ("reduced clauses", "participle-clauses"),
    ("adjective ordering", "adjective-order"),
    ("order of adjectives", "adjective-order"),
    ("so such", "degree-result"),
    ("too enough", "degree-result"),
    ("result clauses", "degree-result"),
];

/// 把模型回傳的標籤正規化到給定的受控清單。
///
/// `points` 是該語言目前的識別碼清單，由呼叫端從 `grammar_def` 取得——
/// `wordforge-core` 不碰 I/O，清單不可能住在這裡。
///
/// 清單非空時，認不出來的回傳 `None`：與其累積一堆各錯一次的垃圾標籤，
/// 不如丟掉。清單是空的（那個語言還沒有定義）時原樣保留——
/// 沒有收斂保證，但總比丟掉全部、或硬套英文分類好。
pub fn normalize_point(points: &[String], raw: &str) -> Option<String> {
    let cleaned = normalized_form(raw)?;

    if points.is_empty() {
        return Some(cleaned);
    }

    // 完全命中
    if let Some(id) = points.iter().find(|id| spaced(id) == cleaned) {
        return Some(id.clone());
    }

    // 已知的別名。**只認清單裡真的有的目標**——日文的清單不該因為
    // 別名表寫著 `past tense → tense` 就生出一個它沒有的 `tense`。
    if let Some((_, id)) = ALIASES
        .iter()
        .find(|(alias, id)| *alias == cleaned && points.iter().any(|p| p == id))
    {
        return Some((*id).to_string());
    }

    // 最後一招：包含某個識別碼的關鍵字。`past perfect tense` 這種
    // 組合說法靠這裡接住。長的優先，避免 `tense` 搶走
    // `subject-verb-agreement`。
    let mut hits: Vec<&String> = points
        .iter()
        .filter(|id| cleaned.contains(&spaced(id)))
        .collect();
    hits.sort_by_key(|id| std::cmp::Reverse(id.len()));
    if let Some(id) = hits.first() {
        return Some((*id).clone());
    }

    // 別名的關鍵字比對，同樣只認清單裡有的目標
    let mut alias_hits: Vec<&(&str, &str)> = ALIASES
        .iter()
        .filter(|(alias, id)| cleaned.contains(alias) && points.iter().any(|p| p == id))
        .collect();
    alias_hits.sort_by_key(|(alias, _)| std::cmp::Reverse(alias.len()));
    alias_hits.first().map(|(_, id)| (*id).to_string())
}

/// 比對用的統一形式：小寫、底線與斜線視為空白、連字號等同空白。
fn normalized_form(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c == '_' || c == '/' || c == '-' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (!cleaned.is_empty()).then_some(cleaned)
}

fn spaced(id: &str) -> String {
    id.replace(['-', '_'], " ")
}

/// 給 prompt 用的清單字串。
///
/// 直接列在 prompt 裡是最有效的手段：與其事後猜模型想說什麼，
/// 不如一開始就限制它只能從這些裡面挑。
///
/// 清單是空的時回傳 `None`，prompt 那邊會退回「請使用一致的術語」。
pub fn prompt_list(points: &[String]) -> Option<String> {
    (!points.is_empty()).then(|| points.join("、"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 英文的受控清單。實際使用時這份來自 `grammar_def` 資料表，
    /// 測試裡直接拿種子——內容一樣，只是省掉一個資料庫。
    fn english() -> Vec<String> {
        seed_for("en")
            .iter()
            .map(|(id, _, _)| id.to_string())
            .collect()
    }

    #[test]
    fn canonical_ids_pass_through() {
        for (id, _, _) in seed_for("en") {
            assert_eq!(
                normalize_point(&english(), id).as_deref(),
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
                normalize_point(&english(), raw).as_deref(),
                Some("tense"),
                "沒收斂：{raw}"
            );
        }
    }

    #[test]
    fn common_aliases_map_correctly() {
        assert_eq!(
            normalize_point(&english(), "article").as_deref(),
            Some("articles")
        );
        assert_eq!(
            normalize_point(&english(), "determiners").as_deref(),
            Some("articles")
        );
        assert_eq!(
            normalize_point(&english(), "subject-verb agreement").as_deref(),
            Some("subject-verb-agreement")
        );
        assert_eq!(
            normalize_point(&english(), "S-V Agreement").as_deref(),
            Some("subject-verb-agreement")
        );
        assert_eq!(
            normalize_point(&english(), "passive").as_deref(),
            Some("passive-voice")
        );
        assert_eq!(
            normalize_point(&english(), "gerund").as_deref(),
            Some("gerund-infinitive")
        );
        assert_eq!(
            normalize_point(&english(), "word order").as_deref(),
            Some("word-order")
        );
    }

    /// 較長的識別碼優先，否則 `subject-verb agreement` 會被 `agreement` 搶走。
    #[test]
    fn longer_matches_win() {
        assert_eq!(
            normalize_point(&english(), "error in subject-verb-agreement").as_deref(),
            Some("subject-verb-agreement")
        );
        // 同時命中兩個識別碼時取較長的那個。規則本身是任意的，
        // 但必須是確定的——否則同一句話兩次跑出不同結果。
        assert_eq!(
            normalize_point(&english(), "phrasal verbs and prepositions").as_deref(),
            Some("phrasal-verbs"),
        );
    }

    /// 認不出來的寧可丟掉，也不要累積一堆各錯一次的垃圾標籤。
    #[test]
    fn unknown_labels_are_rejected() {
        assert_eq!(normalize_point(&english(), ""), None);
        assert_eq!(normalize_point(&english(), "   "), None);
        assert_eq!(normalize_point(&english(), "這句話怪怪的"), None);
        assert_eq!(normalize_point(&english(), "style"), None);
    }

    /// 種子的每一項都要有母語名稱——那是使用者在文法頁上看到的字。
    #[test]
    fn every_seeded_point_has_a_name() {
        for (id, name, _) in seed_for("en") {
            assert!(!name.is_empty(), "{id} 沒有中文名稱");
        }
    }

    #[test]
    fn ids_are_unique_and_lowercase() {
        let mut seen = std::collections::HashSet::new();
        for (id, _, _) in seed_for("en") {
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
                seed_for("en").iter().any(|(id, _, _)| id == target),
                "別名 {alias} 指向不存在的 {target}"
            );
        }
    }

    /// 同一個語言的各種寫法都要認得：資料庫存代碼、prompt 用名稱。
    #[test]
    fn language_is_recognised_by_code_or_name() {
        for lang in ["en", "en-US", "English", "english", "英文"] {
            assert!(!seed_for(lang).is_empty(), "{lang} 應該對到英文的種子");
        }
        assert!(seed_for("ja").is_empty(), "日文沒有種子，開箱是空的");
        assert!(seed_for("").is_empty());
    }

    /// 沒有清單的語言不能硬套英文那一份。
    ///
    /// 拿 articles、gerund-infinitive 去標日文的錯誤只會產生垃圾資料。
    #[test]
    fn a_language_without_a_list_keeps_the_model_labels() {
        assert!(seed_for("ja").is_empty());
        assert_eq!(prompt_list(&[]), None, "沒有清單就不該假裝有");

        // 原樣保留（正規化大小寫），而不是丟掉或硬塞英文分類
        assert_eq!(
            normalize_point(&[], "助詞の使い方"),
            Some("助詞の使い方".to_string())
        );
        assert_eq!(
            normalize_point(&[], "  Particles  "),
            Some("particles".to_string())
        );
        assert_eq!(normalize_point(&[], "  "), None);

        // 英文的別名不該讓別的語言生出它沒有的識別碼
        let japanese = vec!["particles".to_string()];
        assert_eq!(
            normalize_point(&japanese, "past tense"),
            None,
            "別名表寫著 past tense → tense，但這份清單根本沒有 tense"
        );
    }

    #[test]
    fn prompt_list_contains_every_point() {
        let list = prompt_list(&english()).expect("英文要有清單");
        for (id, _, _) in seed_for("en") {
            assert!(list.contains(id), "prompt 清單漏了 {id}");
        }
    }
}

#[cfg(test)]
mod seed_v2_tests {
    use super::*;

    fn english() -> Vec<String> {
        seed_for("en")
            .iter()
            .map(|(id, _, _)| id.to_string())
            .collect()
    }

    /// 新增的點如果沒有別名，模型照自己的習慣寫就會被丟掉——
    /// `normalize_point` 認不出來時回 `None`，那個錯誤不會有人發現。
    #[test]
    fn a_model_writing_in_its_own_words_still_lands_on_the_new_points() {
        let points = english();
        for (raw, want) in [
            ("indirect speech", "reported-speech"),
            ("Reported Speech", "reported-speech"),
            ("tag questions", "question-tags"),
            ("quantifier", "quantifiers"),
            ("there is/there are", "there-be"),
            ("subjunctive", "subjunctive-wish"),
            ("wish", "subjunctive-wish"),
            ("participle clause", "participle-clauses"),
            ("noun clause", "noun-clauses"),
            ("adverbial clause", "adverbial-clauses"),
            ("order of adjectives", "adjective-order"),
            ("causatives", "causative"),
            ("used to", "used-to"),
        ] {
            assert_eq!(
                normalize_point(&points, raw).as_deref(),
                Some(want),
                "{raw} 該收斂到 {want}"
            );
        }
    }

    /// 等級只能是 CEFR 那六級。打錯字的話 UI 的分級篩選會多出一個
    /// 沒有人看得懂的分類，而那看起來完全正常。
    #[test]
    fn every_seed_level_is_a_real_cefr_band() {
        for (point, _, level) in seed_for("en") {
            assert!(
                ["A1", "A2", "B1", "B2", "C1", "C2"].contains(level),
                "{point} 的等級是 {level}"
            );
        }
    }
}
