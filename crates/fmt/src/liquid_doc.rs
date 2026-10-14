//! What a `{% doc %}` tag holds: a description, then `@param`, `@example`, `@description` and
//! `@prompt` annotations. A port of the LiquidDoc parser of Shopify's `liquid-html-parser`.

use crate::ast::{Ast, Id, Kind, Span, is_blank, trim_start};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tok {
    /// `@name`.
    Annotation,
    /// A run of text on one line.
    TextLine,
    Newline,
    EndOfInput,
}

#[derive(Clone, Copy, Debug)]
struct Token {
    kind: Tok,
    start: usize,
    end: usize,
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn skip_blanks(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
        pos += 1;
    }
    pos
}

/// `@\w+` at `pos`: where it ends.
fn annotation_end(bytes: &[u8], pos: usize) -> Option<usize> {
    if bytes.get(pos) != Some(&b'@') || !bytes.get(pos + 1).is_some_and(|byte| is_word(*byte)) {
        return None;
    }
    let mut end = pos + 1;
    while end < bytes.len() && is_word(bytes[end]) {
        end += 1;
    }
    Some(end)
}

fn tokenize(source: &str, body: Span) -> Vec<Token> {
    let bytes = &source.as_bytes()[..body.end];
    let mut tokens = Vec::new();
    let mut pos = body.start;
    let mut after_annotation = false;
    while pos < body.end {
        let newline = if bytes[pos] == b'\n' {
            Some(pos + 1)
        } else if bytes[pos] == b'\r' && bytes.get(pos + 1) == Some(&b'\n') {
            Some(pos + 2)
        } else {
            None
        };
        if let Some(end) = newline {
            tokens.push(Token {
                kind: Tok::Newline,
                start: pos,
                end,
            });
            pos = end;
            after_annotation = false;
            continue;
        }
        let after_blanks = skip_blanks(bytes, pos);
        if let Some(end) = annotation_end(bytes, after_blanks) {
            tokens.push(Token {
                kind: Tok::Annotation,
                start: after_blanks,
                end,
            });
            pos = skip_blanks(bytes, end);
            after_annotation = true;
            continue;
        }
        let line_end = bytes[pos..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(body.end, |index| pos + index);
        let mut end = line_end;
        if !after_annotation {
            // An annotation in the middle of a line ends the text before it.
            let mut index = pos;
            while index < line_end {
                let c = source[index..].chars().next().unwrap();
                let next = index + c.len_utf8();
                if c != '\r' && annotation_end(bytes, next).is_some() {
                    end = next;
                    break;
                }
                index = next;
            }
        }
        if end > pos {
            tokens.push(Token {
                kind: Tok::TextLine,
                start: pos,
                end,
            });
        }
        pos = end;
        after_annotation = false;
    }
    tokens.push(Token {
        kind: Tok::EndOfInput,
        start: body.end,
        end: body.end,
    });
    tokens
}

#[derive(Clone, Copy, PartialEq)]
enum ParamTok {
    Type,
    OptionalName,
    Word,
    Dash,
    Text,
}

/// The end of the `{type}` that starts at `start`, quotes inside it considered.
fn read_type_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'{') {
        return None;
    }
    let mut pos = start + 1;
    while pos < bytes.len() {
        let byte = bytes[pos];
        if byte == b'\'' || byte == b'"' {
            match text[pos + 1..].find(byte as char) {
                Some(close) => pos += 1 + close,
                None => return text[pos + 1..].find('}').map(|end| pos + 1 + end + 1),
            }
        } else if byte == b'}' {
            return Some(pos + 1);
        }
        pos += 1;
    }
    None
}

/// The tokens of what follows `@param` on its line, with their range in the source.
fn tokenize_param(source: &str, line: Token) -> Vec<(ParamTok, usize, usize)> {
    let text = &source[line.start..line.end];
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut pos = 0;
    while pos < text.len() {
        let after_blanks = skip_blanks(bytes, pos);
        if after_blanks > pos {
            pos = after_blanks;
            continue;
        }
        let (kind, end) = if let Some(end) = read_type_end(text, pos) {
            (ParamTok::Type, end)
        } else if bytes[pos] == b'['
            && let Some(close) = text[pos..].find(']')
        {
            (ParamTok::OptionalName, pos + close + 1)
        } else if bytes[pos] == b'-' && matches!(bytes.get(pos + 1), None | Some(b' ' | b'\t')) {
            (ParamTok::Dash, pos + 1)
        } else if is_word(bytes[pos]) {
            let mut end = pos + 1;
            while end < bytes.len() && (is_word(bytes[end]) || bytes[end] == b'-') {
                end += 1;
            }
            (ParamTok::Word, end)
        } else {
            (ParamTok::Text, text.len())
        };
        tokens.push((kind, line.start + pos, line.start + end));
        pos = end;
    }
    tokens
}

struct Collected {
    value: String,
    start: usize,
    end: usize,
}

struct DocParser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    p: usize,
    ast: &'a mut Ast,
}

