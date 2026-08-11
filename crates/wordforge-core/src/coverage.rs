//! 可理解輸入（comprehensible input）的量化。
//!
//! 「90% 法則」的實務版本：一篇文章要能靠上下文推敲、又真的學到東西，
//! 已知詞的**詞元覆蓋率**大約要落在 95%~98%（延伸閱讀）或 90%~95%（精讀）。
//! 覆蓋率太高等於在讀已經會的東西，太低則會退化成查字典。
//!
//! 這個模組負責兩件事：
//! 1. 給一篇文章，算出覆蓋率並判定難度帶（產生後的驗收）
//! 2. 給目標長度與目標覆蓋率，算出可以塞幾個生詞、該塞哪幾個（產生前的規劃）

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::model::LemmaId;

/// 難度帶。以已知詞覆蓋率判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageBand {
    /// > 98%：幾乎沒有新東西，適合流暢度練習，但學不到新詞
    TooEasy,
    /// 95%~98%：延伸閱讀的甜蜜點，不查字典也能讀懂
    Optimal,
    /// 90%~95%：精讀區間，需要一點輔助（詞彙表、翻譯）
    Challenging,
    /// < 90%：閱讀會斷裂成查字典，不建議
    TooHard,
}

impl CoverageBand {
    pub fn from_ratio(ratio: f64) -> Self {
        if ratio > 0.98 {
            CoverageBand::TooEasy
        } else if ratio >= 0.95 {
            CoverageBand::Optimal
        } else if ratio >= 0.90 {
            CoverageBand::Challenging
        } else {
            CoverageBand::TooHard
        }
    }

    /// 這個難度帶適不適合直接拿給學習者閱讀。
    pub fn is_usable(self) -> bool {
        matches!(self, CoverageBand::Optimal | CoverageBand::Challenging)
    }
}

/// 一篇文章相對於某個學習者詞彙量的覆蓋率分析。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    /// 詞元總數（重複的算多次）
    pub total_tokens: usize,
    /// 已知詞元數
    pub known_tokens: usize,
    /// 未知的相異詞及其出現次數，依出現次數由多到少排序
    pub unknown_types: Vec<(String, usize)>,
}

impl Coverage {
    pub fn unknown_tokens(&self) -> usize {
        self.total_tokens.saturating_sub(self.known_tokens)
    }

    /// 已知詞覆蓋率，0.0~1.0。空文本回傳 0.0。
    pub fn ratio(&self) -> f64 {
        if self.total_tokens == 0 {
            return 0.0;
        }
        self.known_tokens as f64 / self.total_tokens as f64
    }

    pub fn band(&self) -> CoverageBand {
        CoverageBand::from_ratio(self.ratio())
    }
}

/// 分析一段文本對特定學習者的覆蓋率。
///
/// `is_known` 判斷一個表面形學習者懂不懂。呼叫端要負責詞形還原
/// （`running`、`ran` 都該算成學過 `run`），查不到的詞一律視為未知。
///
/// 這裡收的是「懂不懂」而不是「對到哪個 lemma」，因為詞形還原有歧義
/// （`saw` 可以是 see 的過去式，也可以是「鋸子」），而判斷懂不懂
/// 不需要解決那個歧義。詳見 `lemmas::family`。
pub fn analyze<F>(text: &str, is_known: F) -> Coverage
where
    F: Fn(&str) -> bool,
{
    let tokens = crate::text::tokenize(text);
    let mut known_tokens = 0usize;
    let mut unknown: HashMap<String, usize> = HashMap::new();

    for token in &tokens {
        if is_known(token) {
            known_tokens += 1;
        } else {
            *unknown.entry(token.clone()).or_insert(0) += 1;
        }
    }

    let mut unknown_types: Vec<(String, usize)> = unknown.into_iter().collect();
    // 出現次數多的生詞優先讓使用者處理；次數相同時用字母序，確保結果可重現
    unknown_types.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    Coverage {
        total_tokens: tokens.len(),
        known_tokens,
        unknown_types,
    }
}

/// 產生文章「之前」的規劃：這個長度、這個目標覆蓋率下，最多能放幾個生詞詞元。
///
/// 例：300 字、目標 96% → 12 個生詞詞元。若每個生詞平均出現 2 次
/// （刻意重複讓學習者從上下文歸納），代表大約可以帶入 6 個新詞。
pub fn unknown_token_budget(total_words: usize, target_ratio: f64) -> usize {
    let allowed = (1.0 - target_ratio.clamp(0.0, 1.0)) * total_words as f64;
    allowed.floor().max(0.0) as usize
}

/// 由生詞詞元預算換算成「可以帶入幾個新詞」。
///
/// `repeats_per_word` 是每個新詞預計在文章中出現幾次；重複出現才有機會
/// 讓學習者從不同上下文推敲出意思，一般設 2~3。
pub fn new_word_budget(total_words: usize, target_ratio: f64, repeats_per_word: usize) -> usize {
    let repeats = repeats_per_word.max(1);
    unknown_token_budget(total_words, target_ratio) / repeats
}

/// 待帶入的候選新詞。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub lemma_id: LemmaId,
    pub text: String,
    /// 詞頻排名，數字越小越常用。未知時給 `None`，排序時會排在最後。
    pub freq_rank: Option<u32>,
    /// 是否來自使用者指定的教材（課本單字優先）
    pub from_material: bool,
}

