//! 決定「現在該出什麼題」。
//!
//! 背單字只是打底，真正把字變成能用的東西要靠產出：翻譯、閱讀、寫作。
//! 但題型不能亂選——只會 200 個字的人拿到一篇文章只會挫折，
//! 會 5000 字的人一直做單句翻譯則太無聊。
//!
//! 這個模組是純函數，決策規則看得見也測得到；實際呼叫 LLM 的部分在
//! `wordforge-practice`。

use serde::{Deserialize, Serialize};

/// 練習題型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExerciseKind {
    /// 中翻英：給一個中文句子，寫出英文
    TranslationToTarget,
    /// 英翻中：給一個英文句子，寫出中文
    TranslationToNative,
    /// 克漏字：短文挖空，選填正確的字
    Cloze,
    /// 閱讀測驗：一篇文章加選擇題
    Reading,
    /// 文法練習：針對犯過的錯出題
    Grammar,
}

impl ExerciseKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExerciseKind::TranslationToTarget => "translation_to_target",
            ExerciseKind::TranslationToNative => "translation_to_native",
            ExerciseKind::Cloze => "cloze",
            ExerciseKind::Reading => "reading",
            ExerciseKind::Grammar => "grammar",
        }
    }

    /// 這個題型需要多少詞彙量才有意義。
    pub fn min_vocabulary(&self) -> i64 {
        match self {
            // 英翻中最寬容：看不懂的字可以猜，而且是被動理解
            ExerciseKind::TranslationToNative => 0,
            // 中翻英要自己產出，但一句話的門檻不高
            ExerciseKind::TranslationToTarget => 100,
            ExerciseKind::Cloze => 300,
            ExerciseKind::Grammar => 300,
            // 一篇文章至少要看得懂九成才不會變成查字典
            ExerciseKind::Reading => 1_500,
        }
    }
}

/// 出題時需要知道的學習者狀態。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearnerProfile {
    /// 估計詞彙量。分級測驗的結果優先，沒有就用已掌握的卡片數。
    pub vocabulary: i64,
    /// 最近犯錯的文法點，由批改結果累積
    pub weak_grammar: Vec<String>,
    /// 已經做過幾次練習，用來避免一直出同一種題型
    pub recent_kinds: Vec<ExerciseKind>,
}

/// 每做幾題就穿插一次文法題（前提是有弱點紀錄）。
const GRAMMAR_EVERY: usize = 4;

/// 選出現在該做的題型。
///
/// 規則刻意簡單，因為使用者要看得懂為什麼給他這一題：
///
/// 1. 累積了文法弱點，而且有一陣子沒練文法了 → 出文法題
/// 2. 否則挑「詞彙量撐得起的最高階題型」
/// 3. 但避開上一題剛做過的，免得連續五題都是同一種
pub fn recommend_kind(profile: &LearnerProfile) -> ExerciseKind {
    let recent_grammar = profile
        .recent_kinds
        .iter()
        .rev()
        .take(GRAMMAR_EVERY)
        .any(|k| *k == ExerciseKind::Grammar);

    if !profile.weak_grammar.is_empty()
        && !recent_grammar
        && profile.vocabulary >= ExerciseKind::Grammar.min_vocabulary()
    {
        return ExerciseKind::Grammar;
    }

    let mut affordable: Vec<ExerciseKind> = [
        ExerciseKind::Reading,
        ExerciseKind::Cloze,
        ExerciseKind::TranslationToTarget,
        ExerciseKind::TranslationToNative,
    ]
    .into_iter()
    .filter(|k| profile.vocabulary >= k.min_vocabulary())
    .collect();

    // 上一題做過的排到最後，但如果只有一種選擇就還是給它
    if let Some(last) = profile.recent_kinds.last()
        && affordable.len() > 1
    {
        affordable.retain(|k| k != last);
    }

    affordable
        .first()
        .copied()
        // 詞彙量掛零時也要有東西可做
        .unwrap_or(ExerciseKind::TranslationToNative)
}

