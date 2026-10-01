//! Time values, lenient date parsing (in the spirit of Ruby's `Time.parse`) and a
//! Ruby-compatible `strftime`.

use std::any::Any;
use std::borrow::Cow;

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset,
    TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;

use crate::context::Context;
use crate::value::{Object, Value};

/// A point in time, displayed in a given time zone.
#[derive(Clone, Debug)]
pub struct Time(pub DateTime<Tz>);

impl Time {
    pub fn value(at: DateTime<Tz>) -> Value {
        Value::object(Time(at))
    }

    /// Parses an ISO 8601 timestamp into a time displayed in `tz`.
    pub fn parse_iso(input: &str, tz: Tz) -> Option<Time> {
        DateTime::parse_from_rfc3339(input)
            .ok()
            .map(|at| Time(at.with_timezone(&tz)))
    }

    /// ISO 8601 with a numeric offset, e.g. `2024-01-15T10:00:00-05:00`.
    pub fn iso8601(&self) -> String {
        self.0.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
    }
}

impl Object for Time {
    fn type_name(&self) -> &str {
        "time"
    }

    fn to_value(&self) -> Option<Value> {
        Some(Value::Int(self.0.timestamp()))
    }

    /// Ruby's `Time#to_s`.
    fn render(&self) -> Cow<'_, str> {
        let zone = if self.0.offset().fix().local_minus_utc() == 0 {
            "UTC".to_string()
        } else {
            self.0.format("%z").to_string()
        };
        Cow::Owned(format!("{} {}", self.0.format("%Y-%m-%d %H:%M:%S"), zone))
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::String(self.iso8601())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Ruby Liquid's `Utils.to_date`: the time a value stands for, if any.
pub fn to_time(input: &Value, ctx: &Context) -> Option<DateTime<Tz>> {
    let tz = ctx.time_zone();
    match input {
        Value::Object(_) => input
            .downcast::<Time>()
            .map(|time| time.0.with_timezone(&tz)),
        Value::Int(seconds) => tz.timestamp_opt(*seconds, 0).single(),
        Value::Str(s) => {
            if s.is_empty() {
                return None;
            }
            let lower = s.to_lowercase();
            if lower == "now" || lower == "today" {
                return Some(ctx.now().with_timezone(&tz));
            }
            if lower.bytes().all(|b| b.is_ascii_digit()) {
                return tz.timestamp_opt(lower.parse().ok()?, 0).single();
            }
            parse_time(&lower, tz, ctx.now())
        }
        _ => None,
    }
}

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
const WEEKDAYS: [&str; 7] = [
    "sunday",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
];

fn local(tz: Tz, naive: NaiveDateTime) -> Option<DateTime<Tz>> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(at) => Some(at),
        LocalResult::Ambiguous(earliest, _) => Some(earliest),
        // The local time falls in a daylight-saving gap: move forward like Ruby does.
        LocalResult::None => tz
            .from_local_datetime(&(naive + Duration::hours(1)))
            .earliest(),
    }
}

/// Parses a lowercase date/time string. Components that are missing default to the current
/// date or to midnight. Times without a zone are local to `tz`.
pub fn parse_time(input: &str, tz: Tz, now: DateTime<Utc>) -> Option<DateTime<Tz>> {
    let input = input.trim();
    if let Ok(at) = DateTime::parse_from_rfc3339(input) {
        return Some(at.with_timezone(&tz));
    }
    if let Ok(at) = DateTime::parse_from_rfc2822(input) {
        return Some(at.with_timezone(&tz));
    }

    let mut rest = input.to_string();
    let time = extract_time(&mut rest);
    let offset = if time.is_some() {
        extract_offset(&mut rest)
    } else {
        None
    };
    let date = extract_date(&rest, now.with_timezone(&tz).date_naive());
    if time.is_none() && date.is_none() {
        return None;
    }
    let date = date.unwrap_or_else(|| now.with_timezone(&tz).date_naive());
    let naive = NaiveDateTime::new(date, time.unwrap_or(NaiveTime::MIN));
    match offset {
        Some(seconds) => {
            let utc = naive - Duration::seconds(i64::from(seconds));
            Some(Utc.from_utc_datetime(&utc).with_timezone(&tz))
        }
        None => local(tz, naive),
    }
}

