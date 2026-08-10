//! 分級測驗：估計學習者的詞彙量，決定新卡從哪裡開始。
//!
//! ## 為什麼需要
//!
//! 依詞頻從第一個字開始加入，對學過幾年英文的人來說前一兩千個字都太簡單，
//! 每天 15 張全在背早就會的東西。但也不能讓使用者自己填「我會幾個字」——
//! 沒有人估得準自己的詞彙量。
//!
//! ## 做法
//!
//! 標準的詞彙量測驗：把詞頻切成幾層，每層抽幾個字問「認不認識」，
//! 用各層的認識率推估總量。
//!
//! ```text
//! 1~500      ██████████ 100%   500 字
//! 500~1000   ██████████ 100%   500 字
//! 1000~2000  ████████░░  80%   800 字
//! 2000~4000  █████░░░░░  50% 1,000 字
//! 4000~8000  ██░░░░░░░░  20%   800 字
//! 8000~16000 ░░░░░░░░░░   0%     0 字
//!                        估計約 3,600 字
//! ```
//!
//! 三十題左右就能得到夠用的估計。這不是精確測量，目的只是找到
//! 「從哪裡開始學才不浪費時間」。
//!
//! ## 已知限制
//!
//! 這是自評式測驗，會有虛報（看起來眼熟就按認識）。標準做法是混入假詞
//! （`morpiate` 這種不存在但看起來像英文的字）來校正，目前還沒做，
//! 所以估計值偏樂觀是正常的。

use serde::{Deserialize, Serialize};

/// 一個詞頻區間，**兩端都含**。
///
/// 詞頻排名從 1 開始，用開區間會讓「第 1 到第 500 名」寫成 `(1, 501)`，
/// 每次讀都要停下來想一次；資料庫查詢也是 `BETWEEN`，含兩端比較自然。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrequencyBand {
    pub start_rank: i64,
    pub end_rank: i64,
}

impl FrequencyBand {
    pub fn size(&self) -> i64 {
        (self.end_rank - self.start_rank + 1).max(0)
    }
}

/// 預設的分層。
///
/// 前面切得細、後面切得粗，因為詞頻曲線就是這個形狀：
/// 前 2000 個字決定日常閱讀能不能通，之後每一層的邊際效益遞減。
pub fn default_bands() -> Vec<FrequencyBand> {
    [
        (1, 500),
        (501, 1_000),
        (1_001, 2_000),
        (2_001, 4_000),
        (4_001, 8_000),
        (8_001, 16_000),
        (16_001, 32_000),
    ]
    .into_iter()
    .map(|(start_rank, end_rank)| FrequencyBand {
        start_rank,
        end_rank,
    })
    .collect()
}

/// 一題的作答。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementAnswer {
    /// 對應 [`default_bands`] 的索引
    pub band_index: usize,
    pub known: bool,
}

/// 測驗結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementResult {
    /// 估計掌握的詞彙量
    pub estimated_vocabulary: i64,
    /// 建議新卡從這個詞頻排名開始。比這更常用的字大多已經會了。
    pub start_rank: i64,
    /// 每層的認識率，供 UI 畫出分佈
    pub band_rates: Vec<(FrequencyBand, f64)>,
}

/// 認識率高於這條線的層，視為「這一層大致都會了」。
///
/// 不用 100%：任何人都會在熟悉的區間漏掉幾個字，
/// 卡在那幾個字上而重學整層並不划算。
const MASTERED_THRESHOLD: f64 = 0.8;

