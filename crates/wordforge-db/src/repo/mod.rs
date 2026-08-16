//! 資料存取函數。
//!
//! 刻意寫成自由函數而不是一堆 struct：查詢就是查詢，不需要為了物件導向
//! 包一層。要換掉實作時，呼叫端改 import 路徑即可。
//!
//! ## 為什麼分成三個檔案
//!
//! 這三組資料的生命週期完全不一樣：
//!
//! - [`profiles`]：使用者是誰、學什麼語言、每天學幾個。設定，不是資料。
//! - [`lemmas`]：匯入的字典。整份重匯是正常操作。
//! - [`cards`]：學習歷史與排程。**最不能弄丟的那一份**。
//!
//! 它們原本擠在同一個檔案裡，改一個查詢要在三千多行裡找位置，
//! 而且「這個函式碰的是哪一組資料」只能靠函式名稱猜。
//!
//! 模組路徑刻意沒變（`repo::cards::ensure` 還是 `repo::cards::ensure`），
//! 呼叫端一行都不用改。

pub mod cards;
pub mod lemmas;
pub mod profiles;

pub use lemmas::NewLemma;

#[cfg(test)]
pub(crate) mod fixture;

#[cfg(test)]
mod tests {
    use time::Duration;

    use crate::repo::fixture::t0;
    use crate::ts;

    #[tokio::test]
    async fn timestamps_round_trip_and_sort_lexicographically() {
        let a = ts::to_sql(t0());
        let b = ts::to_sql(t0() + Duration::milliseconds(500));
        assert!(a < b, "{a} 應該小於 {b}");
        assert_eq!(ts::from_sql(&a), Some(t0()));
    }
}
