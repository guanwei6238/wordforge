//! Wiktionary 匯入器（[kaikki.org](https://kaikki.org/) 的 JSONL 萃取）。
//!
//! 取得資料：
//! ```text
//! wget https://kaikki.org/dictionary/English/kaikki.org-dictionary-English.jsonl
//! ```
//!
//! 檔案是一行一個 JSON 物件、動輒數 GB，所以這裡逐行串流解析，不整份載入記憶體。
//! 授權為 CC BY-SA 4.0，UI 顯示釋義時必須標示出處。

use std::io::BufRead;

use serde::Deserialize;

use crate::{DictEntry, ExampleEntry, PronunciationEntry, Result, SenseEntry};

/// kaikki 的原始 schema。欄位比這裡多很多，只取得用得到的部分。
#[derive(Debug, Deserialize)]
struct RawEntry {
    word: String,
    #[serde(default)]
    pos: String,
    #[serde(default)]
    lang_code: String,
    #[serde(default)]
    senses: Vec<RawSense>,
    #[serde(default)]
    sounds: Vec<RawSound>,
    #[serde(default)]
    forms: Vec<RawForm>,
}

#[derive(Debug, Deserialize)]
struct RawSense {
    #[serde(default)]
    glosses: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    examples: Vec<RawExample>,
}

