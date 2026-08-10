//! # wordforge-dict
//!
//! 把外部字典資料轉成 Wordforge 的統一結構。
//!
//! ## 為什麼不內建字典
//!
//! Cambridge、Oxford 等商業字典的釋義與錄音受著作權保護，散布會有法律風險。
//! 因此本專案**只提供匯入器**，資料由使用者自行取得：
//!
//! - [`kaikki`]：Wiktionary 的機器可讀萃取（CC BY-SA），釋義 / 詞形 / IPA / 例句最齊全
//! - [`tabular`]：通用 CSV / TSV，給自製單字表或你自己合法擁有的字典
//! - [`freq`]：詞頻表，90% 法則排序新詞的依據
//!
//! 所有匯入器都產出 [`DictEntry`]，寫入資料庫的動作由上層負責，
//! 這個 crate 不碰 I/O 之外的東西，也不依賴資料庫。

pub mod ecdict;
pub mod freq;
pub mod kaikki;
pub mod tabular;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum DictError {
    #[error("讀取失敗：{0}")]
    Io(#[from] std::io::Error),

    #[error("JSON 解析失敗：{0}")]
    Json(#[from] serde_json::Error),

    #[error("CSV 解析失敗：{0}")]
    Csv(#[from] csv::Error),

    #[error("第 {line} 行資料不完整：{reason}")]
    Malformed { line: usize, reason: String },
}

pub type Result<T> = std::result::Result<T, DictError>;

/// 匯入來源的中繼資料。授權欄位是必填的設計，
/// 因為 UI 需要標示出處，而使用者也需要知道哪些內容可以分享。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceMeta {
    pub slug: String,
    pub name: String,
    pub license: Option<String>,
    pub attribution: Option<String>,
    pub homepage: Option<String>,
    pub version: Option<String>,
}

impl SourceMeta {
    /// kaikki.org 提供的 Wiktionary 萃取。
    pub fn wiktionary(lang: &str) -> Self {
        Self {
            slug: format!("wiktionary-{lang}"),
            name: format!("Wiktionary ({lang})"),
            license: Some("CC BY-SA 4.0".into()),
            attribution: Some("Wiktionary contributors, via kaikki.org".into()),
            homepage: Some("https://kaikki.org/".into()),
            version: None,
        }
    }
}

/// 一個釋義。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SenseEntry {
    pub gloss: String,
    pub gloss_lang: String,
    pub translation: Option<String>,
    pub register: Option<String>,
    pub domain: Option<String>,
    pub examples: Vec<ExampleEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExampleEntry {
    pub text: String,
    pub translation: Option<String>,
}

/// 發音。`audio_url` 指向可下載的檔案，實際下載由上層決定
/// （使用者可能不想為了幾萬個字下載幾 GB 的音檔）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PronunciationEntry {
    pub accent: Option<String>,
    pub ipa: Option<String>,
    pub audio_url: Option<String>,
    pub audio_license: Option<String>,
}

/// 一個詞條。這是所有匯入器的共同輸出格式。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DictEntry {
    pub lang: String,
    pub headword: String,
    pub pos: String,
    pub freq_rank: Option<i64>,
    pub cefr: Option<String>,
    pub senses: Vec<SenseEntry>,
    /// 詞形變化：(表面形, 標籤)，例如 `("ran", "past")`
    pub forms: Vec<(String, String)>,
    pub pronunciations: Vec<PronunciationEntry>,
    /// 分類標籤，如 `zk`(國中會考) / `gk`(學測) / `cet4` / `oxford3000`。
    /// 「只背國中單字」這類篩選就靠它。
    pub tags: Vec<String>,
}

impl DictEntry {
    /// 沒有任何釋義的詞條通常是解析雜訊（重定向、格式頁），匯入前應該濾掉。
    pub fn is_usable(&self) -> bool {
        !self.headword.trim().is_empty() && !self.senses.is_empty()
    }
}
