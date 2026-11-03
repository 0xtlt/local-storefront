//! The dates of a search: what the filters of type `time` accept in the reference of each
//! connection (2026-10): `now`, a year (`2024`), a month (`2024-01`), a day (`2021-01-01`), a
//! time with or without a zone (`'2020-10-21T23:39:20Z'`, `'2025-01-01T00:00:00+01:00'`), and a
//! range of two of them (`2021-01-01..2021-01-02`). Shopify documents no relative date (`-7d`)
//! for search, only for customer segments.
//!
//! A date names a period: a year is the whole year in the shop's time zone, `2024-01-15` the
//! whole day, a time its whole second, `now` an instant. `:` matches what falls in the period,
//! `:<` what comes before it, `:<=` what comes before its end, `:>` what comes after it, `:>=`
//! what comes from its start: `created_at:<=2024` is up to the last instant of 2024.

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};

use super::Comparator;

/// A period of time: from `start` to `end`, both included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Period {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl Period {
    /// Whether `date` compares to the period as `comparator` asks.
    pub fn compare(&self, comparator: Comparator, date: DateTime<Utc>) -> bool {
        match comparator {
            Comparator::Eq => self.start <= date && date <= self.end,
            Comparator::Lt => date < self.start,
            Comparator::Le => date <= self.end,
            Comparator::Gt => date > self.end,
            Comparator::Ge => date >= self.start,
        }
    }
}

/// The period `text` names, `now` being `now` and dates without a zone being in `offset`.
pub fn period(text: &str, now: DateTime<Utc>, offset: FixedOffset) -> Option<Period> {
    let text = text.trim();
    if let Some((from, to)) = text.split_once("..") {
        let from = period(from, now, offset)?;
        let to = period(to, now, offset)?;
        return Some(Period {
            start: from.start,
            end: to.end,
        });
    }
    if text.eq_ignore_ascii_case("now") {
        return Some(Period {
            start: now,
            end: now,
        });
    }
    let local = |date: NaiveDateTime| offset.from_local_datetime(&date).single();
    let span = |start: DateTime<FixedOffset>, end: DateTime<FixedOffset>| Period {
        start: start.with_timezone(&Utc),
        end: end.with_timezone(&Utc) - Duration::nanoseconds(1),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let parts: Vec<&str> = text.splitn(3, '-').collect();
    match parts.as_slice() {
        [year] if year.len() == 4 && digits(year) => {
            let year: i32 = year.parse().ok()?;
            let start = local(NaiveDate::from_ymd_opt(year, 1, 1)?.and_hms_opt(0, 0, 0)?)?;
            let end = local(NaiveDate::from_ymd_opt(year + 1, 1, 1)?.and_hms_opt(0, 0, 0)?)?;
            Some(span(start, end))
        }
        [year, month] if year.len() == 4 && digits(year) && digits(month) && month.len() <= 2 => {
            let first = NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, 1)?;
            let next = if first.month() == 12 {
                NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)?
            } else {
                NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)?
            };
            Some(span(
                local(first.and_hms_opt(0, 0, 0)?)?,
                local(next.and_hms_opt(0, 0, 0)?)?,
            ))
        }
        [_, _, day] if digits(day) => {
            let date = NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
            Some(span(
                local(date.and_hms_opt(0, 0, 0)?)?,
                local(date.succ_opt()?.and_hms_opt(0, 0, 0)?)?,
            ))
        }
        _ => time(text, offset),
    }
}

