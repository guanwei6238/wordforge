//! 間隔重複排程（Spaced Repetition Scheduling）。
//!
//! 目前提供 FSRS-5。之所以不用 Anki 傳統的 SM-2：
//! SM-2 只用「連續答對次數」推算間隔，忽略了遺忘曲線本身會隨卡片難度改變；
//! FSRS 以 DSR（Difficulty / Stability / Retrievability）三變數建模，
//! 在相同記憶留存率下平均可以少排 20~30% 的複習量。
//!
//! 實作依據 FSRS-5 公開規格：
//! <https://github.com/open-spaced-repetition/fsrs4anki/wiki/The-Algorithm>

mod fsrs;

pub use fsrs::{DECAY, FACTOR, FsrsParams, Scheduler, SchedulerConfig};
