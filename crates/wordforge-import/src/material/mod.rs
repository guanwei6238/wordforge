//! 匯入使用者自己的教材。
//!
//! ## 為什麼要有這個
//!
//! 閱讀測驗是「照你的程度當場生一篇」，這個是相反的需求：
//! 把模型綁死在你指定的課本上，不准它自由發揮。考試只考課本，
//! 模型講到課本以外的東西就是干擾。
//!
//! ## 版權
//!
//! App **不內建也不散布任何教材**，跟字典是同一條政策。
//! 使用者匯入自己合法取得的檔案，`license_note` 讓他自己記下
//! 這份東西能不能分享出去。

pub mod chunk;
pub mod import;
pub mod text;

pub use import::{MaterialImport, MaterialOptions, import_material};
pub use text::MaterialFormat;
