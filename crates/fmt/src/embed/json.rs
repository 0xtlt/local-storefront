//! JSON, laid out as Prettier's `json` parser lays it out: an object stays on one line when
//! it fits and did not start with a line break (unless `objectWrap` is "collapse"), an array of
//! numbers is filled, a blank line between two properties is kept.

use crate::doc::{
    Doc, concat, fill, group, group_with, hardline, indent, line, nothing, softline, text,
};
use crate::options::{ObjectWrap, Options};

use super::number::print_number;

enum Value {
    /// A string, a number or a keyword, as it is printed.
    Scalar {
        printed: String,
        is_number: bool,
    },
    Object {
        start: usize,
        properties: Vec<Property>,
    },
    Array(Vec<Element>),
}

struct Property {
    key: String,
    key_start: usize,
    value: Value,
    end: usize,
}

struct Element {
    value: Value,
    end: usize,
}

struct Parser<'s> {
    source: &'s str,
    pos: usize,
}

/// Prettier's `makeString` with double quotes: the quotes that need no escape lose it, the
/// double quotes that have none get one.
fn print_string(raw: &str) -> String {
    let content = &raw[1..raw.len() - 1];
    let mut printed = String::with_capacity(raw.len());
    printed.push('"');
    let mut chars = content.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('\'') if raw.starts_with('\'') => printed.push('\''),
                Some(escaped) => {
                    printed.push('\\');
                    printed.push(escaped);
                }
                None => printed.push('\\'),
            },
            '"' => printed.push_str("\\\""),
            other => printed.push(other),
        }
    }
    printed.push('"');
    printed
}

impl Parser<'_> {
    fn skip_whitespace(&mut self) {
        let rest = &self.source[self.pos..];
        self.pos += rest.len() - rest.trim_start().len();
    }

    fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.pos).copied()
    }

    fn string(&mut self) -> Option<String> {
        let quote = self.peek()?;
        let bytes = self.source.as_bytes();
        let mut end = self.pos + 1;
        while *bytes.get(end)? != quote {
            if bytes[end] == b'\n' {
                return None;
            }
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        let raw = self.source.get(self.pos..end + 1)?;
        self.pos = end + 1;
        Some(print_string(raw))
    }

    /// A number or a keyword: a run of what they are made of.
    fn word(&mut self) -> &str {
        let rest = &self.source[self.pos..];
        let length = rest
            .bytes()
            .take_while(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-' | b'_' | b'$')
            })
            .count();
        self.pos += length;
        &rest[..length]
    }

    fn value(&mut self) -> Option<Value> {
        self.skip_whitespace();
        match self.peek()? {
            b'{' => {
                let start = self.pos;
                self.pos += 1;
                let mut properties = Vec::new();
                loop {
                    self.skip_whitespace();
                    if self.peek()? == b'}' {
                        self.pos += 1;
                        break;
                    }
                    let key_start = self.pos;
                    let key = match self.peek()? {
                        b'"' | b'\'' => self.string()?,
                        _ => {
                            let word = self.word();
                            if word.is_empty() {
                                return None;
                            }
                            if word.as_bytes()[0].is_ascii_digit() {
                                print_number(word)
                            } else {
                                format!("\"{word}\"")
                            }
                        }
                    };
                    self.skip_whitespace();
                    if self.peek()? != b':' {
                        return None;
                    }
                    self.pos += 1;
                    let value = self.value()?;
                    properties.push(Property {
                        key,
                        key_start,
                        value,
                        end: self.pos,
                    });
                    self.skip_whitespace();
                    match self.peek()? {
                        b',' => self.pos += 1,
                        b'}' => {}
                        _ => return None,
                    }
                }
                Some(Value::Object { start, properties })
            }
            b'[' => {
                self.pos += 1;
                let mut elements = Vec::new();
                loop {
                    self.skip_whitespace();
                    if self.peek()? == b']' {
                        self.pos += 1;
                        break;
                    }
                    let value = self.value()?;
                    elements.push(Element {
                        value,
                        end: self.pos,
                    });
                    self.skip_whitespace();
                    match self.peek()? {
                        b',' => self.pos += 1,
                        b']' => {}
                        _ => return None,
                    }
                }
                Some(Value::Array(elements))
            }
            b'"' | b'\'' => Some(Value::Scalar {
                printed: self.string()?,
                is_number: false,
            }),
            _ => {
                let word = self.word();
                let digits = word.strip_prefix(['-', '+']).unwrap_or(word);
                if matches!(word, "true" | "false" | "null") {
                    Some(Value::Scalar {
                        printed: word.to_string(),
                        is_number: false,
                    })
                } else if digits
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_digit() || byte == b'.')
                    && digits.bytes().all(|byte| {
                        byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E' | b'+' | b'-')
                    })
                {
                    Some(Value::Scalar {
                        printed: print_number(word),
                        is_number: true,
                    })
                } else {
                    None
                }
            }
        }
    }
}

/// Prettier's `isNextLineEmpty`: whether a blank line follows what ends at `end`.
fn is_next_line_empty(source: &str, end: usize) -> bool {
    let rest = source[end..].trim_start_matches([',', ';', ' ', '\t']);
    let Some(rest) = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix(['\n', '\r']))
    else {
        return false;
    };
    rest.trim_start_matches([' ', '\t'])
        .starts_with(['\n', '\r'])
}