/// 閱讀文章該多長。
///
/// 詞彙量越大能讀越長，但上限壓在 400 字：再長就不是「一次練習」，
/// 而是一件會被拖延的事。
pub fn reading_length(vocabulary: i64) -> usize {
    match vocabulary {
        v if v < 2_000 => 120,
        v if v < 4_000 => 200,
        v if v < 8_000 => 300,
        _ => 400,
    }
}

/// 情境主題池。
///
/// 沒有指定主題時，模型會自己挑——而它挑出來的永遠是那幾個
/// （學校生活、天氣、旅行）。同樣的字彙樣本加同樣的主題，
/// 出來的文章會越來越像。
///
/// 主題本身也是學習內容：同一個字在點餐、看醫生、談工作時的用法不同，
/// 換情境等於多練一次。
pub const TOPICS: &[&str] = &[
    "日常對話：問路、點餐、購物",
    "校園生活：課程、考試、社團",
    "職場：面試、開會、寫信",
    "旅行：訂房、機場、迷路",
    "健康：看醫生、描述症狀、運動習慣",
    "科技：手機、網路、人工智慧",
    "環境：氣候、回收、動物保育",
    "飲食：食譜、餐廳評論、飲食習慣",
    "娛樂：電影、音樂、遊戲",
    "人際關係：朋友、家庭、衝突",
    "新聞事件：報導一則虛構但合理的地方新聞",
    "說明文：解釋一個日常現象為什麼會發生",
];

/// 挑一個主題，避開最近用過的。
///
/// `recent` 是最近幾次用過的主題（新的在後）。用 `seed` 決定從哪裡開始挑，
/// 讓同一批候選不會每次都給同一個——呼叫端傳練習次數或時間戳即可。
pub fn pick_topic(recent: &[String], seed: u64) -> &'static str {
    let available: Vec<&&str> = TOPICS
        .iter()
        .filter(|t| !recent.iter().any(|r| r == **t))
        .collect();

    // 全部都用過就重新開始，總不能沒有主題
    if available.is_empty() {
        return TOPICS[(seed as usize) % TOPICS.len()];
    }
    available[(seed as usize) % available.len()]
}

/// 一個可以拿來當「這篇要教的新詞」的候選。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewWord {
    pub lemma_id: i64,
    pub text: String,
    /// 這個字可以當哪些詞性。字典常給多個（`backward` 是 adj/adv/noun/verb）。
    pub pos: Vec<String>,
    pub freq_rank: i64,
}

/// 挑新詞時想要的詞性配比。
///
/// 不指定的話，照詞頻挑出來的清單會嚴重偏名詞——詞頻表前段的實詞
/// 大多是名詞，而學習者需要的是能組成句子的各種詞類。動詞尤其重要，
/// 一篇文章沒有新動詞就只是在換主題，句型不會有變化。
///
/// 這個樣式會循環使用：要 14 個字的話就跑兩輪多。
pub const DESIRED_POS: &[&str] = &["verb", "noun", "adj", "verb", "noun", "adv"];

