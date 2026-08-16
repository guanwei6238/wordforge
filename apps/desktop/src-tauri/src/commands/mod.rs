//! 前端呼叫得到的 command，依畫面上的功能分檔。
//!
//! 這一層刻意只做三件事：組裝依賴、轉換型別、把錯誤變成前端看得懂的字串。
//! 任何演算法都不該寫在這裡——它們在 `wordforge-core`、`wordforge-db`
//! 與 `wordforge-practice`。
//!
//! 分檔的方式跟前端的分頁對得起來：複習、查字典、練習、文法、匯入……
//! 找一個 command 的時候不必在一千七百行裡捲。

pub mod audio;
pub mod cards;
pub mod dict;
pub mod grammar;
pub mod import;
pub mod llm;
pub mod material;
pub mod placement;
pub mod practice;
pub mod profile;
pub mod topics;
