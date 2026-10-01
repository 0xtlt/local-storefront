//! The expression lexer and the strict markup parser (ports of Ruby Liquid's `Lexer` and
//! `Parser`).
//!
//! The strict parser does not build expression nodes itself: like the reference implementation it
//! validates the token stream and re-serialises each expression into a normalised string, which
//! is then handed to [`crate::expr::Expr::parse`]. Keeping that indirection is what makes strict
//! and lax parsing agree on every quirk.

use crate::error::{Error, Result};
use crate::number::is_ruby_space;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Pipe,
    Dot,
    Colon,
    Comma,
    OpenSquare,
    CloseSquare,
    OpenRound,
    CloseRound,
    Question,
    Dash,
    DotDot,
    Comparison,
    Id,
    Number,
    String,
    EndOfString,
}

impl TokenKind {
    /// The Ruby symbol name, as it appears in error messages.
    pub fn name(self) -> &'static str {
        match self {
            TokenKind::Pipe => "pipe",
            TokenKind::Dot => "dot",
            TokenKind::Colon => "colon",
            TokenKind::Comma => "comma",
            TokenKind::OpenSquare => "open_square",
            TokenKind::CloseSquare => "close_square",
            TokenKind::OpenRound => "open_round",
            TokenKind::CloseRound => "close_round",
            TokenKind::Question => "question",
            TokenKind::Dash => "dash",
            TokenKind::DotDot => "dotdot",
            TokenKind::Comparison => "comparison",
            TokenKind::Id => "id",
            TokenKind::Number => "number",
            TokenKind::String => "string",
            TokenKind::EndOfString => "end_of_string",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Token<'s> {
    pub kind: TokenKind,
    pub text: &'s str,
}

impl Token<'_> {
    fn inspect(&self) -> String {
        if self.kind == TokenKind::EndOfString {
            "[:end_of_string]".to_string()
        } else {
            format!("[:{}, \"{}\"]", self.kind.name(), self.text)
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Tokenizes tag or output markup.
pub fn tokenize(input: &str) -> Result<Vec<Token<'_>>> {
    let bytes = input.as_bytes();
    let mut out: Vec<Token<'_>> = Vec::new();
    let mut pos = 0;
    loop {
        while let Some(c) = input[pos..].chars().next() {
            if is_ruby_space(c) {
                pos += c.len_utf8();
            } else {
                break;
            }
        }
        if pos >= bytes.len() {
            break;
        }
        let start = pos;
        let byte = bytes[pos];
        let next = bytes.get(pos + 1).copied();
        let simple = |kind: TokenKind, len: usize| Token {
            kind,
            text: &input[start..start + len],
        };
        let token = match byte {
            b'|' => simple(TokenKind::Pipe, 1),
            b':' => simple(TokenKind::Colon, 1),
            b',' => simple(TokenKind::Comma, 1),
            b'[' => simple(TokenKind::OpenSquare, 1),
            b']' => simple(TokenKind::CloseSquare, 1),
            b'(' => simple(TokenKind::OpenRound, 1),
            b')' => simple(TokenKind::CloseRound, 1),
            b'?' => simple(TokenKind::Question, 1),
            b'.' if next == Some(b'.') => simple(TokenKind::DotDot, 2),
            b'.' => simple(TokenKind::Dot, 1),
            b'-' if next.is_some_and(|b| b.is_ascii_digit()) => scan_number(input, start),
            b'-' => simple(TokenKind::Dash, 1),
            b'=' | b'!' => {
                if next == Some(b'=') {
                    simple(TokenKind::Comparison, 2)
                } else {
                    return Err(unexpected_character(input, start));
                }
            }
            b'<' if matches!(next, Some(b'=' | b'>')) => simple(TokenKind::Comparison, 2),
            b'>' if next == Some(b'=') => simple(TokenKind::Comparison, 2),
            b'<' | b'>' => simple(TokenKind::Comparison, 1),
            b'0'..=b'9' => scan_number(input, start),
            b'\'' | b'"' => match input[start + 1..].find(byte as char) {
                Some(end) => simple(TokenKind::String, end + 2),
                None => return Err(unexpected_character(input, start)),
            },
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let mut end = start + 1;
                for c in input[end..].chars() {
                    if is_word_char(c) || c == '-' {
                        end += c.len_utf8();
                    } else {
                        break;
                    }
                }
                if bytes.get(end) == Some(&b'?') {
                    end += 1;
                }
                let text = &input[start..end];
                let after_dot = out.last().is_some_and(|t| t.kind == TokenKind::Dot);
                Token {
                    kind: if text == "contains" && !after_dot {
                        TokenKind::Comparison
                    } else {
                        TokenKind::Id
                    },
                    text,
                }
            }
            _ => return Err(unexpected_character(input, start)),
        };
        pos = start + token.text.len();
        out.push(token);
    }
    out.push(Token {
        kind: TokenKind::EndOfString,
        text: "",
    });
    Ok(out)
}

/// Matches `-?\d+(\.\d+)?`.
fn scan_number(input: &str, start: usize) -> Token<'_> {
    let bytes = input.as_bytes();
    let mut end = start;
    if bytes[end] == b'-' {
        end += 1;
    }
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        end += 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
    }
    Token {
        kind: TokenKind::Number,
        text: &input[start..end],
    }
}