/// 從候選裡挑出詞性分散、難度也分散的新詞。
///
/// ## 兩個要顧的維度
///
/// **詞性**：一個字可以有多個詞性，所以這是個指派問題。依 `desired`
/// 的順序（不夠長就循環），每一格挑一個具備該詞性的字。
///
/// **難度**：候選是照詞頻排序的，直接取前 N 個會全部擠在同一小段。
/// 實測要 14 個字時挑出來的全落在 rank 5201～5217——讀起來像在背
/// 同一頁單字表。所以把候選切成 `budget` 段，第 k 格優先從第 k 段挑，
/// 找不到才放寬到整個池子。
///
/// 兩者都湊不齊時用剩下的補滿：生詞太少會讓文章太簡單，那才是
/// 真正要避免的事。
pub fn balance_by_pos(candidates: &[NewWord], desired: &[&str], budget: usize) -> Vec<NewWord> {
    if budget == 0 || candidates.is_empty() || desired.is_empty() {
        return Vec::new();
    }

    let mut sorted: Vec<&NewWord> = candidates.iter().collect();
    sorted.sort_by_key(|c| (c.freq_rank, c.lemma_id));

    let mut picked: Vec<NewWord> = Vec::new();
    let mut used: std::collections::HashSet<i64> = std::collections::HashSet::new();

    // 每一格對應候選池的一段，讓選出來的字難度分散開
    let slice = sorted.len().div_ceil(budget).max(1);

    for slot in 0..budget {
        let want = desired[slot % desired.len()];
        let from = slot * slice;

        // 先在自己那一段裡找，找不到再放寬到整個池子
        let in_slice = sorted
            .iter()
            .skip(from)
            .take(slice)
            .find(|c| !used.contains(&c.lemma_id) && c.pos.iter().any(|p| p == want));
        let anywhere = || {
            sorted
                .iter()
                .find(|c| !used.contains(&c.lemma_id) && c.pos.iter().any(|p| p == want))
        };

        if let Some(c) = in_slice.or_else(anywhere) {
            used.insert(c.lemma_id);
            picked.push((*c).clone());
        }
    }

    // 詞性湊不齊時用剩下的補滿，常用的優先
    if picked.len() < budget {
        let rest: Vec<&NewWord> = sorted
            .iter()
            .filter(|c| !used.contains(&c.lemma_id))
            .take(budget - picked.len())
            .copied()
            .collect();
        picked.extend(rest.into_iter().cloned());
    }

    picked
}

/// 一次翻譯練習出幾題。
pub fn translation_count(vocabulary: i64) -> usize {
    if vocabulary < 500 { 3 } else { 5 }
}

/// 翻譯題的選字配比：幾題考今天到期的、幾題從學過的字裡隨機抽。
///
/// 回傳 `(到期, 隨機)`，兩者相加等於 `count`。
///
/// ## 為什麼不能全部用到期的字
///
/// 到期的順序是**固定的**（`ORDER BY due`），所以一天之內連出幾份翻譯題，
/// 每份都拿到同一批字。使用者的原話是「翻譯用的單字都是相同的」。
/// 這跟生詞選詞曾經完全決定性是同一個毛病。
///
/// 留一部分給隨機抽的舊字還有第二個好處：那些字 FSRS 認為已經穩了、
/// 短期內不會再排到，但「以為學會了」正是最需要抽查的狀態。
/// 這條路徑不動排程，只是拿來造句。
pub fn translation_mix(count: usize) -> (usize, usize) {
    // 3:2。整數運算避免浮點；count = 5 → 3，count = 3 → 2。
    //
    // 上界 `count - 1` 是為了保證兩題以上時隨機那邊一定拿得到名額：
    // 純算比例的話 count = 2 會四捨五入成 2/0，等於這個機制沒開。
    let due = (count * 3)
        .div_ceil(5)
        .min(count.saturating_sub(1))
        .max(1)
        .min(count);
    (due, count - due)
}

// ------------------------------------------------------------------ 克漏字

/// 克漏字挖空的標記。模型照這個格式寫，`{{1}}` 是第一格。
///
/// 用雙大括號而不是底線或 `___`：文章裡本來就可能有破折號或底線，
/// 而且要知道「這一格是第幾題」才對得回選項。
pub const BLANK_OPEN: &str = "{{";
pub const BLANK_CLOSE: &str = "}}";

/// 找出文章裡的挖空編號，依出現順序。
///
/// 模型常常漏掉一格、或編號從 0 開始、或跳號。呼叫端要拿這個結果去
/// 對照題目數量——挖了三格卻給五題選項的話，多出來的兩題永遠答不到，
/// 而使用者只會看到「怎麼有兩題沒有空格」。
pub fn blank_numbers(passage: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = passage;

    while let Some(start) = rest.find(BLANK_OPEN) {
        let after = &rest[start + BLANK_OPEN.len()..];
        let Some(end) = after.find(BLANK_CLOSE) else {
            break;
        };
        if let Ok(n) = after[..end].trim().parse::<usize>() {
            out.push(n);
        }
        rest = &after[end + BLANK_CLOSE.len()..];
    }
    out
}

