//! The small text helpers of graphql-codegen: the name case it converts to, the sort order of
//! graphql-js, and the layout of comments.

use std::cmp::Ordering;

/// `pascalCase` of `change-case-all`, as graphql-codegen applies it to every name it writes:
/// the words of the name (split where a lower case letter or a digit meets an upper case letter,
/// and at anything that is not a letter or a digit) get a capital first letter and a lower case
/// rest, and a word after the first that starts with a digit gets a `_` in front.
pub(super) fn pascal_case(input: &str) -> String {
    let split = split_before_capitals(&split_before_capitals(input, false), true);
    let mut words: Vec<String> = vec![String::new()];
    for c in split.chars() {
        if c == '\0' || !c.is_ascii_alphanumeric() {
            if !words.last().is_some_and(String::is_empty) {
                words.push(String::new());
            }
        } else {
            words.last_mut().unwrap().push(c);
        }
    }
    let mut result = String::new();
    // A word is only kept when it holds something; a separator at either end gives no word.
    let words: Vec<&String> = words.iter().filter(|word| !word.is_empty()).collect();
    for (index, word) in words.iter().enumerate() {
        let mut chars = word.chars();
        let first = chars.next().unwrap();
        let rest = chars.as_str().to_ascii_lowercase();
        if index > 0 && first.is_ascii_digit() {
            result.push('_');
            result.push(first);
        } else {
            result.push(first.to_ascii_uppercase());
        }
        result.push_str(&rest);
    }
    result
}

/// The two regular expressions `no-case` runs before it strips the separators, written by hand:
/// `([a-z0-9])([A-Z])` (`acronym` false) and `([A-Z])([A-Z][a-z])` (`acronym` true), each
/// replaced by its two groups with a NUL between them. Matches do not overlap, as in a global
/// regular expression.
fn split_before_capitals(input: &str, acronym: bool) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let matched = if acronym {
            i + 2 < chars.len()
                && chars[i].is_ascii_uppercase()
                && chars[i + 1].is_ascii_uppercase()
                && chars[i + 2].is_ascii_lowercase()
        } else {
            i + 1 < chars.len()
                && (chars[i].is_ascii_lowercase() || chars[i].is_ascii_digit())
                && chars[i + 1].is_ascii_uppercase()
        };
        if matched {
            out.push(chars[i]);
            out.push('\0');
            if acronym {
                // The second group is two characters long.
                out.push(chars[i + 1]);
                out.push(chars[i + 2]);
                i += 3;
            } else {
                out.push(chars[i + 1]);
                i += 2;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// What graphql-codegen's `convertName` writes for a name: `pascal_case` of each part between
/// underscores (the underscores stay), or of the whole name when `transform_underscore` is set
/// (the values of enums).
pub(super) fn convert_name(name: &str, transform_underscore: bool) -> String {
    if transform_underscore {
        return pascal_case(name);
    }
    name.split('_')
        .map(pascal_case)
        .collect::<Vec<_>>()
        .join("_")
}

/// The order graphql-js sorts names in (`lexicographicSortSchema`): by character code, with the
/// digits of both names compared as numbers.
pub(super) fn natural_compare(a: &str, b: &str) -> Ordering {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (number_a, next_a) = read_number(&a, i);
            let (number_b, next_b) = read_number(&b, j);
            i = next_a;
            j = next_b;
            match number_a.cmp(&number_b) {
                Ordering::Equal => {}
                other => return other,
            }
        } else {
            match a[i].cmp(&b[j]) {
                Ordering::Equal => {}
                other => return other,
            }
            i += 1;
            j += 1;
        }
    }
    a.len().cmp(&b.len())
}

/// The number at `at`, as graphql-js reads it: digits are taken while the number so far is not
/// zero, so `007` is the numbers 0, 0, 7. Returns the number and where it ends.
fn read_number(chars: &[char], at: usize) -> (u64, usize) {
    let mut number = 0u64;
    let mut i = at;
    loop {
        number = number
            .saturating_mul(10)
            .saturating_add(u64::from(chars[i] as u32 - '0' as u32));
        i += 1;
        if !(i < chars.len() && chars[i].is_ascii_digit() && number > 0) {
            return (number, i);
        }
    }
}

/// Two spaces for each level.
pub(super) fn indent(text: &str, level: usize) -> String {
    format!("{}{text}", "  ".repeat(level))
}

/// Removes the spaces in front of each line break, as graphql-codegen does to everything it
/// writes.
pub(super) fn strip_trailing_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut spaces = 0;
    for c in text.chars() {
        match c {
            ' ' => spaces += 1,
            '\n' => {
                spaces = 0;
                out.push('\n');
            }
            other => {
                out.push_str(&" ".repeat(spaces));
                spaces = 0;
                out.push(other);
            }
        }
    }
    out.push_str(&" ".repeat(spaces));
    out
}

/// A description as a `/** */` comment, indented `level` times, with a line break after it. An
/// empty description is no comment.
pub(super) fn transform_comment(comment: &str, level: usize) -> String {
    if comment.is_empty() {
        return String::new();
    }
    let comment = comment.replace("*/", "*\\/");
    let lines: Vec<&str> = comment.split('\n').collect();
    if lines.len() == 1 {
        return indent(&format!("/** {} */\n", lines[0]), level);
    }
    let mut all = vec!["/**".to_string()];
    all.extend(lines.iter().map(|line| format!(" * {line}")));
    all.push(" */\n".to_string());
    strip_trailing_spaces(
        &all.iter()
            .map(|line| indent(line, level))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// `'value'`, or the bare value when it reads as a plain number.
pub(super) fn wrap_with_single_quotes(value: &str, skip_numeric_check: bool) -> String {
    if !skip_numeric_check && reads_as_number(value) {
        return value.to_string();
    }
    format!("'{value}'")
}

/// JavaScript's `!Number.isNaN(parseInt(value)) && parseFloat(value).toString() === value`, for
/// the values of an enum: a decimal integer without a sign, a leading zero or a fraction.
fn reads_as_number(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|c| c.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
        && value.len() < 16
}
