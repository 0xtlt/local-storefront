//! Selectors: cut the way `postcss-selector-parser` cuts them, and printed by a port of
//! Prettier's selector printer.

use crate::doc::{Doc, concat, group, hardline, indent, join, line, nothing, softline, text};

use super::misc::{adjust_numbers, adjust_strings, maybe_to_lower_case};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tok {
    Space,
    Combinator,
    Asterisk,
    Ampersand,
    Comma,
    OpenSquare,
    CloseSquare,
    Colon,
    Semicolon,
    OpenParen,
    CloseParen,
    Str,
    Word,
}

#[derive(Debug, Clone, Copy)]
struct Token {
    kind: Tok,
    start: usize,
    end: usize,
}

#[derive(Debug)]
enum Node {
    Tag(String),
    Class(String),
    Id(String),
    Universal,
    Nesting,
    Str(String),
    /// `+`, `>`, `~`, or the whitespace of a descendant combinator.
    Combinator(String),
    Attribute {
        attribute: String,
        operator: Option<String>,
        value: Option<String>,
        insensitive: bool,
    },
    Pseudo {
        value: String,
        selectors: Vec<Vec<Node>>,
    },
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | b'\t' | b'\r' | b'\x0c')
}

fn tokenize(selector: &str) -> Option<Vec<Token>> {
    let bytes = selector.as_bytes();
    let mut tokens = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let start = pos;
        let byte = bytes[pos];
        let kind = match byte {
            _ if is_space(byte) => {
                while pos < bytes.len() && is_space(bytes[pos]) {
                    pos += 1;
                }
                Tok::Space
            }
            b'+' | b'>' | b'~' | b'|' => {
                while pos < bytes.len() && matches!(bytes[pos], b'+' | b'>' | b'~' | b'|') {
                    pos += 1;
                }
                Tok::Combinator
            }
            b'*' => {
                pos += 1;
                Tok::Asterisk
            }
            b'&' => {
                pos += 1;
                Tok::Ampersand
            }
            b',' => {
                pos += 1;
                Tok::Comma
            }
            b'[' => {
                pos += 1;
                Tok::OpenSquare
            }
            b']' => {
                pos += 1;
                Tok::CloseSquare
            }
            b':' => {
                pos += 1;
                Tok::Colon
            }
            b';' => {
                pos += 1;
                Tok::Semicolon
            }
            b'(' => {
                pos += 1;
                Tok::OpenParen
            }
            b')' => {
                pos += 1;
                Tok::CloseParen
            }
            b'\'' | b'"' => {
                let mut next = pos;
                loop {
                    next = next + 1 + selector.get(next + 1..)?.find(byte as char)?;
                    let backslashes = bytes[..next]
                        .iter()
                        .rev()
                        .take_while(|b| **b == b'\\')
                        .count();
                    if backslashes % 2 == 0 {
                        break;
                    }
                }
                pos = next + 1;
                Tok::Str
            }
            b'\\' => {
                // A backslash and what it escapes.
                let mut escape = true;
                while bytes.get(pos + 1) == Some(&b'\\') {
                    pos += 1;
                    escape = !escape;
                }
                pos += 1;
                if escape
                    && let Some(escaped) = selector[pos..].chars().next()
                    && !(escaped == '/' || (escaped.is_ascii() && is_space(escaped as u8)))
                {
                    pos += escaped.len_utf8();
                }
                Tok::Word
            }
            _ => {
                pos += selector[pos..].chars().next()?.len_utf8();
                while pos < bytes.len() {
                    let ends = match bytes[pos] {
                        b' ' | b'\n' | b'\t' | b'\r' | b'(' | b')' | b'*' | b':' | b';' | b'@'
                        | b'!' | b'&' | b'\'' | b'"' | b'+' | b'|' | b'~' | b'>' | b',' | b'['
                        | b']' | b'\\' => true,
                        b'/' => bytes.get(pos + 1) == Some(&b'*'),
                        _ => false,
                    };
                    if ends {
                        break;
                    }
                    pos += 1;
                }
                Tok::Word
            }
        };
        tokens.push(Token {
            kind,
            start,
            end: pos,
        });
    }
    Some(tokens)
}

