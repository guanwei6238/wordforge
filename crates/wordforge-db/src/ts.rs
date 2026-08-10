//! 時間欄位的序列化格式。
//!
//! SQLite 存的時間一律是這個格式：UTC、固定 6 位微秒。
//!
//! 固定寬度很重要——`due` 欄位靠字串比較排序，長度不一致（有的帶毫秒、有的不帶）
//! 會讓 `'...:00Z' > '...:00.5Z'` 這種荒謬結果出現。

use time::format_description::BorrowedFormatItem;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset};

use crate::{DbError, Result};

const FMT: &[BorrowedFormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z");

pub fn to_sql(dt: OffsetDateTime) -> String {
    dt.to_offset(UtcOffset::UTC)
        .format(&FMT)
        .expect("固定格式必定可序列化")
}

pub fn from_sql(s: &str) -> Option<OffsetDateTime> {
    PrimitiveDateTime::parse(s, &FMT)
        .ok()
        .map(|dt| dt.assume_utc())
        .or_else(|| {
            // 容忍手動改過或舊版寫入的 RFC 3339 值
            OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        })
}

pub trait ParseTs {
    fn parse_ts(self, field: &'static str) -> Result<OffsetDateTime>;
}

impl ParseTs for String {
    fn parse_ts(self, field: &'static str) -> Result<OffsetDateTime> {
        from_sql(&self).ok_or(DbError::Decode { field, value: self })
    }
}