fn print_value(value: &Value, source: &str, options: &Options) -> Doc {
    match value {
        Value::Scalar { printed, .. } => text(printed.as_str()),
        Value::Object { start, properties } => {
            let Some(first) = properties.first() else {
                return text("{}");
            };
            // An object that started on several lines stays on several lines.
            let should_break = options.object_wrap == ObjectWrap::Preserve
                && source[*start..first.key_start].contains('\n');
            let bracket_line = || {
                if options.bracket_spacing {
                    line()
                } else {
                    softline()
                }
            };
            let mut parts = Vec::new();
            for (index, property) in properties.iter().enumerate() {
                if index > 0 {
                    parts.push(text(","));
                    parts.push(line());
                    if is_next_line_empty(source, properties[index - 1].end) {
                        parts.push(hardline());
                    }
                }
                parts.push(group(vec![
                    text(property.key.as_str()),
                    text(": "),
                    print_value(&property.value, source, options),
                ]));
            }
            group_with(
                vec![
                    text("{"),
                    indent(vec![bracket_line(), concat(parts)]),
                    bracket_line(),
                    text("}"),
                ],
                None,
                should_break,
            )
        }
        Value::Array(elements) => {
            if elements.is_empty() {
                return text("[]");
            }
            // Several objects, or several arrays, each of more than one item: one per line.
            let length = |value: &Value| match value {
                Value::Object { properties, .. } => Some((true, properties.len())),
                Value::Array(elements) => Some((false, elements.len())),
                Value::Scalar { .. } => None,
            };
            let first_kind = length(&elements[0].value).map(|(is_object, _)| is_object);
            let should_break = elements.len() > 1
                && elements.iter().all(|element| {
                    length(&element.value).is_some_and(|(is_object, length)| {
                        Some(is_object) == first_kind && length > 1
                    })
                });
            let all_numbers = elements.len() > 1
                && elements.iter().all(|element| {
                    matches!(
                        element.value,
                        Value::Scalar {
                            is_number: true,
                            ..
                        }
                    )
                });
            let last = elements.len() - 1;
            let items = if all_numbers {
                let mut parts = Vec::new();
                for (index, element) in elements.iter().enumerate() {
                    parts.push(concat(vec![
                        print_value(&element.value, source, options),
                        text(if index == last { "" } else { "," }),
                    ]));
                    if index != last {
                        parts.push(if is_next_line_empty(source, element.end) {
                            concat(vec![hardline(), hardline()])
                        } else {
                            line()
                        });
                    }
                }
                fill(parts)
            } else {
                let mut parts = Vec::new();
                for (index, element) in elements.iter().enumerate() {
                    parts.push(print_value(&element.value, source, options));
                    if index != last {
                        parts.push(text(","));
                        parts.push(line());
                        parts.push(if is_next_line_empty(source, element.end) {
                            softline()
                        } else {
                            nothing()
                        });
                    }
                }
                concat(parts)
            };
            group_with(
                vec![
                    text("["),
                    indent(vec![softline(), items]),
                    softline(),
                    text("]"),
                ],
                None,
                should_break,
            )
        }
    }
}

/// The document of a JSON source, or `None` when it is not JSON this formatter reads: it has
/// a syntax error, or comments.
pub(crate) fn format(source: &str, options: &Options) -> Option<Doc> {
    let mut parser = Parser { source, pos: 0 };
    let value = parser.value()?;
    parser.skip_whitespace();
    (parser.pos == source.len()).then(|| print_value(&value, source, options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{PrintOptions, print};

    fn fmt_with(source: &str, print_width: usize, options: &Options) -> String {
        print(
            format(source, options).unwrap(),
            &PrintOptions {
                print_width,
                tab_width: 2,
                use_tabs: false,
            },
        )
    }

    fn fmt(source: &str, print_width: usize) -> String {
        fmt_with(source, print_width, &Options::default())
    }

    #[test]
    fn follows_bracket_spacing_and_object_wrap() {
        let source = "{\"a\": {\n\"b\": 1}}";
        assert_eq!(fmt(source, 80), "{\n  \"a\": {\n    \"b\": 1\n  }\n}");
        let options = Options {
            bracket_spacing: false,
            object_wrap: ObjectWrap::Collapse,
            ..Options::default()
        };
        assert_eq!(fmt_with(source, 80, &options), "{\"a\": {\"b\": 1}}");
    }

    #[test]
    fn an_object_breaks_when_it_did_or_when_it_does_not_fit() {
        assert_eq!(
            fmt("{\"a\":1,\"b\":[1,2]}", 80),
            "{ \"a\": 1, \"b\": [1, 2] }"
        );
        assert_eq!(fmt("{\n\"a\":1}", 80), "{\n  \"a\": 1\n}");
        assert_eq!(
            fmt("{\"a\":1,\"b\":2}", 10),
            "{\n  \"a\": 1,\n  \"b\": 2\n}"
        );
    }

    #[test]
    fn several_objects_in_an_array_go_one_per_line() {
        assert_eq!(
            fmt("[{\"a\":1,\"b\":2},{\"a\":3,\"b\":4}]", 80),
            "[\n  { \"a\": 1, \"b\": 2 },\n  { \"a\": 3, \"b\": 4 }\n]"
        );
        assert_eq!(
            fmt("[{\"a\":1},{\"b\":2}]", 80),
            "[{ \"a\": 1 }, { \"b\": 2 }]"
        );
    }

    #[test]
    fn keeps_a_blank_line_between_properties() {
        assert_eq!(
            fmt("{\n\"a\":1,\n\n\n\"b\":2}", 80),
            "{\n  \"a\": 1,\n\n  \"b\": 2\n}"
        );
    }

    #[test]
    fn gives_up_on_what_is_not_plain_json() {
        let options = Options::default();
        assert!(format("{\"a\": 1 // why\n}", &options).is_none());
        assert!(format("{\"a\": }", &options).is_none());
        assert!(format("{{ a | json }}", &options).is_none());
    }
}
