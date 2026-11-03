//! The grammar of shopify.dev's "Search query syntax", parsed into a tree:
//!
//! ```text
//! Query      = Term { [ whitespace Connective ] whitespace Term }
//! Connective = "AND" | "OR"                       (AND when there is none; OR binds tighter)
//! Term       = [ Modifier ] ( "(" Query ")" | [ name Comparator ] value )
//! Modifier   = "-" | "NOT" whitespace
//! Comparator = ":" | ":<" | ":>" | ":<=" | ":>="
//! ```
//!
//! A value is a name or a quoted string (single or double quotes); a backslash escapes the
//! special characters `: \ ( )` and a quote inside a quoted string. An unquoted value ending
//! with `*` is a prefix (`norm*`), and `*` alone means the field has a value (`published_at:*`).
//! A name can contain `-`, `'` and `"` but not start with them; a value can contain `:`
//! (`id:gid://shopify/Product/1`, `created_at:>2020-10-21T23:39:20Z`). `first_name: Bob`, with a
//! space, is the two terms `first_name` and `Bob`.
//!
//! Parentheses nest at most [`MAX_DEPTH`] deep: the parser recurses once per `(`, on the stack
//! of the request's thread. A chain of modifiers (`NOT NOT -x`) is counted, not recursed into.

use super::{Comparator, Node, SyntaxError, Term, Value, Warning};

/// How deeply parentheses may nest. A group deeper than this is not read: lenient parsing
/// ignores it with a warning, as Shopify ignores what it cannot read of a query, and strict
/// parsing (a webhook filter) refuses it. Real queries nest a few levels; this is the GraphQL
/// parser's limit too.
pub(super) const MAX_DEPTH: usize = 100;

/// A token of a query.
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Open,
    Close,
    And,
    Or,
    /// `NOT` or a `-` stuck to what follows.
    Not,
    Term(Term),
}

/// A character of a chunk, with how it was written.
#[derive(Clone, Copy, Debug)]
struct Char {
    c: char,
    /// After a backslash, or inside quotes: never special.
    literal: bool,
    quoted: bool,
}

/// Parses `query`. `strict` refuses what does not follow the grammar (an unbalanced
/// parenthesis, an unterminated quote, a connective or a modifier without a term, a field
/// without a value); otherwise the parser keeps what it can read, as Shopify answers a malformed
/// query with warnings rather than an error.
pub(super) fn parse(
    query: &str,
    strict: bool,
) -> Result<(Option<Node>, Vec<Warning>), SyntaxError> {
    let mut warnings = Vec::new();
    let tokens = tokenize(query, strict, &mut warnings)?;
    let mut parser = Parser {
        tokens,
        at: 0,
        depth: 0,
        strict,
        warnings,
    };
    let mut root = parser.and()?;
    while parser.at < parser.tokens.len() {
        // A `)` without its `(`.
        parser.problem("a `)` closes no parenthesis")?;
        parser.at += 1;
        let rest = parser.and()?;
        root = join(root, rest, Node::And);
    }
    Ok((root, parser.warnings))
}

fn tokenize(
    query: &str,
    strict: bool,
    warnings: &mut Vec<Warning>,
) -> Result<Vec<Token>, SyntaxError> {
    let chars: Vec<char> = query.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            tokens.push(Token::Open);
            i += 1;
        } else if c == ')' {
            tokens.push(Token::Close);
            i += 1;
        } else if c == '-' {
            // A `-` stuck to a term negates it; a name never starts with one.
            if chars
                .get(i + 1)
                .is_some_and(|next| !next.is_whitespace() && *next != ')')
            {
                tokens.push(Token::Not);
            } else if strict {
                return Err(SyntaxError::new("`-` applies to nothing", tokens.len()));
            } else {
                warnings.push(Warning::new(None, "`-` applies to nothing."));
            }
            i += 1;
        } else {
            let (chunk, next) = read_chunk(&chars, i, strict, warnings)?;
            i = next;
            let plain: String = chunk.iter().map(|ch| ch.c).collect();
            let special = chunk.iter().all(|ch| !ch.literal);
            match (plain.as_str(), special) {
                ("AND", true) => tokens.push(Token::And),
                ("OR", true) => tokens.push(Token::Or),
                ("NOT", true) => tokens.push(Token::Not),
                _ => tokens.push(term(&chunk, strict)?),
            }
        }
    }
    Ok(tokens)
}