/// 把挖空重新編號成「出現順序」，並算出題目該怎麼跟著重排。
///
/// ## 為什麼要這一步
///
/// 模型很常給出 `[2, 5, 1, 3, 4, 7, 6, 8]` 這種順序——編號本身沒有
/// 少也沒有重複，只是**沒有照文章順序寫**。那樣讀起來是：第一個空格
/// 標著 2、第二個標著 5，而右邊的題目卻是 1、2、3…，作答時要一直
/// 來回找對應。
///
/// 這件事在本地修得掉，所以不要只在 prompt 裡拜託它——照出現順序
/// 重新編號，題目跟著同樣的排列重排，結果一定對得上。
///
/// 回傳 `(改寫後的文章, 題目的新順序)`。`order[k]` 是新的第 k 格
/// 對應**原本**的第幾題。編號超出範圍或重複的空格會被移除：
/// 留著會變成一個永遠答不到的洞，而 `{{9}}` 直接印在文章裡更糟。
pub fn renumber_blanks(passage: &str, item_count: usize) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(passage.len());
    let mut order: Vec<usize> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut rest = passage;

    while let Some(start) = rest.find(BLANK_OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + BLANK_OPEN.len()..];

        let Some(end) = after.find(BLANK_CLOSE) else {
            // 沒收尾的標記：原樣留著，不要吃掉後面的內容
            out.push_str(&rest[start..]);
            return (out, order);
        };

        match after[..end].trim().parse::<usize>() {
            Ok(n) if (1..=item_count).contains(&n) && seen.insert(n) => {
                out.push_str(&format!("{BLANK_OPEN}{}{BLANK_CLOSE}", order.len() + 1));
                order.push(n - 1);
            }
            // 編號壞掉或重複，整個標記拿掉
            _ => {}
        }

        rest = &after[end + BLANK_CLOSE.len()..];
    }

    out.push_str(rest);
    (out, order)
}

// ------------------------------------------------------------------ 選項洗牌

