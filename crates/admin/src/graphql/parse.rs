//! Reads an executable GraphQL document (operations and fragments) into the tree of
//! [`super::ast`], or fails as Shopify does: graphql-ruby's parser (its C parser, which keeps
//! the messages of the original Racc grammar) answers `Parse error on "}" (RCURLY) at [1, 9]`,
//! with the location of the token it did not expect, and `Unexpected end of document`, without a
//! location, when the document stops too soon.
//!
//! A type system definition (`type Foo { ... }`) is read over and kept as a
//! [`Definition::TypeSystem`], since validation, not the parser, refuses it.

use super::ast::*;
use super::error::Error;
use crate::schema::TypeRef;

/// How deeply selections and values may nest. A query nested deeper is refused, with the
/// error graphql-ruby answers when its stack runs out, rather than running out of stack here
/// (the deepest real queries, graphql-js's introspection query among them, nest about 15 deep).
pub const MAX_DEPTH: usize = 100;

/// Parses a document.
pub fn parse(source: &str) -> Result<Document<'_>, Box<Error>> {
    let mut parser = Parser {
        lexer: Lexer::new(source),
        token: Token::end(Pos { line: 1, column: 1 }),
        depth: 0,
    };
    parser.advance()?;
    if parser.token.kind == Kind::End {
        return Err(end_of_document());
    }
    let mut definitions = Vec::new();
    while parser.token.kind != Kind::End {
        definitions.push(parser.definition()?);
    }
    Ok(Document { definitions })
}

fn too_large() -> Box<Error> {
    Box::new(Error::new("This query is too large to execute."))
}

fn end_of_document() -> Box<Error> {
    Box::new(Error::new("Unexpected end of document").with_locations(Vec::new()))
}

/// The kinds of tokens, named as graphql-ruby names them in its errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    End,
    LCurly,
    RCurly,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Colon,
    VarSign,
    DirSign,
    Ellipsis,
    Equals,
    Bang,
    Pipe,
    Amp,
    Name,
    Int,
    Float,
    String,
    UnknownChar,
    BadUnicodeEscape,
}

#[derive(Clone, Debug)]
struct Token<'q> {
    kind: Kind,
    /// The source of the token (for a string, with its quotes).
    text: &'q str,
    /// The value of a string token.
    value: Option<String>,
    pos: Pos,
}

impl<'q> Token<'q> {
    fn end(pos: Pos) -> Token<'q> {
        Token {
            kind: Kind::End,
            text: "",
            value: None,
            pos,
        }
    }

    /// The name graphql-ruby gives the token: `RCURLY`, `IDENTIFIER`, or the keyword a name is
    /// (`QUERY`, `ON`, ...).
    fn name(&self) -> &'static str {
        match self.kind {
            Kind::End => "END",
            Kind::LCurly => "LCURLY",
            Kind::RCurly => "RCURLY",
            Kind::LParen => "LPAREN",
            Kind::RParen => "RPAREN",
            Kind::LBracket => "LBRACKET",
            Kind::RBracket => "RBRACKET",
            Kind::Colon => "COLON",
            Kind::VarSign => "VAR_SIGN",
            Kind::DirSign => "DIR_SIGN",
            Kind::Ellipsis => "ELLIPSIS",
            Kind::Equals => "EQUALS",
            Kind::Bang => "BANG",
            Kind::Pipe => "PIPE",
            Kind::Amp => "AMP",
            Kind::Int => "INT",
            Kind::Float => "FLOAT",
            Kind::String => "STRING",
            Kind::UnknownChar => "UNKNOWN_CHAR",
            Kind::BadUnicodeEscape => "BAD_UNICODE_ESCAPE",
            Kind::Name => match self.text {
                "on" => "ON",
                "fragment" => "FRAGMENT",
                "true" => "TRUE",
                "false" => "FALSE",
                "null" => "NULL",
                "query" => "QUERY",
                "mutation" => "MUTATION",
                "subscription" => "SUBSCRIPTION",
                "schema" => "SCHEMA",
                "scalar" => "SCALAR",
                "type" => "TYPE",
                "extend" => "EXTEND",
                "implements" => "IMPLEMENTS",
                "interface" => "INTERFACE",
                "union" => "UNION",
                "enum" => "ENUM",
                "input" => "INPUT",
                "directive" => "DIRECTIVE",
                "repeatable" => "REPEATABLE",
                _ => "IDENTIFIER",
            },
        }
    }

    fn is(&self, kind: Kind) -> bool {
        self.kind == kind
    }

    fn is_name(&self, text: &str) -> bool {
        self.kind == Kind::Name && self.text == text
    }
}

