//! Source → tokens: where Liquid and HTML start and end. A port of the tokenizer of Shopify's
//! `liquid-html-parser`.

use crate::ast::is_space;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    Text,
    LiquidTagOpen,
    LiquidTagClose,
    LiquidVariableOutputOpen,
    LiquidVariableOutputClose,
    HtmlTagOpen,
    HtmlCloseTagOpen,
    HtmlTagClose,
    HtmlSelfClose,
    HtmlCommentOpen,
    HtmlCommentClose,
    HtmlDoctypeOpen,
    HtmlEquals,
    HtmlQuoteOpen,
    HtmlQuoteClose,
    YamlFrontmatter,
    EndOfInput,
}

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub kind: TokenType,
    pub start: usize,
    pub end: usize,
}

impl Token {
    /// How many bytes the token is long: 3 for a delimiter that trims whitespace.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.end - self.start
    }
}

#[derive(Default)]
pub struct TokenizeOptions {
    /// The source is not the start of a document: `---` does not open a front matter.
    pub skip_frontmatter: bool,
    /// Start inside the quoted value of an attribute, which this quote closes.
    pub inside_quoted_attribute: Option<char>,
    /// Start inside the attributes of an open tag.
    pub inside_html_tag: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Default,
    HtmlTag,
    QuotedValue,
    LiquidTag,
    LiquidVariableOutput,
}

/// A curly quote closes on its partner, a straight one on itself.
fn closing_quote_for(open: char) -> char {
    match open {
        '“' => '”',
        '”' => '“',
        '‘' => '’',
        '’' => '‘',
        other => other,
    }
}

struct Tokenizer<'s> {
    source: &'s str,
    tokens: Vec<Token>,
    mode_stack: Vec<Mode>,
    mode: Mode,
    pos: usize,
    text_start: Option<usize>,
    /// Whether the pending text is whitespace. Only read inside an HTML tag.
    text_is_space: bool,
}

impl Tokenizer<'_> {
    fn ch(&self, offset: usize) -> Option<char> {
        self.source[self.pos..].chars().nth(offset)
    }

    fn matches(&self, expected: &str) -> bool {
        self.source[self.pos..].starts_with(expected)
    }

    fn flush_text(&mut self) {
        if let Some(start) = self.text_start.take() {
            self.tokens.push(Token {
                kind: TokenType::Text,
                start,
                end: self.pos,
            });
        }
    }

    fn emit(&mut self, kind: TokenType, length: usize) {
        self.flush_text();
        self.tokens.push(Token {
            kind,
            start: self.pos,
            end: self.pos + length,
        });
        self.pos += length;
    }

    fn push_mode(&mut self, next: Mode) {
        self.mode_stack.push(self.mode);
        self.mode = next;
    }

    fn pop_mode(&mut self) {
        self.mode = self.mode_stack.pop().unwrap_or(Mode::Default);
    }

    /// Takes the current character into the pending text.
    fn take_text(&mut self) {
        if self.text_start.is_none() {
            self.text_start = Some(self.pos);
        }
        self.pos += self.ch(0).map_or(1, char::len_utf8);
    }

    fn scan_liquid_open(&mut self) -> bool {
        for (delimiter, kind, mode) in [
            (
                "{{-",
                TokenType::LiquidVariableOutputOpen,
                Mode::LiquidVariableOutput,
            ),
            (
                "{{",
                TokenType::LiquidVariableOutputOpen,
                Mode::LiquidVariableOutput,
            ),
            ("{%-", TokenType::LiquidTagOpen, Mode::LiquidTag),
            ("{%", TokenType::LiquidTagOpen, Mode::LiquidTag),
        ] {
            if self.matches(delimiter) {
                self.emit(kind, delimiter.len());
                self.push_mode(mode);
                return true;
            }
        }
        false
    }
}

