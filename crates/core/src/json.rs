//! JSON helpers shared by the theme and store loaders.

use crate::error::{Error, Result};

/// Removes `/* */` and `//` comments. Shopify writes a comment header into the JSON files the
/// theme editor manages, so theme JSON has to be read leniently.
pub fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        out.push(escaped);
                    }
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = ' ';
                for c in chars.by_ref() {
                    if previous == '*' && c == '/' {
                        break;
                    }
                    // Keep line numbers stable for error messages.
                    if c == '\n' {
                        out.push('\n');
                    }
                    previous = c;
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// The line of each value of a JSON text, by JSON pointer (`/settings/2/default`), for messages
/// that name a line. The text is valid JSON, comments stripped (which keeps the lines).
pub fn value_lines(text: &str) -> std::collections::HashMap<String, usize> {
    struct Walker<'a> {
        bytes: &'a [u8],
        at: usize,
        line: usize,
        lines: std::collections::HashMap<String, usize>,
    }
    impl Walker<'_> {
        fn skip(&mut self) {
            while let Some(&byte) = self.bytes.get(self.at) {
                match byte {
                    b'\n' => self.line += 1,
                    b' ' | b'\t' | b'\r' | b',' | b':' => {}
                    _ => return,
                }
                self.at += 1;
            }
        }
        fn string(&mut self) -> String {
            // At the opening quote.
            self.at += 1;
            let start = self.at;
            while let Some(&byte) = self.bytes.get(self.at) {
                match byte {
                    b'\\' => self.at += 1,
                    b'"' => break,
                    b'\n' => self.line += 1,
                    _ => {}
                }
                self.at += 1;
            }
            let raw = String::from_utf8_lossy(&self.bytes[start..self.at.min(self.bytes.len())]);
            self.at += 1;
            serde_json::from_str::<String>(&format!("\"{raw}\"")).unwrap_or_else(|_| raw.into())
        }
        fn value(&mut self, pointer: String) {
            self.skip();
            self.lines.insert(pointer.clone(), self.line);
            match self.bytes.get(self.at) {
                Some(b'{') => {
                    self.at += 1;
                    loop {
                        self.skip();
                        match self.bytes.get(self.at) {
                            Some(b'}') | None => {
                                self.at += 1;
                                return;
                            }
                            Some(b'"') => {
                                let key = self.string();
                                let escaped = key.replace('~', "~0").replace('/', "~1");
                                self.value(format!("{pointer}/{escaped}"));
                            }
                            Some(_) => self.at += 1,
                        }
                    }
                }
                Some(b'[') => {
                    self.at += 1;
                    let mut index = 0;
                    loop {
                        self.skip();
                        match self.bytes.get(self.at) {
                            Some(b']') | None => {
                                self.at += 1;
                                return;
                            }
                            Some(_) => {
                                self.value(format!("{pointer}/{index}"));
                                index += 1;
                            }
                        }
                    }
                }
                Some(b'"') => {
                    self.string();
                }
                Some(_) => {
                    while let Some(&byte) = self.bytes.get(self.at) {
                        if matches!(byte, b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t') {
                            break;
                        }
                        self.at += 1;
                    }
                }
                None => {}
            }
        }
    }
    let mut walker = Walker {
        bytes: text.as_bytes(),
        at: 0,
        line: 1,
        lines: std::collections::HashMap::new(),
    };
    walker.value(String::new());
    walker.lines
}

/// What Shopify lets through in a theme JSON file that strict JSON does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leniency {
    /// A comma before `}` or `]`.
    TrailingComma,
    /// An escape JSON does not have, such as `\“`: Shopify keeps the character.
    UnknownEscape(char),
}

impl std::fmt::Display for Leniency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Leniency::TrailingComma => f.write_str("a trailing comma"),
            Leniency::UnknownEscape(c) => write!(f, "an unknown escape `\\{c}`"),
        }
    }
}

