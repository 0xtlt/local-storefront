//! String filters.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};

use super::{check_arity, ruby_args};
use crate::context::Context;
use crate::environment::Environment;
use crate::error::{Error, Result};
use crate::lax;
use crate::number::{is_ruby_space, to_integer};
use crate::value::Value;
use crate::variable::FilterArgs;

type Args<'a> = &'a FilterArgs;

fn no_args(args: Args<'_>) -> Result<()> {
    check_arity(&ruby_args(args), 0, 0)
}

/// Escapes `& < > " '` like Ruby's `CGI.escapeHTML`.
pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Percent-encodes like Ruby's `CGI.escape`: spaces become `+`.
pub fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn downcase(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    Ok(Value::from(input.to_str().to_lowercase()))
}

fn upcase(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    Ok(Value::from(input.to_str().to_uppercase()))
}

fn capitalize(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    let s = input.to_str();
    let mut chars = s.chars();
    Ok(Value::from(match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect::<String>(),
        None => String::new(),
    }))
}

fn escape(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    if input.is_nil() {
        return Ok(Value::Nil);
    }
    Ok(Value::from(escape_html(&input.to_str())))
}

/// Escapes without touching entities that are already escaped:
/// `/["><']|&(?!([a-zA-Z]+|(#\d+));)/`.
fn escape_once(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    let s = input.to_str();
    let mut out = String::with_capacity(s.len() + 8);
    for (i, c) in s.char_indices() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '&' if !starts_entity(&s[i + 1..]) => out.push_str("&amp;"),
            c => out.push(c),
        }
    }
    Ok(Value::from(out))
}

fn starts_entity(rest: &str) -> bool {
    let Some(end) = rest.find(';') else {
        return false;
    };
    let name = &rest[..end];
    if name.is_empty() {
        return false;
    }
    match name.strip_prefix('#') {
        Some(digits) => !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
        None => name.bytes().all(|b| b.is_ascii_alphabetic()),
    }
}

fn url_encode_filter(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    if input.is_nil() {
        return Ok(Value::Nil);
    }
    Ok(Value::from(url_encode(&input.to_str())))
}

fn url_decode(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    if input.is_nil() {
        return Ok(Value::Nil);
    }
    let s = input.to_str();
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8(out)
        .map(Value::from)
        .map_err(|_| Error::argument("invalid byte sequence in UTF-8"))
}

fn base64_encode(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    Ok(Value::from(STANDARD.encode(input.to_str().as_bytes())))
}

fn base64_decode(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    let decoded = STANDARD
        .decode(input.to_str().as_bytes())
        .map_err(|_| Error::argument("invalid base64 provided to base64_decode"))?;
    Ok(Value::from(String::from_utf8_lossy(&decoded).into_owned()))
}

fn base64_url_safe_encode(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    Ok(Value::from(URL_SAFE.encode(input.to_str().as_bytes())))
}

fn base64_url_safe_decode(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    let s = input.to_str();
    let decoded = URL_SAFE
        .decode(s.as_bytes())
        .or_else(|_| URL_SAFE_NO_PAD.decode(s.trim_end_matches('=').as_bytes()))
        .map_err(|_| Error::argument("invalid base64 provided to base64_url_safe_decode"))?;
    Ok(Value::from(String::from_utf8_lossy(&decoded).into_owned()))
}

/// Ruby's `slice(offset, length)` bounds: `None` when the offset is out of range.
pub(crate) fn slice_bounds(len: usize, offset: i64, length: i64) -> Option<(usize, usize)> {
    let len = len as i64;
    let start = if offset < 0 { offset + len } else { offset };
    if start < 0 || start > len || length < 0 {
        return None;
    }
    let end = start.saturating_add(length).min(len);
    Some((start as usize, end as usize))
}

fn slice(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 2)?;
    let offset = to_integer(&args[0])?;
    let length = match args.get(1) {
        Some(value) if value.is_truthy() => to_integer(value)?,
        _ => 1,
    };
    if let Value::Array(items) = input {
        return Ok(match slice_bounds(items.len(), offset, length) {
            Some((start, end)) => Value::array(items[start..end].to_vec()),
            None => Value::array(Vec::new()),
        });
    }
    let chars: Vec<char> = input.to_str().chars().collect();
    Ok(match slice_bounds(chars.len(), offset, length) {
        Some((start, end)) => Value::from(chars[start..end].iter().collect::<String>()),
        None => Value::empty_string(),
    })
}