/// 偽亂數（SplitMix64）。
///
/// 用 seed 驅動而不是拉 `rand`：`wordforge-core` 不碰 I/O 也不碰時鐘，
/// 亂源由呼叫端傳進來。同一個 seed 給同一串，所以測試跑得動，
/// 而實際使用時 seed 來自時間戳，每次出題都不一樣。
fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 原地 Fisher-Yates。
fn shuffle_in_place<T>(items: &mut [T], state: &mut u64) {
    for i in (1..items.len()).rev() {
        let j = (next_u64(state) % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// 洗牌後的順序：`order[k]` 是新的第 k 個位置原本放的是第幾個。
///
/// 回傳**排列**而不是洗好的陣列，因為一個選項身上不只掛著文字——
/// 還有「這個選項為什麼不對」的解說。兩個平行陣列必須用同一個排列
/// 搬動，只搬其中一個的話每個選項會配到別人的解說，
/// 而那個畫面看起來完全合理，不會有人發現。
///
/// 每題各自獨立、位置均勻隨機。不去安排「這份四題要剛好用掉四個
/// 位置」：那種安排本身就是一種規律，而且會讓前面幾題的答案位置
/// 洩漏後面的。
pub fn shuffle_order(len: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    if len < 2 {
        return order;
    }
    let mut state = seed | 1;
    shuffle_in_place(&mut order, &mut state);
    order
}

/// 依 `order` 重排一個陣列。長度對不上時原樣回傳——
/// 那代表資料已經壞了，硬搬只會把錯誤藏得更深。
pub fn reorder<T: Clone>(items: &[T], order: &[usize]) -> Vec<T> {
    if order.len() != items.len() || order.iter().any(|&i| i >= items.len()) {
        return items.to_vec();
    }
    order.iter().map(|&i| items[i].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn learner(vocabulary: i64) -> LearnerProfile {
        LearnerProfile {
            vocabulary,
            weak_grammar: Vec::new(),
            recent_kinds: Vec::new(),
        }
    }

    /// 剛起步的人只能做「看得懂就好」的題型。
    #[test]
    fn a_beginner_gets_translation_not_an_article() {
        assert_eq!(
            recommend_kind(&learner(50)),
            ExerciseKind::TranslationToNative
        );
        assert_eq!(
            recommend_kind(&learner(200)),
            ExerciseKind::TranslationToTarget
        );
    }

    /// 詞彙量夠了才給閱讀測驗——看不懂九成的文章只會變成查字典。
    #[test]
    fn reading_needs_enough_vocabulary() {
        assert_ne!(recommend_kind(&learner(1_000)), ExerciseKind::Reading);
        assert_eq!(recommend_kind(&learner(2_000)), ExerciseKind::Reading);
        assert_eq!(recommend_kind(&learner(6_000)), ExerciseKind::Reading);
    }

    /// 有文法弱點就該練，但不能每題都練文法。
    #[test]
    fn grammar_is_interleaved_not_repeated() {
        let mut p = learner(3_000);
        p.weak_grammar = vec!["past tense".into()];
        assert_eq!(recommend_kind(&p), ExerciseKind::Grammar);

        // 剛做過文法題就換別的
        p.recent_kinds = vec![ExerciseKind::Grammar];
        assert_ne!(recommend_kind(&p), ExerciseKind::Grammar);

        // 隔了幾題之後又輪到文法
        p.recent_kinds = vec![
            ExerciseKind::Grammar,
            ExerciseKind::Reading,
            ExerciseKind::Cloze,
            ExerciseKind::Reading,
            ExerciseKind::Cloze,
        ];
        assert_eq!(recommend_kind(&p), ExerciseKind::Grammar);
    }

    /// 沒有弱點紀錄就不要硬出文法題。
    #[test]
    fn no_grammar_without_recorded_mistakes() {
        assert_ne!(recommend_kind(&learner(5_000)), ExerciseKind::Grammar);
    }

    /// 連續做同一種題型會膩。
    #[test]
    fn avoids_repeating_the_last_kind() {
        let mut p = learner(5_000);
        p.recent_kinds = vec![ExerciseKind::Reading];
        assert_ne!(recommend_kind(&p), ExerciseKind::Reading);
    }

    /// 只剩一種題型可選時，重複也得給——總不能沒題目。
    #[test]
    fn repeats_when_there_is_no_alternative() {
        let mut p = learner(10);
        p.recent_kinds = vec![ExerciseKind::TranslationToNative];
        assert_eq!(
            recommend_kind(&p),
            ExerciseKind::TranslationToNative,
            "詞彙量太低時只有這一種選擇"
        );
    }

    #[test]
    fn article_length_grows_with_vocabulary_but_stays_bounded() {
        assert_eq!(reading_length(1_500), 120);
        assert_eq!(reading_length(3_000), 200);
        assert_eq!(reading_length(50_000), 400, "再長就不會有人做完");
        assert!(reading_length(0) < reading_length(10_000));
    }

    /// 同樣的主題重複出現，文章會越來越像。
    #[test]
    fn topics_rotate_away_from_recent_ones() {
        let recent: Vec<String> = TOPICS[..3].iter().map(|t| t.to_string()).collect();

        for seed in 0..20 {
            let picked = pick_topic(&recent, seed);
            assert!(
                !recent.iter().any(|r| r == picked),
                "挑到了最近用過的主題：{picked}"
            );
        }
    }

    /// 不同的 seed 要挑到不同主題，否則等於沒輪換。
    #[test]
    fn different_seeds_give_different_topics() {
        let picked: std::collections::HashSet<&str> = (0..TOPICS.len() as u64)
            .map(|s| pick_topic(&[], s))
            .collect();
        assert!(
            picked.len() > TOPICS.len() / 2,
            "輪換不夠分散，只出現 {} 種",
            picked.len()
        );
    }

    /// 全部主題都用過之後不能卡住。
    #[test]
    fn exhausting_every_topic_starts_over() {
        let all: Vec<String> = TOPICS.iter().map(|t| t.to_string()).collect();
        let picked = pick_topic(&all, 5);
        assert!(TOPICS.contains(&picked));
    }

    fn word(id: i64, text: &str, pos: &[&str], rank: i64) -> NewWord {
        NewWord {
            lemma_id: id,
            text: text.into(),
            pos: pos.iter().map(|p| p.to_string()).collect(),
            freq_rank: rank,
        }
    }

    /// 照詞頻挑會挑出一整排名詞，那樣的文章句型不會有變化。
    #[test]
    fn new_words_span_several_parts_of_speech() {
        let candidates = vec![
            word(1, "hierarchy", &["noun"], 5206),
            word(2, "appetite", &["noun"], 5205),
            word(3, "offend", &["verb"], 5207),
            word(4, "sympathetic", &["adj"], 5209),
            word(5, "hostility", &["noun"], 5211),
            word(6, "infect", &["verb"], 5200),
        ];

        let picked = balance_by_pos(&candidates, DESIRED_POS, 4);
        let kinds: std::collections::HashSet<&str> = picked
            .iter()
            .flat_map(|w| w.pos.iter().map(|p| p.as_str()))
            .collect();

        assert_eq!(picked.len(), 4);
        assert!(kinds.contains("verb"), "沒有動詞：{picked:?}");
        assert!(kinds.contains("noun"));
        assert!(kinds.contains("adj"));
    }

    /// 同一個字不能被選兩次，即使它符合多個詞性格子。
    #[test]
    fn a_word_is_never_picked_twice() {
        let candidates = vec![
            word(1, "backward", &["adj", "adv", "noun", "verb"], 100),
            word(2, "offend", &["verb"], 200),
            word(3, "appetite", &["noun"], 300),
        ];

        let picked = balance_by_pos(&candidates, DESIRED_POS, 3);
        let ids: std::collections::HashSet<i64> = picked.iter().map(|w| w.lemma_id).collect();
        assert_eq!(ids.len(), 3, "重複選了同一個字：{picked:?}");
    }

    /// 要的字比詞性樣式長時，樣式要循環，不能後面全變成照詞頻挑。
    #[test]
    fn the_pos_pattern_repeats_for_larger_budgets() {
        let mut candidates = Vec::new();
        for i in 0..40i64 {
            let pos = match i % 4 {
                0 => "verb",
                1 => "noun",
                2 => "adj",
                _ => "adv",
            };
            candidates.push(word(i + 1, &format!("w{i}"), &[pos], 5_000 + i));
        }

        let picked = balance_by_pos(&candidates, DESIRED_POS, 12);
        let verbs = picked.iter().filter(|w| w.pos[0] == "verb").count();
        assert_eq!(picked.len(), 12);
        assert!(verbs >= 3, "循環之後動詞還是要夠多：{picked:?}");
    }

    /// 全部擠在同一小段的話，讀起來像在背同一頁單字表。
    #[test]
    fn picks_spread_across_the_difficulty_range() {
        let mut candidates = Vec::new();
        for i in 0..100i64 {
            candidates.push(word(
                i + 1,
                &format!("w{i}"),
                &["noun", "verb"],
                5_000 + i * 10,
            ));
        }

        let picked = balance_by_pos(&candidates, DESIRED_POS, 10);
        let ranks: Vec<i64> = picked.iter().map(|w| w.freq_rank).collect();
        let spread = ranks.iter().max().unwrap() - ranks.iter().min().unwrap();

        assert_eq!(picked.len(), 10);
        assert!(
            spread > 500,
            "難度沒有分散，全落在 {spread} 的範圍內：{ranks:?}"
        );
    }

    /// 詞性湊不齊時要補滿——生詞太少會讓文章太簡單，那才是真正的問題。
    #[test]
    fn a_short_on_variety_pool_still_fills_the_budget() {
        let candidates = vec![
            word(1, "hierarchy", &["noun"], 10),
            word(2, "appetite", &["noun"], 20),
            word(3, "hostility", &["noun"], 30),
        ];

        let picked = balance_by_pos(&candidates, DESIRED_POS, 3);
        assert_eq!(picked.len(), 3, "寧可全是名詞也不要少給");
    }

    #[test]
    fn an_empty_pool_yields_nothing() {
        assert!(balance_by_pos(&[], DESIRED_POS, 5).is_empty());
    }

    /// 常用的字優先——學習者比較可能再遇到。
    #[test]
    fn the_more_common_word_wins_within_a_part_of_speech() {
        let candidates = vec![
            word(1, "rare_verb", &["verb"], 9000),
            word(2, "common_verb", &["verb"], 5200),
        ];
        let picked = balance_by_pos(&candidates, &["verb"], 1);
        assert_eq!(picked[0].text, "common_verb");
    }

    #[test]
    fn beginners_get_fewer_translation_items() {
        assert_eq!(translation_count(100), 3);
        assert_eq!(translation_count(3_000), 5);
    }

    /// 這條測試存在的理由是翻譯題曾經全部用到期的字，而到期的順序是固定的
    /// （`ORDER BY due`），所以連出幾份會拿到一模一樣的單字。
    #[test]
    fn some_translation_items_come_from_words_that_are_not_due() {
        assert_eq!(translation_mix(5), (3, 2));
        assert_eq!(translation_mix(3), (2, 1));

        // 不管幾題，兩邊都要有——全給其中一邊就等於沒有這個機制
        for count in 1..=20 {
            let (due, extra) = translation_mix(count);
            assert_eq!(due + extra, count, "count = {count}");
            if count >= 2 {
                assert!(due > 0 && extra > 0, "count = {count} 時是 {due}/{extra}");
            }
        }
    }

    #[test]
    fn blanks_are_found_in_order() {
        assert_eq!(blank_numbers("I {{1}} to the {{2}} yesterday."), vec![1, 2]);
        assert_eq!(blank_numbers("沒有空格"), Vec::<usize>::new());
    }

    /// 模型會跳號、會漏掉、會多給。這裡只負責誠實回報看到什麼，
    /// 要不要接受由呼叫端決定——自己偷偷補號會讓選項對到錯的空格。
    #[test]
    fn broken_numbering_is_reported_as_is() {
        assert_eq!(blank_numbers("a {{1}} b {{3}} c"), vec![1, 3]);
        assert_eq!(blank_numbers("a {{2}} b {{1}}"), vec![2, 1]);
        assert_eq!(blank_numbers("a {{ 1 }} b"), vec![1], "容忍空白");
    }

    /// 沒收尾的標記不能讓解析卡住或吃掉後面的內容。
    #[test]
    fn an_unclosed_blank_does_not_hang() {
        assert_eq!(blank_numbers("a {{1}} b {{2"), vec![1]);
        assert_eq!(blank_numbers("{{}}"), Vec::<usize>::new());
        assert_eq!(blank_numbers("{{abc}} {{2}}"), vec![2], "非數字略過");
    }

    /// 這條測試存在的理由是它真的發生過：模型回了
    /// `numbers=[2, 5, 1, 3, 4, 7, 6, 8]`——編號沒少也沒重複，就只是
    /// 沒照文章順序寫。使用者看到的是第一個空格標著 2、第二個標著 5，
    /// 而右邊的題目是 1、2、3…，作答時要一直來回找對應。
    #[test]
    fn out_of_order_blanks_are_renumbered_to_reading_order() {
        let passage = "a {{2}} b {{5}} c {{1}} d {{3}}";
        let (rewritten, order) = renumber_blanks(passage, 5);

        assert_eq!(rewritten, "a {{1}} b {{2}} c {{3}} d {{4}}");
        assert_eq!(
            order,
            vec![1, 4, 0, 2],
            "新的第 k 格要指回原本的第幾題（0-based）"
        );
        assert_eq!(blank_numbers(&rewritten), vec![1, 2, 3, 4]);
    }

    /// 已經是順的就不該被動到。
    #[test]
    fn well_numbered_blanks_pass_through_unchanged() {
        let passage = "I {{1}} to the {{2}} yesterday.";
        let (rewritten, order) = renumber_blanks(passage, 2);
        assert_eq!(rewritten, passage);
        assert_eq!(order, vec![0, 1]);
    }

    /// 超出題數、重複、非數字的標記留著會變成永遠答不到的洞，
    /// 而把 `{{9}}` 直接印在文章裡更糟。
    #[test]
    fn unusable_blanks_are_removed_from_the_text() {
        let (rewritten, order) = renumber_blanks("a {{9}} b {{1}} c {{1}} d {{x}} e", 2);
        assert_eq!(rewritten, "a  b {{1}} c  d  e");
        assert_eq!(order, vec![0]);
    }

    /// 沒收尾的標記不能吃掉後面的文章。
    #[test]
    fn renumbering_keeps_the_tail_after_an_unclosed_blank() {
        let (rewritten, order) = renumber_blanks("a {{1}} b {{2 還有後面", 2);
        assert_eq!(rewritten, "a {{1}} b {{2 還有後面");
        assert_eq!(order, vec![0]);
    }

    fn options(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    /// 洗牌不能弄丟或弄壞答案，也不能把平行陣列拆散。
    ///
    /// 選項身上不只掛著文字，還有「這個選項為什麼不對」的解說。
    /// 兩者用同一個排列搬動，配錯的話畫面看起來完全合理，沒有人會發現。
    #[test]
    fn one_order_moves_the_options_and_their_notes_together() {
        let opts = options(&["went", "go", "gone", "going"]);
        let notes = options(&["對", "現在式", "過去分詞", "進行式"]);

        for seed in 0..200u64 {
            let order = shuffle_order(opts.len(), seed);
            let moved = reorder(&opts, &order);
            let moved_notes = reorder(&notes, &order);

            for (option, note) in moved.iter().zip(&moved_notes) {
                let original = opts.iter().position(|o| o == option).unwrap();
                assert_eq!(note, &notes[original], "seed {seed}：解說配到別的選項了");
            }

            let mut sorted = moved.clone();
            sorted.sort();
            let mut original = opts.clone();
            original.sort();
            assert_eq!(sorted, original, "選項被弄丟或重複了");
        }
    }

    /// 長度對不上時原樣回傳。硬搬只會把壞掉的資料藏得更深。
    #[test]
    fn a_mismatched_order_leaves_the_array_alone() {
        let opts = options(&["a", "b", "c"]);
        assert_eq!(reorder(&opts, &[0, 1]), opts);
        assert_eq!(reorder(&opts, &[0, 1, 9]), opts);
    }

    /// 這條測試存在的理由：模型出的選擇題答案會集中在某一個位置
    /// （實際看到過一整份都是第一個），使用者不用讀題就會猜。
    ///
    /// 驗的是分布而不是「每一份剛好用掉四個位置」——刻意去湊那種均勻
    /// 本身也是一種規律，而且會讓前面幾題的答案洩漏後面的。
    #[test]
    fn the_answer_position_is_uniformly_random() {
        let mut counts = [0usize; 4];
        for seed in 0..4_000u64 {
            // 正確答案原本在第 0 個，看它被搬到哪裡
            let order = shuffle_order(4, seed);
            counts[order.iter().position(|&i| i == 0).unwrap()] += 1;
        }

        // 期望值 1000，標準差約 27。±30% 的寬容遠大於正常波動，
        // 但小到足以抓出「偏心」或「根本沒洗」。
        for (position, n) in counts.iter().enumerate() {
            assert!(
                (700..=1_300).contains(n),
                "位置 {position} 出現 {n} 次，分布歪掉了：{counts:?}"
            );
        }
    }

    /// 只有一個選項的話沒得洗，但也不能 panic。
    #[test]
    fn a_degenerate_option_list_is_left_alone() {
        assert_eq!(shuffle_order(0, 3), Vec::<usize>::new());
        assert_eq!(shuffle_order(1, 3), vec![0]);
    }
}