impl DocParser<'_> {
    fn peek(&self) -> Token {
        self.tokens[self.p]
    }

    fn check(&self, kind: Tok) -> bool {
        self.tokens[self.p].kind == kind
    }

    fn is_at_end(&self) -> bool {
        self.check(Tok::EndOfInput)
    }

    fn advance(&mut self) -> Token {
        self.p += 1;
        self.tokens[self.p - 1]
    }

    fn accept(&mut self, kind: Tok) -> Option<Token> {
        self.check(kind).then(|| self.advance())
    }

    fn value(&self, token: Token) -> &str {
        &self.source[token.start..token.end]
    }

    /// Whether the current token is a line of whitespace only.
    fn at_blank_line(&self) -> bool {
        self.check(Tok::TextLine) && is_blank(self.value(self.peek()))
    }

    fn skip_blank_lines(&mut self) {
        while !self.is_at_end() && (self.check(Tok::Newline) || self.at_blank_line()) {
            self.p += 1;
        }
    }

    fn text_node(&mut self, value: String, start: usize, end: usize) -> Id {
        self.ast.add(Kind::Text { value }, Span::new(start, end))
    }

    /// The lines of text up to the next annotation, joined as they were written.
    fn collect_content_lines(
        &mut self,
        stop_at_paragraph_break: bool,
        trim_trailing_newlines: bool,
    ) -> Collected {
        let mut value = String::new();
        let mut start = self.peek().start;
        let mut end = start;
        while !self.is_at_end() {
            let current = self.peek();
            if current.kind == Tok::Annotation {
                break;
            }
            if current.kind == Tok::Newline {
                let mut ahead = self.p + 1;
                while ahead < self.tokens.len() {
                    let token = self.tokens[ahead];
                    let blank = token.kind == Tok::TextLine && is_blank(self.value(token));
                    if token.kind != Tok::Newline && !blank {
                        break;
                    }
                    ahead += 1;
                }
                let next = self.tokens.get(ahead).copied();
                let next_is_text = next.is_some_and(|token| {
                    token.kind == Tok::TextLine && !is_blank(self.value(token))
                });
                if stop_at_paragraph_break && ahead - self.p >= 2 && next_is_text {
                    break;
                }
                if next.is_none_or(|token| matches!(token.kind, Tok::Annotation | Tok::EndOfInput))
                {
                    // The blank lines before an annotation, or at the end.
                    while self.p < ahead.min(self.tokens.len() - 1)
                        && (self.check(Tok::Newline) || self.at_blank_line())
                    {
                        if !trim_trailing_newlines {
                            if self.check(Tok::Newline) {
                                value.push('\n');
                            }
                            end = self.peek().end;
                        }
                        self.p += 1;
                    }
                    break;
                }
                value.push('\n');
                end = current.end;
                self.p += 1;
                continue;
            }
            let text = self.value(current);
            if is_blank(text) {
                self.p += 1;
                continue;
            }
            if value.is_empty() {
                let trimmed = trim_start(text);
                start = current.start + text.len() - trimmed.len();
                value.push_str(trimmed);
            } else {
                value.push_str(text);
            }
            end = current.end;
            self.p += 1;
        }
        Collected { value, start, end }
    }

    fn parse(&mut self) -> Vec<Id> {
        let mut nodes = Vec::new();
        self.skip_blank_lines();
        if self.check(Tok::TextLine) {
            let content = self.collect_content_lines(false, false);
            nodes.push(self.ast.add(
                Kind::DocDescription {
                    content: content.value,
                    implicit: true,
                    inline: true,
                },
                Span::new(content.start, content.end),
            ));
        }
        self.skip_blank_lines();
        while !self.is_at_end() {
            if self.check(Tok::Newline) || self.at_blank_line() {
                self.p += 1;
                continue;
            }
            if self.check(Tok::Annotation) {
                nodes.push(self.parse_annotation());
                continue;
            }
            let content = self.collect_content_lines(false, true);
            if !is_blank(&content.value) {
                nodes.push(self.text_node(content.value, content.start, content.end));
            }
        }
        nodes
    }

    fn parse_annotation(&mut self) -> Id {
        let annotation = self.advance();
        let name = &self.source[annotation.start + 1..annotation.end];
        match name {
            "param" => self.parse_param(annotation),
            "description" => {
                let (content, inline, end) = self.parse_content(annotation);
                self.ast.add(
                    Kind::DocDescription {
                        content,
                        implicit: false,
                        inline,
                    },
                    Span::new(annotation.start, end),
                )
            }
            "example" => {
                let (content, inline, end) = self.parse_content(annotation);
                self.ast.add(
                    Kind::DocExample { content, inline },
                    Span::new(annotation.start, end),
                )
            }
            "prompt" => {
                let content = self.collect_content_lines(false, false);
                let end = if content.value.is_empty() {
                    annotation.end
                } else {
                    content.end
                };
                self.ast.add(
                    Kind::DocPrompt {
                        content: content.value,
                    },
                    Span::new(annotation.start, end),
                )
            }
            _ if name.starts_with("description") => {
                // `@descriptionfoo`: a description glued to its annotation.
                let content_start = annotation.start + 1 + "description".len();
                let inline = self.accept(Tok::TextLine);
                let rest = self.collect_content_lines(false, false);
                let end = if !is_blank(&rest.value) {
                    rest.end
                } else {
                    inline.map_or(annotation.end, |inline| inline.end)
                };
                self.ast.add(
                    Kind::DocDescription {
                        content: self.source[content_start..end].to_string(),
                        implicit: false,
                        inline: true,
                    },
                    Span::new(annotation.start, end),
                )
            }
            _ => {
                // An annotation that is not known is text.
                let end = self
                    .accept(Tok::TextLine)
                    .map_or(annotation.end, |inline| inline.end);
                self.text_node(
                    self.source[annotation.start..end].to_string(),
                    annotation.start,
                    end,
                )
            }
        }
    }

    /// The content of `@description` and `@example`: on the line of the annotation, or below.
    fn parse_content(&mut self, annotation: Token) -> (String, bool, usize) {
        if let Some(inline) = self.accept(Tok::TextLine) {
            let rest = self.collect_content_lines(false, false);
            let mut content = self.value(inline).to_string();
            if is_blank(&rest.value) {
                return (content, true, inline.end);
            }
            content.push_str(&rest.value);
            return (content, true, rest.end);
        }
        self.skip_blank_lines();
        let content = self.collect_content_lines(false, false);
        let end = if content.value.is_empty() {
            annotation.end
        } else {
            content.end
        };
        (content.value, false, end)
    }

    fn parse_param(&mut self, annotation: Token) -> Id {
        let Some(inline) = self.accept(Tok::TextLine) else {
            return self.ast.add(
                Kind::DocParam {
                    name: String::new(),
                    description: None,
                    param_type: None,
                    required: true,
                },
                Span::new(annotation.start, annotation.end),
            );
        };
        let tokens = tokenize_param(self.source, inline);
        let mut index = 0;
        let mut accept = |kind: ParamTok| {
            let token = tokens.get(index).filter(|token| token.0 == kind).copied();
            if token.is_some() {
                index += 1;
            }
            token
        };
        let param_type = accept(ParamTok::Type)
            .map(|(_, start, end)| self.source[start + 1..end - 1].to_string());
        let (name, required) = match accept(ParamTok::OptionalName) {
            Some((_, start, end)) => (self.source[start + 1..end - 1].to_string(), false),
            None => (
                accept(ParamTok::Word)
                    .map(|(_, start, end)| self.source[start..end].to_string())
                    .unwrap_or_default(),
                true,
            ),
        };
        accept(ParamTok::Dash);
        let inline_description = match (tokens.get(index), tokens.last()) {
            (Some(first), Some(last)) => &self.source[first.1..last.2],
            _ => "",
        };
        let inline_end = tokens.last().map_or(inline.end, |last| last.2);
        let continuation = self.collect_content_lines(true, true);
        let has_continuation = !is_blank(&continuation.value);
        let mut end = inline.end;
        let mut description = None;
        if !inline_description.is_empty() || has_continuation {
            let mut full = inline_description.to_string();
            if has_continuation {
                full.push_str(&continuation.value);
            }
            end = if has_continuation {
                continuation.end
            } else {
                inline_end
            };
            description = Some(full);
        }
        self.ast.add(
            Kind::DocParam {
                name,
                description,
                param_type,
                required,
            },
            Span::new(annotation.start, end),
        )
    }
}