struct Parser<'s> {
    selector: &'s str,
    tokens: Vec<Token>,
    position: usize,
}

impl Parser<'_> {
    fn text(&self, token: Token) -> &str {
        &self.selector[token.start..token.end]
    }

    fn kind(&self, position: usize) -> Option<Tok> {
        self.tokens.get(position).map(|token| token.kind)
    }

    /// The selectors up to the `)` that closes a pseudo-class, or to the end.
    fn selectors(&mut self, in_parens: bool) -> Option<Vec<Vec<Node>>> {
        let mut selectors: Vec<Vec<Node>> = vec![Vec::new()];
        while let Some(token) = self.tokens.get(self.position).copied() {
            match token.kind {
                Tok::CloseParen => {
                    if !in_parens {
                        return None;
                    }
                    self.position += 1;
                    return Some(selectors);
                }
                Tok::Space => {
                    let before = self
                        .position
                        .checked_sub(1)
                        .and_then(|before| self.kind(before));
                    let after = self.kind(self.position + 1);
                    let at_edge = self.position == 0
                        || matches!(before, Some(Tok::Comma | Tok::OpenParen))
                        || after.is_none()
                        || matches!(after, Some(Tok::Comma | Tok::CloseParen));
                    if at_edge {
                        self.position += 1;
                    } else {
                        let combinator = self.combinator();
                        selectors.last_mut().unwrap().push(combinator);
                    }
                }
                Tok::Combinator => {
                    if self.text(token).contains('|') {
                        // A namespace: not something this parser is trusted with.
                        return None;
                    }
                    let combinator = self.combinator();
                    selectors.last_mut().unwrap().push(combinator);
                }
                Tok::OpenParen => {
                    self.position += 1;
                    let after_pseudo = match selectors.last().unwrap().last() {
                        Some(Node::Pseudo { .. }) => true,
                        Some(Node::Tag(_) | Node::Class(_) | Node::Id(_)) => false,
                        _ => return None,
                    };
                    if after_pseudo {
                        let parsed = self.selectors(true)?;
                        if let Some(Node::Pseudo {
                            selectors: inner, ..
                        }) = selectors.last_mut().unwrap().last_mut()
                        {
                            *inner = parsed;
                        }
                    } else {
                        // Not a pseudo-class: the parentheses are part of the word.
                        let mut balanced = 1;
                        let mut end = token.end;
                        while balanced > 0 {
                            let token = self.tokens.get(self.position)?;
                            match token.kind {
                                Tok::OpenParen => balanced += 1,
                                Tok::CloseParen => balanced -= 1,
                                _ => {}
                            }
                            end = token.end;
                            self.position += 1;
                        }
                        let parentheses = self.selector[token.start..end].to_string();
                        if let Some(Node::Tag(value) | Node::Class(value) | Node::Id(value)) =
                            selectors.last_mut().unwrap().last_mut()
                        {
                            value.push_str(&parentheses);
                        }
                    }
                }
                Tok::OpenSquare => {
                    let node = self.attribute()?;
                    selectors.last_mut().unwrap().push(node);
                }
                Tok::CloseSquare | Tok::Semicolon => return None,
                Tok::Word => {
                    if self.kind(self.position + 1) == Some(Tok::Combinator)
                        && self.text(self.tokens[self.position + 1]).starts_with('|')
                    {
                        return None;
                    }
                    let nodes = self.split_word(false);
                    selectors.last_mut().unwrap().extend(nodes);
                }
                Tok::Colon => {
                    let mut value = String::new();
                    while self.kind(self.position) == Some(Tok::Colon) {
                        value.push(':');
                        self.position += 1;
                    }
                    if self.kind(self.position) != Some(Tok::Word) {
                        return None;
                    }
                    let mut nodes = self.split_word(true).into_iter();
                    let Some(Node::Tag(first)) = nodes.next() else {
                        return None;
                    };
                    value.push_str(&first);
                    let rest: Vec<Node> = nodes.collect();
                    // `:not.a(…)` is not something the parser accepts.
                    if !rest.is_empty() && self.kind(self.position) == Some(Tok::OpenParen) {
                        return None;
                    }
                    let current = selectors.last_mut().unwrap();
                    current.push(Node::Pseudo {
                        value,
                        selectors: Vec::new(),
                    });
                    current.extend(rest);
                }
                Tok::Comma => {
                    self.position += 1;
                    // A comma at the very end adds no selector.
                    if self.position < self.tokens.len() {
                        selectors.push(Vec::new());
                    }
                }
                Tok::Asterisk => {
                    selectors.last_mut().unwrap().push(Node::Universal);
                    self.position += 1;
                }
                Tok::Ampersand => {
                    selectors.last_mut().unwrap().push(Node::Nesting);
                    self.position += 1;
                }
                Tok::Str => {
                    let string = Node::Str(self.text(token).to_string());
                    selectors.last_mut().unwrap().push(string);
                    self.position += 1;
                }
            }
        }
        (!in_parens).then_some(selectors)
    }

    /// A combinator, with the whitespace around it.
    fn combinator(&mut self) -> Node {
        let mut value = String::new();
        while let Some(token) = self.tokens.get(self.position).copied() {
            if !matches!(token.kind, Tok::Space | Tok::Combinator) {
                break;
            }
            let next_is_combinator = self.kind(self.position + 1) == Some(Tok::Combinator);
            let previous_is_combinator = self
                .position
                .checked_sub(1)
                .and_then(|previous| self.kind(previous))
                == Some(Tok::Combinator);
            if !next_is_combinator && (token.kind == Tok::Combinator || !previous_is_combinator) {
                value = self.text(token).to_string();
            }
            self.position += 1;
        }
        Node::Combinator(value)
    }

    /// `[attribute operator value flag]`
    fn attribute(&mut self) -> Option<Node> {
        self.position += 1;
        let start = self.tokens.get(self.position)?.start;
        let mut end = start;
        while self.kind(self.position)? != Tok::CloseSquare {
            end = self.tokens[self.position].end;
            self.position += 1;
        }
        self.position += 1;
        let text = &self.selector[start..end];

        // The first `=`, with the character before it when it makes an operator.
        let Some(equals) = text.find('=') else {
            return Some(Node::Attribute {
                attribute: text.to_string(),
                operator: None,
                value: None,
                insensitive: false,
            });
        };
        let operator_start = match text[..equals].bytes().next_back() {
            Some(b'*' | b'~' | b'^' | b'$' | b'|') => equals - 1,
            _ => equals,
        };
        let attribute = &text[..operator_start];
        let mut value = &text[equals + 1..];
        // `[a="b" i]`: the flag that makes the match insensitive to case.
        let trimmed = value.trim_end_matches(|c: char| c.is_ascii() && is_space(c as u8));
        let insensitive = trimmed.strip_suffix('i').is_some_and(|before| {
            let flagless = before.trim_end_matches(|c: char| c.is_ascii() && is_space(c as u8));
            let found = flagless.len() < before.len();
            if found {
                value = flagless;
            }
            found
        });
        Some(Node::Attribute {
            attribute: attribute.to_string(),
            operator: Some(text[operator_start..=equals].to_string()),
            value: (!value.is_empty()).then(|| value.to_string()),
            insensitive,
        })
    }

    /// A word, with the words glued to it, cut at each `.` and `#`: a tag, classes and ids.
    /// The first piece is always given as a tag when `first_is_name`.
    fn split_word(&mut self, first_is_name: bool) -> Vec<Node> {
        let mut word = self.text(self.tokens[self.position]).to_string();
        while self.kind(self.position + 1) == Some(Tok::Word) {
            self.position += 1;
            let current = self.text(self.tokens[self.position]).to_string();
            word.push_str(&current);
            // An escaped space is part of the word.
            if current.ends_with('\\')
                && let Some(next) = self.tokens.get(self.position + 1)
                && next.kind == Tok::Space
            {
                word.push_str(&self.selector[next.start..next.end]);
                self.position += 1;
            }
        }
        self.position += 1;

        let mut cuts: Vec<usize> = vec![0];
        for (index, c) in word.char_indices() {
            let interpolation = c == '#' && word[index..].starts_with("#{");
            if (c == '.' || c == '#') && !interpolation && index > 0 {
                cuts.push(index);
            }
        }
        let mut nodes = Vec::new();
        for (index, cut) in cuts.iter().enumerate() {
            let piece = &word[*cut..cuts.get(index + 1).copied().unwrap_or(word.len())];
            let interpolation = piece.starts_with("#{");
            nodes.push(if index == 0 && first_is_name {
                Node::Tag(piece.to_string())
            } else if let Some(class) = piece.strip_prefix('.') {
                Node::Class(class.to_string())
            } else if let Some(id) = piece.strip_prefix('#').filter(|_| !interpolation) {
                Node::Id(id.to_string())
            } else {
                Node::Tag(piece.to_string())
            });
        }
        nodes
    }
}

