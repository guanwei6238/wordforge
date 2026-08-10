//! [ECDICT](https://github.com/skywind3000/ECDICT) 英漢字典匯入器（MIT 授權）。
//!
//! 對中文母語的英文學習者來說，這份資料比 Wiktionary 更直接：
//! 中文翻譯、音標、詞形變化、詞頻排名，還有 `zk`(國中會考) / `gk`(學測) /
//! `cet4` / `ielts` 這類考試範圍標籤——想「只背國中單字」就靠它。
//!
//! ```bash
//! curl -LO https://raw.githubusercontent.com/skywind3000/ECDICT/master/ecdict.csv
//! ```
//!
//! ## 注意
//!
//! 翻譯是**簡體中文**。目前原樣匯入並標記 `gloss_lang = "zh-CN"`，
//! 繁體轉換還沒做（需要 OpenCC 等級的對照表，不是逐字替換就能解決的）。

use std::io::Read;

use serde::Deserialize;

use crate::{DictEntry, PronunciationEntry, Result, SenseEntry, SourceMeta};

/// ECDICT 的來源資訊。
pub fn source_meta() -> SourceMeta {
    SourceMeta {
        slug: "ecdict".into(),
        name: "ECDICT 英漢字典".into(),
        license: Some("MIT".into()),
        attribution: Some("ECDICT by skywind3000".into()),
        homepage: Some("https://github.com/skywind3000/ECDICT".into()),
        version: None,
    }
}

#[derive(Debug, Deserialize)]
struct Row {
    word: String,
    #[serde(default)]
    phonetic: String,
    /// 英文釋義，多條以字面的 `\n` 分隔
    #[serde(default)]
    definition: String,
    /// 中文翻譯，多條以字面的 `\n` 分隔
    #[serde(default)]
    translation: String,
    /// 詞性分佈，如 `v:45/n:55`
    #[serde(default)]
    pos: String,
    /// 柯林斯星級 1~5，越高越常用
    #[serde(default)]
    collins: String,
    /// `1` 表示屬於牛津三千核心詞
    #[serde(default)]
    oxford: String,
    /// 考試範圍：`zk gk cet4 cet6 ky toefl ielts gre`
    #[serde(default)]
    tag: String,
    /// BNC 語料庫詞頻排名
    #[serde(default)]
    bnc: String,
    /// 當代語料庫詞頻排名
    #[serde(default)]
    frq: String,
    /// 詞形變化，如 `p:ran/d:run/i:running/3:runs`
    #[serde(default)]
    exchange: String,
    #[serde(default)]
    #[allow(dead_code)]
    detail: String,
    #[serde(default)]
    audio: String,
}

/// `exchange` 欄位的代碼對照。
fn exchange_tag(code: &str) -> Option<&'static str> {
    Some(match code {
        "p" => "past",
        "d" => "past-participle",
        "i" => "present-participle",
        "3" => "third-person",
        "r" => "comparative",
        "t" => "superlative",
        "s" => "plural",
        // `0` 是「這個詞的原形」、`1` 是變化類型，都不是這個詞條的衍生形
        _ => return None,
    })
}

/// 串流解析 ECDICT CSV。
pub fn parse<R: Read>(reader: R) -> impl Iterator<Item = Result<DictEntry>> {
    let rdr = csv::ReaderBuilder::new().flexible(true).from_reader(reader);
    rdr.into_deserialize::<Row>()
        .map(|row| row.map_err(Into::into).map(convert))
}

/// 拆開 ECDICT 用字面 `\n` 串起來的多條釋義。
fn split_lines(field: &str) -> impl Iterator<Item = &str> {
    field.split("\\n").map(str::trim).filter(|s| !s.is_empty())
}

fn convert(row: Row) -> DictEntry {
    let mut senses = Vec::new();

    // 中文翻譯排前面：這是中文母語者第一眼要看的東西
    for line in split_lines(&row.translation) {
        senses.push(SenseEntry {
            gloss: line.to_string(),
            gloss_lang: "zh-CN".into(),
            translation: Some(line.to_string()),
            ..Default::default()
        });
    }
    for line in split_lines(&row.definition) {
        senses.push(SenseEntry {
            gloss: line.to_string(),
            gloss_lang: "en".into(),
            ..Default::default()
        });
    }

    let pronunciations = if row.phonetic.trim().is_empty() {
        Vec::new()
    } else {
        vec![PronunciationEntry {
            accent: None,
            // ECDICT 存的是不含斜線的 IPA，補上斜線讓顯示一致
            ipa: Some(format!("/{}/", row.phonetic.trim())),
            audio_url: (!row.audio.trim().is_empty()).then(|| row.audio.trim().to_string()),
            audio_license: None,
        }]
    };

    let forms = row
        .exchange
        .split('/')
        .filter_map(|part| {
            let (code, form) = part.split_once(':')?;
            let tag = exchange_tag(code.trim())?;
            let form = form.trim();
            (!form.is_empty()).then(|| (form.to_string(), tag.to_string()))
        })
        .collect();

    // 當代語料的排名比 BNC 貼近現在的用法，優先採用
    let freq_rank = parse_rank(&row.frq).or_else(|| parse_rank(&row.bnc));

    let mut tags: Vec<String> = row.tag.split_whitespace().map(str::to_string).collect();
    if row.oxford.trim() == "1" {
        tags.push("oxford3000".into());
    }
    if let Ok(stars) = row.collins.trim().parse::<u8>()
        && (1..=5).contains(&stars)
    {
        tags.push(format!("collins{stars}"));
    }

    DictEntry {
        lang: "en".into(),
        headword: row.word.trim().to_string(),
        // pos 欄位是 `v:45/n:55` 這種分佈，取比例最高的當主要詞性
        pos: dominant_pos(&row.pos),
        freq_rank,
        cefr: None,
        senses,
        forms,
        pronunciations,
        tags,
    }
}