fn unexpected_character(input: &str, pos: usize) -> Error {
    let c = input[pos..].chars().next().unwrap_or(' ');
    Error::syntax(format!("Unexpected character {c}"))
}

/// A cursor over lexed markup with the helpers tags use to parse their arguments.
pub struct MarkupParser<'s> {
    tokens: Vec<Token<'s>>,
    pos: usize,
}

impl<'s> MarkupParser<'s> {
    pub fn new(input: &'s str) -> Result<Self> {
        Ok(MarkupParser {
            tokens: tokenize(input)?,
            pos: 0,
        })
    }

    fn current(&self) -> Token<'s> {
        self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    /// Consumes the current token, whatever it is.
    pub fn consume_any(&mut self) -> &'s str {
        let token = self.current();
        self.pos += 1;
        token.text
    }

    /// Consumes a token of the given kind or fails.
    pub fn consume(&mut self, kind: TokenKind) -> Result<&'s str> {
        let token = self.current();
        if token.kind != kind {
            return Err(Error::syntax(format!(
                "Expected {} but found {}",
                kind.name(),
                token.kind.name()
            )));
        }
        self.pos += 1;
        Ok(token.text)
    }

    /// Consumes a token of the given kind if it is next.
    pub fn consume_if(&mut self, kind: TokenKind) -> Option<&'s str> {
        let token = self.current();
        if token.kind == kind {
            self.pos += 1;
            Some(token.text)
        } else {
            None
        }
    }

    /// Consumes the given identifier if it is next.
    pub fn id(&mut self, name: &str) -> bool {
        let token = self.current();
        if token.kind == TokenKind::Id && token.text == name {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub fn look(&self, kind: TokenKind) -> bool {
        self.look_ahead(kind, 0)
    }

    pub fn look_ahead(&self, kind: TokenKind, ahead: usize) -> bool {
        self.tokens
            .get(self.pos + ahead)
            .is_some_and(|t| t.kind == kind)
    }

    pub fn at_end(&self) -> bool {
        self.look(TokenKind::EndOfString)
    }

    /// Validates one expression and returns its normalised markup.
    pub fn expression(&mut self) -> Result<String> {
        let token = self.current();
        match token.kind {
            TokenKind::Id => {
                let mut out = self.consume_any().to_string();
                out.push_str(&self.variable_lookups()?);
                Ok(out)
            }
            TokenKind::OpenSquare => {
                let mut out = self.consume_any().to_string();
                out.push_str(&self.expression()?);
                out.push_str(self.consume(TokenKind::CloseSquare)?);
                out.push_str(&self.variable_lookups()?);
                Ok(out)
            }
            TokenKind::String | TokenKind::Number => Ok(self.consume_any().to_string()),
            TokenKind::OpenRound => {
                self.consume_any();
                let first = self.expression()?;
                self.consume(TokenKind::DotDot)?;
                let last = self.expression()?;
                self.consume(TokenKind::CloseRound)?;
                Ok(format!("({first}..{last})"))
            }
            _ => Err(Error::syntax(format!(
                "{} is not a valid expression",
                token.inspect()
            ))),
        }
    }

    /// Validates a filter argument, `expr` or `key: expr`, and returns its normalised markup.
    pub fn argument(&mut self) -> Result<String> {
        let mut out = String::new();
        if self.look(TokenKind::Id) && self.look_ahead(TokenKind::Colon, 1) {
            out.push_str(self.consume_any());
            out.push_str(self.consume_any());
            out.push(' ');
        }
        out.push_str(&self.expression()?);
        Ok(out)
    }

    fn variable_lookups(&mut self) -> Result<String> {
        let mut out = String::new();
        loop {
            if self.look(TokenKind::OpenSquare) {
                out.push_str(self.consume_any());
                out.push_str(&self.expression()?);
                out.push_str(self.consume(TokenKind::CloseSquare)?);
            } else if self.look(TokenKind::Dot) {
                out.push_str(self.consume_any());
                out.push_str(self.consume(TokenKind::Id)?);
            } else {
                break;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(input: &str) -> Vec<TokenKind> {
        tokenize(input).unwrap().iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_expressions() {
        use TokenKind::*;
        assert_eq!(
            kinds("a.b | f: 1, 'x'"),
            vec![
                Id,
                Dot,
                Id,
                Pipe,
                Id,
                Colon,
                Number,
                Comma,
                String,
                EndOfString
            ]
        );
        assert_eq!(kinds("a contains b"), vec![Id, Comparison, Id, EndOfString]);
        assert_eq!(kinds("a.contains"), vec![Id, Dot, Id, EndOfString]);
        assert_eq!(
            kinds("(1..n)"),
            vec![OpenRound, Number, DotDot, Id, CloseRound, EndOfString]
        );
        assert_eq!(kinds("-1 - x"), vec![Number, Dash, Id, EndOfString]);
        assert!(tokenize("a = b").is_err());
    }

    #[test]
    fn normalises_expressions() {
        let mut parser = MarkupParser::new("a . b [ 'c' ] .d").unwrap();
        assert_eq!(parser.expression().unwrap(), "a.b['c'].d");
        let mut parser = MarkupParser::new("( 1 .. x.y )").unwrap();
        assert_eq!(parser.expression().unwrap(), "(1..x.y)");
        let mut parser = MarkupParser::new("key : value").unwrap();
        assert_eq!(parser.argument().unwrap(), "key: value");
    }
}
