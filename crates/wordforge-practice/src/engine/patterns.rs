//! 偵測規則的把關：寫進資料庫之前，每一條都要通過自己的控制組。
//!
//! ## 為什麼這件事要獨立成一個地方
//!
//! 句型的偵測規則有三條路可以進來——手動編輯、匯入 JSON、編輯器的
//! 「試試看」——而**驗收標準只能有一份**。各寫一份的結果是某一條路
//! 悄悄放行了壞掉的 regex，然後那個句型從此偵測不到，而畫面上完全正常。
//!
//! 真正的檢查在 [`compile_detector`]：編譯得過、`positive` 例句真的命中、
//! `negative` 例句真的沒命中，三項缺一就丟掉那一條。這裡只是把它包成
//! 「一批」的形狀，並且**把被丟掉的說出來**。

use wordforge_core::patterns::{Detector, compile_detector};

/// 匯入一批句型之後的結果。
///
/// **被丟掉的那些一定要說出來。** 使用者看到「寫入 20 筆」，卻不知道
/// 其中 12 筆的偵測規則沒過、難度上限對它們不生效——那正是這個專案
/// 最熟悉的一種壞法：功能看起來好好的，只是它沒有作用。
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct PatternReport {
    /// 寫進資料庫幾筆
    pub written: usize,
    /// 其中有幾筆**偵測得到**（難度上限與正向驗收對它們才生效）
    pub detectable: usize,
    /// 沒通過控制組、被丟掉的偵測規則，一條一句話
    pub rejected: Vec<String>,
}

/// 把一批偵測規則分成「通過控制組的」與「被丟掉的」。
pub fn vet_detectors(point: &str, detectors: &[Detector]) -> (Vec<Detector>, Vec<String>) {
    let mut kept = Vec::new();
    let mut rejected = Vec::new();
    for (i, detector) in detectors.iter().enumerate() {
        match compile_detector(detector) {
            Ok(_) => kept.push(detector.clone()),
            Err(detail) => rejected.push(format!("{point}（第 {} 條偵測規則）：{detail}", i + 1)),
        }
    }
    (kept, rejected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detector(value: &str, positive: &[&str]) -> Detector {
        Detector {
            kind: "regex".into(),
            value: value.into(),
            positive: positive.iter().map(|s| s.to_string()).collect(),
            negative: Vec::new(),
        }
    }

    /// 壞的那條被丟掉、好的照收，而且**說得出是哪一條**——
    /// 一份 80 條的清單只說「有一條壞了」等於沒說。
    #[test]
    fn a_broken_rule_is_dropped_by_name_and_the_rest_survive() {
        let (kept, rejected) = vet_detectors(
            "there-be",
            &[
                detector(r"\bthere\s+(is|are)\b", &["There is a cat."]),
                detector(r"\bnope\b", &["這句不含那個字"]),
            ],
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].contains("there-be"), "{}", rejected[0]);
        assert!(rejected[0].contains("第 2 條"), "{}", rejected[0]);
    }
}
