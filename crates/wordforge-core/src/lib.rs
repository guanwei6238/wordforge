//! # wordforge-core
//!
//! Wordforge 的領域核心：純運算、無 I/O、無資料庫、無網路。
//!
//! 這一層刻意不依賴 `sqlx`、`tauri` 或任何 HTTP client，讓排程演算法與
//! 詞彙覆蓋率計算可以被單元測試涵蓋，也方便未來搬到 CLI 或手機版重用。
//!
//! - [`srs`]：FSRS-5 間隔重複排程
//! - [`coverage`]：可理解輸入（90% 法則）的覆蓋率計算與目標詞挑選
//! - [`grammar_points`]：文法點的受控詞彙表，避免模型每次換一套說法
//! - [`model`]：跨層共用的領域型別
//! - [`placement`]：分級測驗，估計詞彙量決定從哪裡開始學
//! - [`practice`]：決定現在該出什麼題
//! - [`text`]：字串正規化與斷詞的共用工具
//! - [`wordlist`]：不適合做成單字卡的功能詞

pub mod coverage;
pub mod grammar_points;
pub mod model;
pub mod placement;
pub mod practice;
pub mod srs;
pub mod text;
pub mod wordlist;

pub use coverage::{Coverage, CoverageBand};
pub use model::{Card, CardKind, CardState, LemmaId, ProfileId, Rating, ReviewLog};
pub use srs::{FsrsParams, Scheduler, SchedulerConfig};

/// 領域層錯誤。I/O 相關錯誤屬於外層 crate，不會出現在這裡。
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("FSRS 參數數量錯誤：預期 {expected} 個，收到 {got} 個")]
    ParamCount { expected: usize, got: usize },

    #[error("desired_retention 必須落在 (0, 1) 之間，收到 {0}")]
    InvalidRetention(f64),

    #[error("文本沒有任何可計算的詞元")]
    EmptyText,
}

pub type Result<T> = std::result::Result<T, CoreError>;