struct Lexer<'q> {
    source: &'q str,
    /// The byte offset of the next character.
    offset: usize,
    /// Where the next character is: kept as the lexer moves, so that a long line (a minified
    /// query) costs no more than a short one.
    line: usize,
    column: usize,
}

impl<'q> Lexer<'q> {
    fn new(source: &'q str) -> Lexer<'q> {
        Lexer {
            source,
            offset: 0,
            line: 1,
            column: 1,
        }
    }

    fn pos(&self) -> Pos {
        Pos {
            line: self.line,
            column: self.column,
        }
    }

    /// Moves over `count` ASCII characters on the line.
    fn skip(&mut self, count: usize) {
        self.offset += count;
        self.column += count;
    }

    /// Goes back to where a token started, then over its first character: what is left of an
    /// unterminated string is read again as tokens.
    fn rewind(&mut self, start: usize, pos: Pos) {
        self.offset = start;
        self.line = pos.line;
        self.column = pos.column;
        self.bump();
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.offset += c.len_utf8();
        self.column += 1;
        Some(c)
    }

    fn newline(&mut self) {
        self.line += 1;
        self.column = 1;
    }

    /// Skips what GraphQL ignores: white space, line terminators, commas, comments, the BOM.
    fn skip_ignored(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | ',' | '\u{feff}' => {
                    self.bump();
                }
                '\n' => {
                    self.bump();
                    self.newline();
                }
                '\r' => {
                    self.bump();
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    self.newline();
                }
                '#' => {
                    while let Some(c) = self.peek() {
                        if c == '\n' || c == '\r' {
                            break;
                        }
                        self.bump();
                    }
                }
                _ => break,
            }
        }
    }

    fn next(&mut self) -> Token<'q> {
        self.skip_ignored();
        let pos = self.pos();
        let start = self.offset;
        let Some(c) = self.bump() else {
            return Token::end(pos);
        };
        let kind = match c {
            '{' => Kind::LCurly,
            '}' => Kind::RCurly,
            '(' => Kind::LParen,
            ')' => Kind::RParen,
            '[' => Kind::LBracket,
            ']' => Kind::RBracket,
            ':' => Kind::Colon,
            '$' => Kind::VarSign,
            '@' => Kind::DirSign,
            '=' => Kind::Equals,
            '!' => Kind::Bang,
            '|' => Kind::Pipe,
            '&' => Kind::Amp,
            '.' if self.source[self.offset..].starts_with("..") => {
                self.skip(2);
                Kind::Ellipsis
            }
            '_' | 'a'..='z' | 'A'..='Z' => {
                while matches!(self.peek(), Some('_' | 'a'..='z' | 'A'..='Z' | '0'..='9')) {
                    self.bump();
                }
                Kind::Name
            }
            '-' | '0'..='9' => return self.number(start, pos),
            '"' => return self.string(start, pos),
            _ => Kind::UnknownChar,
        };
        Token {
            kind,
            text: &self.source[start..self.offset],
            value: None,
            pos,
        }
    }

    fn digits(&mut self) -> usize {
        let mut count = 0;
        while matches!(self.peek(), Some('0'..='9')) {
            self.bump();
            count += 1;
        }
        count
    }

    fn number(&mut self, start: usize, pos: Pos) -> Token<'q> {
        // The first character (`-` or a digit) is read.
        let first = &self.source[start..self.offset];
        if first == "-" && self.digits() == 0 {
            return Token {
                kind: Kind::UnknownChar,
                text: first,
                value: None,
                pos,
            };
        }
        self.digits();
        let mut kind = Kind::Int;
        let rest = &self.source[self.offset..];
        if rest.starts_with('.') && rest[1..].starts_with(|c: char| c.is_ascii_digit()) {
            self.bump();
            self.digits();
            kind = Kind::Float;
        }
        let rest = &self.source[self.offset..];
        if rest.starts_with(['e', 'E']) {
            let mut lookahead = rest[1..].chars();
            let exponent = match lookahead.next() {
                Some('+' | '-') => lookahead.next().is_some_and(|c| c.is_ascii_digit()),
                Some(c) => c.is_ascii_digit(),
                None => false,
            };
            if exponent {
                self.bump();
                if matches!(self.peek(), Some('+' | '-')) {
                    self.bump();
                }
                self.digits();
                kind = Kind::Float;
            }
        }
        Token {
            kind,
            text: &self.source[start..self.offset],
            value: None,
            pos,
        }
    }

    fn string(&mut self, start: usize, pos: Pos) -> Token<'q> {
        if self.source[self.offset..].starts_with("\"\"") {
            self.skip(2);
            return self.block_string(start, pos);
        }
        let mut value = String::new();
        let mut bad_escape = false;
        loop {
            match self.bump() {
                None | Some('\n' | '\r') => {
                    // Unterminated: graphql-ruby's lexer stops at the quote.
                    self.rewind(start, pos);
                    return Token {
                        kind: Kind::UnknownChar,
                        text: "\"",
                        value: None,
                        pos,
                    };
                }
                Some('"') => break,
                Some('\\') => match self.bump() {
                    Some('"') => value.push('"'),
                    Some('\\') => value.push('\\'),
                    Some('/') => value.push('/'),
                    Some('b') => value.push('\u{8}'),
                    Some('f') => value.push('\u{c}'),
                    Some('n') => value.push('\n'),
                    Some('r') => value.push('\r'),
                    Some('t') => value.push('\t'),
                    Some('u') => match self.unicode_escape() {
                        Some(c) => value.push(c),
                        None => bad_escape = true,
                    },
                    _ => bad_escape = true,
                },
                Some(c) => value.push(c),
            }
        }
        let text = &self.source[start..self.offset];
        Token {
            kind: if bad_escape {
                Kind::BadUnicodeEscape
            } else {
                Kind::String
            },
            text,
            value: Some(if bad_escape {
                text[1..text.len() - 1].to_string()
            } else {
                value
            }),
            pos,
        }
    }

    /// The character of `\uXXXX` (a surrogate pair is two of them) or `\u{X...}`, after `\u`.
    fn unicode_escape(&mut self) -> Option<char> {
        let rest = &self.source[self.offset..];
        if let Some(braced) = rest.strip_prefix('{') {
            let end = braced.find('}')?;
            let code = u32::from_str_radix(&braced[..end], 16).ok()?;
            self.skip(end + 2);
            return char::from_u32(code);
        }
        let code = u32::from_str_radix(rest.get(..4)?, 16).ok()?;
        self.skip(4);
        if (0xD800..0xDC00).contains(&code) {
            let low = self.source[self.offset..].strip_prefix("\\u")?;
            let low = u32::from_str_radix(low.get(..4)?, 16).ok()?;
            if !(0xDC00..0xE000).contains(&low) {
                return None;
            }
            self.skip(6);
            return char::from_u32(0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00));
        }
        char::from_u32(code)
    }

    fn block_string(&mut self, start: usize, pos: Pos) -> Token<'q> {
        let mut raw = String::new();
        loop {
            let rest = &self.source[self.offset..];
            if rest.starts_with("\"\"\"") {
                self.skip(3);
                break;
            }
            if rest.starts_with("\\\"\"\"") {
                self.skip(4);
                raw.push_str("\"\"\"");
                continue;
            }
            match self.bump() {
                None => {
                    self.rewind(start, pos);
                    return Token {
                        kind: Kind::UnknownChar,
                        text: "\"",
                        value: None,
                        pos,
                    };
                }
                Some('\n') => {
                    raw.push('\n');
                    self.newline();
                }
                Some('\r') => {
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    raw.push('\n');
                    self.newline();
                }
                Some(c) => raw.push(c),
            }
        }
        Token {
            kind: Kind::String,
            text: &self.source[start..self.offset],
            value: Some(block_string_value(&raw)),
            pos,
        }
    }
}