fn truncate(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 0, 2)?;
    if input.is_nil() {
        return Ok(Value::Nil);
    }
    let s = input.to_str();
    let length = match args.first() {
        Some(value) => to_integer(value)?,
        None => 50,
    };
    let ellipsis = args
        .get(1)
        .map_or_else(|| "...".to_string(), |v| v.to_str().into_owned());
    let char_count = s.chars().count() as i64;
    if char_count <= length {
        return Ok(Value::from(s.into_owned()));
    }
    let keep = length
        .saturating_sub(ellipsis.chars().count() as i64)
        .max(0) as usize;
    let mut out: String = s.chars().take(keep).collect();
    out.push_str(&ellipsis);
    Ok(Value::from(out))
}

/// Ruby's `split(" ", limit)`: whitespace-separated fields, the last one holding the remainder.
fn awk_split(input: &str, limit: Option<usize>) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut rest = input.trim_start_matches(is_ruby_space);
    while !rest.is_empty() {
        if limit.is_some_and(|limit| fields.len() + 1 == limit) {
            fields.push(rest);
            return fields;
        }
        let end = rest.find(is_ruby_space).unwrap_or(rest.len());
        fields.push(&rest[..end]);
        let after = rest[end..].trim_start_matches(is_ruby_space);
        // With a limit, a trailing separator yields a final empty field.
        if after.is_empty() && end < rest.len() && limit.is_some() {
            fields.push(after);
        }
        rest = after;
    }
    fields
}

fn truncatewords(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 0, 2)?;
    if input.is_nil() {
        return Ok(Value::Nil);
    }
    let s = input.to_str();
    let words = match args.first() {
        Some(value) => to_integer(value)?.max(1) as usize,
        None => 15,
    };
    let mut fields = awk_split(&s, Some(words + 1));
    if fields.len() <= words {
        return Ok(Value::from(s.into_owned()));
    }
    fields.pop();
    let ellipsis = args
        .get(1)
        .map_or_else(|| "...".to_string(), |v| v.to_str().into_owned());
    let mut out = fields.join(" ");
    out.push_str(&ellipsis);
    Ok(Value::from(out))
}

/// Ruby's `String#split(pattern)` for a string pattern.
pub(crate) fn ruby_split(input: &str, pattern: &str) -> Vec<String> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut fields: Vec<String> = if pattern == " " {
        awk_split(input, None)
            .into_iter()
            .map(str::to_string)
            .collect()
    } else if pattern.is_empty() {
        input.chars().map(|c| c.to_string()).collect()
    } else {
        input.split(pattern).map(str::to_string).collect()
    };
    while fields.last().is_some_and(String::is_empty) {
        fields.pop();
    }
    fields
}

fn split(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 1)?;
    let fields = ruby_split(&input.to_str(), &args[0].to_str());
    Ok(Value::array(fields.into_iter().map(Value::from).collect()))
}

fn squish(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    if input.is_nil() {
        return Ok(Value::Nil);
    }
    let s = input.to_str();
    Ok(Value::from(awk_split(lax::strip(&s), None).join(" ")))
}

fn strip(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    Ok(Value::str(lax::strip(&input.to_str())))
}

fn lstrip(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    Ok(Value::str(lax::lstrip(&input.to_str())))
}

fn rstrip(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    Ok(Value::str(lax::rstrip(&input.to_str())))
}