#[derive(Debug, Deserialize)]
struct RawExample {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    english: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawSound {
    #[serde(default)]
    ipa: Option<String>,
    #[serde(default)]
    audio: Option<String>,
    #[serde(default)]
    ogg_url: Option<String>,
    #[serde(default)]
    mp3_url: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawForm {
    #[serde(default)]
    form: String,
    #[serde(default)]
    tags: Vec<String>,
}

/// 這些 tag 代表「這不是一個真的詞形」，例如表格標題或無變化標記。
const FORM_TAG_BLOCKLIST: [&str; 4] = ["table-tags", "inflection-template", "class", "no-form"];

/// 解析單一行 JSON。
pub fn parse_line(line: &str) -> Result<DictEntry> {
    let raw: RawEntry = serde_json::from_str(line)?;
    Ok(convert(raw))
}

/// 串流解析整份 JSONL。
///
/// 單行解析失敗不會中斷整批匯入——幾百萬行裡有幾行壞掉是常態，
/// 呼叫端自己決定要記 log 還是計數。
pub fn parse_reader<R: BufRead>(reader: R) -> impl Iterator<Item = Result<DictEntry>> {
    reader.lines().filter_map(|line| match line {
        Err(e) => Some(Err(e.into())),
        Ok(l) if l.trim().is_empty() => None,
        Ok(l) => Some(parse_line(&l)),
    })
}

fn convert(raw: RawEntry) -> DictEntry {
    let lang = if raw.lang_code.is_empty() {
        "en".to_string()
    } else {
        raw.lang_code.clone()
    };

    let senses = raw
        .senses
        .into_iter()
        .filter_map(|s| {
            // glosses 可能有多層（大分類 / 細分類），取最細的那一層
            let gloss = s.glosses.last()?.trim().to_string();
            if gloss.is_empty() {
                return None;
            }
            Some(SenseEntry {
                gloss,
                gloss_lang: lang.clone(),
                translation: None,
                register: s.tags.first().cloned(),
                domain: s.topics.first().cloned(),
                examples: s
                    .examples
                    .into_iter()
                    .filter_map(|e| {
                        e.text.map(|text| ExampleEntry {
                            text,
                            translation: e.english,
                        })
                    })
                    .collect(),
            })
        })
        .collect();

    let pronunciations = raw
        .sounds
        .into_iter()
        .filter(|s| s.ipa.is_some() || s.audio.is_some())
        .map(|s| PronunciationEntry {
            accent: s.tags.first().map(|t| t.to_lowercase()),
            ipa: s.ipa,
            audio_url: s.mp3_url.or(s.ogg_url),
            audio_license: Some("CC BY-SA / Wikimedia Commons".into()),
        })
        .collect();

    let forms = raw
        .forms
        .into_iter()
        .filter(|f| {
            !f.form.trim().is_empty()
                && !f
                    .tags
                    .iter()
                    .any(|t| FORM_TAG_BLOCKLIST.contains(&t.as_str()))
        })
        .map(|f| (f.form, f.tags.join(",")))
        .collect();

    DictEntry {
        lang,
        headword: raw.word,
        pos: raw.pos,
        freq_rank: None,
        cefr: None,
        senses,
        forms,
        pronunciations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "word": "run",
        "pos": "verb",
        "lang_code": "en",
        "senses": [
            {
                "glosses": ["To move", "To move swiftly on foot"],
                "tags": ["intransitive"],
                "topics": ["sports"],
                "examples": [{"text": "She ran to the station."}]
            },
            {"glosses": [], "examples": []}
        ],
        "sounds": [
            {"ipa": "/ɹʌn/", "tags": ["UK"]},
            {"audio": "en-us-run.ogg", "ogg_url": "https://example.org/run.ogg", "tags": ["US"]},
            {"other": "nothing useful"}
        ],
        "forms": [
            {"form": "ran", "tags": ["past"]},
            {"form": "runs", "tags": ["present", "third-person"]},
            {"form": "-", "tags": ["table-tags"]}
        ]
    }"#;

    #[test]
    fn parses_a_full_entry() {
        let e = parse_line(SAMPLE).unwrap();
        assert_eq!(e.headword, "run");
        assert_eq!(e.pos, "verb");
        assert_eq!(e.lang, "en");
        assert!(e.is_usable());
    }

    /// glosses 有多層時要取最細的那一層，空的 sense 要丟掉。
    #[test]
    fn takes_most_specific_gloss_and_drops_empty_senses() {
        let e = parse_line(SAMPLE).unwrap();
        assert_eq!(e.senses.len(), 1);
        assert_eq!(e.senses[0].gloss, "To move swiftly on foot");
        assert_eq!(e.senses[0].domain.as_deref(), Some("sports"));
        assert_eq!(e.senses[0].examples[0].text, "She ran to the station.");
    }

    /// 沒有 IPA 也沒有音檔的 sound 條目沒有價值，應該濾掉。
    #[test]
    fn keeps_only_meaningful_pronunciations() {
        let e = parse_line(SAMPLE).unwrap();
        assert_eq!(e.pronunciations.len(), 2);
        assert_eq!(e.pronunciations[0].ipa.as_deref(), Some("/ɹʌn/"));
        assert_eq!(e.pronunciations[0].accent.as_deref(), Some("uk"));
        assert_eq!(
            e.pronunciations[1].audio_url.as_deref(),
            Some("https://example.org/run.ogg")
        );
    }

    /// 詞形表裡的排版列不是真的詞形。
    #[test]
    fn filters_layout_rows_from_forms() {
        let e = parse_line(SAMPLE).unwrap();
        let forms: Vec<&str> = e.forms.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(forms, vec!["ran", "runs"]);
    }

    #[test]
    fn entry_without_senses_is_not_usable() {
        let e = parse_line(r#"{"word": "redirect", "senses": []}"#).unwrap();
        assert!(!e.is_usable());
        assert_eq!(e.lang, "en", "缺 lang_code 時預設英文");
    }

    #[test]
    fn stream_skips_blank_lines_and_reports_bad_ones() {
        // 真實檔案是一行一個物件，這裡也用單行樣本
        const ONE_LINE: &str = r#"{"word":"cat","pos":"noun","lang_code":"en","senses":[{"glosses":["A small feline"]}]}"#;
        let input = String::from(ONE_LINE) + "\n\n{not json}\n";
        let results: Vec<_> = parse_reader(input.as_bytes()).collect();
        assert_eq!(results.len(), 2, "空行應該被跳過");
        assert!(results[0].is_ok());
        assert!(results[1].is_err(), "壞掉的行要回報錯誤而不是 panic");
    }
}