/// Finds `h:mm[:ss[.fff]] [am|pm]`, removes it from `text` and returns it.
fn extract_time(text: &mut String) -> Option<NaiveTime> {
    let bytes = text.as_bytes();
    let colon = (1..bytes.len().saturating_sub(2)).find(|&i| {
        bytes[i] == b':'
            && bytes[i - 1].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
    })?;
    let start = if colon >= 2 && bytes[colon - 2].is_ascii_digit() {
        colon - 2
    } else {
        colon - 1
    };
    let mut hour: u32 = text[start..colon].parse().ok()?;
    let minute: u32 = text[colon + 1..colon + 3].parse().ok()?;
    let mut end = colon + 3;
    let mut second = 0;
    let mut nanos = 0;
    if bytes.get(end) == Some(&b':') && bytes.get(end + 2).is_some_and(u8::is_ascii_digit) {
        second = text[end + 1..end + 3].parse().ok()?;
        end += 3;
        if matches!(bytes.get(end), Some(b'.' | b',')) {
            let digits = bytes[end + 1..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digits > 0 {
                let fraction = format!("{:0<9}", &text[end + 1..end + 1 + digits.min(9)]);
                nanos = fraction.parse().unwrap_or(0);
                end += 1 + digits;
            }
        }
    }
    let after = text[end..].trim_start();
    let skipped = text.len() - end - after.len();
    if after.starts_with("am") || after.starts_with("pm") {
        if after.starts_with("pm") && hour < 12 {
            hour += 12;
        } else if after.starts_with("am") && hour == 12 {
            hour = 0;
        }
        end += skipped + 2;
    }
    // Drop the ISO `t` separator between the date and the time.
    let prefix_end = if start > 0 && bytes[start - 1] == b't' {
        start - 1
    } else {
        start
    };
    let time = NaiveTime::from_hms_nano_opt(hour, minute, second.min(59), nanos)?;
    text.replace_range(prefix_end..end, " ");
    Some(time)
}

/// Finds a UTC offset (`z`, `utc`, `gmt`, `+hh:mm`, common abbreviations), removes it from
/// `text` and returns it in seconds east of UTC.
fn extract_offset(text: &mut String) -> Option<i32> {
    const NAMED: [(&str, i32); 12] = [
        ("utc", 0),
        ("gmt", 0),
        ("est", -5),
        ("edt", -4),
        ("cst", -6),
        ("cdt", -5),
        ("mst", -7),
        ("mdt", -6),
        ("pst", -8),
        ("pdt", -7),
        ("cet", 1),
        ("cest", 2),
    ];
    let tokens: Vec<(usize, &str)> = text
        .split_whitespace()
        .map(|token| (token.as_ptr() as usize - text.as_ptr() as usize, token))
        .collect();
    for (start, token) in tokens {
        let found = if token == "z" {
            Some(0)
        } else if let Some((_, hours)) = NAMED.iter().find(|(name, _)| *name == token) {
            Some(hours * 3600)
        } else if let Some(digits) = token.strip_prefix(['+', '-']) {
            let sign = if token.starts_with('-') { -1 } else { 1 };
            let digits: String = digits.chars().filter(|c| *c != ':').collect();
            if (digits.len() == 4 || digits.len() == 2)
                && digits.bytes().all(|b| b.is_ascii_digit())
            {
                let hours: i32 = digits[..2].parse().ok()?;
                let minutes: i32 = if digits.len() == 4 {
                    digits[2..].parse().ok()?
                } else {
                    0
                };
                Some(sign * (hours * 3600 + minutes * 60))
            } else {
                None
            }
        } else {
            None
        };
        if let Some(seconds) = found {
            let end = start + token.len();
            text.replace_range(start..end, " ");
            return Some(seconds);
        }
    }
    None
}

fn month_from_name(token: &str) -> Option<u32> {
    let token = token.trim_end_matches('.');
    if token.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|month| {
            month.starts_with(token)
                || (token.len() == 4 && token == "sept" && *month == "september")
        })
        .map(|index| index as u32 + 1)
}