/// Prettier's `quoteAttributeValue`: quotes around a value that has none.
fn quote_attribute_value(value: &str, single_quote: bool) -> String {
    let quote = if single_quote { '\'' } else { '"' };
    // A flag the parser does not know: `[a=b s]`.
    let (value, flag) = match value.char_indices().next_back() {
        Some((last, flag)) if flag.is_ascii_alphabetic() => {
            let before = &value[..last];
            let trimmed = before.trim_end();
            if trimmed.len() < before.len() && !trimmed.is_empty() {
                (trimmed, Some(flag))
            } else {
                (value, None)
            }
        }
        _ => (value, None),
    };
    let mut quoted = if value.contains(['"', '\'']) {
        value.to_string()
    } else {
        format!("{quote}{value}{quote}")
    };
    if let Some(flag) = flag {
        quoted.push(' ');
        quoted.push(flag);
    }
    quoted
}

pub(super) struct Context {
    pub single_quote: bool,
    /// Inside `@keyframes`, where `FROM` and `TO` are keywords.
    pub in_keyframes: bool,
}

fn print_selector(nodes: &[Node], context: &Context) -> Doc {
    let mut parts = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        let previous = index.checked_sub(1).map(|index| &nodes[index]);
        parts.push(match node {
            Node::Tag(value) => {
                if matches!(previous, Some(Node::Nesting)) {
                    text(value.as_str())
                } else if context.in_keyframes
                    && matches!(value.to_lowercase().as_str(), "from" | "to")
                {
                    text(adjust_numbers(&value.to_lowercase()))
                } else {
                    text(adjust_numbers(value))
                }
            }
            Node::Id(value) => text(format!("#{value}")),
            Node::Class(value) => text(format!(
                ".{}",
                adjust_numbers(&adjust_strings(value, context.single_quote))
            )),
            Node::Universal => text("*"),
            Node::Nesting => text("&"),
            Node::Str(value) => text(adjust_strings(value, context.single_quote)),
            Node::Combinator(value) => {
                if matches!(value.as_str(), "+" | ">" | "~" | ">>>") {
                    concat(vec![
                        if index == 0 { nothing() } else { line() },
                        text(value.as_str()),
                        text(if index == nodes.len() - 1 { "" } else { " " }),
                    ])
                } else {
                    let trimmed = value.trim();
                    concat(vec![
                        if trimmed.starts_with('(') {
                            line()
                        } else {
                            nothing()
                        },
                        if trimmed.is_empty() {
                            line()
                        } else {
                            text(adjust_numbers(&adjust_strings(
                                trimmed,
                                context.single_quote,
                            )))
                        },
                    ])
                }
            }
            Node::Attribute {
                attribute,
                operator,
                value,
                insensitive,
            } => {
                let value = value.as_deref().map_or(String::new(), |value| {
                    quote_attribute_value(
                        &adjust_strings(value.trim(), context.single_quote),
                        context.single_quote,
                    )
                });
                // `[svg|href]`: a namespace.
                let name = match attribute.split_once('|') {
                    Some((namespace, name)) => format!("{}|{}", namespace.trim(), name.trim()),
                    None => attribute.trim().to_string(),
                };
                let printed = format!(
                    "[{name}{}{value}{}]",
                    operator.as_deref().unwrap_or_default(),
                    if *insensitive { " i" } else { "" }
                );
                // The line breaks of a value are kept, and break nothing around them.
                join(
                    &Doc::Line {
                        hard: true,
                        soft: false,
                        literal: true,
                    },
                    printed.split('\n').map(text).collect(),
                )
            }
            Node::Pseudo { value, selectors } => {
                let name = text(maybe_to_lower_case(value));
                if selectors.is_empty() {
                    name
                } else {
                    let separator = concat(vec![text(","), line()]);
                    concat(vec![
                        name,
                        group(vec![
                            text("("),
                            indent(vec![
                                softline(),
                                join(
                                    &separator,
                                    selectors
                                        .iter()
                                        .map(|selector| print_selector(selector, context))
                                        .collect(),
                                ),
                            ]),
                            softline(),
                            text(")"),
                        ]),
                    ])
                }
            }
        });
    }
    let parts = concat(parts);
    group(if nodes.len() > 2 {
        indent(parts)
    } else {
        parts
    })
}

