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

/// 把一段文字切成句子。
///
/// 規則只有「句末標點 + 換行」，刻意不處理縮寫（`Mr.`、`e.g.`）：
/// 那需要語言相關的例外表，而切錯的代價只是某一句被斷成兩半——
/// 使用者看到的仍然是自己讀過的字，不是錯的翻譯。
///
/// 中英文的句末標點都認：文章是目標語言寫的，翻譯是母語寫的，
/// 這個函式兩邊都要用。
pub fn split_sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();

    for (i, c) in text.char_indices() {
        let ends = matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | '\n');
        if !ends {
            continue;
        }
        // 標點後面還黏著引號或右括號時一起收進來，不然會多出一句 `」`
        let mut end = i + c.len_utf8();
        while end < bytes.len() {
            let Some(next) = text[end..].chars().next() else {
                break;
            };
            if matches!(next, '"' | '\'' | '」' | '』' | '）' | ')' | '”') {
                end += next.len_utf8();
            } else {
                break;
            }
        }
        let piece = text[start..end].trim();
        if !piece.is_empty() {
            out.push(piece);
        }
        start = end;
    }

    let tail = text[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

/// 段落：空行分隔。
fn split_paragraphs(text: &str) -> Vec<&str> {
    text.split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

/// 把原文與它的翻譯逐句配對。
///
/// ## 為什麼要降級
///
/// 模型寫全文翻譯時會**合併或拆分**句子——那是好翻譯該做的事，中文常常
/// 把兩個英文短句併成一句才自然。實測四篇文章有三篇句數剛好對得上，
/// 一篇少一句。
///
/// 對不上的時候寧可退回**整段**也不要硬配：配錯的譯文看起來完全正常，
/// 只是講的是別句話——那是最難發現的一種壞法。段落也對不上就不給翻譯，
/// 句子本身仍然有用（那是他讀過的原文）。
pub fn align_sentences<'a>(
    source: &'a str,
    translation: &'a str,
) -> Vec<(&'a str, Option<String>)> {
    let src = split_sentences(source);
    if translation.trim().is_empty() {
        return src.into_iter().map(|s| (s, None)).collect();
    }

    let dst = split_sentences(translation);
    if src.len() == dst.len() {
        return src
            .into_iter()
            .zip(dst.into_iter().map(|d| Some(d.to_string())))
            .collect();
    }

    // 句數差一句就整段降級太粗糙：實測一篇 24 句的文章，模型只是把
    // 其中兩句併成一句翻，結果每一句拿到的都是整段譯文。
    // 先試比例對齊，它處理得了「一句對兩句」這種局部差異。
    if let Some(aligned) = align_by_length(&src, &dst) {
        return aligned;
    }

    // 對齊不出來（一邊是空的、或長到不值得算）才退到段落：
    // 同一段裡的每一句都配那一段的譯文。
    //
    // **至少要兩段**才算降級：整篇只有一段時，「那一段」就是整篇翻譯，
    // 把二十句的譯文掛在每一句底下不是幫忙，是把畫面塞滿。
    let src_paras = split_paragraphs(source);
    let dst_paras = split_paragraphs(translation);
    if src_paras.len() >= 2 && src_paras.len() == dst_paras.len() {
        let mut out = Vec::new();
        for (sp, dp) in src_paras.iter().zip(&dst_paras) {
            for sentence in split_sentences(sp) {
                out.push((sentence, Some((*dp).to_string())));
            }
        }
        return out;
    }

    src.into_iter().map(|s| (s, None)).collect()
}

/// DP 回溯用：(從哪一格來, 吃掉幾句原文, 吃掉幾句譯文)。
type Backtrack = Option<(usize, usize, usize, usize)>;

/// 對齊時最多處理幾句。超過就不算了——這是 O(n×m) 的動態規劃，
/// 而正常的文章不會有兩百句。
const MAX_ALIGN: usize = 200;

/// 合併兩句譯文時要不要加空格。中文句子之間加空格會多出一道縫，
/// 英文不加會黏成一個字。
const SKEW_PENALTY: f64 = 0.4;

/// 用「句子長度成比例」把兩邊對起來。
///
/// 這是 Gale–Church 對齊的簡化版：翻譯後的句子長度大致與原文成比例，
/// 所以把「哪一句對哪一句」變成一個最小成本路徑問題。允許的配對是
/// 1:1、1:2、2:1——那涵蓋了模型實際會做的事（把兩個短句併成一句翻、
/// 或把一個長句拆開）。1:2 與 2:1 帶一點懲罰，讓 1:1 在成本相近時勝出。
///
/// 回傳 `None` 代表算不出來（有一邊是空的，或長到超過 [`MAX_ALIGN`]）。
fn align_by_length<'a>(src: &[&'a str], dst: &[&str]) -> Option<Vec<(&'a str, Option<String>)>> {
    if src.is_empty() || dst.is_empty() || src.len() > MAX_ALIGN || dst.len() > MAX_ALIGN {
        return None;
    }

    // 中文譯文比英文原文短得多，所以比例要從整篇估，不能假設 1:1
    let total_src: usize = src.iter().map(|s| s.chars().count()).sum();
    let total_dst: usize = dst.iter().map(|s| s.chars().count()).sum();
    if total_src == 0 || total_dst == 0 {
        return None;
    }
    let ratio = total_dst as f64 / total_src as f64;
    let len_of = |s: &str| s.chars().count() as f64;

    let (n, m) = (src.len(), dst.len());
    let mut cost = vec![vec![f64::INFINITY; m + 1]; n + 1];
    // (前一格, 吃掉幾句原文, 吃掉幾句譯文)
    let mut from = vec![vec![None as Backtrack; m + 1]; n + 1];
    cost[0][0] = 0.0;

    for i in 0..=n {
        for j in 0..=m {
            if !cost[i][j].is_finite() {
                continue;
            }
            let here = cost[i][j];
            let relax = |di: usize,
                         dj: usize,
                         c: f64,
                         cost: &mut Vec<Vec<f64>>,
                         from: &mut Vec<Vec<Backtrack>>| {
                if i + di <= n && j + dj <= m && here + c < cost[i + di][j + dj] {
                    cost[i + di][j + dj] = here + c;
                    from[i + di][j + dj] = Some((i, j, di, dj));
                }
            };

            if i < n && j < m {
                let c = (len_of(src[i]) * ratio - len_of(dst[j])).abs();
                relax(1, 1, c, &mut cost, &mut from);
            }
            // 一句原文對兩句譯文
            if i < n && j + 1 < m {
                let want = len_of(src[i]) * ratio;
                let c = (want - len_of(dst[j]) - len_of(dst[j + 1])).abs() + SKEW_PENALTY * want;
                relax(1, 2, c, &mut cost, &mut from);
            }
            // 兩句原文對一句譯文
            if i + 1 < n && j < m {
                let want = (len_of(src[i]) + len_of(src[i + 1])) * ratio;
                let c = (want - len_of(dst[j])).abs() + SKEW_PENALTY * len_of(dst[j]);
                relax(2, 1, c, &mut cost, &mut from);
            }
        }
    }

    if !cost[n][m].is_finite() {
        return None;
    }

    let mut out = vec![None; n];
    let (mut i, mut j) = (n, m);
    while (i, j) != (0, 0) {
        let (pi, pj, di, dj) = from[i][j]?;
        let joined = join_sentences(&dst[pj..pj + dj]);
        for k in 0..di {
            out[pi + k] = Some((src[pi + k], joined.clone()));
        }
        (i, j) = (pi, pj);
    }

    Some(
        out.into_iter()
            .enumerate()
            .map(|(i, pair)| match pair {
                Some((s, t)) if !t.trim().is_empty() => (s, Some(t)),
                _ => (src[i], None),
            })
            .collect(),
    )
}