/// 挑出這篇文章要教的目標新詞。
///
/// 排序原則：
/// 1. 教材指定的單字優先（使用者匯入課本就是為了考那些字）
/// 2. 其次依詞頻，先學常用的字投資報酬率最高
/// 3. 已經會的字直接排除
pub fn select_target_words(
    candidates: &[Candidate],
    known: &HashSet<LemmaId>,
    budget: usize,
) -> Vec<Candidate> {
    let mut pool: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| !known.contains(&c.lemma_id))
        .collect();

    pool.sort_by(|a, b| {
        b.from_material
            .cmp(&a.from_material)
            .then_with(|| {
                a.freq_rank
                    .unwrap_or(u32::MAX)
                    .cmp(&b.freq_rank.unwrap_or(u32::MAX))
            })
            .then_with(|| a.text.cmp(&b.text))
    });

    pool.into_iter().take(budget).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known_set(ids: &[i64]) -> HashSet<LemmaId> {
        ids.iter().copied().map(LemmaId).collect()
    }

    /// 會的字清單，模擬呼叫端做完詞形還原之後的結果。
    fn knows<'a>(words: &'a [&'static str]) -> impl Fn(&str) -> bool + 'a {
        move |w: &str| words.contains(&w)
    }

    #[test]
    fn coverage_counts_known_and_unknown() {
        let cov = analyze("The cat sat on the mat", knows(&["the", "cat", "sat"]));

        assert_eq!(cov.total_tokens, 6);
        assert_eq!(cov.known_tokens, 4); // the, cat, sat, the
        assert_eq!(cov.unknown_tokens(), 2); // on, mat
        assert!((cov.ratio() - 4.0 / 6.0).abs() < 1e-9);
    }

    /// 詞形變化算不算會，由呼叫端的還原決定；這裡確認 analyze 忠實反映它。
    #[test]
    fn inflections_count_as_known_when_the_caller_says_so() {
        let cov = analyze("He ran and she runs", |w| {
            matches!(w, "he" | "she" | "and" | "ran" | "runs")
        });
        assert_eq!(cov.known_tokens, 5);
        assert_eq!(cov.ratio(), 1.0);
    }

    #[test]
    fn unknown_types_sorted_by_frequency() {
        let cov = analyze("zebra apple apple apple zebra kiwi", |_| false);
        assert_eq!(
            cov.unknown_types,
            vec![
                ("apple".to_string(), 3),
                ("zebra".to_string(), 2),
                ("kiwi".to_string(), 1),
            ]
        );
    }

    #[test]
    fn band_boundaries() {
        assert_eq!(CoverageBand::from_ratio(0.99), CoverageBand::TooEasy);
        assert_eq!(CoverageBand::from_ratio(0.98), CoverageBand::Optimal);
        assert_eq!(CoverageBand::from_ratio(0.95), CoverageBand::Optimal);
        assert_eq!(CoverageBand::from_ratio(0.94), CoverageBand::Challenging);
        assert_eq!(CoverageBand::from_ratio(0.90), CoverageBand::Challenging);
        assert_eq!(CoverageBand::from_ratio(0.89), CoverageBand::TooHard);
        assert!(!CoverageBand::from_ratio(0.5).is_usable());
        assert!(CoverageBand::from_ratio(0.96).is_usable());
    }

    #[test]
    fn empty_text_is_not_usable() {
        let cov = analyze("", |_| false);
        assert_eq!(cov.total_tokens, 0);
        assert_eq!(cov.ratio(), 0.0);
        assert_eq!(cov.band(), CoverageBand::TooHard);
    }

    #[test]
    fn budget_matches_target_ratio() {
        assert_eq!(unknown_token_budget(300, 0.96), 12);
        assert_eq!(unknown_token_budget(200, 0.95), 10);
        assert_eq!(unknown_token_budget(100, 1.0), 0);
        assert_eq!(new_word_budget(300, 0.96, 2), 6);
        assert_eq!(new_word_budget(300, 0.96, 0), 12); // repeats 0 視為 1，不能除以零
    }

    #[test]
    fn target_selection_prefers_material_then_frequency() {
        let candidates = vec![
            Candidate {
                lemma_id: LemmaId(1),
                text: "common".into(),
                freq_rank: Some(100),
                from_material: false,
            },
            Candidate {
                lemma_id: LemmaId(2),
                text: "rare".into(),
                freq_rank: Some(50_000),
                from_material: false,
            },
            Candidate {
                lemma_id: LemmaId(3),
                text: "textbook".into(),
                freq_rank: Some(20_000),
                from_material: true,
            },
            Candidate {
                lemma_id: LemmaId(4),
                text: "mastered".into(),
                freq_rank: Some(1),
                from_material: true,
            },
            Candidate {
                lemma_id: LemmaId(5),
                text: "unranked".into(),
                freq_rank: None,
                from_material: false,
            },
        ];
        let known = known_set(&[4]);

        let picked = select_target_words(&candidates, &known, 3);
        let texts: Vec<&str> = picked.iter().map(|c| c.text.as_str()).collect();

        // mastered 已經會 → 排除；textbook 來自教材 → 第一；其餘照詞頻
        assert_eq!(texts, vec!["textbook", "common", "rare"]);
    }

    #[test]
    fn target_selection_respects_budget_and_empty_pool() {
        let candidates = vec![Candidate {
            lemma_id: LemmaId(1),
            text: "only".into(),
            freq_rank: Some(1),
            from_material: false,
        }];
        assert_eq!(
            select_target_words(&candidates, &known_set(&[]), 0).len(),
            0
        );
        assert_eq!(
            select_target_words(&candidates, &known_set(&[1]), 5).len(),
            0
        );
        assert_eq!(select_target_words(&[], &known_set(&[]), 5).len(), 0);
    }
}