/// Parses the body of a `{% doc %}` tag into the nodes of its raw markup.
pub fn parse_liquid_doc(ast: &mut Ast, source: &str, body: Span) -> Vec<Id> {
    let mut parser = DocParser {
        source,
        tokens: tokenize(source, body),
        p: 0,
        ast,
    };
    parser.parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Vec<Kind> {
        let mut ast = Ast::default();
        let nodes = parse_liquid_doc(&mut ast, source, Span::new(0, source.len()));
        nodes
            .into_iter()
            .map(|id| ast.get(id).kind.clone())
            .collect()
    }

    #[test]
    fn reads_a_description_and_its_params() {
        let nodes =
            parse("\n  Renders a card.\n\n  @param {string} title - The title\n  @param [size]\n");
        assert_eq!(nodes.len(), 3);
        assert!(
            matches!(&nodes[0], Kind::DocDescription { implicit: true, content, .. }
            if content.trim() == "Renders a card.")
        );
        assert!(
            matches!(&nodes[1], Kind::DocParam { name, param_type: Some(kind), description: Some(description), required: true }
            if name == "title" && kind == "string" && description == "The title")
        );
        assert!(
            matches!(&nodes[2], Kind::DocParam { name, required: false, description: None, .. }
            if name == "size")
        );
    }

    #[test]
    fn an_example_may_be_inline_or_below() {
        let nodes = parse("@example {% render 'a' %}\n@example\n  {% render 'b' %}\n");
        assert!(matches!(&nodes[0], Kind::DocExample { inline: true, .. }));
        assert!(
            matches!(&nodes[1], Kind::DocExample { inline: false, content }
            if content.trim() == "{% render 'b' %}")
        );
    }
}