/// 把幾句譯文接成一句。中文之間不加空格，英文要加——
/// 不判斷的話不是多一道縫就是黏成一個字。
fn join_sentences(parts: &[&str]) -> String {
    let mut out = String::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let needs_space = out
            .chars()
            .last()
            .zip(part.chars().next())
            .is_some_and(|(a, b)| a.is_ascii() && b.is_ascii());
        if needs_space {
            out.push(' ');
        }
        out.push_str(part);
    }
    out
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
    fn sentences_split_on_terminal_punctuation() {
        let s = split_sentences("The dog barks. Then it stops! Why? Nobody knows");
        assert_eq!(
            s,
            vec!["The dog barks.", "Then it stops!", "Why?", "Nobody knows"]
        );
    }

    /// 中文的句號也要認：翻譯是母語寫的，同一個函式兩邊都要用。
    #[test]
    fn chinese_sentences_split_too() {
        let s = split_sentences("狗會吠。然後就停了！為什麼？");
        assert_eq!(s, vec!["狗會吠。", "然後就停了！", "為什麼？"]);
    }

    /// 收尾的引號要跟著上一句走，不然會多出一句只有引號的「句子」。
    #[test]
    fn a_closing_quote_stays_with_its_sentence() {
        assert_eq!(
            split_sentences("He said \"stop.\" Then he left."),
            vec!["He said \"stop.\"", "Then he left."]
        );
    }

    /// 對齊結果攤平成好比對的形狀。
    fn shown(pairs: &[(&str, Option<String>)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(s, t)| ((*s).to_string(), t.clone().unwrap_or_default()))
            .collect()
    }

    /// 句數對得上就逐句配對——這是四篇實測資料裡三篇的情況。
    #[test]
    fn matching_sentence_counts_pair_up_one_to_one() {
        let pairs = align_sentences("A dog barks. It stops.", "狗會吠。牠停了。");
        assert_eq!(
            shown(&pairs),
            vec![
                ("A dog barks.".to_string(), "狗會吠。".to_string()),
                ("It stops.".to_string(), "牠停了。".to_string()),
            ]
        );
    }

    /// 這條測試存在的理由是它曾經是錯的：模型把兩句併成一句翻，
    /// 句數就差一句，而差一句就整段降級——實測那篇 24 句的文章裡，
    /// `fall` 那句拿到的是整個第一段的譯文，看起來就像切錯了。
    ///
    /// 比例對齊處理得了這種局部差異：合併掉的那兩句共用同一句譯文，
    /// 其餘每一句仍然各自對到自己的。
    #[test]
    fn a_merged_translation_still_lines_up_sentence_by_sentence() {
        let source = "It was quiet inside. \
            I saw a small sign beside the plate. \
            It said the food came from a farm. \
            We shared a cake.";
        let translation = "裡面很安靜。\
            我在盤子旁看到一個小標誌，上面寫著食材來自農場。\
            我們分享了一塊蛋糕。";
        let pairs = align_sentences(source, translation);

        assert_eq!(pairs.len(), 4, "句子本身不該被合併掉：{pairs:?}");
        assert_eq!(pairs[0].1.as_deref(), Some("裡面很安靜。"));
        assert_eq!(pairs[3].1.as_deref(), Some("我們分享了一塊蛋糕。"));
        // 併在一起翻的那兩句共用同一句譯文，但**不是**整段
        assert_eq!(pairs[1].1, pairs[2].1);
        assert!(
            pairs[1].1.as_deref().is_some_and(|t| t.contains("標誌")),
            "{:?}",
            pairs[1].1
        );
    }

    /// 這條測試存在的理由：模型翻譯時會把兩句併成一句，那是好翻譯該做的事。
    /// 硬配的話第二句會配到第三句的譯文——看起來完全正常，只是講的是別句話。
    /// 退回整段比配錯好。
    #[test]
    fn mismatched_counts_fall_back_to_the_paragraph() {
        let source = "A dog barks. It stops.\n\nHe left. She stayed.";
        let translation = "狗會吠，然後停了。\n\n他走了。她留下。";
        let pairs = align_sentences(source, translation);

        assert_eq!(pairs.len(), 4, "句子本身不該被合併掉：{pairs:?}");
        assert_eq!(pairs[0].0, "A dog barks.");
        assert_eq!(pairs[1].0, "It stops.");
        // 前兩句併成一句翻，所以共用同一句譯文
        assert_eq!(pairs[0].1, pairs[1].1);
        assert!(pairs[0].1.as_deref().is_some_and(|t| t.contains("狗會吠")));
    }

    /// 段落也對不上就不給翻譯。句子本身仍然有用——那是他讀過的原文。
    #[test]
    fn a_hopeless_mismatch_still_yields_the_sentences() {
        let pairs = align_sentences("A. B. C.", "只有一句");
        assert_eq!(pairs.len(), 3);
        assert!(pairs.iter().all(|(_, t)| t.is_none()));
    }

    #[test]
    fn no_translation_means_no_pairing() {
        let pairs = align_sentences("A dog barks.", "   ");
        assert_eq!(pairs, vec![("A dog barks.", None)]);
    }

    /// 中文之間不加空格，英文要加——不判斷的話不是多一道縫就是黏成一個字。
    #[test]
    fn joined_translations_respect_the_script() {
        assert_eq!(
            join_sentences(&["他走了。", "她留下。"]),
            "他走了。她留下。"
        );
        assert_eq!(
            join_sentences(&["He left.", "She stayed."]),
            "He left. She stayed."
        );
    }

    #[test]
    fn counts_tokens_and_types() {
        let (tokens, types) = token_type_counts("the cat and the hat");
        assert_eq!(tokens, 5);
        assert_eq!(types, 4);
    }
}