/// A theme JSON text as Shopify reads it, turned into strict JSON: comments removed, trailing
/// commas removed, and an unknown escape (`\“`) replaced with its character. The lines stay
/// where they were, for messages. Also returns what was let through, with its line.
pub fn theme_json(source: &str) -> (String, Vec<(usize, Leniency)>) {
    let text = strip_comments(source);
    let mut out = String::with_capacity(text.len());
    let mut found = Vec::new();
    let mut line = 1;
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    // The last character outside strings and whitespace: a comma is trailing after a value,
    // not after `{`, `[` or another comma (`{,}` stays an error, as on Shopify).
    let mut previous = ' ';
    while let Some(c) = chars.next() {
        if c == '\n' {
            line += 1;
        }
        if !in_string && !c.is_whitespace() {
            let before = previous;
            previous = c;
            if c == ','
                && !matches!(before, '{' | '[' | ',')
                && matches!(chars.clone().find(|c| !c.is_whitespace()), Some('}' | ']'))
            {
                found.push((line, Leniency::TrailingComma));
                out.push(' ');
                continue;
            }
        }
        if in_string {
            match c {
                '\\' => match chars.next() {
                    Some(escaped @ ('"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u')) => {
                        out.push('\\');
                        out.push(escaped);
                    }
                    Some(other) => {
                        if other == '\n' {
                            line += 1;
                        }
                        found.push((line, Leniency::UnknownEscape(other)));
                        // Shopify drops the backslash and keeps the character.
                        out.push(other);
                    }
                    None => out.push('\\'),
                },
                '"' => {
                    in_string = false;
                    out.push(c);
                }
                _ => out.push(c),
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    (out, found)
}

/// Parses a theme JSON file as Shopify reads it ([`theme_json`]), naming the file in errors.
/// A UTF-8 byte order mark is still an error: Shopify refuses a file that starts with one.
pub fn parse_lenient(path: &str, source: &str) -> Result<serde_json::Value> {
    let error = |message: String| Error::Json {
        path: path.to_string(),
        message,
    };
    if source.starts_with('\u{feff}') {
        return Err(error(
            "invalid JSON at line 1, column 1: the file starts with a UTF-8 byte order mark (BOM), \
             which Shopify refuses"
                .to_string(),
        ));
    }
    serde_json::from_str(&theme_json(source).0).map_err(|e| {
        error(format!(
            "invalid JSON at line {}, column {}: {e}",
            e.line(),
            e.column()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_outside_strings() {
        let source =
            "/* header\n */\n{\"a\": \"// not a comment /* nor this */\", // trailing\n \"b\": 1}";
        let value = parse_lenient("test.json", source).unwrap();
        assert_eq!(value["a"], "// not a comment /* nor this */");
        assert_eq!(value["b"], 1);
    }

    #[test]
    fn reads_trailing_commas_and_unknown_escapes_as_shopify_does() {
        let source = "{\n  \"a\": [1, 2, ],\n  \"b\": \"\\“q\\” \\n \\\" \\u00e9\", // note\n}\n";
        let value = parse_lenient("test.json", source).unwrap();
        assert_eq!(value["a"], serde_json::json!([1, 2]));
        assert_eq!(value["b"], "“q” \n \" é");
        assert_eq!(
            theme_json(source).1,
            [
                (2, Leniency::TrailingComma),
                (3, Leniency::UnknownEscape('“')),
                (3, Leniency::UnknownEscape('”')),
                (3, Leniency::TrailingComma),
            ]
        );
        // A comma with no value before it is not a trailing comma.
        for source in ["{,}", "[,]", "[1,,]"] {
            assert!(parse_lenient("t.json", source).is_err(), "{source}");
        }
        // A comma in a string stays.
        assert_eq!(parse_lenient("t.json", "[\"a,]\"]").unwrap()[0], "a,]");
    }

    #[test]
    fn a_byte_order_mark_is_refused() {
        let error = parse_lenient("t.json", "\u{feff}{}")
            .unwrap_err()
            .to_string();
        assert!(error.contains("byte order mark"), "{error}");
    }
}