fn parse_rank(s: &str) -> Option<i64> {
    match s.trim().parse::<i64>() {
        // 0 代表「沒有排名資料」，不是「最常用」
        Ok(n) if n > 0 => Some(n),
        _ => None,
    }
}

/// `v:45/n:55` → `n`
fn dominant_pos(field: &str) -> String {
    field
        .split('/')
        .filter_map(|p| {
            let (pos, pct) = p.split_once(':')?;
            Some((pos.trim().to_string(), pct.trim().parse::<i64>().ok()?))
        })
        .max_by_key(|(_, pct)| *pct)
        .map(|(pos, _)| pos)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "word,phonetic,definition,translation,pos,collins,oxford,tag,bnc,frq,exchange,detail,audio\n";

    fn parse_one(row: &str) -> DictEntry {
        let csv = format!("{HEADER}{row}");
        parse(csv.as_bytes()).next().unwrap().unwrap()
    }

    #[test]
    fn parses_a_typical_word() {
        // 翻譯欄含逗號，真實檔案裡是有引號的
        let e = parse_one(
            r#"apple,'æpl,"n. fruit with red skin\nn. a tree","n. 苹果, 家伙",n:100,3,1,zk gk,2446,2695,s:apples,,"#,
        );

        assert_eq!(e.headword, "apple");
        assert_eq!(e.pos, "n");
        assert_eq!(e.freq_rank, Some(2695), "有 frq 就用 frq");
        assert_eq!(e.pronunciations[0].ipa.as_deref(), Some("/'æpl/"));
        assert_eq!(e.forms, vec![("apples".to_string(), "plural".to_string())]);
        assert!(e.is_usable());
    }

    /// 中文翻譯要排在英文釋義前面：中文母語者第一眼看的是它。
    #[test]
    fn chinese_translation_comes_first() {
        let e = parse_one(r#"apple,,"n. a fruit","n. 苹果\nn. 家伙",,,,,0,0,,,"#);
        assert_eq!(e.senses.len(), 3);
        assert_eq!(e.senses[0].gloss, "n. 苹果");
        assert_eq!(e.senses[0].gloss_lang, "zh-CN");
        assert_eq!(e.senses[0].translation.as_deref(), Some("n. 苹果"));
        assert_eq!(e.senses[1].gloss, "n. 家伙");
        assert_eq!(e.senses[2].gloss_lang, "en", "英文釋義排在後面");
    }

    /// 考試標籤是「只背國中單字」這種功能的基礎，不能弄丟。
    #[test]
    fn keeps_exam_and_difficulty_tags() {
        let e = parse_one(r#"beautiful,,,a. 美丽的,,4,1,zk gk ielts,1161,992,,,"#);
        assert_eq!(e.tags, vec!["zk", "gk", "ielts", "oxford3000", "collins4"]);
    }

    #[test]
    fn parses_all_inflection_codes() {
        let e = parse_one(r#"run,,,v. 跑,,,,,0,0,"p:ran/d:run/i:running/3:runs/0:run/1:x",,"#);
        let mut forms: Vec<String> = e.forms.iter().map(|(f, t)| format!("{f}:{t}")).collect();
        forms.sort();
        assert_eq!(
            forms,
            vec![
                "ran:past",
                "run:past-participle",
                "running:present-participle",
                "runs:third-person",
            ],
            "0（原形）與 1（變化類型）不是這個詞條的衍生形，要濾掉"
        );
    }

    /// 排名 0 代表「沒有資料」，不能當成「最常用的第 0 名」。
    #[test]
    fn rank_zero_means_unknown() {
        let e = parse_one(r#"obscure,,,a. 晦涩的,,,,,0,0,,,"#);
        assert_eq!(e.freq_rank, None);

        let e = parse_one(r#"obscure,,,a. 晦涩的,,,,,5000,0,,,"#);
        assert_eq!(e.freq_rank, Some(5000), "frq 沒有就退回 BNC");
    }

    #[test]
    fn word_without_meaning_is_not_usable() {
        let e = parse_one(r#"zzz,,,,,,,,0,0,,,"#);
        assert!(!e.is_usable());
        assert!(e.pronunciations.is_empty());
    }

    #[test]
    fn dominant_pos_picks_the_highest_share() {
        assert_eq!(dominant_pos("v:45/n:55"), "n");
        assert_eq!(dominant_pos("v:80/n:20"), "v");
        assert_eq!(dominant_pos(""), "");
        assert_eq!(dominant_pos("garbage"), "");
    }

    #[test]
    fn streams_multiple_rows() {
        let csv = format!(
            "{HEADER}{}\n{}\n",
            r#"apple,,,n. 苹果,,,,,0,0,,,"#, r#"book,,,n. 书,,,,,0,0,,,"#
        );
        let entries: Vec<_> = parse(csv.as_bytes()).collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].headword, "book");
    }
}
