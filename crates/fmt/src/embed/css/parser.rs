//! A style sheet → rules, declarations, at-rules and comments, cut where PostCSS cuts them.

/// A range of the style sheet, in bytes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Range {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug)]
pub(super) enum Kind {
    Comment,
    Rule {
        /// The selector as written, without the whitespace around it.
        selector: Range,
        nodes: Vec<Node>,
    },
    AtRule {
        name: String,
        /// What stands between the name and the block or the semicolon, trimmed.
        params: Range,
        nodes: Option<Vec<Node>>,
    },
    Declaration {
        /// The hack some properties start with for old browsers: `*` or `_`.
        hack: String,
        prop: String,
        /// The colon, and what PostCSS keeps with it.
        between: String,
        value: Range,
        important: bool,
        /// The rules of a custom property whose value is a block: `--a: { b: c }`.
        block: Option<Vec<Node>>,
    },
}

#[derive(Debug)]
pub(super) struct Node {
    pub kind: Kind,
    /// From the first character to past the `;` or the `}` that ends the node.
    pub range: Range,
}

pub(super) struct Parser<'s> {
    source: &'s str,
    bytes: &'s [u8],
    pos: usize,
}

/// The end of the string that starts at `start`, past its closing quote.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let quote = bytes[start];
    let mut pos = start + 1;
    while pos < bytes.len() {
        match bytes[pos] {
            b'\\' => pos += 2,
            byte if byte == quote => return Some(pos + 1),
            _ => pos += 1,
        }
    }
    None
}

/// The end of the comment that starts at `start`, past its `*/`.
fn comment_end(source: &str, start: usize) -> Option<usize> {
    source[start + 2..]
        .find("*/")
        .map(|end| start + 2 + end + 2)
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | b'\t' | b'\r' | b'\x0c')
}