fn extract_date(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|token| !token.is_empty())
        .collect();
    // Numeric forms: yyyy-mm-dd, yyyy/mm/dd, mm/dd/yyyy, dd-mm-yyyy, dd.mm.yyyy.
    for token in &tokens {
        for separator in ['-', '/', '.'] {
            let parts: Vec<&str> = token.split(separator).collect();
            if parts.len() != 3
                || !parts
                    .iter()
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            {
                continue;
            }
            let numbers: Vec<u32> = parts.iter().filter_map(|p| p.parse().ok()).collect();
            if numbers.len() != 3 {
                continue;
            }
            let (year, month, day) = if parts[0].len() == 4 {
                (numbers[0], numbers[1], numbers[2])
            } else if parts[2].len() == 4 && separator == '/' {
                (numbers[2], numbers[0], numbers[1])
            } else if parts[2].len() == 4 {
                (numbers[2], numbers[1], numbers[0])
            } else {
                continue;
            };
            return NaiveDate::from_ymd_opt(year as i32, month, day);
        }
    }
    // Forms with a month name: "15 january 2024", "jan 15, 2024", "january 2024".
    let month_index = tokens
        .iter()
        .position(|token| month_from_name(token).is_some())?;
    let month = month_from_name(tokens[month_index])?;
    let mut day = None;
    let mut year = None;
    for (index, token) in tokens.iter().enumerate() {
        if index == month_index
            || WEEKDAYS
                .iter()
                .any(|weekday| weekday.starts_with(token.trim_end_matches('.')) && token.len() >= 3)
        {
            continue;
        }
        let digits = token.trim_end_matches(|c: char| c.is_ascii_alphabetic());
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(number) = digits.parse::<u32>() else {
            continue;
        };
        if digits.len() >= 3 {
            year = Some(number as i32);
        } else if day.is_none() {
            day = Some(number);
        } else if year.is_none() {
            year = Some(number as i32 + if number < 69 { 2000 } else { 1900 });
        }
    }
    NaiveDate::from_ymd_opt(year.unwrap_or(today.year()), month, day.unwrap_or(1))
}

#[derive(Default)]
struct Spec {
    no_padding: bool,
    padding: Option<char>,
    upcase: bool,
    swap_case: bool,
    width: Option<usize>,
    colons: usize,
}

impl Spec {
    fn number(&self, value: i64, default_width: usize, default_padding: char) -> String {
        if self.no_padding {
            return value.to_string();
        }
        let padding = self.padding.unwrap_or(default_padding);
        let width = self.width.unwrap_or(default_width);
        let digits = value.abs().to_string();
        let sign = if value < 0 { "-" } else { "" };
        let pad = width.saturating_sub(digits.len() + sign.len());
        if padding == '0' {
            format!("{sign}{}{digits}", "0".repeat(pad))
        } else {
            format!("{}{sign}{digits}", " ".repeat(pad))
        }
    }

    fn text(&self, value: &str) -> String {
        let value = if self.upcase || self.swap_case {
            value.to_uppercase()
        } else {
            value.to_string()
        };
        self.pad_text(value)
    }

    fn pad_text(&self, value: String) -> String {
        match self.width {
            Some(width) if !self.no_padding && value.chars().count() < width => {
                let padding = self.padding.unwrap_or(' ');
                let pad: String =
                    std::iter::repeat_n(padding, width - value.chars().count()).collect();
                format!("{pad}{value}")
            }
            _ => value,
        }
    }
}

fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Formats a time with Ruby's `Time#strftime` directives, flags and widths.
///
/// Fails with Ruby's `invalid format` message when the format ends in an incomplete directive.
pub fn strftime(time: &DateTime<Tz>, format: &str) -> std::result::Result<String, String> {
    let invalid = || format!("invalid format: {format}");
    let mut out = String::with_capacity(format.len() + 16);
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut spec = Spec::default();
        let mut raw = String::from("%");
        // Flags and width.
        while let Some(&next) = chars.peek() {
            match next {
                '-' => spec.no_padding = true,
                '_' => spec.padding = Some(' '),
                '0' if spec.width.is_none() => spec.padding = Some('0'),
                '^' => spec.upcase = true,
                '#' => spec.swap_case = true,
                '0'..='9' => {
                    let digit = next.to_digit(10).unwrap_or(0) as usize;
                    spec.width = Some(
                        spec.width
                            .unwrap_or(0)
                            .saturating_mul(10)
                            .saturating_add(digit),
                    );
                }
                _ => break,
            }
            raw.push(next);
            chars.next();
        }
        if spec.width == Some(0) {
            spec.width = None;
        }
        while chars.peek() == Some(&':') {
            spec.colons += 1;
            raw.push(':');
            chars.next();
        }
        let Some(mut conversion) = chars.next() else {
            // `%:` is printed as is; a dangling `%`, flag or width is an error.
            if spec.colons > 0 {
                out.push_str(&raw);
                continue;
            }
            return Err(invalid());
        };
        raw.push(conversion);
        // The `E` and `O` modifiers select alternative representations we do not have: they are
        // accepted in front of the directives they apply to and ignored.
        if conversion == 'E' || conversion == 'O' {
            let allowed = if conversion == 'E' {
                "cCxXyY"
            } else {
                "deHkIlmMSuUVwWy"
            };
            match chars.peek() {
                Some(&next) if allowed.contains(next) => {
                    conversion = next;
                    raw.push(next);
                    chars.next();
                }
                _ => {
                    out.push_str(&raw);
                    continue;
                }
            }
        }
        if spec.colons > 0 && (conversion != 'z' || spec.colons > 3) {
            out.push_str(&raw);
            continue;
        }
        match format_directive(time, conversion, &spec) {
            Some(text) => out.push_str(&text),
            None => out.push_str(&raw),
        }
    }
    Ok(out)
}

