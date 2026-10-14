//! Strings, units and numbers inside text that is not parsed further: selectors, media
//! queries.

use super::super::number::print_css_number;

/// The units CSS has, as they are written.
const CSS_UNITS: [&str; 62] = [
    "em", "rem", "ex", "rex", "cap", "rcap", "ch", "rch", "ic", "ric", "lh", "rlh", "vw", "svw",
    "lvw", "dvw", "vh", "svh", "lvh", "dvh", "vi", "svi", "lvi", "dvi", "vb", "svb", "lvb", "dvb",
    "vmin", "svmin", "lvmin", "dvmin", "vmax", "svmax", "lvmax", "dvmax", "cm", "mm", "Q", "in",
    "pt", "pc", "px", "deg", "grad", "rad", "turn", "s", "ms", "Hz", "kHz", "dpi", "dpcm", "dppx",
    "x", "cqw", "cqh", "cqi", "cqb", "cqmin", "cqmax", "fr",
];

fn known_unit(unit: &str) -> Option<&'static str> {
    CSS_UNITS
        .iter()
        .find(|known| known.eq_ignore_ascii_case(unit))
        .copied()
}

/// A unit in the case CSS writes it in, when it is one CSS has.
pub(super) fn print_unit(unit: &str) -> String {
    known_unit(unit).map_or_else(|| unit.to_string(), str::to_string)
}

/// Prettier's `printString`: the string in the preferred quotes, unless it holds more of them
/// than of the others.
pub(super) fn print_string(raw: &str, single_quote: bool) -> String {
    let content = &raw[1..raw.len() - 1];
    let (preferred, alternate) = if single_quote {
        ('\'', '"')
    } else {
        ('"', '\'')
    };
    let count = |quote: char| content.matches(quote).count();
    let enclosing = if count(preferred) > count(alternate) {
        alternate
    } else {
        preferred
    };
    if raw.starts_with(enclosing) {
        return raw.to_string();
    }
    let other = if enclosing == '"' { '\'' } else { '"' };
    let mut printed = String::with_capacity(raw.len());
    printed.push(enclosing);
    let mut chars = content.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek().copied() {
                // The other quote needs no escape anymore.
                Some(escaped) if escaped == other => {
                    printed.push(escaped);
                    chars.next();
                }
                Some(escaped @ ('"' | '\'' | '\\')) => {
                    printed.push('\\');
                    printed.push(escaped);
                    chars.next();
                }
                _ => printed.push('\\'),
            },
            quote if quote == enclosing => {
                printed.push('\\');
                printed.push(quote);
            }
            other => printed.push(other),
        }
    }
    printed.push(enclosing);
    printed
}

/// The end of the string that starts at `start`: `(["'])(?:(?!\1)[^\\]|\\.)*\1`.
fn string_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let quote = bytes[start];
    let mut pos = start + 1;
    while pos < bytes.len() {
        match bytes[pos] {
            b'\\' => {
                pos += 1;
                pos += text[pos..].chars().next()?.len_utf8();
            }
            byte if byte == quote => return Some(pos + 1),
            _ => pos += 1,
        }
    }
    None
}

/// Prettier's `adjustStrings`: every string of a text, in the preferred quotes.
pub(super) fn adjust_strings(text: &str, single_quote: bool) -> String {
    let bytes = text.as_bytes();
    let mut adjusted = String::with_capacity(text.len());
    let mut pos = 0;
    let mut copied = 0;
    while pos < bytes.len() {
        if matches!(bytes[pos], b'"' | b'\'')
            && let Some(end) = string_end(text, pos)
        {
            adjusted.push_str(&text[copied..pos]);
            adjusted.push_str(&print_string(&text[pos..end], single_quote));
            pos = end;
            copied = end;
        } else {
            pos += 1;
        }
    }
    adjusted.push_str(&text[copied..]);
    adjusted
}

/// `(?:\d*\.\d+|\d+\.?)(?:e[+-]?\d+)?` at `start`: where the number ends.
fn number_end(bytes: &[u8], start: usize) -> Option<usize> {
    let digits = |mut pos: usize| {
        while bytes.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1;
        }
        pos
    };
    let whole_end = digits(start);
    let mut end = if bytes.get(whole_end) == Some(&b'.') && digits(whole_end + 1) > whole_end + 1 {
        digits(whole_end + 1)
    } else if whole_end > start {
        whole_end + usize::from(bytes.get(whole_end) == Some(&b'.'))
    } else {
        return None;
    };
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exponent = end + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        if digits(exponent) > exponent {
            end = digits(exponent);
        }
    }
    Some(end)
}

fn is_word_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic() || c >= '\u{80}'
}

fn is_word_char(c: char) -> bool {
    c == '_' || c == '-' || c.is_ascii_alphanumeric() || c >= '\u{80}'
}