/// A time: `2020-10-21T23:39:20Z`, `2020-10-21T23:39:20.5+02:00`, `2020-10-21 23:39`; its whole
/// second (or minute), unless it has a fraction.
fn time(text: &str, offset: FixedOffset) -> Option<Period> {
    let normalized = text.replacen(' ', "T", 1);
    if let Ok(date) = DateTime::parse_from_rfc3339(&normalized) {
        let start = date.with_timezone(&Utc);
        let fraction = normalized.contains('.');
        return Some(Period {
            start,
            end: if fraction {
                start
            } else {
                start + Duration::seconds(1) - Duration::nanoseconds(1)
            },
        });
    }
    // Without a zone, or without seconds.
    for (format, length) in [
        ("%Y-%m-%dT%H:%M:%S%.f", 0),
        ("%Y-%m-%dT%H:%M:%S", 1),
        ("%Y-%m-%dT%H:%M", 60),
    ] {
        if let Ok(date) = NaiveDateTime::parse_from_str(&normalized, format) {
            let start = offset
                .from_local_datetime(&date)
                .single()?
                .with_timezone(&Utc);
            let end = if length == 0 {
                start
            } else {
                start + Duration::seconds(length) - Duration::nanoseconds(1)
            };
            return Some(Period { start, end });
        }
    }
    // A zone without seconds: `2025-01-01T00:00+01:00`, `2025-01-01T00:00Z`.
    let zoned = match normalized.strip_suffix(['Z', 'z']) {
        Some(rest) => format!("{rest}+00:00"),
        None => normalized,
    };
    let start = DateTime::parse_from_str(&zoned, "%Y-%m-%dT%H:%M%:z")
        .ok()?
        .with_timezone(&Utc);
    Some(Period {
        start,
        end: start + Duration::seconds(60) - Duration::nanoseconds(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn now() -> DateTime<Utc> {
        utc("2026-10-31T12:00:00Z")
    }

    #[test]
    fn reads_the_documented_dates() {
        let zero = FixedOffset::east_opt(0).unwrap();
        let year = period("2024", now(), zero).unwrap();
        assert_eq!(year.start, utc("2024-01-01T00:00:00Z"));
        assert!(year.compare(Comparator::Le, utc("2024-12-31T23:59:59Z")));
        assert!(!year.compare(Comparator::Le, utc("2025-01-01T00:00:00Z")));
        assert!(year.compare(Comparator::Lt, utc("2023-12-31T23:59:59Z")));
        assert!(!year.compare(Comparator::Lt, utc("2024-06-01T00:00:00Z")));

        let day = period("2021-01-01", now(), zero).unwrap();
        assert!(day.compare(Comparator::Eq, utc("2021-01-01T18:00:00Z")));
        assert!(!day.compare(Comparator::Eq, utc("2021-01-02T00:00:00Z")));
        assert!(day.compare(Comparator::Gt, utc("2021-01-02T00:00:00Z")));
        assert!(!day.compare(Comparator::Gt, utc("2021-01-01T23:00:00Z")));
        assert!(day.compare(Comparator::Ge, utc("2021-01-01T00:00:00Z")));

        let month = period("2024-02", now(), zero).unwrap();
        assert!(month.compare(Comparator::Eq, utc("2024-02-29T10:00:00Z")));
        assert!(!month.compare(Comparator::Eq, utc("2024-03-01T00:00:00Z")));

        let instant = period("2020-10-21T23:39:20Z", now(), zero).unwrap();
        assert!(instant.compare(Comparator::Eq, utc("2020-10-21T23:39:20Z")));
        assert!(instant.compare(Comparator::Gt, utc("2020-10-21T23:39:21Z")));
        let zoned = period("2025-01-01T00:00:00+01:00", now(), zero).unwrap();
        assert_eq!(zoned.start, utc("2024-12-31T23:00:00Z"));

        let range = period("2021-01-01..2021-01-02", now(), zero).unwrap();
        assert!(range.compare(Comparator::Eq, utc("2021-01-02T23:00:00Z")));
        assert!(!range.compare(Comparator::Eq, utc("2021-01-03T00:00:00Z")));

        let now_period = period("now", now(), zero).unwrap();
        assert!(now_period.compare(Comparator::Lt, utc("2026-10-31T11:59:59Z")));
        assert!(!now_period.compare(Comparator::Lt, now()));

        assert_eq!(period("-7d", now(), zero), None);
        assert_eq!(period("yesterday", now(), zero), None);
        assert_eq!(period("2024-13-01", now(), zero), None);
    }

    #[test]
    fn reads_dates_without_a_zone_in_the_shops() {
        let paris = FixedOffset::east_opt(3_600).unwrap();
        let day = period("2024-01-15", now(), paris).unwrap();
        assert_eq!(day.start, utc("2024-01-14T23:00:00Z"));
        let local = period("2024-01-15T10:00:00", now(), paris).unwrap();
        assert_eq!(local.start, utc("2024-01-15T09:00:00Z"));
        let minute = period("2024-01-15 10:00", now(), paris).unwrap();
        assert!(minute.compare(Comparator::Eq, utc("2024-01-15T09:00:59Z")));
    }
}
