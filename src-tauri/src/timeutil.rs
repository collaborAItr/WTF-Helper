//! Small date helpers, so the helper needs no date library.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn digits(text: &str, from: usize, len: usize) -> Option<i64> {
    let part = text.get(from..from + len)?;
    if !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

/// `YYYY-MM-DD?HH:MM:SS` at the start of `text`, as epoch milliseconds in UTC.
fn parse_prefix_ms(text: &str) -> Option<i64> {
    let year = digits(text, 0, 4)?;
    let month = digits(text, 5, 2)?;
    let day = digits(text, 8, 2)?;
    let hour = digits(text, 11, 2)?;
    let minute = digits(text, 14, 2)?;
    let second = digits(text, 17, 2)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000)
}

/// ISO 8601 in UTC as sent by the service (`Date.toISOString()`), e.g. `2026-09-28T07:14:52.546Z`.
pub fn parse_iso_utc_ms(text: &str) -> Option<i64> {
    if text.len() < 20 || !text.ends_with('Z') || text.as_bytes().get(10) != Some(&b'T') {
        return None;
    }
    let base = parse_prefix_ms(text)?;
    let fraction = &text[19..text.len() - 1];
    let millis = if let Some(f) = fraction.strip_prefix('.') {
        let padded: String = f.chars().chain("000".chars()).take(3).collect();
        if !padded.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        padded.parse::<i64>().ok()?
    } else if fraction.is_empty() {
        0
    } else {
        return None;
    };
    Some(base + millis)
}

/// HTTP `Date` header, e.g. `Mon, 28 Sep 2026 07:14:22 GMT`.
pub fn parse_http_date_ms(text: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let rest = text.split_once(", ")?.1;
    let mut parts = rest.split(' ');
    let day: i64 = parts.next()?.parse().ok()?;
    let month_name = parts.next()?;
    let month = MONTHS.iter().position(|m| *m == month_name)? as i64 + 1;
    let year = parts.next()?;
    let time = parts.next()?;
    if parts.next()? != "GMT" || time.len() != 8 {
        return None;
    }
    parse_iso_utc_ms(&format!("{year}-{month:02}-{day:02}T{time}Z"))
}

/// macOS crash report time, e.g. `2026-09-26 18:10:06.0129 -0400`.
pub fn parse_mac_crash_ms(text: &str) -> Option<i64> {
    let base = parse_prefix_ms(text)?;
    let offset = text.rsplit(' ').next()?;
    let sign = match offset.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return Some(base),
    };
    let hours = digits(offset, 1, 2)?;
    let minutes = digits(offset, 3, 2)?;
    Some(base - sign * (hours * 60 + minutes) * 60_000)
}

/// Epoch milliseconds as `2026-09-28T07:14:52.546Z`, for building test events.
#[cfg(test)]
pub fn format_iso_utc(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_parses_round_trip() {
        for ms in [0, 1_790_579_692_546, 951_782_400_123, 4_102_444_799_999] {
            assert_eq!(
                parse_iso_utc_ms(&format_iso_utc(ms)),
                Some(ms),
                "{}",
                format_iso_utc(ms)
            );
        }
        assert_eq!(
            format_iso_utc(1_790_579_692_546),
            "2026-09-28T07:14:52.546Z"
        );
    }

    #[test]
    fn parses_service_timestamps() {
        assert_eq!(parse_iso_utc_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_iso_utc_ms("2026-09-28T07:14:52.546Z"),
            Some(1_790_579_692_546)
        );
        assert_eq!(
            parse_iso_utc_ms("2026-09-28T07:14:52Z"),
            Some(1_790_579_692_000)
        );
        assert_eq!(
            parse_iso_utc_ms("2026-09-28T07:14:52.5Z"),
            Some(1_790_579_692_500)
        );
        assert_eq!(parse_iso_utc_ms("2026-09-28 07:14:52Z"), None);
        assert_eq!(parse_iso_utc_ms("2026-09-28T07:14:52+01:00"), None);
        assert_eq!(parse_iso_utc_ms("soon"), None);
    }

    #[test]
    fn parses_http_dates() {
        assert_eq!(
            parse_http_date_ms("Mon, 28 Sep 2026 07:14:52 GMT"),
            Some(1_790_579_692_000)
        );
        assert_eq!(parse_http_date_ms("Mon, 28 Foo 2026 07:14:52 GMT"), None);
        assert_eq!(parse_http_date_ms("yesterday"), None);
    }

    #[test]
    fn parses_mac_crash_times_with_offset() {
        let utc = parse_iso_utc_ms("2026-09-26T22:10:06.000Z").unwrap();
        assert_eq!(
            parse_mac_crash_ms("2026-09-26 18:10:06.0129 -0400"),
            Some(utc)
        );
        assert_eq!(parse_mac_crash_ms("2026-09-27 00:10:06 +0200"), Some(utc));
        assert_eq!(parse_mac_crash_ms("garbage"), None);
    }
}
