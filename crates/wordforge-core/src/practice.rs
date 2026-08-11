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

/// 一次翻譯練習出幾題。
pub fn translation_count(vocabulary: i64) -> usize {
    if vocabulary < 500 { 3 } else { 5 }
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

    #[test]
    fn beginners_get_fewer_translation_items() {
        assert_eq!(translation_count(100), 3);
        assert_eq!(translation_count(3_000), 5);
    }
}
