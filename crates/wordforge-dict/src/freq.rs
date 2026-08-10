//! 詞頻表。
//!
//! 90% 法則要決定「先教哪些新詞」，最有效的排序就是詞頻：
//! 英文最常用的 2000 個詞就覆蓋一般文本約 80% 的詞元，
//! 到 5000 詞大約 88%，到 8000 詞才勉強接近 95%。
//! 這條曲線正是為什麼「多背常用字」的投資報酬率遠高於背冷僻字。
//!
//! 支援兩種常見格式：
//! - 已排序的純文字（一行一個字，行號即排名）
//! - `word<TAB>count` 或 `word,count`（依次數由多到少換算排名）

use std::collections::HashMap;
use std::io::BufRead;

use crate::Result;

/// 詞 → 排名（1 為最常用）。
pub type FreqTable = HashMap<String, i64>;

/// 讀取「一行一個字」的排序清單。
pub fn load_ranked_list<R: BufRead>(reader: R) -> Result<FreqTable> {
    let mut table = FreqTable::new();
    let mut rank = 0i64;
    for line in reader.lines() {
        let line = line?;
        let word = wordforge_core::text::normalize(line.trim());
        if word.is_empty() {
            continue;
        }
        rank += 1;
        // 重複出現時保留較前面（較常用）的排名
        table.entry(word).or_insert(rank);
    }
    Ok(table)
}

/// 讀取 `word<sep>count` 格式，依次數換算排名。
pub fn load_counts<R: BufRead>(reader: R, sep: char) -> Result<FreqTable> {
    let mut counts: Vec<(String, f64)> = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let Some((word, count)) = line.split_once(sep) else {
            continue;
        };
        let word = wordforge_core::text::normalize(word.trim());
        let Ok(count) = count.trim().parse::<f64>() else {
            continue;
        };
        if !word.is_empty() {
            counts.push((word, count));
        }
    }

    // 次數多的排前面；同次數用字母序，確保結果可重現
    counts.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut table = FreqTable::new();
    for (i, (word, _)) in counts.into_iter().enumerate() {
        table.entry(word).or_insert(i as i64 + 1);
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranked_list_uses_line_order() {
        let input = "the\nbe\n\nTo\n";
        let table = load_ranked_list(input.as_bytes()).unwrap();
        assert_eq!(table.get("the"), Some(&1));
        assert_eq!(table.get("be"), Some(&2));
        assert_eq!(table.get("to"), Some(&3), "空行不佔排名，且會正規化大小寫");
    }

    #[test]
    fn counts_are_converted_to_ranks() {
        let input = "cat\t10\nthe\t1000\nbad line\ndog\t500\nbroken\tnot-a-number\n";
        let table = load_counts(input.as_bytes(), '\t').unwrap();
        assert_eq!(table.get("the"), Some(&1));
        assert_eq!(table.get("dog"), Some(&2));
        assert_eq!(table.get("cat"), Some(&3));
        assert_eq!(table.get("broken"), None, "數字解析失敗的行要跳過");
        assert_eq!(table.len(), 3);
    }
}