fn format_directive(time: &DateTime<Tz>, conversion: char, spec: &Spec) -> Option<String> {
    let hour12 = match time.hour() % 12 {
        0 => 12,
        hour => hour,
    };
    let month_name = MONTHS[time.month0() as usize];
    let weekday_name = WEEKDAYS[time.weekday().num_days_from_sunday() as usize];
    let composite = |format: &str| {
        let text = strftime(time, format).unwrap_or_default();
        spec.pad_text(if spec.upcase {
            text.to_uppercase()
        } else {
            text
        })
    };
    Some(match conversion {
        'Y' => spec.number(i64::from(time.year()), 1, '0'),
        'C' => spec.number(i64::from(time.year().div_euclid(100)), 2, '0'),
        'y' => spec.number(i64::from(time.year().rem_euclid(100)), 2, '0'),
        'm' => spec.number(i64::from(time.month()), 2, '0'),
        'B' => spec.text(&capitalize(month_name)),
        'b' | 'h' => spec.text(&capitalize(&month_name[..3])),
        'd' => spec.number(i64::from(time.day()), 2, '0'),
        'e' => spec.number(i64::from(time.day()), 2, ' '),
        'j' => spec.number(i64::from(time.ordinal()), 3, '0'),
        'H' => spec.number(i64::from(time.hour()), 2, '0'),
        'k' => spec.number(i64::from(time.hour()), 2, ' '),
        'I' => spec.number(i64::from(hour12), 2, '0'),
        'l' => spec.number(i64::from(hour12), 2, ' '),
        'P' => spec.text(if time.hour() < 12 { "am" } else { "pm" }),
        'p' => {
            let meridian = if time.hour() < 12 { "AM" } else { "PM" };
            if spec.swap_case {
                spec.pad_text(meridian.to_lowercase())
            } else {
                spec.text(meridian)
            }
        }
        'M' => spec.number(i64::from(time.minute()), 2, '0'),
        'S' => spec.number(i64::from(time.second()), 2, '0'),
        'L' | 'N' => {
            let digits = spec.width.unwrap_or(if conversion == 'L' { 3 } else { 9 });
            let nanos = format!("{:09}", time.nanosecond() % 1_000_000_000);
            if digits <= 9 {
                nanos[..digits].to_string()
            } else {
                format!("{nanos}{}", "0".repeat(digits - 9))
            }
        }
        'z' => {
            let total = time.offset().fix().local_minus_utc();
            let sign = if total < 0 { '-' } else { '+' };
            let (hours, minutes, seconds) = (
                total.abs() / 3600,
                total.abs() % 3600 / 60,
                total.abs() % 60,
            );
            let body = match spec.colons {
                0 => format!("{hours:02}{minutes:02}"),
                1 => format!("{hours:02}:{minutes:02}"),
                2 => format!("{hours:02}:{minutes:02}:{seconds:02}"),
                _ if seconds != 0 => format!("{hours:02}:{minutes:02}:{seconds:02}"),
                _ if minutes != 0 => format!("{hours:02}:{minutes:02}"),
                _ => format!("{hours:02}"),
            };
            let fill = spec.width.unwrap_or(0).saturating_sub(body.len() + 1);
            format!("{sign}{}{body}", "0".repeat(fill))
        }
        'Z' => {
            let name = time.format("%Z").to_string();
            if spec.swap_case {
                spec.pad_text(name.to_lowercase())
            } else {
                spec.text(&name)
            }
        }
        'A' => spec.text(&capitalize(weekday_name)),
        'a' => spec.text(&capitalize(&weekday_name[..3])),
        'u' => spec.number(i64::from(time.weekday().number_from_monday()), 1, '0'),
        'w' => spec.number(i64::from(time.weekday().num_days_from_sunday()), 1, '0'),
        'G' => spec.number(i64::from(time.iso_week().year()), 1, '0'),
        'g' => spec.number(i64::from(time.iso_week().year().rem_euclid(100)), 2, '0'),
        'V' => spec.number(i64::from(time.iso_week().week()), 2, '0'),
        'U' => {
            let week = (time.ordinal0() + 7 - time.weekday().num_days_from_sunday()) / 7;
            spec.number(i64::from(week), 2, '0')
        }
        'W' => {
            let week = (time.ordinal0() + 7 - time.weekday().num_days_from_monday()) / 7;
            spec.number(i64::from(week), 2, '0')
        }
        's' => spec.number(time.timestamp(), 1, '0'),
        'n' => spec.pad_text("\n".to_string()),
        't' => spec.pad_text("\t".to_string()),
        '%' => spec.pad_text("%".to_string()),
        'c' => composite("%a %b %e %H:%M:%S %Y"),
        'D' | 'x' => composite("%m/%d/%y"),
        'F' => composite("%Y-%m-%d"),
        'v' => composite("%e-%^b-%Y"),
        'X' | 'T' => composite("%H:%M:%S"),
        'r' => composite("%I:%M:%S %p"),
        'R' => composite("%H:%M"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(input: &str) -> DateTime<Tz> {
        let now = Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap();
        parse_time(&input.to_lowercase(), Tz::UTC, now)
            .unwrap_or_else(|| panic!("cannot parse {input}"))
    }

    #[test]
    fn parses_common_formats() {
        assert_eq!(
            at("2024-01-15T10:30:00Z").to_rfc3339(),
            "2024-01-15T10:30:00+00:00"
        );
        assert_eq!(
            at("2024-01-15 10:30:00 -0500").to_rfc3339(),
            "2024-01-15T15:30:00+00:00"
        );
        assert_eq!(at("2024-01-15").to_rfc3339(), "2024-01-15T00:00:00+00:00");
        assert_eq!(
            at("March 14, 2016").to_rfc3339(),
            "2016-03-14T00:00:00+00:00"
        );
        assert_eq!(
            at("14 Mar 2016 3:05 pm").to_rfc3339(),
            "2016-03-14T15:05:00+00:00"
        );
        assert_eq!(
            at("Mon, 15 Jan 2024 10:00:00 +0000").to_rfc3339(),
            "2024-01-15T10:00:00+00:00"
        );
        assert_eq!(at("10:30").to_rfc3339(), "2024-06-01T10:30:00+00:00");
    }

    #[test]
    fn formats_like_ruby() {
        let time = at("2024-01-05T09:07:03Z");
        let strftime = |time: &DateTime<Tz>, format: &str| strftime(time, format).unwrap();
        assert_eq!(strftime(&time, "%Y-%m-%d %H:%M:%S"), "2024-01-05 09:07:03");
        assert_eq!(strftime(&time, "%a %b %e, %-d %j"), "Fri Jan  5, 5 005");
        assert_eq!(
            strftime(&time, "%B %-d, %Y %l:%M %p %P"),
            "January 5, 2024  9:07 AM am"
        );
        assert_eq!(
            strftime(&time, "%^a %^B %#p %10A|%-m|%_m|%05d"),
            "FRI JANUARY am     Friday|1| 1|00005"
        );
        assert_eq!(
            strftime(&time, "%z %:z %Z %s %% %Q"),
            "+0000 +00:00 UTC 1704445623 % %Q"
        );
        assert_eq!(
            strftime(&time, "%U %W %V %G %u %w %C %y"),
            "00 01 01 2024 5 5 20 24"
        );
        assert_eq!(
            strftime(&time, "%F %T %D %R %r %c"),
            "2024-01-05 09:07:03 01/05/24 09:07 09:07:03 AM Fri Jan  5 09:07:03 2024"
        );
        assert_eq!(strftime(&time, "%L %3N %N"), "000 000 000000000");
    }
}
