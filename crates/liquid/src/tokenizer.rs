//! Splits a template into text, `{{ output }}` and `{% tag %}` tokens.
//!
//! This is a port of Ruby Liquid's `Tokenizer`, including how it treats unterminated and
//! malformed delimiters, because those tokens are what the block parser then reports errors on.

/// A stream of raw tokens with line tracking.
pub struct Tokenizer<'s> {
    tokens: Vec<&'s str>,
    offset: usize,
    line_number: u32,
    for_liquid_tag: bool,
}

impl<'s> Tokenizer<'s> {
    /// Tokenizes a whole template.
    pub fn new(source: &'s str) -> Self {
        Tokenizer {
            tokens: Scanner { source, pos: 0 }.tokenize(),
            offset: 0,
            line_number: 1,
            for_liquid_tag: false,
        }
    }

    /// Tokenizes the body of a `{% liquid %}` tag: one token per line.
    pub fn for_liquid_tag(source: &'s str, line_number: u32) -> Self {
        Tokenizer {
            tokens: source.split('\n').collect(),
            offset: 0,
            line_number,
            for_liquid_tag: true,
        }
    }

    pub fn is_for_liquid_tag(&self) -> bool {
        self.for_liquid_tag
    }

    /// The line the *next* token starts on.
    pub fn line_number(&self) -> u32 {
        self.line_number
    }

    pub fn shift(&mut self) -> Option<&'s str> {
        let token = *self.tokens.get(self.offset)?;
        self.offset += 1;
        self.line_number += if self.for_liquid_tag {
            1
        } else {
            token.bytes().filter(|b| *b == b'\n').count() as u32
        };
        Some(token)
    }
}

struct Scanner<'s> {
    source: &'s str,
    pos: usize,
}

impl<'s> Scanner<'s> {
    fn tokenize(mut self) -> Vec<&'s str> {
        let mut tokens = Vec::new();
        while self.pos < self.source.len() {
            tokens.push(self.next_token());
        }
        tokens
    }

    fn bytes(&self) -> &'s [u8] {
        self.source.as_bytes()
    }

    fn peek(&self) -> Option<u8> {
        self.bytes().get(self.pos).copied()
    }

    fn scan_byte(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        Some(byte)
    }

    fn next_token(&mut self) -> &'s str {
        if self.peek() == Some(b'{') {
            match self.bytes().get(self.pos + 1) {
                Some(b'%') => {
                    self.pos += 2;
                    return self.next_tag_token();
                }
                Some(b'{') => {
                    self.pos += 2;
                    return self.next_variable_token();
                }
                _ => {}
            }
        }
        self.next_text_token()
    }

    fn next_text_token(&mut self) -> &'s str {
        let start = self.pos;
        let bytes = self.bytes();
        // Skip the first byte: when we get here a leading `{` is known not to open a delimiter.
        let mut i = start + 1;
        while i + 1 < bytes.len() {
            if bytes[i] == b'{' && (bytes[i + 1] == b'{' || bytes[i + 1] == b'%') {
                self.pos = i;
                return &self.source[start..i];
            }
            i += 1;
        }
        self.pos = bytes.len();
        &self.source[start..]
    }

    fn next_variable_token(&mut self) -> &'s str {
        let start = self.pos - 2;
        let mut byte_a = self.scan_byte();
        let mut byte_b = byte_a;
        while byte_b.is_some() {
            while let Some(a) = byte_a {
                if a == b'}' || a == b'{' {
                    break;
                }
                byte_a = self.scan_byte();
            }
            let Some(a) = byte_a else { break };
            if self.pos >= self.source.len() {
                return if a == b'}' {
                    &self.source[start..self.pos]
                } else {
                    "{{"
                };
            }
            byte_b = self.scan_byte();
            if a == b'}' {
                if byte_b != Some(b'}') {
                    self.pos -= 1;
                }
                return &self.source[start..self.pos];
            } else if a == b'{' && byte_b == Some(b'%') {
                return self.next_tag_token_with_start(start);
            }
            byte_a = byte_b;
        }
        "{{"
    }

    fn find_tag_end(&self) -> Option<usize> {
        self.source[self.pos..].find("%}").map(|i| self.pos + i + 2)
    }

    fn next_tag_token(&mut self) -> &'s str {
        let start = self.pos - 2;
        match self.find_tag_end() {
            Some(end) => {
                self.pos = end;
                &self.source[start..end]
            }
            None => "{%",
        }
    }

    fn next_tag_token_with_start(&mut self, start: usize) -> &'s str {
        if let Some(end) = self.find_tag_end() {
            self.pos = end;
        }
        &self.source[start..self.pos]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(source: &str) -> Vec<&str> {
        let mut tokenizer = Tokenizer::new(source);
        let mut out = Vec::new();
        while let Some(token) = tokenizer.shift() {
            out.push(token);
        }
        out
    }

    #[test]
    fn splits_text_tags_and_variables() {
        assert_eq!(
            tokens("a {{ b }} c {% d %} e"),
            vec!["a ", "{{ b }}", " c ", "{% d %}", " e"]
        );
        assert_eq!(tokens("{ {a} }"), vec!["{ {a} }"]);
        assert_eq!(tokens("{{ a }"), vec!["{{ a }"]);
        assert_eq!(tokens("{{ a } b"), vec!["{{ a }", " b"]);
        assert_eq!(tokens("x{{y"), vec!["x", "{{"]);
    }

    #[test]
    fn tracks_lines() {
        let mut tokenizer = Tokenizer::new("a\n{{ b }}\n\n{% c %}");
        assert_eq!(tokenizer.line_number(), 1);
        tokenizer.shift();
        assert_eq!(tokenizer.line_number(), 2);
        tokenizer.shift();
        tokenizer.shift();
        assert_eq!(tokenizer.line_number(), 4);
    }
}
