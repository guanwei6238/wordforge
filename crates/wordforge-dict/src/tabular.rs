//! 通用 CSV / TSV 匯入器。
//!
//! 給兩種情境：
//! 1. 使用者自己整理的單字表（例如從課本抄下來的單字）
//! 2. 從 Anki 匯出的牌組
//!
//! 欄位以標頭名稱對應，順序不拘。只有 `word` 是必填：
//!
//! ```csv
//! word,pos,translation,gloss,example,ipa,cefr
//! apple,noun,蘋果,A round fruit,I ate an apple.,/ˈæp.əl/,A1
//! ```

use std::io::Read;

use serde::Deserialize;

use crate::{DictEntry, ExampleEntry, PronunciationEntry, Result, SenseEntry};

#[derive(Debug, Deserialize)]
struct Row {
    word: String,
    #[serde(default)]
    pos: Option<String>,
    /// 母語翻譯
    #[serde(default)]
    translation: Option<String>,
    /// 目標語定義
    #[serde(default)]
    gloss: Option<String>,
    #[serde(default)]
    example: Option<String>,
    #[serde(default)]
    ipa: Option<String>,
    #[serde(default)]
    cefr: Option<String>,
    #[serde(default)]
    freq_rank: Option<i64>,
}

/// 解析 CSV（`delimiter` 傳 `b'\t'` 就是 TSV）。
pub fn parse<R: Read>(reader: R, lang: &str, delimiter: u8) -> Result<Vec<DictEntry>> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(reader);

    let mut out = Vec::new();
    for (i, record) in rdr.deserialize::<Row>().enumerate() {
        let row = record?;
        if row.word.trim().is_empty() {
            return Err(crate::DictError::Malformed {
                line: i + 2, // +1 表頭，+1 轉成 1-based
                reason: "word 欄位不可為空".into(),
            });
        }
        out.push(to_entry(row, lang));
    }
    Ok(out)
}

fn to_entry(row: Row, lang: &str) -> DictEntry {
    // 使用者最常只填翻譯，所以 gloss 缺席時用翻譯頂上，避免整筆被當成無釋義而丟棄
    let gloss = row
        .gloss
        .clone()
        .or_else(|| row.translation.clone())
        .unwrap_or_default();

    let senses = if gloss.is_empty() {
        Vec::new()
    } else {
        vec![SenseEntry {
            gloss,
            gloss_lang: lang.to_string(),
            translation: row.translation,
            register: None,
            domain: None,
            examples: row
                .example
                .filter(|e| !e.is_empty())
                .map(|text| {
                    vec![ExampleEntry {
                        text,
                        translation: None,
                    }]
                })
                .unwrap_or_default(),
        }]
    };

    let pronunciations = row
        .ipa
        .filter(|s| !s.is_empty())
        .map(|ipa| {
            vec![PronunciationEntry {
                accent: None,
                ipa: Some(ipa),
                audio_url: None,
                audio_license: None,
            }]
        })
        .unwrap_or_default();

    DictEntry {
        lang: lang.to_string(),
        headword: row.word,
        pos: row.pos.unwrap_or_default(),
        freq_rank: row.freq_rank,
        cefr: row.cefr,
        senses,
        forms: Vec::new(),
        pronunciations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_rows() {
        let csv = "word,pos,translation,gloss,example,ipa,cefr\n\
                   apple,noun,蘋果,A round fruit,I ate an apple.,/ˈæp.əl/,A1\n";
        let entries = parse(csv.as_bytes(), "en", b',').unwrap();

        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.headword, "apple");
        assert_eq!(e.pos, "noun");
        assert_eq!(e.cefr.as_deref(), Some("A1"));
        assert_eq!(e.senses[0].gloss, "A round fruit");
        assert_eq!(e.senses[0].translation.as_deref(), Some("蘋果"));
        assert_eq!(e.senses[0].examples[0].text, "I ate an apple.");
        assert_eq!(e.pronunciations[0].ipa.as_deref(), Some("/ˈæp.əl/"));
    }

    /// 只有單字和翻譯是最常見的手抄格式，必須可用。
    #[test]
    fn translation_only_row_is_usable() {
        let csv = "word,translation\nbook,書\n";
        let entries = parse(csv.as_bytes(), "en", b',').unwrap();
        assert!(entries[0].is_usable());
        assert_eq!(entries[0].senses[0].gloss, "書");
    }

    #[test]
    fn supports_tsv_and_column_reordering() {
        let tsv = "translation\tword\n貓\tcat\n";
        let entries = parse(tsv.as_bytes(), "en", b'\t').unwrap();
        assert_eq!(entries[0].headword, "cat");
        assert_eq!(entries[0].senses[0].translation.as_deref(), Some("貓"));
    }

    #[test]
    fn rejects_rows_without_a_word() {
        let csv = "word,translation\n,空的\n";
        let err = parse(csv.as_bytes(), "en", b',').unwrap_err();
        assert!(matches!(err, crate::DictError::Malformed { line: 2, .. }));
    }

    #[test]
    fn word_without_any_meaning_is_not_usable() {
        let csv = "word\nmystery\n";
        let entries = parse(csv.as_bytes(), "en", b',').unwrap();
        assert!(!entries[0].is_usable(), "沒有任何釋義的詞條不該被匯入");
    }
}
