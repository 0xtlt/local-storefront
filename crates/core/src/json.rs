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