/// The chunk starting at `start`: up to a blank or a parenthesis that are not escaped or
/// quoted. A quote opens a quoted string at the start of a value: at the start of the chunk or
/// right after a comparator.
fn read_chunk(
    chars: &[char],
    start: usize,
    strict: bool,
    warnings: &mut Vec<Warning>,
) -> Result<(Vec<Char>, usize), SyntaxError> {
    let mut chunk: Vec<Char> = Vec::new();
    let mut i = start;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || c == '(' || c == ')' {
            break;
        }
        if c == '\\' {
            if let Some(&next) = chars.get(i + 1) {
                chunk.push(Char {
                    c: next,
                    literal: true,
                    quoted: false,
                });
                i += 2;
            } else {
                chunk.push(Char {
                    c,
                    literal: true,
                    quoted: false,
                });
                i += 1;
            }
            continue;
        }
        let value_start = chunk.is_empty()
            || chunk
                .last()
                .is_some_and(|last| !last.literal && matches!(last.c, ':' | '<' | '>' | '='));
        if (c == '"' || c == '\'') && value_start {
            let quote = c;
            i += 1;
            let mut closed = false;
            let mut quoted = Vec::new();
            while i < chars.len() {
                match chars[i] {
                    '\\' if i + 1 < chars.len() => {
                        quoted.push(Char {
                            c: chars[i + 1],
                            literal: true,
                            quoted: true,
                        });
                        i += 2;
                    }
                    ch if ch == quote => {
                        closed = true;
                        i += 1;
                        break;
                    }
                    ch => {
                        quoted.push(Char {
                            c: ch,
                            literal: true,
                            quoted: true,
                        });
                        i += 1;
                    }
                }
            }
            if !closed {
                if strict {
                    return Err(SyntaxError::new("a quoted string is not closed", start));
                }
                warnings.push(Warning::new(None, "A quoted string is not closed."));
            }
            if quoted.is_empty() {
                // `""`: an empty phrase.
                chunk.push(Char {
                    c: '\0',
                    literal: true,
                    quoted: true,
                });
            }
            chunk.extend(quoted);
            continue;
        }
        chunk.push(Char {
            c,
            literal: false,
            quoted: false,
        });
        i += 1;
    }
    Ok((chunk, i))
}

/// The term a chunk is: `field:value`, `field:<value`, `value`, `field:*`, `prefix*`.
fn term(chunk: &[Char], strict: bool) -> Result<Token, SyntaxError> {
    let colon = chunk
        .iter()
        .position(|ch| ch.c == ':' && !ch.literal)
        .filter(|&at| at > 0 && !matches!(chunk[0].c, '-' | '\'' | '"'));
    let Some(colon) = colon else {
        return Ok(Token::Term(Term {
            field: None,
            comparator: Comparator::Eq,
            value: value(chunk),
        }));
    };
    let field: String = chunk[..colon].iter().map(|ch| ch.c).collect();
    let rest = &chunk[colon + 1..];
    let unquoted = |at: usize, c: char| rest.get(at).is_some_and(|ch| ch.c == c && !ch.literal);
    let (comparator, skip) = if unquoted(0, '<') && unquoted(1, '=') {
        (Comparator::Le, 2)
    } else if unquoted(0, '>') && unquoted(1, '=') {
        (Comparator::Ge, 2)
    } else if unquoted(0, '<') {
        (Comparator::Lt, 1)
    } else if unquoted(0, '>') {
        (Comparator::Gt, 1)
    } else {
        (Comparator::Eq, 0)
    };
    let rest = &rest[skip..];
    if rest.is_empty() {
        if strict {
            return Err(SyntaxError::new(
                format!("`{field}:` has no value: write the value right after the colon"),
                0,
            ));
        }
        // `first_name: Bob` is the terms `first_name` and `Bob`.
        return Ok(Token::Term(Term {
            field: None,
            comparator: Comparator::Eq,
            value: Value::Word(field),
        }));
    }
    Ok(Token::Term(Term {
        field: Some(field),
        comparator,
        value: value(rest),
    }))
}

