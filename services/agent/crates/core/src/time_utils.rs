//! 统一时间来源，便于测试时替换。

use time::OffsetDateTime;

/// 当前 UTC 时间。
pub fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// 把时间格式化为 tracing 事件常用的 RFC 3339 字符串。
pub fn rfc3339(timestamp: OffsetDateTime) -> String {
    timestamp
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::new())
}
