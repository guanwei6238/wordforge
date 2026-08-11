//! # wordforge-practice
//!
//! 把「你會哪些字」變成「你能用這些字做什麼」。
//!
//! ```text
//! 詞彙狀態 ──┐
//!            ├─→ 選題型 ─→ 組 prompt ─→ LLM ─→ 解析 ─→ 驗收 ─→ 存檔
//! 文法弱點 ──┘                                              │
//!                                                           ▼
//! 複習牌組 ←── 建卡 ←── 不懂的字 ←── 批改 ←── 作答 ←────────┘
//! ```
//!
//! 最後那條回路是整套設計的重點：**批改不只是打分數，還要找出
//! 「他其實不會這個字」並排進複習**。使用者不需要自己判斷哪裡不會——
//! 從錯誤裡看出來本來就是老師的工作。
//!
//! 這一層只做編排。決策規則在 `wordforge_core::practice`（純函數、好測試），
//! prompt 在 `wordforge_llm::prompts`，SQL 在 `wordforge_db`。

pub mod engine;
pub mod payload;

pub use engine::{PracticeEngine, PracticeError, Result};
pub use payload::{ExerciseView, Feedback, GradeInput};
