//! 通用工具：时间戳等。
//!
//! 时间统一存为 ISO 8601 UTC 字符串（形如 `2026-08-25T10:00:00Z`），
//! 不依赖第三方时间库。

use std::time::{SystemTime, UNIX_EPOCH};

/// 当前 UTC 时间的 ISO 8601 字符串（秒级精度）。
pub fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    timestamp_to_iso8601(secs)
}

/// Unix 秒 → `YYYY-MM-DDTHH:MM:SSZ`。
pub fn timestamp_to_iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Howard Hinnant 的 civil_from_days 算法：
/// 自 1970-01-01 起的天数 → (年, 月, 日)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_1970() {
        assert_eq!(timestamp_to_iso8601(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamps() {
        // 2026-08-25T10:00:00Z = 1787652000
        assert_eq!(timestamp_to_iso8601(1_787_652_000), "2026-08-25T10:00:00Z");
        // 2000-02-29T00:00:00Z = 951782400（闰年）
        assert_eq!(timestamp_to_iso8601(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn now_has_expected_shape() {
        let s = now_iso8601();
        assert!(s.ends_with('Z'));
        assert_eq!(s.len(), 20);
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[10..11], "T");
    }
}
