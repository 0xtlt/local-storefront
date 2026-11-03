//! The dates of the store data, which are ISO 8601.

use chrono::{
    DateTime, Duration, FixedOffset, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone,
    Utc,
};
use chrono_tz::Tz;

/// Parses a date of the store data, in upper or lower case:
///
/// - a date, `2024-05-01`: its midnight in the shop's timezone;
/// - a date and a time with `Z` or an offset: `2024-05-01T10:00:00Z`, `2024-05-01T10:00:00+02:00`
///   (or `+0200`, `+02`);
/// - a date and a time without one, in the shop's timezone: `2024-05-01T10:00:00`, or with a
///   space in place of the `T`.
///
/// The time has seconds or not, and the seconds a fraction or not: `10:00`, `10:00:00.250`.
/// `None` for anything else.
pub fn parse(text: &str, timezone: Tz) -> Option<DateTime<Utc>> {
    let text = text.trim().to_ascii_uppercase();
    let (date, time) = match text.split_once(['T', ' ']) {
        Some((date, time)) => (date, Some(time)),
        None => (text.as_str(), None),
    };
    // Four digits for the year and two for the month and the day: chrono reads `2024-5-1` too.
    let shape = date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-';
    if !shape {
        return None;
    }
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let Some(time) = time else {
        return local(timezone, date.and_time(NaiveTime::MIN));
    };
    let (clock, offset) = match time.find(['Z', '+', '-']) {
        Some(at) => (&time[..at], Some(offset(&time[at..])?)),
        None => (time, None),
    };
    let clock = clock_time(clock)?;
    let naive = date.and_time(clock);
    match offset {
        Some(offset) => offset
            .from_local_datetime(&naive)
            .single()
            .map(|at| at.with_timezone(&Utc)),
        None => local(timezone, naive),
    }
}

/// `10:00`, `10:00:00` or `10:00:00.250`, with two digits for the hours and the minutes.
fn clock_time(text: &str) -> Option<NaiveTime> {
    let bytes = text.as_bytes();
    if bytes.len() < 5 || bytes[2] != b':' || (bytes.len() > 5 && bytes[5] != b':') {
        return None;
    }
    let format = if bytes.len() == 5 {
        "%H:%M"
    } else {
        "%H:%M:%S%.f"
    };
    NaiveTime::parse_from_str(text, format).ok()
}

/// `Z`, `+02:00`, `+0200` or `+02`.
fn offset(text: &str) -> Option<FixedOffset> {
    if text == "Z" {
        return FixedOffset::east_opt(0);
    }
    let (sign, digits) = match text.split_at_checked(1)? {
        ("+", digits) => (1, digits),
        ("-", digits) => (-1, digits),
        _ => return None,
    };
    let digits = match digits.as_bytes() {
        [_, _, b':', _, _] => digits.replacen(':', "", 1),
        [_, _, _, _] => digits.to_string(),
        [_, _] => format!("{digits}00"),
        _ => return None,
    };
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..].parse().ok()?;
    if minutes >= 60 {
        return None;
    }
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))
}

/// A local time of the shop's timezone. One that happens twice is the first; one that does not
/// happen, in the gap of a change to daylight saving time, is moved forward an hour, as Ruby
/// moves it.
fn local(timezone: Tz, naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    let at = match timezone.from_local_datetime(&naive) {
        LocalResult::Single(at) => at,
        LocalResult::Ambiguous(earliest, _) => earliest,
        LocalResult::None => timezone
            .from_local_datetime(&(naive + Duration::hours(1)))
            .earliest()?,
    };
    Some(at.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn reads_the_forms_of_iso_8601() {
        let paris: Tz = "Europe/Paris".parse().unwrap();
        let cases = [
            ("2024-05-01", "2024-04-30T22:00:00Z"),
            ("2024-05-01T10:00:00Z", "2024-05-01T10:00:00Z"),
            ("2024-05-01t10:00:00z", "2024-05-01T10:00:00Z"),
            ("2024-05-01T10:00:00.250Z", "2024-05-01T10:00:00.250Z"),
            ("2024-05-01T10:00:00+02:00", "2024-05-01T08:00:00Z"),
            ("2024-05-01T10:00:00-0430", "2024-05-01T14:30:00Z"),
            ("2024-05-01T10:00+02", "2024-05-01T08:00:00Z"),
            ("2024-05-01 10:00:00Z", "2024-05-01T10:00:00Z"),
            ("2024-05-01T10:00:00", "2024-05-01T08:00:00Z"),
            ("2024-05-01 10:00", "2024-05-01T08:00:00Z"),
            (" 2024-01-15T09:30:00 ", "2024-01-15T08:30:00Z"),
        ];
        for (text, expected) in cases {
            assert_eq!(parse(text, paris), Some(utc(expected)), "{text}");
        }
    }

    #[test]
    fn moves_a_time_that_does_not_happen_forward() {
        let paris: Tz = "Europe/Paris".parse().unwrap();
        // 02:30 does not happen on 2024-03-31 in Paris, and happens twice on 2024-10-27.
        assert_eq!(
            parse("2024-03-31T02:30:00", paris),
            Some(utc("2024-03-31T01:30:00Z"))
        );
        assert_eq!(
            parse("2024-10-27T02:30:00", paris),
            Some(utc("2024-10-27T00:30:00Z"))
        );
    }

    #[test]
    fn refuses_what_is_not_iso_8601() {
        for text in [
            "",
            "yesterday",
            "now",
            "2024-5-1",
            "2024/05/01",
            "May 1, 2024",
            "01-05-2024",
            "Wed, 01 May 2024 10:00:00 +0000",
            "2024-05-01 10:00:00 +0200",
            "2024-05-01T10",
            "2024-05-01T1:00:00",
            "2024-05-01T10:00:00+2:00",
            "2024-05-01T10:00:00+02:60",
            "2024-13-01",
            "2024-02-30",
            "10:00",
        ] {
            assert_eq!(parse(text, Tz::UTC), None, "{text}");
        }
    }
}