/// Removes every `open ... close` span (case-sensitive, non-greedy, across lines).
fn remove_spans(input: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find(open) {
        match rest[start + open.len()..].find(close) {
            Some(end) => {
                out.push_str(&rest[..start]);
                rest = &rest[start + open.len() + end + close.len()..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// `STRIP_HTML_BLOCKS` then `STRIP_HTML_TAGS`.
pub(crate) fn strip_html_str(input: &str) -> String {
    // The three block patterns are alternatives of one regex: at each position the first one
    // that matches wins, so they have to be removed in a single left-to-right pass.
    let blocks = [
        ("<script", "</script>"),
        ("<!--", "-->"),
        ("<style", "</style>"),
    ];
    let mut without_blocks = String::with_capacity(input.len());
    let mut rest = input;
    'outer: while !rest.is_empty() {
        let next = blocks
            .iter()
            .filter_map(|(open, close)| {
                let start = rest.find(open)?;
                let end = rest[start + open.len()..].find(close)?;
                Some((start, start + open.len() + end + close.len()))
            })
            .min_by_key(|(start, _)| *start);
        match next {
            Some((start, end)) => {
                without_blocks.push_str(&rest[..start]);
                rest = &rest[end..];
            }
            None => {
                without_blocks.push_str(rest);
                break 'outer;
            }
        }
    }
    remove_spans(&without_blocks, "<", ">")
}

fn strip_html(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    Ok(Value::from(strip_html_str(&input.to_str())))
}

fn strip_newlines(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    Ok(Value::from(
        input.to_str().replace("\r\n", "").replace('\n', ""),
    ))
}

fn newline_to_br(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    no_args(args)?;
    Ok(Value::from(
        input
            .to_str()
            .replace("\r\n", "\n")
            .replace('\n', "<br />\n"),
    ))
}

/// Expands the back-references Ruby interprets in a `sub`/`gsub` replacement string, for the
/// match `input[start..end]` of a string pattern: `\0` and `\&` are the match, `` \` `` and
/// `\'` what precedes and follows it. A string pattern has no groups, so `\1` to `\9` and `\+`
/// are empty and `\k<name>` is the `IndexError` Ruby raises.
fn expand_replacement(
    replacement: &str,
    input: &str,
    start: usize,
    end: usize,
    out: &mut String,
) -> Result<()> {
    if !replacement.contains('\\') {
        out.push_str(replacement);
        return Ok(());
    }
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('0' | '&') => {
                out.push_str(&input[start..end]);
                chars.next();
            }
            Some('`') => {
                out.push_str(&input[..start]);
                chars.next();
            }
            Some('\'') => {
                out.push_str(&input[end..]);
                chars.next();
            }
            Some('1'..='9' | '+') => {
                chars.next();
            }
            Some('k') => {
                chars.next();
                if chars.peek() == Some(&'<') {
                    return Err(Error::internal());
                }
                out.push_str("\\k");
            }
            Some('\\') => {
                out.push('\\');
                chars.next();
            }
            _ => out.push('\\'),
        }
    }
    Ok(())
}

fn gsub(input: &str, pattern: &str, replacement: &str, first_only: bool) -> Result<String> {
    let mut out = String::with_capacity(input.len());
    if pattern.is_empty() {
        // An empty pattern matches between every character.
        expand_replacement(replacement, input, 0, 0, &mut out)?;
        if first_only {
            out.push_str(input);
            return Ok(out);
        }
        for (index, c) in input.char_indices() {
            out.push(c);
            let at = index + c.len_utf8();
            expand_replacement(replacement, input, at, at, &mut out)?;
        }
        return Ok(out);
    }
    let mut pos = 0;
    while let Some(index) = input[pos..].find(pattern) {
        let start = pos + index;
        let end = start + pattern.len();
        out.push_str(&input[pos..start]);
        expand_replacement(replacement, input, start, end, &mut out)?;
        pos = end;
        if first_only {
            break;
        }
    }
    out.push_str(&input[pos..]);
    Ok(out)
}

fn replace_with(input: &Value, args: Args<'_>, min: usize, first_only: bool) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, min, 2)?;
    let replacement = args
        .get(1)
        .map(|v| v.to_str().into_owned())
        .unwrap_or_default();
    gsub(&input.to_str(), &args[0].to_str(), &replacement, first_only).map(Value::from)
}

fn replace(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    replace_with(input, args, 1, false)
}

fn replace_first(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    replace_with(input, args, 1, true)
}

fn replace_last_str(input: &str, pattern: &str, replacement: &str) -> String {
    match input.rfind(pattern) {
        Some(index) => {
            let mut out = String::with_capacity(input.len() + replacement.len());
            out.push_str(&input[..index]);
            out.push_str(replacement);
            out.push_str(&input[index + pattern.len()..]);
            out
        }
        None => input.to_string(),
    }
}

fn replace_last(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 2, 2)?;
    Ok(Value::from(replace_last_str(
        &input.to_str(),
        &args[0].to_str(),
        &args[1].to_str(),
    )))
}

fn remove(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 1)?;
    gsub(&input.to_str(), &args[0].to_str(), "", false).map(Value::from)
}

