//! Hand-written scanners for the regular expressions Ruby Liquid uses in its lax parser.
//!
//! The `regex` crate supports neither recursion (`VariableParser`) nor look-ahead, so each
//! pattern is implemented directly. Every function documents the pattern it stands for.

use crate::number::is_ruby_space;

pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn char_at(s: &str, pos: usize) -> Option<char> {
    s.get(pos..)?.chars().next()
}

/// `QuotedString = /"[^"]*"|'[^']*'/`, anchored at `pos`. Returns the end offset.
fn quoted_string(s: &str, pos: usize) -> Option<usize> {
    let quote = char_at(s, pos)?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    s[pos + 1..].find(quote).map(|i| pos + 1 + i + 1)
}

/// `QuotedFragment = /#{QuotedString}|(?:[^\s,\|'"]|#{QuotedString})+/`, anchored at `pos`.
/// Returns the end offset of the match.
pub fn quoted_fragment(s: &str, pos: usize) -> Option<usize> {
    if let Some(end) = quoted_string(s, pos) {
        return Some(end);
    }
    let mut end = pos;
    while let Some(c) = char_at(s, end) {
        if c == '"' || c == '\'' {
            match quoted_string(s, end) {
                Some(next) => end = next,
                None => break,
            }
        } else if is_ruby_space(c) || c == ',' || c == '|' {
            break;
        } else {
            end += c.len_utf8();
        }
    }
    (end > pos).then_some(end)
}

/// Finds the first `QuotedFragment` at or after `from`, as an unanchored regex search would.
pub fn find_quoted_fragment(s: &str, from: usize) -> Option<(usize, usize)> {
    let mut pos = from;
    while pos <= s.len() {
        if let Some(end) = quoted_fragment(s, pos) {
            return Some((pos, end));
        }
        pos += char_at(s, pos)?.len_utf8();
    }
    None
}

/// `QuotedFragment+` anchored at `pos` (consecutive fragments with nothing in between).
pub fn quoted_fragments(s: &str, pos: usize) -> Option<usize> {
    let mut end = quoted_fragment(s, pos)?;
    while let Some(next) = quoted_fragment(s, end) {
        end = next;
    }
    Some(end)
}

pub fn skip_space(s: &str, mut pos: usize) -> usize {
    while let Some(c) = char_at(s, pos) {
        if is_ruby_space(c) {
            pos += c.len_utf8();
        } else {
            break;
        }
    }
    pos
}

/// `\s+` anchored at `pos`: returns the end offset when at least one space matched.
pub fn require_space(s: &str, pos: usize) -> Option<usize> {
    let end = skip_space(s, pos);
    (end > pos).then_some(end)
}

/// `/\w[\w-]*/` anchored at `pos`.
fn attribute_name(s: &str, pos: usize) -> Option<usize> {
    let first = char_at(s, pos)?;
    if !is_word(first) {
        return None;
    }
    let mut end = pos + first.len_utf8();
    while let Some(c) = char_at(s, end) {
        if is_word(c) || c == '-' {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    Some(end)
}

/// `TagAttributes = /(\w[\w-]*)\s*\:\s*(#{QuotedFragment})/` anchored at `pos`.
/// Returns `(key, value, end)`.
fn tag_attribute(s: &str, pos: usize) -> Option<(&str, &str, usize)> {
    // The name is matched greedily but the regex can backtrack to a shorter name; a shorter name
    // would be followed by a word character or dash rather than `\s*:`, so it can never match.
    let name_end = attribute_name(s, pos)?;
    let colon = skip_space(s, name_end);
    if char_at(s, colon) != Some(':') {
        return None;
    }
    let value_start = skip_space(s, colon + 1);
    let value_end = quoted_fragment(s, value_start)?;
    Some((&s[pos..name_end], &s[value_start..value_end], value_end))
}

/// `markup.scan(TagAttributes)`: every `key: value` pair found anywhere in the markup.
pub fn scan_tag_attributes(s: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < s.len() {
        match tag_attribute(s, pos) {
            Some((key, value, end)) => {
                out.push((key, value));
                pos = end;
            }
            None => match char_at(s, pos) {
                Some(c) => pos += c.len_utf8(),
                None => break,
            },
        }
    }
    out
}

/// `/\A#{TagAttributes}\z/`: the whole string is a single `key: value` pair.
pub fn just_tag_attribute(s: &str) -> Option<(&str, &str)> {
    let (key, value, end) = tag_attribute(s, 0)?;
    (end == s.len()).then_some((key, value))
}

/// Matches `\[(?>[^\[\]]+|\g<0>)*\]` at `pos`: a balanced bracket group.
fn bracket_group(s: &str, pos: usize) -> Option<usize> {
    if char_at(s, pos) != Some('[') {
        return None;
    }
    let mut depth = 0usize;
    for (i, c) in s[pos..].char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(pos + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// `markup.scan(VariableParser)` where
/// `VariableParser = /\[(?>[^\[\]]+|\g<0>)*\]|[\w\-]+\??/`.
pub fn scan_variable_parts(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(c) = char_at(s, pos) {
        if c == '[' {
            if let Some(end) = bracket_group(s, pos) {
                out.push(&s[pos..end]);
                pos = end;
                continue;
            }
        } else if is_word(c) || c == '-' {
            let mut end = pos;
            while let Some(c) = char_at(s, end) {
                if is_word(c) || c == '-' {
                    end += c.len_utf8();
                } else {
                    break;
                }
            }
            if char_at(s, end) == Some('?') {
                end += 1;
            }
            out.push(&s[pos..end]);
            pos = end;
            continue;
        }
        pos += c.len_utf8();
    }
    out
}

/// Ruby's `String#strip`: leading whitespace, trailing whitespace and NULs.
pub fn strip(s: &str) -> &str {
    lstrip(rstrip(s))
}

pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_ruby_space)
}

pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(|c| is_ruby_space(c) || c == '\0')
}

/// `/\A\s*\z/`.
pub fn is_blank(s: &str) -> bool {
    s.chars().all(is_ruby_space)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_quoted_fragments() {
        assert_eq!(quoted_fragment("'a b' c", 0), Some(5));
        assert_eq!(quoted_fragment("a.b['x y'].c | d", 0), Some(12));
        assert_eq!(quoted_fragment(" a", 0), None);
        assert_eq!(quoted_fragment("a,b", 0), Some(1));
        assert_eq!(find_quoted_fragment("  , x", 0), Some((4, 5)));
    }

    #[test]
    fn scans_attributes() {
        assert_eq!(
            scan_tag_attributes("a in b limit: 2, offset:x.y"),
            vec![("limit", "2"), ("offset", "x.y")]
        );
        assert_eq!(just_tag_attribute("key: 'v w'"), Some(("key", "'v w'")));
        assert_eq!(just_tag_attribute("key"), None);
    }

    #[test]
    fn scans_variable_parts() {
        assert_eq!(
            scan_variable_parts("a.b[c[0]].d-e?"),
            vec!["a", "b", "[c[0]]", "d-e?"]
        );
        assert_eq!(scan_variable_parts("['x'].y"), vec!["['x']", "y"]);
    }
}