/// The value of a block string: its lines without their common indentation (the first line
/// aside) and without the blank lines that start and end it, as the spec defines it.
fn block_string_value(raw: &str) -> String {
    let lines: Vec<&str> = raw.split('\n').collect();
    let indent = |line: &str| line.len() - line.trim_start_matches([' ', '\t']).len();
    let blank = |line: &str| indent(line) == line.len();
    let common = lines
        .iter()
        .skip(1)
        .filter(|line| !blank(line))
        .map(|line| indent(line))
        .min()
        .unwrap_or(0);
    let mut lines: Vec<&str> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line
            } else {
                &line[common.min(line.len())..]
            }
        })
        .collect();
    while lines.first().is_some_and(|line| blank(line)) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| blank(line)) {
        lines.pop();
    }
    lines.join("\n")
}

struct Parser<'q> {
    lexer: Lexer<'q>,
    token: Token<'q>,
    /// How deep the selection sets and values being read are.
    depth: usize,
}

type Parsed<T> = Result<T, Box<Error>>;

impl<'q> Parser<'q> {
    /// Moves to the next token. A token the lexer could not read fails at once, as in
    /// graphql-ruby, whose grammar has no rule for it.
    fn advance(&mut self) -> Parsed<Token<'q>> {
        let next = self.lexer.next();
        let previous = std::mem::replace(&mut self.token, next);
        if matches!(self.token.kind, Kind::UnknownChar | Kind::BadUnicodeEscape) {
            return Err(self.unexpected());
        }
        Ok(previous)
    }

    /// The error for the current token.
    fn unexpected(&self) -> Box<Error> {
        let token = &self.token;
        let at = format!("at [{}, {}]", token.pos.line, token.pos.column);
        let message = match token.kind {
            Kind::End => return end_of_document(),
            Kind::BadUnicodeEscape => format!(
                "Parse error on bad Unicode escape sequence: {} ({}) {at}",
                ruby_inspect(token.value.as_deref().unwrap_or(token.text)),
                token.name()
            ),
            _ => format!(
                "Parse error on {} ({}) {at}",
                ruby_inspect(token.value.as_deref().unwrap_or(token.text)),
                token.name()
            ),
        };
        Box::new(Error::new(message).with_locations(vec![token.pos]))
    }

    fn expect(&mut self, kind: Kind) -> Parsed<Token<'q>> {
        if self.token.kind != kind {
            return Err(self.unexpected());
        }
        self.advance()
    }

    fn name(&mut self) -> Parsed<&'q str> {
        Ok(self.expect(Kind::Name)?.text)
    }

    fn definition(&mut self) -> Parsed<Definition<'q>> {
        let token = &self.token;
        match token.kind {
            Kind::LCurly => {
                let pos = token.pos;
                Ok(Definition::Operation(Operation {
                    pos,
                    kind: OperationKind::Query,
                    name: None,
                    variables: Vec::new(),
                    directives: Vec::new(),
                    selection_set: self.selection_set()?,
                }))
            }
            Kind::Name => match token.text {
                "query" | "mutation" | "subscription" => self.operation(),
                "fragment" => self.fragment(),
                "schema" | "scalar" | "type" | "interface" | "union" | "enum" | "input"
                | "directive" | "extend" => self.type_system(),
                _ => Err(self.unexpected()),
            },
            Kind::String => self.type_system(),
            _ => Err(self.unexpected()),
        }
    }

    fn operation(&mut self) -> Parsed<Definition<'q>> {
        let token = self.advance()?;
        let kind = match token.text {
            "mutation" => OperationKind::Mutation,
            "subscription" => OperationKind::Subscription,
            _ => OperationKind::Query,
        };
        let name = if self.token.is(Kind::Name) {
            Some(self.name()?)
        } else {
            None
        };
        let variables = if self.token.is(Kind::LParen) {
            self.variable_definitions()?
        } else {
            Vec::new()
        };
        let directives = self.directives()?;
        let selection_set = self.selection_set()?;
        Ok(Definition::Operation(Operation {
            pos: token.pos,
            kind,
            name,
            variables,
            directives,
            selection_set,
        }))
    }

    fn variable_definitions(&mut self) -> Parsed<Vec<VariableDefinition<'q>>> {
        self.expect(Kind::LParen)?;
        let mut definitions = Vec::new();
        loop {
            let pos = self.expect(Kind::VarSign)?.pos;
            let name = self.name()?;
            self.expect(Kind::Colon)?;
            let ty = self.type_ref()?;
            let default_value = if self.token.is(Kind::Equals) {
                self.advance()?;
                Some(self.value()?)
            } else {
                None
            };
            let directives = self.directives()?;
            definitions.push(VariableDefinition {
                pos,
                name,
                ty,
                default_value,
                directives,
            });
            if self.token.is(Kind::RParen) {
                self.advance()?;
                return Ok(definitions);
            }
        }
    }

    fn type_ref(&mut self) -> Parsed<TypeRef> {
        let ty = if self.token.is(Kind::LBracket) {
            self.advance()?;
            let inner = self.type_ref()?;
            self.expect(Kind::RBracket)?;
            TypeRef::List(Box::new(inner))
        } else {
            TypeRef::Named(self.name()?.to_string())
        };
        if self.token.is(Kind::Bang) {
            self.advance()?;
            return Ok(TypeRef::NonNull(Box::new(ty)));
        }
        Ok(ty)
    }

    fn fragment(&mut self) -> Parsed<Definition<'q>> {
        let pos = self.advance()?.pos;
        if self.token.is_name("on") {
            return Err(self.unexpected());
        }
        let name = self.name()?;
        if !self.token.is_name("on") {
            return Err(self.unexpected());
        }
        self.advance()?;
        let type_condition = self.name()?;
        let directives = self.directives()?;
        let selection_set = self.selection_set()?;
        Ok(Definition::Fragment(Fragment {
            pos,
            name,
            type_condition,
            directives,
            selection_set,
        }))
    }

    /// Reads over a type system definition: to the end of its braces (`type Foo { ... }`), or of
    /// its names and punctuation (`scalar Foo`, `union U = A | B`).
    fn type_system(&mut self) -> Parsed<Definition<'q>> {
        let pos = self.token.pos;
        let mut depth = 0usize;
        let mut keyword = None;
        loop {
            let token = &self.token;
            match token.kind {
                Kind::End if depth > 0 => return Err(end_of_document()),
                Kind::End => break,
                // `scalar`, `union` and `directive` have no braces: one starts an operation.
                Kind::LCurly
                    if depth == 0 && matches!(keyword, Some("scalar" | "union" | "directive")) =>
                {
                    break;
                }
                Kind::LCurly | Kind::LParen | Kind::LBracket => depth += 1,
                Kind::RCurly | Kind::RParen | Kind::RBracket => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && token.kind == Kind::RCurly {
                        self.advance()?;
                        break;
                    }
                }
                // `extend type Foo`: the keyword is the second name.
                Kind::Name if depth == 0 && matches!(keyword, None | Some("extend")) => {
                    keyword = Some(token.text);
                }
                Kind::Name if depth == 0 && starts_definition(token.text) => break,
                _ => {}
            }
            self.advance()?;
        }
        Ok(Definition::TypeSystem(pos))
    }

    fn selection_set(&mut self) -> Parsed<SelectionSet<'q>> {
        self.expect(Kind::LCurly)?;
        self.enter()?;
        let mut items = Vec::new();
        loop {
            items.push(self.selection()?);
            if self.token.is(Kind::RCurly) {
                self.advance()?;
                self.depth -= 1;
                return Ok(SelectionSet { items });
            }
        }
    }

    fn enter(&mut self) -> Parsed<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(too_large());
        }
        Ok(())
    }

    fn selection(&mut self) -> Parsed<Selection<'q>> {
        if self.token.is(Kind::Ellipsis) {
            let pos = self.advance()?.pos;
            if self.token.is(Kind::Name) && !self.token.is_name("on") {
                let name = self.name()?;
                let directives = self.directives()?;
                return Ok(Selection::FragmentSpread(FragmentSpread {
                    pos,
                    name,
                    directives,
                }));
            }
            let type_condition = if self.token.is_name("on") {
                self.advance()?;
                Some(self.name()?)
            } else {
                None
            };
            let directives = self.directives()?;
            let selection_set = self.selection_set()?;
            return Ok(Selection::InlineFragment(InlineFragment {
                pos,
                type_condition,
                directives,
                selection_set,
            }));
        }
        let pos = self.token.pos;
        let first = self.name()?;
        let (alias, name) = if self.token.is(Kind::Colon) {
            self.advance()?;
            (Some(first), self.name()?)
        } else {
            (None, first)
        };
        let arguments = if self.token.is(Kind::LParen) {
            self.arguments()?
        } else {
            Vec::new()
        };
        let directives = self.directives()?;
        let selection_set = if self.token.is(Kind::LCurly) {
            self.selection_set()?
        } else {
            SelectionSet::default()
        };
        Ok(Selection::Field(Field {
            pos,
            alias,
            name,
            arguments,
            directives,
            selection_set,
        }))
    }

    fn arguments(&mut self) -> Parsed<Vec<Argument<'q>>> {
        self.expect(Kind::LParen)?;
        let mut arguments = Vec::new();
        loop {
            arguments.push(self.argument()?);
            if self.token.is(Kind::RParen) {
                self.advance()?;
                return Ok(arguments);
            }
        }
    }

    fn argument(&mut self) -> Parsed<Argument<'q>> {
        let pos = self.token.pos;
        let name = self.name()?;
        self.expect(Kind::Colon)?;
        let value = self.value()?;
        Ok(Argument { pos, name, value })
    }

    fn directives(&mut self) -> Parsed<Vec<Directive<'q>>> {
        let mut directives = Vec::new();
        while self.token.is(Kind::DirSign) {
            let pos = self.advance()?.pos;
            let name = self.name()?;
            let arguments = if self.token.is(Kind::LParen) {
                self.arguments()?
            } else {
                Vec::new()
            };
            directives.push(Directive {
                pos,
                name,
                arguments,
            });
        }
        Ok(directives)
    }

    fn value(&mut self) -> Parsed<Value<'q>> {
        let pos = self.token.pos;
        let kind = match self.token.kind {
            Kind::VarSign => {
                self.advance()?;
                ValueKind::Variable(self.name()?)
            }
            Kind::Int => ValueKind::Int(self.advance()?.text),
            Kind::Float => ValueKind::Float(self.advance()?.text),
            Kind::String => ValueKind::String(self.advance()?.value.unwrap_or_default()),
            Kind::Name => {
                let token = self.advance()?;
                match token.text {
                    "true" => ValueKind::Boolean(true),
                    "false" => ValueKind::Boolean(false),
                    "null" => ValueKind::Null,
                    name => ValueKind::Enum(name),
                }
            }
            Kind::LBracket => {
                self.advance()?;
                self.enter()?;
                let mut items = Vec::new();
                while !self.token.is(Kind::RBracket) {
                    items.push(self.value()?);
                }
                self.advance()?;
                self.depth -= 1;
                ValueKind::List(items)
            }
            Kind::LCurly => {
                self.advance()?;
                self.enter()?;
                let mut fields = Vec::new();
                while !self.token.is(Kind::RCurly) {
                    fields.push(self.argument()?);
                }
                self.advance()?;
                self.depth -= 1;
                ValueKind::Object(fields)
            }
            _ => return Err(self.unexpected()),
        };
        Ok(Value { pos, kind })
    }
}