/// Prettier's `adjustNumbers`: the numbers of a text that are not part of a word, printed as
/// CSS prints numbers, with their unit in its usual case.
pub(super) fn adjust_numbers(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut adjusted = String::with_capacity(text.len());
    let mut pos = 0;
    'scan: while pos < bytes.len() {
        let c = text[pos..].chars().next().unwrap();
        // A string is left as it is.
        if matches!(c, '"' | '\'')
            && let Some(end) = string_end(text, pos)
        {
            adjusted.push_str(&text[pos..end]);
            pos = end;
            continue;
        }
        // A number glued to the end of a word is part of the word: `col-12`, `h1`.
        let word_start = pos + usize::from(matches!(c, '$' | '@'));
        if text[word_start..].chars().next().is_some_and(is_word_start) {
            let word_end = word_start
                + text[word_start..]
                    .char_indices()
                    .find(|(_, c)| !is_word_char(*c))
                    .map_or(text.len() - word_start, |(index, _)| index);
            let mut candidates: Vec<usize> = text[word_start..word_end]
                .char_indices()
                .map(|(index, c)| word_start + index + c.len_utf8())
                .collect();
            candidates.reverse();
            for candidate in candidates {
                if let Some(number_end) = number_end(bytes, candidate) {
                    let unit_end = number_end
                        + bytes[number_end..]
                            .iter()
                            .take_while(|byte| byte.is_ascii_alphabetic())
                            .count();
                    adjusted.push_str(&text[pos..unit_end]);
                    pos = unit_end;
                    continue 'scan;
                }
            }
        }
        if let Some(number_end) = number_end(bytes, pos) {
            let unit_end = number_end
                + bytes[number_end..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_alphabetic())
                    .count();
            let unit = &text[number_end..unit_end];
            if unit.is_empty() || unit.eq_ignore_ascii_case("n") || known_unit(unit).is_some() {
                adjusted.push_str(&print_css_number(&text[pos..number_end]));
                adjusted.push_str(&match unit {
                    "" => String::new(),
                    unit => print_unit(&unit.to_lowercase()),
                });
            } else {
                adjusted.push_str(&text[pos..unit_end]);
            }
            pos = unit_end;
            continue;
        }
        adjusted.push(c);
        pos += c.len_utf8();
    }
    adjusted
}

/// Prettier's `maybeToLowerCase`: lowercase, unless the text holds what case matters in.
pub(super) fn maybe_to_lower_case(value: &str) -> String {
    if value.contains(['$', '@', '#'])
        || value.starts_with('%')
        || value.starts_with("--")
        || value.starts_with(":--")
        || (value.contains('(') && value.contains(')'))
    {
        value.to_string()
    } else {
        value.to_lowercase()
    }
}

/// Prettier's `isNextLineEmpty`: whether a blank line follows what ends at `end`.
pub(super) fn is_next_line_empty(source: &str, end: usize) -> bool {
    let mut rest = &source[end.min(source.len())..];
    loop {
        let before = rest.len();
        rest = rest.trim_start_matches([',', ';', ' ', '\t']);
        if let Some(after) = rest.strip_prefix("/*") {
            // A comment that stays on the line.
            match after.find("*/") {
                Some(close) if !after[..close].contains(['\n', '\r']) => rest = &after[close + 2..],
                _ => {}
            }
        }
        rest = rest.trim_start_matches([' ', '\t']);
        if rest.len() == before {
            break;
        }
    }
    let Some(rest) = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix(['\n', '\r']))
    else {
        return false;
    };
    rest.trim_start_matches([' ', '\t'])
        .starts_with(['\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_take_the_preferred_quotes() {
        assert_eq!(print_string("\"a\"", true), "'a'");
        assert_eq!(print_string("\"it's\"", true), "\"it's\"");
        assert_eq!(print_string("\"a\\\"b\"", true), "'a\"b'");
        assert_eq!(print_string("'a\"b'", false), "'a\"b'");
        assert_eq!(adjust_strings("[a=\"b\"] 'c'", true), "[a='b'] 'c'");
    }

    #[test]
    fn numbers_outside_words_are_adjusted() {
        assert_eq!(adjust_numbers("1.50em"), "1.5em");
        assert_eq!(adjust_numbers("2N+1"), "2n+1");
        assert_eq!(adjust_numbers("col-1.50 h1 a1.50"), "col-1.50 h1 a1.50");
        assert_eq!(adjust_numbers("10PX .5X 1.0foo"), "10px 0.5x 1.0foo");
        assert_eq!(adjust_numbers("'1.50' 1.50"), "'1.50' 1.5");
    }

    #[test]
    fn finds_the_blank_line_after_a_node() {
        assert!(is_next_line_empty("a;\n\nb", 1));
        assert!(is_next_line_empty("a; /* c */\n  \nb", 1));
        assert!(!is_next_line_empty("a;\nb", 1));
        assert!(!is_next_line_empty("a; b\n\n", 1));
    }
}