/// The value of a chunk: quoted, a prefix, `*`, or a word.
fn value(chunk: &[Char]) -> Value {
    let text: String = chunk
        .iter()
        .filter(|ch| ch.c != '\0')
        .map(|ch| ch.c)
        .collect();
    if !chunk.is_empty() && chunk.iter().all(|ch| ch.quoted) {
        return Value::Phrase(text);
    }
    match chunk.last() {
        Some(last) if last.c == '*' && !last.literal => {
            let prefix: String = chunk[..chunk.len() - 1].iter().map(|ch| ch.c).collect();
            if prefix.is_empty() {
                Value::Any
            } else {
                Value::Prefix(prefix)
            }
        }
        _ => Value::Word(text),
    }
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
    /// The parentheses open around the token at `at`.
    depth: usize,
    strict: bool,
    warnings: Vec<Warning>,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    /// Something the grammar does not allow: an error when strict, a warning otherwise (once:
    /// the groups a query nested too deeply leaves unclosed say so in one warning).
    fn problem(&mut self, message: &str) -> Result<(), SyntaxError> {
        if self.strict {
            return Err(SyntaxError::new(message, self.at));
        }
        let mut sentence = message.to_string();
        if let Some(first) = sentence.get(..1) {
            sentence.replace_range(..1, &first.to_uppercase());
        }
        let warning = Warning::new(None, &format!("{sentence}."));
        if !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
        Ok(())
    }

    /// Terms and alternatives joined by `AND`, written or implied, up to a `)` or the end.
    fn and(&mut self) -> Result<Option<Node>, SyntaxError> {
        let mut node = None;
        loop {
            match self.peek() {
                None | Some(Token::Close) => return Ok(node),
                Some(Token::And) => {
                    self.at += 1;
                    if node.is_none()
                        || matches!(
                            self.peek(),
                            None | Some(Token::Close | Token::And | Token::Or)
                        )
                    {
                        self.problem("`AND` joins nothing")?;
                    }
                }
                Some(Token::Or) => {
                    self.at += 1;
                    self.problem("`OR` joins nothing")?;
                }
                _ => {
                    let next = self.or()?;
                    node = join(node, next, Node::And);
                }
            }
        }
    }

    /// Terms joined by `OR`.
    fn or(&mut self) -> Result<Option<Node>, SyntaxError> {
        let mut node = self.unary()?;
        while self.peek() == Some(&Token::Or) {
            self.at += 1;
            if matches!(
                self.peek(),
                None | Some(Token::Close | Token::And | Token::Or)
            ) {
                self.problem("`OR` joins nothing")?;
                break;
            }
            let next = self.unary()?;
            node = join(node, next, Node::Or);
        }
        Ok(node)
    }

    /// A term, with its modifiers: `NOT NOT x` is `x`.
    fn unary(&mut self) -> Result<Option<Node>, SyntaxError> {
        let mut negated = false;
        while self.peek() == Some(&Token::Not) {
            self.at += 1;
            negated = !negated;
            if matches!(
                self.peek(),
                None | Some(Token::Close | Token::And | Token::Or)
            ) {
                self.problem("`NOT` or `-` applies to nothing")?;
                return Ok(None);
            }
        }
        let node = match self.peek() {
            Some(Token::Open) => {
                self.at += 1;
                if self.depth >= MAX_DEPTH {
                    self.problem(&format!(
                        "parentheses nest more than {MAX_DEPTH} deep: what they hold is ignored"
                    ))?;
                    self.skip_group();
                    return Ok(None);
                }
                self.depth += 1;
                let inner = self.and();
                self.depth -= 1;
                let inner = inner?;
                if self.peek() == Some(&Token::Close) {
                    self.at += 1;
                } else {
                    self.problem("a `(` is not closed")?;
                }
                inner
            }
            Some(Token::Term(term)) => {
                let term = term.clone();
                self.at += 1;
                Some(Node::Term(term))
            }
            _ => None,
        };
        Ok(node.map(|node| match node {
            Node::Not(inner) if negated => *inner,
            node if negated => Node::Not(Box::new(node)),
            node => node,
        }))
    }

    /// Moves past the group whose `(` was just read: up to its `)`, or the end.
    fn skip_group(&mut self) {
        let mut open = 1usize;
        while let Some(token) = self.peek() {
            match token {
                Token::Open => open += 1,
                Token::Close => open -= 1,
                _ => {}
            }
            self.at += 1;
            if open == 0 {
                return;
            }
        }
    }
}

/// `left` and `right` joined into an `And` or an `Or`, flattened; either may be missing.
fn join(left: Option<Node>, right: Option<Node>, kind: fn(Vec<Node>) -> Node) -> Option<Node> {
    let (left, right) = match (left, right) {
        (None, node) | (node, None) => return node,
        (Some(left), Some(right)) => (left, right),
    };
    let is_and = matches!(kind(Vec::new()), Node::And(_));
    let mut items = Vec::new();
    for node in [left, right] {
        match node {
            Node::And(inner) if is_and => items.extend(inner),
            Node::Or(inner) if !is_and => items.extend(inner),
            node => items.push(node),
        }
    }
    Some(kind(items))
}