/// 由作答估計詞彙量。
///
/// 沒有作答的層一律當成 0%，不做外插——寧可低估讓使用者多背幾個會的字，
/// 也不要高估而讓他直接跳到讀不懂的難度。
pub fn estimate(bands: &[FrequencyBand], answers: &[PlacementAnswer]) -> PlacementResult {
    let mut band_rates = Vec::with_capacity(bands.len());
    let mut vocabulary = 0i64;

    for (i, band) in bands.iter().enumerate() {
        let asked: Vec<bool> = answers
            .iter()
            .filter(|a| a.band_index == i)
            .map(|a| a.known)
            .collect();

        let rate = if asked.is_empty() {
            0.0
        } else {
            asked.iter().filter(|k| **k).count() as f64 / asked.len() as f64
        };

        vocabulary += (band.size() as f64 * rate).round() as i64;
        band_rates.push((*band, rate));
    }

    // 從最常用的一端往後走，找到第一個「還沒掌握」的層當起點。
    // 用連續的層而不是整體比率：中間有一層掉下來就該從那裡開始補。
    let start_rank = band_rates
        .iter()
        .find(|(_, rate)| *rate < MASTERED_THRESHOLD)
        .map(|(band, _)| band.start_rank)
        // 每層都掌握了：起點落在測驗範圍之後
        .unwrap_or_else(|| bands.last().map(|b| b.end_rank + 1).unwrap_or(1));

    PlacementResult {
        estimated_vocabulary: vocabulary,
        start_rank,
        band_rates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answers(band: usize, known: usize, unknown: usize) -> Vec<PlacementAnswer> {
        let mut v = Vec::new();
        for _ in 0..known {
            v.push(PlacementAnswer {
                band_index: band,
                known: true,
            });
        }
        for _ in 0..unknown {
            v.push(PlacementAnswer {
                band_index: band,
                known: false,
            });
        }
        v
    }

    #[test]
    fn bands_cover_a_reasonable_range_without_gaps() {
        let bands = default_bands();
        assert_eq!(bands[0].start_rank, 1);
        for pair in bands.windows(2) {
            assert_eq!(
                pair[0].end_rank + 1,
                pair[1].start_rank,
                "分層之間不該有空隙"
            );
        }
        assert_eq!(bands[0].size(), 500, "第 1 到第 500 名就是 500 個字");
        assert!(bands.last().unwrap().end_rank >= 30_000);
    }

    /// 完全不會的人：估計為 0，從第一個字開始。
    #[test]
    fn a_complete_beginner_starts_from_the_top() {
        let bands = default_bands();
        let mut all = Vec::new();
        for i in 0..bands.len() {
            all.extend(answers(i, 0, 5));
        }
        let r = estimate(&bands, &all);
        assert_eq!(r.estimated_vocabulary, 0);
        assert_eq!(r.start_rank, 1);
    }

    /// 學過幾年英文的人：前幾層都會，起點應該跳過那些。
    #[test]
    fn an_intermediate_learner_skips_the_easy_bands() {
        let bands = default_bands();
        let mut all = Vec::new();
        all.extend(answers(0, 5, 0)); // 1~500      100%
        all.extend(answers(1, 5, 0)); // 500~1000   100%
        all.extend(answers(2, 4, 1)); // 1000~2000   80%
        all.extend(answers(3, 2, 3)); // 2000~4000   40%
        all.extend(answers(4, 1, 4)); // 4000~8000   20%
        all.extend(answers(5, 0, 5));
        all.extend(answers(6, 0, 5));

        let r = estimate(&bands, &all);
        // 500 + 500 + 800 + 800 + 800 = 3400
        assert_eq!(r.estimated_vocabulary, 3_400);
        assert_eq!(r.start_rank, 2_001, "80% 那層算掌握，從下一層開始");
    }

    /// 中間某層掉下來，就該從那層開始補，而不是從更後面。
    #[test]
    fn a_gap_in_the_middle_becomes_the_starting_point() {
        let bands = default_bands();
        let mut all = Vec::new();
        all.extend(answers(0, 5, 0));
        all.extend(answers(1, 2, 3)); // 這層只有 40%
        all.extend(answers(2, 5, 0)); // 後面反而全會
        let r = estimate(&bands, &all);
        assert_eq!(r.start_rank, 501);
    }

    /// 每一層都掌握的人：起點落在測驗範圍之外。
    #[test]
    fn a_fluent_learner_gets_a_start_beyond_the_test() {
        let bands = default_bands();
        let mut all = Vec::new();
        for i in 0..bands.len() {
            all.extend(answers(i, 5, 0));
        }
        let r = estimate(&bands, &all);
        assert_eq!(
            r.estimated_vocabulary, 32_000,
            "所有層加起來就是 32000 個字"
        );
        assert_eq!(r.start_rank, 32_001);
    }

    /// 沒作答的層不外插，一律當成不會。
    #[test]
    fn unanswered_bands_count_as_zero() {
        let bands = default_bands();
        let r = estimate(&bands, &answers(0, 5, 0));
        assert_eq!(r.estimated_vocabulary, 500, "只有第一層有作答");
        assert_eq!(r.start_rank, 501);
        assert_eq!(r.band_rates.len(), bands.len());
        assert_eq!(r.band_rates[3].1, 0.0);
    }

    #[test]
    fn no_answers_at_all_is_not_a_crash() {
        let r = estimate(&default_bands(), &[]);
        assert_eq!(r.estimated_vocabulary, 0);
        assert_eq!(r.start_rank, 1);
    }
}