impl<'s> Parser<'s> {
    pub(super) fn new(source: &'s str) -> Parser<'s> {
        Parser {
            source,
            bytes: source.as_bytes(),
            pos: 0,
        }
    }

    fn skip_spaces(&mut self) {
        while self.pos < self.bytes.len() && is_space(self.bytes[self.pos]) {
            self.pos += 1;
        }
    }

    /// The range without the whitespace and the comments at its end.
    fn trim_end(&self, start: usize, mut end: usize) -> usize {
        loop {
            while end > start && is_space(self.bytes[end - 1]) {
                end -= 1;
            }
            let text = &self.source[start..end];
            match text.rfind("/*") {
                Some(comment) if text.ends_with("*/") && comment + 4 <= text.len() => {
                    end = start + comment;
                }
                _ => return end,
            }
        }
    }

    /// The nodes up to the `}` that closes the block, or to the end at the root. `None` when
    /// the style sheet is one PostCSS refuses.
    pub(super) fn nodes(&mut self, in_block: bool) -> Option<Vec<Node>> {
        let mut nodes = Vec::new();
        loop {
            self.skip_spaces();
            let Some(&byte) = self.bytes.get(self.pos) else {
                // A block that is never closed.
                return (!in_block).then_some(nodes);
            };
            match byte {
                b'}' => {
                    if !in_block {
                        return None;
                    }
                    self.pos += 1;
                    return Some(nodes);
                }
                b';' => self.pos += 1,
                b'/' if self.bytes.get(self.pos + 1) == Some(&b'*') => {
                    let start = self.pos;
                    self.pos = comment_end(self.source, start)?;
                    nodes.push(Node {
                        kind: Kind::Comment,
                        range: Range {
                            start,
                            end: self.pos,
                        },
                    });
                }
                b'@' => nodes.push(self.at_rule()?),
                _ => nodes.push(self.rule_or_declaration()?),
            }
        }
    }

    /// Moves to the first `;`, `{` or `}` outside strings, comments and brackets. Says whether
    /// a colon was met on the way.
    fn scan_statement(&mut self, custom_property: bool) -> Option<bool> {
        let mut closers: Vec<u8> = Vec::new();
        let mut colon = false;
        while let Some(&byte) = self.bytes.get(self.pos) {
            match byte {
                b'"' | b'\'' => {
                    self.pos = string_end(self.bytes, self.pos)?;
                    continue;
                }
                b'/' if self.bytes.get(self.pos + 1) == Some(&b'*') => {
                    self.pos = comment_end(self.source, self.pos)?;
                    continue;
                }
                b'\\' => {
                    self.pos += 2;
                    continue;
                }
                b'(' => closers.push(b')'),
                b'[' => closers.push(b']'),
                // The value of a custom property may be a block.
                b'{' if custom_property && colon => closers.push(b'}'),
                _ if closers.last() == Some(&byte) => {
                    closers.pop();
                }
                b';' | b'{' | b'}' if closers.is_empty() => return Some(colon),
                b':' if closers.is_empty() => colon = true,
                _ => {}
            }
            self.pos += 1;
        }
        closers.is_empty().then_some(colon)
    }

    fn at_rule(&mut self) -> Option<Node> {
        let start = self.pos;
        self.pos += 1;
        let name_start = self.pos;
        while self.bytes.get(self.pos).is_some_and(|byte| {
            !is_space(*byte)
                && !matches!(
                    byte,
                    b'"' | b'#'
                        | b'\''
                        | b'('
                        | b')'
                        | b'/'
                        | b';'
                        | b'['
                        | b'\\'
                        | b']'
                        | b'{'
                        | b'}'
                )
        }) {
            self.pos += 1;
        }
        let name = self.source[name_start..self.pos].to_string();
        if name.is_empty() {
            return None;
        }
        self.skip_spaces();
        let params_start = self.pos;
        self.scan_statement(false)?;
        let params = Range {
            start: params_start,
            end: params_start
                + self.source[params_start..self.pos]
                    .trim_end_matches(|c: char| c.is_ascii() && is_space(c as u8))
                    .len(),
        };
        let nodes = match self.bytes.get(self.pos) {
            Some(b'{') => {
                self.pos += 1;
                Some(self.nodes(true)?)
            }
            Some(b';') => {
                self.pos += 1;
                None
            }
            _ => None,
        };
        Some(Node {
            kind: Kind::AtRule {
                name,
                params,
                nodes,
            },
            range: Range {
                start,
                end: self.pos,
            },
        })
    }

    fn rule_or_declaration(&mut self) -> Option<Node> {
        let start = self.pos;
        let custom_property = self.source[start..].starts_with("--");
        let colon = self.scan_statement(custom_property)?;
        match self.bytes.get(self.pos) {
            Some(b'{') => {
                let selector = Range {
                    start,
                    end: start
                        + self.source[start..self.pos]
                            .trim_end_matches(|c: char| c.is_ascii() && is_space(c as u8))
                            .len(),
                };
                self.pos += 1;
                let nodes = self.nodes(true)?;
                Some(Node {
                    kind: Kind::Rule { selector, nodes },
                    range: Range {
                        start,
                        end: self.pos,
                    },
                })
            }
            // A statement without a colon is not a declaration: PostCSS stops there.
            _ if !colon => None,
            terminator => {
                let mut end = self.pos;
                if terminator == Some(&b';') {
                    self.pos += 1;
                    let kind = self.declaration(start, end, custom_property)?;
                    return Some(Node {
                        kind,
                        range: Range {
                            start,
                            end: self.pos,
                        },
                    });
                }
                // The last declaration of a block: the comments after it are nodes.
                if !custom_property {
                    end = self.trim_end(start, end);
                    self.pos = end;
                }
                let kind = self.declaration(start, end, custom_property)?;
                Some(Node {
                    kind,
                    range: Range { start, end },
                })
            }
        }
    }

    fn declaration(&mut self, start: usize, end: usize, custom_property: bool) -> Option<Kind> {
        let text = &self.source[start..end];
        // The property ends at the colon, at whitespace, or at a comment.
        let prop_end = text
            .find(|c: char| c == ':' || (c.is_ascii() && is_space(c as u8)))
            .map(|end| text[..end].find("/*").unwrap_or(end))?;
        let mut prop = &text[..prop_end];
        let colon = prop_end + text[prop_end..].find(':')?;
        // Between the property and the colon there may only be whitespace and comments.
        let mut gap = text[prop_end..colon].trim();
        while let Some(rest) = gap.strip_prefix("/*") {
            gap = rest.split_once("*/")?.1.trim();
        }
        if !gap.is_empty() || prop.is_empty() {
            return None;
        }
        let mut hack = String::new();
        if prop.starts_with(['*', '_']) {
            hack.push_str(&prop[..1]);
            prop = &prop[1..];
        }

        // The whitespace and the comments after the colon belong with it.
        let mut value_start = colon + 1;
        loop {
            let rest = &text[value_start..];
            let trimmed = rest.trim_start_matches(|c: char| c.is_ascii() && is_space(c as u8));
            value_start += rest.len() - trimmed.len();
            match trimmed.strip_prefix("/*").and_then(|rest| rest.find("*/")) {
                Some(comment_end) => value_start += comment_end + 4,
                None => break,
            }
        }
        let mut value_end = text.len();
        let mut between_end = value_start;
        let mut blank_value = None;
        if text[value_start..].trim().is_empty() {
            // Nothing but whitespace. A custom property keeps it as its value.
            between_end = colon + 1;
            let end = if custom_property {
                text.len()
            } else {
                colon + 1
            };
            blank_value = Some(Range {
                start: start + colon + 1,
                end: start + end,
            });
            value_start = colon + 1;
        }

        let mut important = false;
        if !custom_property || !text[value_start..].trim_start().starts_with('{') {
            let trimmed = text[value_start..value_end].trim_end();
            let lower = trimmed.to_ascii_lowercase();
            if let Some(before) = lower.strip_suffix("important")
                && let Some(before) = before.trim_end().strip_suffix('!')
            {
                important = true;
                value_end = value_start + before.len();
            }
        }
        let value_text = &text[value_start..value_end];
        let trimmed_end = value_start + value_text.trim_end().len();

        let mut block = None;
        if custom_property && value_text.trim_start().starts_with('{') {
            // `--a: { b: c }`: the value is a block when what it holds parses as one.
            let inner_start = start + value_start + value_text.find('{')? + 1;
            if value_text.trim_end().ends_with('}') {
                let mut inner = Parser::new(&self.source[..start + trimmed_end]);
                inner.pos = inner_start;
                // The last `}` is that of the block.
                inner.bytes = &inner.bytes[..start + trimmed_end - 1];
                inner.source = &self.source[..start + trimmed_end - 1];
                block = inner.nodes(false);
            }
        } else if !custom_property && has_top_level_colon(value_text) {
            // PostCSS: "Missed semicolon".
            return None;
        }

        Some(Kind::Declaration {
            hack,
            prop: prop.to_string(),
            between: text[prop_end..between_end].to_string(),
            value: blank_value.unwrap_or(Range {
                start: start + value_start,
                end: start + trimmed_end,
            }),
            important,
            block,
        })
    }
}

/// Whether a value holds a colon outside strings, comments and brackets, other than the one
/// of `progid:`.
fn has_top_level_colon(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut depth = 0;
    let mut pos = 0;
    while pos < bytes.len() {
        match bytes[pos] {
            b'"' | b'\'' => match string_end(bytes, pos) {
                Some(end) => {
                    pos = end;
                    continue;
                }
                None => return false,
            },
            b'/' if bytes.get(pos + 1) == Some(&b'*') => match comment_end(value, pos) {
                Some(end) => {
                    pos = end;
                    continue;
                }
                None => return false,
            },
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b':' if depth == 0 && !value[..pos].ends_with("progid") => return true,
            _ => {}
        }
        pos += 1;
    }
    false
}

/// Parses a style sheet, or says with `None` that PostCSS would refuse it.
pub(super) fn parse(source: &str) -> Option<Vec<Node>> {
    Parser::new(source).nodes(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline(nodes: &[Node], source: &str) -> Vec<String> {
        nodes
            .iter()
            .map(|node| match &node.kind {
                Kind::Comment => "comment".to_string(),
                Kind::Rule { selector, nodes } => format!(
                    "rule {} [{}]",
                    &source[selector.start..selector.end],
                    outline(nodes, source).join(", ")
                ),
                Kind::AtRule {
                    name,
                    params,
                    nodes,
                } => format!(
                    "@{name} {} [{}]",
                    &source[params.start..params.end],
                    nodes
                        .as_ref()
                        .map_or("-".to_string(), |nodes| outline(nodes, source).join(", "))
                ),
                Kind::Declaration {
                    prop,
                    value,
                    important,
                    ..
                } => format!(
                    "{prop}={}{}",
                    &source[value.start..value.end],
                    if *important { "!" } else { "" }
                ),
            })
            .collect()
    }

    #[test]
    fn tells_rules_from_declarations() {
        let source =
            "a:hover{color:red;b : c !IMPORTANT}@media (min-width:1px){a{b:c}}@import 'x';/* c */";
        assert_eq!(
            outline(&parse(source).unwrap(), source),
            vec![
                "rule a:hover [color=red, b=c!]",
                "@media (min-width:1px) [rule a [b=c]]",
                "@import 'x' [-]",
                "comment",
            ]
        );
    }

    #[test]
    fn nested_rules_and_custom_properties() {
        let source = ".a{&:hover{b:c}--x:{d:e};--y: f;g:url(a;b)}";
        assert_eq!(
            outline(&parse(source).unwrap(), source),
            vec!["rule .a [rule &:hover [b=c], --x={d:e}, --y=f, g=url(a;b)]"]
        );
    }

    #[test]
    fn refuses_what_postcss_refuses() {
        assert!(parse("a{b:c").is_none());
        assert!(parse("a{b}").is_none());
        assert!(parse("a{b:c d:e}").is_none());
        assert!(parse("}").is_none());
        assert!(parse("a{b:'c}").is_none());
    }
}