pub fn tokenize(source: &str, options: TokenizeOptions) -> Vec<Token> {
    let mut t = Tokenizer {
        source,
        tokens: Vec::new(),
        mode_stack: Vec::new(),
        mode: Mode::Default,
        pos: 0,
        text_start: None,
        text_is_space: false,
    };
    let mut quote_char = '\0';
    if let Some(quote) = options.inside_quoted_attribute {
        quote_char = quote;
        t.mode_stack.push(Mode::HtmlTag);
        t.mode = Mode::QuotedValue;
    } else if options.inside_html_tag {
        t.mode = Mode::HtmlTag;
    }

    if !options.skip_frontmatter && (t.matches("---\n") || t.matches("---\r\n")) {
        let search_start = source.find('\n').unwrap() + 1;
        if let Some(close) = source[search_start..].find("\n---") {
            let mut end = search_start + close + 4;
            if source[end..].starts_with('\r') {
                end += 1;
            }
            if source[end..].starts_with('\n') {
                end += 1;
            }
            t.tokens.push(Token {
                kind: TokenType::YamlFrontmatter,
                start: 0,
                end,
            });
            t.pos = end;
        }
    }

    while t.pos < source.len() {
        match t.mode {
            Mode::LiquidTag => {
                if t.matches("-%}") {
                    t.emit(TokenType::LiquidTagClose, 3);
                    t.pop_mode();
                } else if t.matches("%}") {
                    t.emit(TokenType::LiquidTagClose, 2);
                    t.pop_mode();
                } else {
                    t.take_text();
                }
            }
            Mode::LiquidVariableOutput => {
                if t.matches("-}}") {
                    t.emit(TokenType::LiquidVariableOutputClose, 3);
                    t.pop_mode();
                } else if t.matches("}}") {
                    t.emit(TokenType::LiquidVariableOutputClose, 2);
                    t.pop_mode();
                } else {
                    t.take_text();
                }
            }
            Mode::Default => {
                if t.scan_liquid_open() {
                    continue;
                }
                if t.matches("<!--") {
                    t.emit(TokenType::HtmlCommentOpen, 4);
                    continue;
                }
                if t.matches("-->") {
                    t.emit(TokenType::HtmlCommentClose, 3);
                    continue;
                }
                if t.matches("<!") {
                    t.emit(TokenType::HtmlDoctypeOpen, 2);
                    t.push_mode(Mode::HtmlTag);
                    continue;
                }
                let opens_name =
                    |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphabetic() || c == '{');
                if t.matches("</") && opens_name(t.ch(2)) {
                    t.emit(TokenType::HtmlCloseTagOpen, 2);
                    t.push_mode(Mode::HtmlTag);
                    continue;
                }
                if t.matches("<") && opens_name(t.ch(1)) {
                    t.emit(TokenType::HtmlTagOpen, 1);
                    t.push_mode(Mode::HtmlTag);
                    continue;
                }
                t.take_text();
            }
            Mode::HtmlTag => {
                if t.scan_liquid_open() {
                    continue;
                }
                if t.matches("/>") {
                    t.emit(TokenType::HtmlSelfClose, 2);
                    t.pop_mode();
                    continue;
                }
                let c = t.ch(0).unwrap();
                if c == '>' {
                    t.emit(TokenType::HtmlTagClose, 1);
                    t.pop_mode();
                    continue;
                }
                if c == '=' {
                    t.emit(TokenType::HtmlEquals, 1);
                    continue;
                }
                if matches!(c, '"' | '\'' | '“' | '”' | '‘' | '’') {
                    quote_char = c;
                    t.emit(TokenType::HtmlQuoteOpen, c.len_utf8());
                    t.push_mode(Mode::QuotedValue);
                    continue;
                }
                // A run of whitespace inside a tag is a token of its own, never part of a name or
                // of an unquoted value.
                let space = is_space(c);
                if t.text_start.is_some() && space != t.text_is_space {
                    t.flush_text();
                }
                if t.text_start.is_none() {
                    t.text_is_space = space;
                }
                t.take_text();
            }
            Mode::QuotedValue => {
                if t.scan_liquid_open() {
                    continue;
                }
                let c = t.ch(0).unwrap();
                if c == closing_quote_for(quote_char) || c == quote_char {
                    t.emit(TokenType::HtmlQuoteClose, c.len_utf8());
                    t.pop_mode();
                    continue;
                }
                t.take_text();
            }
        }
    }
    t.flush_text();
    t.tokens.push(Token {
        kind: TokenType::EndOfInput,
        start: source.len(),
        end: source.len(),
    });
    t.tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<(TokenType, &str)> {
        tokenize(source, TokenizeOptions::default())
            .into_iter()
            .map(|token| (token.kind, &source[token.start..token.end]))
            .collect()
    }

    #[test]
    fn splits_liquid_from_html() {
        use TokenType::*;
        assert_eq!(
            kinds("<a href=\"{{ url }}\">x</a>"),
            vec![
                (HtmlTagOpen, "<"),
                (Text, "a"),
                (Text, " "),
                (Text, "href"),
                (HtmlEquals, "="),
                (HtmlQuoteOpen, "\""),
                (LiquidVariableOutputOpen, "{{"),
                (Text, " url "),
                (LiquidVariableOutputClose, "}}"),
                (HtmlQuoteClose, "\""),
                (HtmlTagClose, ">"),
                (Text, "x"),
                (HtmlCloseTagOpen, "</"),
                (Text, "a"),
                (HtmlTagClose, ">"),
                (EndOfInput, ""),
            ]
        );
    }

    #[test]
    fn a_front_matter_is_only_read_at_the_start() {
        assert_eq!(kinds("---\na: 1\n---\nx")[0].0, TokenType::YamlFrontmatter);
        assert_eq!(kinds("x\n---\na: 1\n---\n")[0].0, TokenType::Text);
    }
}