/// Whether a selector holds a comment, outside strings: Prettier then prints it as written.
fn holds_comment(selector: &str) -> bool {
    let bytes = selector.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        match bytes[pos] {
            // `"[^"]+"|'[^']+'`
            quote @ (b'"' | b'\'') => match selector[pos + 1..].find(quote as char) {
                Some(close) if close > 0 => pos += close + 2,
                _ => pos += 1,
            },
            b'/' if matches!(bytes.get(pos + 1), Some(b'/' | b'*')) => return true,
            _ => pos += 1,
        }
    }
    false
}

/// The document of a selector. A selector that is not understood is printed as it was
/// written.
pub(super) fn print(selector: &str, context: &Context) -> Doc {
    let parsed = if holds_comment(selector) {
        None
    } else {
        tokenize(selector).and_then(|tokens| {
            Parser {
                selector,
                tokens,
                position: 0,
            }
            .selectors(false)
        })
    };
    let Some(selectors) = parsed else {
        return text(selector.trim());
    };
    let separator = concat(vec![text(","), hardline()]);
    group(join(
        &separator,
        selectors
            .iter()
            .map(|selector| print_selector(selector, context))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{PrintOptions, print as print_doc};

    fn fmt(selector: &str) -> String {
        let context = Context {
            single_quote: true,
            in_keyframes: false,
        };
        print_doc(
            print(selector, &context),
            &PrintOptions {
                print_width: 80,
                tab_width: 2,
                use_tabs: false,
            },
        )
    }

    #[test]
    fn spaces_combinators_and_puts_one_selector_per_line() {
        assert_eq!(fmt("A,B > C,d+e~f   g"), "A,\nB > C,\nd + e ~ f g");
        assert_eq!(fmt(".a .b>.c+.d~.e"), ".a .b > .c + .d ~ .e");
        assert_eq!(fmt("& > .j"), "& > .j");
        assert_eq!(fmt("> a"), "> a");
    }

    #[test]
    fn lowercases_pseudo_classes_and_quotes_attributes() {
        assert_eq!(fmt("a:HOVER"), "a:hover");
        assert_eq!(fmt("a::BEFORE"), "a::before");
        assert_eq!(fmt("a:not(.b,.c)"), "a:not(.b, .c)");
        assert_eq!(fmt("a:nth-child( 2n + 1 )"), "a:nth-child(2n + 1)");
        assert_eq!(fmt("input[type=text]"), "input[type='text']");
        assert_eq!(fmt("a[data-x=\"a b\" i]"), "a[data-x='a b' i]");
        assert_eq!(fmt("a[b~=c]"), "a[b~='c']");
        assert_eq!(fmt("a[b]"), "a[b]");
        assert_eq!(fmt("[ svg | href = x ]"), "[svg|href='x']");
    }

    #[test]
    fn keeps_what_it_does_not_understand() {
        assert_eq!(fmt(" a /* b */ c "), "a /* b */ c");
        assert_eq!(fmt("a)"), "a)");
        assert_eq!(fmt("50.0%"), "50.0%");
        assert_eq!(fmt(".sm\\:flex"), ".sm\\:flex");
    }
}
