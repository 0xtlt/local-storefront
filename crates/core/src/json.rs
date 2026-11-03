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

/// Parses JSON that may contain comments, naming the file in errors.
pub fn parse_lenient(path: &str, source: &str) -> Result<serde_json::Value> {
    serde_json::from_str(&strip_comments(source)).map_err(|error| Error::Json {
        path: path.to_string(),
        message: format!(
            "invalid JSON at line {}, column {}: {error}",
            error.line(),
            error.column()
        ),
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
}