/// Whether a name at the start of a definition starts one.
fn starts_definition(name: &str) -> bool {
    matches!(
        name,
        "query"
            | "mutation"
            | "subscription"
            | "fragment"
            | "schema"
            | "scalar"
            | "type"
            | "interface"
            | "union"
            | "enum"
            | "input"
            | "directive"
            | "extend"
    )
}

/// A string as Ruby's `String#inspect` writes it, which graphql-ruby's messages quote tokens
/// with: in double quotes, `"` and `\` escaped, control characters as `\n`, `\t`, `\e` or
/// `\u0001`.
pub(super) fn ruby_inspect(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // `#{`, `#$` and `#@` would interpolate in Ruby: `inspect` escapes them.
            '#' if matches!(chars.peek(), Some('{' | '$' | '@')) => out.push_str("\\#"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{c}' => out.push_str("\\f"),
            '\u{b}' => out.push_str("\\v"),
            '\u{8}' => out.push_str("\\b"),
            '\u{7}' => out.push_str("\\a"),
            '\u{1b}' => out.push_str("\\e"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(source: &str) -> serde_json::Value {
        parse(source).unwrap_err().to_json()
    }

    #[test]
    fn parses_operations_and_fragments() {
        let source = "query Products($first: Int = 10, $ids: [ID!]!) @skip(if: false) {\n  \
            list: products(first: $first, query: \"tag:\\\"a\\\"\") { edges { node { ...P } } }\n  \
            node(id: \"gid://shopify/Product/1\") { ... on Product { id } ... @include(if: true) { id } }\n}\n\
            fragment P on Product { title tags input: metafields(keys: [\"a\", \"b\"], obj: {a: 1, a: -2.5e3, b: ENUM, c: null}) }";
        let document = parse(source).unwrap();
        assert_eq!(document.definitions.len(), 2);
        let operation = document.operations().next().unwrap();
        assert_eq!(operation.name, Some("Products"));
        assert_eq!(operation.variables[0].ty.to_string(), "Int");
        assert_eq!(operation.variables[1].ty.to_string(), "[ID!]!");
        let Selection::Field(list) = &operation.selection_set.items[0] else {
            panic!()
        };
        assert_eq!(list.alias, Some("list"));
        assert_eq!(list.name, "products");
        assert_eq!(list.pos, Pos { line: 2, column: 3 });
        assert_eq!(
            list.arguments[1].pos,
            Pos {
                line: 2,
                column: 33
            }
        );
        let ValueKind::String(query) = &list.arguments[1].value.kind else {
            panic!()
        };
        assert_eq!(query, "tag:\"a\"");
        let fragment = document.fragment("P").unwrap();
        let Selection::Field(metafields) = &fragment.selection_set.items[2] else {
            panic!()
        };
        let ValueKind::Object(fields) = &metafields.arguments[1].value.kind else {
            panic!()
        };
        // Duplicates and order are kept, for validation.
        let names: Vec<&str> = fields.iter().map(|field| field.name).collect();
        assert_eq!(names, ["a", "a", "b", "c"]);
        assert!(matches!(fields[1].value.kind, ValueKind::Float("-2.5e3")));
    }

    #[test]
    fn reads_strings_and_block_strings() {
        let document =
            parse("{ a(s: \"\\u00e9\\u{1F600}\\ud83d\\ude00\", b: \"\"\"\n    one\n      two\n  \"\"\") }")
                .unwrap();
        let Some(Definition::Operation(operation)) = document.definitions.first() else {
            panic!()
        };
        let Selection::Field(field) = &operation.selection_set.items[0] else {
            panic!()
        };
        let ValueKind::String(s) = &field.arguments[0].value.kind else {
            panic!()
        };
        assert_eq!(s, "é😀😀");
        let ValueKind::String(b) = &field.arguments[1].value.kind else {
            panic!()
        };
        assert_eq!(b, "one\n  two");
    }

    #[test]
    fn fails_as_graphql_ruby() {
        assert_eq!(
            error("{ shop { name }}}"),
            serde_json::json!({
                "message": "Parse error on \"}\" (RCURLY) at [1, 17]",
                "locations": [{"line": 1, "column": 17}],
            })
        );
        assert_eq!(
            error("{ shop { name }"),
            serde_json::json!({"message": "Unexpected end of document", "locations": []})
        );
        assert_eq!(
            error(""),
            serde_json::json!({"message": "Unexpected end of document", "locations": []})
        );
        assert_eq!(
            error("{ shop {} }")["message"],
            "Parse error on \"}\" (RCURLY) at [1, 9]"
        );
        assert_eq!(
            error("{\"query\": \"{ shop }\"}")["message"],
            "Parse error on \"query\" (STRING) at [1, 2]"
        );
        assert_eq!(
            error("query { a(b: ) }")["message"],
            "Parse error on \")\" (RPAREN) at [1, 14]"
        );
        assert_eq!(
            error("query q { a }\nfragment on on X { a }")["message"],
            "Parse error on \"on\" (ON) at [2, 10]"
        );
        assert_eq!(
            error("{ a(b: \"x\\q\") }")["message"],
            "Parse error on bad Unicode escape sequence: \"x\\\\q\" (BAD_UNICODE_ESCAPE) at [1, 8]"
        );
        assert_eq!(
            error("{ a ? }")["message"],
            "Parse error on \"?\" (UNKNOWN_CHAR) at [1, 5]"
        );
    }

    #[test]
    fn counts_columns_on_long_lines() {
        let fields = "a ".repeat(50_000);
        let source = format!("{{ {fields}b(c: \"é\" d: [1 2 3]) }}");
        let start = std::time::Instant::now();
        let document = parse(&source).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        let Some(Definition::Operation(operation)) = document.definitions.first() else {
            panic!()
        };
        let Some(Selection::Field(b)) = operation.selection_set.items.last() else {
            panic!()
        };
        assert_eq!(b.pos.column, 100_003);
        assert_eq!(b.arguments[1].pos.column, 100_012);
    }

    #[test]
    fn refuses_a_query_nested_too_deeply() {
        let deep = format!(
            "{}{}",
            "{ a ".repeat(MAX_DEPTH + 1),
            "}".repeat(MAX_DEPTH + 1)
        );
        assert_eq!(
            error(&deep),
            serde_json::json!({"message": "This query is too large to execute."})
        );
        let fine = format!("{}{}", "{ a ".repeat(MAX_DEPTH), "}".repeat(MAX_DEPTH));
        assert!(parse(&fine).is_ok());
        let values = format!(
            "{{ a(b: {}{}) }}",
            "[".repeat(MAX_DEPTH + 1),
            "]".repeat(MAX_DEPTH + 1)
        );
        assert_eq!(
            error(&values)["message"],
            "This query is too large to execute."
        );
        assert_eq!(ruby_inspect("#{x} #a"), "\"\\#{x} #a\"");
    }

    #[test]
    fn keeps_type_system_definitions_for_validation() {
        let document = parse("type Foo { a: Int }\nscalar Bar\n{ shop { name } }").unwrap();
        assert!(matches!(
            document.definitions[0],
            Definition::TypeSystem(Pos { line: 1, column: 1 })
        ));
        assert!(matches!(
            document.definitions[1],
            Definition::TypeSystem(Pos { line: 2, column: 1 })
        ));
        assert!(matches!(document.definitions[2], Definition::Operation(_)));
    }
}