fn remove_first(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 1)?;
    gsub(&input.to_str(), &args[0].to_str(), "", true).map(Value::from)
}

fn remove_last(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 1)?;
    Ok(Value::from(replace_last_str(
        &input.to_str(),
        &args[0].to_str(),
        "",
    )))
}

fn append(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 1)?;
    let mut out = input.to_str().into_owned();
    out.push_str(&args[0].to_str());
    Ok(Value::from(out))
}

fn prepend(input: &Value, args: Args<'_>, _ctx: &Context) -> Result<Value> {
    let args = ruby_args(args);
    check_arity(&args, 1, 1)?;
    let mut out = args[0].to_str().into_owned();
    out.push_str(&input.to_str());
    Ok(Value::from(out))
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("downcase", downcase);
    env.register_filter("upcase", upcase);
    env.register_filter("capitalize", capitalize);
    env.register_filter("escape", escape);
    env.register_filter("h", escape);
    env.register_filter("escape_once", escape_once);
    env.register_filter("url_encode", url_encode_filter);
    env.register_filter("url_decode", url_decode);
    env.register_filter("base64_encode", base64_encode);
    env.register_filter("base64_decode", base64_decode);
    env.register_filter("base64_url_safe_encode", base64_url_safe_encode);
    env.register_filter("base64_url_safe_decode", base64_url_safe_decode);
    env.register_filter("slice", slice);
    env.register_filter("truncate", truncate);
    env.register_filter("truncatewords", truncatewords);
    env.register_filter("split", split);
    env.register_filter("squish", squish);
    env.register_filter("strip", strip);
    env.register_filter("lstrip", lstrip);
    env.register_filter("rstrip", rstrip);
    env.register_filter("strip_html", strip_html);
    env.register_filter("strip_newlines", strip_newlines);
    env.register_filter("newline_to_br", newline_to_br);
    env.register_filter("replace", replace);
    env.register_filter("replace_first", replace_first);
    env.register_filter("replace_last", replace_last);
    env.register_filter("remove", remove);
    env.register_filter("remove_first", remove_first);
    env.register_filter("remove_last", remove_last);
    env.register_filter("append", append);
    env.register_filter("prepend", prepend);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_ruby() {
        assert_eq!(ruby_split("a,b,,", ","), vec!["a", "b"]);
        assert_eq!(ruby_split(",a", ","), vec!["", "a"]);
        assert_eq!(ruby_split("  a  b ", " "), vec!["a", "b"]);
        assert_eq!(ruby_split("abc", ""), vec!["a", "b", "c"]);
        assert_eq!(awk_split(" a b  c d ", Some(3)), vec!["a", "b", "c d "]);
    }

    #[test]
    fn strips_html() {
        assert_eq!(strip_html_str("<p>a <b>b</b></p>"), "a b");
        assert_eq!(
            strip_html_str("a<script>x</script>b<!-- c -->d<style>e</style>f"),
            "abdf"
        );
    }

    #[test]
    fn substitutes() {
        let gsub = |input, pattern, replacement, first_only| {
            gsub(input, pattern, replacement, first_only).unwrap()
        };
        assert_eq!(gsub("a-b-c", "-", "+", false), "a+b+c");
        assert_eq!(gsub("a-b-c", "-", "+", true), "a+b-c");
        assert_eq!(gsub("abc", "", "-", false), "-a-b-c-");
        assert_eq!(gsub("abc", "b", "[\\0]", false), "a[b]c");
        assert_eq!(gsub("toto", "t", "\\'", false), "otoooo");
        assert_eq!(gsub("toto", "t", "\\`", false), "otoo");
        assert_eq!(gsub("toto", "t", "\\1\\+", false), "oo");
        assert_eq!(gsub("toto", "t", "\\k", false), "\\ko\\ko");
        assert_eq!(replace_last_str("a-b-c", "-", "+"), "a-b+c");
        assert!(super::gsub("toto", "t", "\\k<name>", false).is_err());
    }
}
