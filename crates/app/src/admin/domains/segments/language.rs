//! The segment query language: the `WHERE` part of ShopifyQL that segments are written in
//! (`amount_spent > 100 AND customer_tags CONTAINS 'vip'`), parsed into a [`Query`] and checked
//! against the filters lsf knows ([`super::filters`]).
//!
//! What it reads, as shopify.dev's "Segment query language reference" (2026-10) and the Help
//! Center's "Shopify-based customer segment filters" write it:
//!
//! - a clause is `<attribute> <operator> <condition>`; clauses join with `AND` and `OR`, `AND`
//!   binding tighter, and parentheses group them;
//! - operators: `=`, `!=`, `>`, `>=`, `<`, `<=`, `BETWEEN … AND …`, `CONTAINS`, `NOT CONTAINS`,
//!   `IS NULL`, `IS NOT NULL`, and for functions `MATCHES (…)` and `NOT MATCHES (…)` with named
//!   parameters (`products_purchased MATCHES (id IN (1, 2), date >= -3m)`);
//! - conditions: quoted strings (`'vip'`, `\'` inside), integers and decimals, dates
//!   (`2020-05-30`), date-times in the shop's time zone (`2020-05-30T16:00:00`), offsets
//!   (`-7d`, `-2w`, `-1m`, `+1y`), `today`, `yesterday`, the deprecated `7_days_ago`,
//!   `30_days_ago`, `90_days_ago`, `12_months_ago`, `true`, `false`, and the coordinates of
//!   `customer_within_distance` (`(45.4, -75.6)`);
//! - `anniversary('metafields.facts.birth_date') = today`, a date filter on a metafield;
//! - at most 10 clauses ("Invalid query: Too many clauses (>10)").
//!
//! Errors are Shopify's form, `Query Line 1 Column 79: 'MATCHES' is unexpected.` (an answer of
//! `segmentCreate` quoted on community.shopify.dev): the line from 1, the column from 0 at the
//! start of the clause at fault, as both quoted answers point. That one message is Shopify's;
//! the others (an unknown filter, a value of the wrong type, an operator a filter does not take,
//! too many clauses) are lsf's words in the same form: Shopify does not publish them.

use std::fmt;

use super::filters::{self, Filter, Kind, ParamKind};

/// The most clauses a segment query has: shopify.dev's reference shows a query of 11 as invalid.
const MAX_CLAUSES: usize = 10;

/// How deep parentheses nest: Shopify publishes no bound; lsf's is the search syntax's
/// (`search::syntax::MAX_DEPTH`), as the parser recurses once per `(` on the request's stack,
/// which 400,000 of them overflowed.
pub const MAX_DEPTH: usize = 100;

/// The furthest a date offset (`-7d`, `+1y`) reaches, in years either way. Shopify publishes no
/// bound; lsf's keeps every date it computes within what a date holds: `-99999999999d` is not
/// a valid value.
pub const MAX_OFFSET_YEARS: i64 = 10_000;

/// Whether `n` units from today stay within [`MAX_OFFSET_YEARS`].
fn offset_in_range(n: i64, unit: Unit) -> bool {
    let most = match unit {
        Unit::Day => MAX_OFFSET_YEARS * 366,
        Unit::Week => MAX_OFFSET_YEARS * 53,
        Unit::Month => MAX_OFFSET_YEARS * 12,
        Unit::Year => MAX_OFFSET_YEARS,
    };
    n.unsigned_abs() <= most.unsigned_abs()
}

/// A query, parsed and checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub root: Expr,
}

/// A query's tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Clause(Clause),
}

/// One clause: a filter and what its values must be.
#[derive(Clone, Debug, PartialEq)]
pub struct Clause {
    /// The filter's query name: `amount_spent`, `shopify_email.opened`, `anniversary`.
    pub attribute: String,
    /// The argument of `anniversary('metafields.facts.birth_date')`.
    pub argument: Option<String>,
    pub test: Test,
}

/// What a clause asks of a filter's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Test {
    Compare(Op, Value),
    Between(Value, Value),
    /// `CONTAINS` (`false`) or `NOT CONTAINS` (`true`).
    Contains(bool, Value),
    /// `IS NULL` (`false`) or `IS NOT NULL` (`true`).
    Null(bool),
    /// `MATCHES` (`false`) or `NOT MATCHES` (`true`), with the function's parameters.
    Matches(bool, Vec<Param>),
}

/// A named parameter of a function: `date >= -3m`, `id IN (1, 2)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub test: ParamTest,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParamTest {
    Compare(Op, Value),
    Between(Value, Value),
    /// `IN (…)` (`false`) or `NOT IN (…)` (`true`).
    In(bool, Vec<Value>),
    /// `CONTAINS` (`false`) or `NOT CONTAINS` (`true`): `tag CONTAINS 'jeans'`.
    Contains(bool, Value),
}

/// A comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Op {
    fn text(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Lt => "<",
            Op::Le => "<=",
        }
    }

    /// Whether `ordering` (of a value against the condition) satisfies the comparison.
    pub fn holds(self, ordering: std::cmp::Ordering) -> bool {
        use std::cmp::Ordering::*;
        match self {
            Op::Eq => ordering == Equal,
            Op::Ne => ordering != Equal,
            Op::Gt => ordering == Greater,
            Op::Ge => ordering != Less,
            Op::Lt => ordering == Less,
            Op::Le => ordering != Greater,
        }
    }
}

/// A condition's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    /// A number as written (`99`, `10.99`, `-5`), so that an integer filter can refuse a decimal.
    Number(String),
    Date(DateValue),
    Bool(bool),
    /// `(latitude, longitude)`.
    Coordinates(f64, f64),
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(text) => text.parse().ok(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Number(text) => text.parse().ok(),
            _ => None,
        }
    }

    fn written(&self) -> String {
        match self {
            Value::Text(text) => text.clone(),
            Value::Number(text) => text.clone(),
            Value::Date(date) => date.to_string(),
            Value::Bool(value) => value.to_string(),
            Value::Coordinates(lat, lng) => format!("({lat}, {lng})"),
        }
    }
}

/// A date as a condition gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DateValue {
    Day(chrono::NaiveDate),
    /// A date and a time, in the shop's time zone.
    Time(chrono::NaiveDateTime),
    /// `-7d`: a number of days, weeks, months or years from today.
    Offset(i64, Unit),
    Today,
    Yesterday,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Day,
    Week,
    Month,
    Year,
}

impl fmt::Display for DateValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DateValue::Day(day) => write!(f, "{day}"),
            DateValue::Time(time) => write!(f, "{}", time.format("%Y-%m-%dT%H:%M:%S")),
            DateValue::Offset(n, unit) => {
                let unit = match unit {
                    Unit::Day => 'd',
                    Unit::Week => 'w',
                    Unit::Month => 'm',
                    Unit::Year => 'y',
                };
                write!(f, "{n:+}{unit}")
            }
            DateValue::Today => write!(f, "today"),
            DateValue::Yesterday => write!(f, "yesterday"),
        }
    }
}

/// What is wrong with a query: its message names the line and the column, as Shopify's does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub column: usize,
    pub problem: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Query Line {} Column {}: {}",
            self.line, self.column, self.problem
        )
    }
}

impl std::error::Error for Error {}

/// Parses and checks a segment query.
pub fn parse(text: &str) -> Result<Query, Error> {
    let tokens = lex(text)?;
    let mut parser = Parser {
        tokens,
        position: 0,
        clauses: 0,
        depth: 0,
        end: end_position(text),
    };
    if parser.peek().is_none() {
        return Err(Error {
            line: 1,
            column: 0,
            // lsf's words: an empty query defines no segment.
            problem: "the query is empty.".into(),
        });
    }
    let root = parser.or()?;
    if let Some(token) = parser.peek() {
        return Err(token.unexpected(token.at));
    }
    Ok(Query { root })
}

/// Where the end of the text is: after its last character.
fn end_position(text: &str) -> At {
    let line = text.lines().count().max(1);
    let column = text.lines().last().map_or(0, |last| last.chars().count());
    At { line, column }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct At {
    line: usize,
    column: usize,
}

#[derive(Clone, Debug, PartialEq)]
enum Kw {
    And,
    Or,
    Not,
    Between,
    Contains,
    Matches,
    Is,
    Null,
    In,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Keyword(Kw, String),
    Value(Value),
    Op(Op),
    Open,
    Close,
    Comma,
}

#[derive(Clone, Debug, PartialEq)]
struct Token {
    tok: Tok,
    /// The text as written: what an error quotes.
    text: String,
    at: At,
}

impl Token {
    fn unexpected(&self, clause: At) -> Error {
        Error {
            line: clause.line,
            column: clause.column,
            problem: format!("'{}' is unexpected.", self.text),
        }
    }
}

fn lex(text: &str) -> Result<Vec<Token>, Error> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let (mut line, mut column) = (1usize, 0usize);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let at = At { line, column };
        let start = i;
        let advance = |i: &mut usize, column: &mut usize, n: usize| {
            *i += n;
            *column += n;
        };
        if c == '\n' {
            i += 1;
            line += 1;
            column = 0;
            continue;
        }
        if c.is_whitespace() {
            advance(&mut i, &mut column, 1);
            continue;
        }
        let tok = match c {
            '(' => {
                advance(&mut i, &mut column, 1);
                Tok::Open
            }
            ')' => {
                advance(&mut i, &mut column, 1);
                Tok::Close
            }
            ',' => {
                advance(&mut i, &mut column, 1);
                Tok::Comma
            }
            '=' => {
                advance(&mut i, &mut column, 1);
                Tok::Op(Op::Eq)
            }
            '!' if chars.get(i + 1) == Some(&'=') => {
                advance(&mut i, &mut column, 2);
                Tok::Op(Op::Ne)
            }
            '<' | '>' => {
                let or_equal = chars.get(i + 1) == Some(&'=');
                advance(&mut i, &mut column, if or_equal { 2 } else { 1 });
                Tok::Op(match (c, or_equal) {
                    ('<', false) => Op::Lt,
                    ('<', true) => Op::Le,
                    ('>', false) => Op::Gt,
                    _ => Op::Ge,
                })
            }
            '\'' => {
                let mut value = String::new();
                let mut j = i + 1;
                let mut closed = false;
                while j < chars.len() {
                    match chars[j] {
                        '\\' if j + 1 < chars.len() => {
                            value.push(chars[j + 1]);
                            j += 2;
                        }
                        '\'' => {
                            closed = true;
                            j += 1;
                            break;
                        }
                        '\n' => break,
                        other => {
                            value.push(other);
                            j += 1;
                        }
                    }
                }
                if !closed {
                    return Err(Error {
                        line: at.line,
                        column: at.column,
                        problem: "the string is not closed with '.".into(),
                    });
                }
                let n = j - i;
                advance(&mut i, &mut column, n);
                Tok::Value(Value::Text(value))
            }
            '-' | '+' | '0'..='9' => {
                let mut j = i;
                if c == '-' || c == '+' {
                    j += 1;
                }
                let digits_from = j;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j == digits_from {
                    // A sign alone.
                    advance(&mut i, &mut column, 1);
                    tokens.push(Token {
                        tok: Tok::Ident(c.to_string()),
                        text: c.to_string(),
                        at,
                    });
                    continue;
                }
                let word_end = (j..chars.len())
                    .find(|&k| {
                        !(chars[k].is_ascii_alphanumeric()
                            || matches!(chars[k], '_' | '.' | '-' | ':'))
                    })
                    .unwrap_or(chars.len());
                let word: String = chars[i..word_end].iter().collect();
                let value = number_or_date(&word).ok_or_else(|| Error {
                    line: at.line,
                    column: at.column,
                    problem: format!("'{word}' is not a valid value."),
                })?;
                let n = word_end - i;
                advance(&mut i, &mut column, n);
                Tok::Value(value)
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut j = i;
                while j < chars.len()
                    && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '.')
                {
                    j += 1;
                }
                let word: String = chars[i..j].iter().collect();
                let n = j - i;
                advance(&mut i, &mut column, n);
                match word.to_ascii_uppercase().as_str() {
                    "AND" => Tok::Keyword(Kw::And, word.clone()),
                    "OR" => Tok::Keyword(Kw::Or, word.clone()),
                    "NOT" => Tok::Keyword(Kw::Not, word.clone()),
                    "BETWEEN" => Tok::Keyword(Kw::Between, word.clone()),
                    "CONTAINS" => Tok::Keyword(Kw::Contains, word.clone()),
                    "MATCHES" => Tok::Keyword(Kw::Matches, word.clone()),
                    "IS" => Tok::Keyword(Kw::Is, word.clone()),
                    "NULL" => Tok::Keyword(Kw::Null, word.clone()),
                    "IN" => Tok::Keyword(Kw::In, word.clone()),
                    "TRUE" => Tok::Value(Value::Bool(true)),
                    "FALSE" => Tok::Value(Value::Bool(false)),
                    "TODAY" => Tok::Value(Value::Date(DateValue::Today)),
                    "YESTERDAY" => Tok::Value(Value::Date(DateValue::Yesterday)),
                    _ => Tok::Ident(word.clone()),
                }
            }
            other => {
                return Err(Error {
                    line: at.line,
                    column: at.column,
                    problem: format!("'{other}' is unexpected."),
                });
            }
        };
        let written: String = chars[start..i].iter().collect();
        tokens.push(Token {
            tok,
            text: written,
            at,
        });
    }
    Ok(tokens)
}

/// A number, a date, a date-time, an offset (`-7d`) or a deprecated named date
/// (`30_days_ago`).
fn number_or_date(word: &str) -> Option<Value> {
    let deprecated = match word {
        "7_days_ago" => Some(DateValue::Offset(-7, Unit::Day)),
        "30_days_ago" => Some(DateValue::Offset(-30, Unit::Day)),
        "90_days_ago" => Some(DateValue::Offset(-90, Unit::Day)),
        "12_months_ago" => Some(DateValue::Offset(-12, Unit::Month)),
        _ => None,
    };
    if let Some(date) = deprecated {
        return Some(Value::Date(date));
    }
    if let Some(last) = word.chars().last()
        && matches!(last, 'd' | 'w' | 'm' | 'y')
    {
        let number = &word[..word.len() - 1];
        let signed = number.starts_with(['-', '+']);
        if signed && let Ok(n) = number.parse::<i64>() {
            let unit = match last {
                'd' => Unit::Day,
                'w' => Unit::Week,
                'm' => Unit::Month,
                _ => Unit::Year,
            };
            return offset_in_range(n, unit).then_some(Value::Date(DateValue::Offset(n, unit)));
        }
        return None;
    }
    if let Ok(day) = chrono::NaiveDate::parse_from_str(word, "%Y-%m-%d")
        && word.len() == 10
    {
        return Some(Value::Date(DateValue::Day(day)));
    }
    if word.len() == 19
        && let Ok(time) = chrono::NaiveDateTime::parse_from_str(word, "%Y-%m-%dT%H:%M:%S")
    {
        return Some(Value::Date(DateValue::Time(time)));
    }
    let digits = word.trim_start_matches(['-', '+']);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let fraction_ok = !digits.contains('.') || !fraction.is_empty();
    (!whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && fraction_ok)
        .then(|| Value::Number(word.trim_start_matches('+').to_string()))
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
    clauses: usize,
    /// How many parentheses are open.
    depth: usize,
    end: At,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.position).cloned();
        self.position += 1;
        token
    }

    fn keyword(&self, kw: Kw) -> bool {
        matches!(self.peek(), Some(Token { tok: Tok::Keyword(k, _), .. }) if *k == kw)
    }

    /// The error of a query that ends where something more was needed. lsf's words.
    fn ended(&self, clause: At) -> Error {
        let _ = self.end;
        Error {
            line: clause.line,
            column: clause.column,
            problem: "the query ends before the clause does.".into(),
        }
    }

    fn expect(&mut self, wanted: &Tok, clause: At) -> Result<(), Error> {
        match self.next() {
            Some(token) if token.tok == *wanted => Ok(()),
            Some(token) => Err(token.unexpected(clause)),
            None => Err(self.ended(clause)),
        }
    }

    fn or(&mut self) -> Result<Expr, Error> {
        let mut parts = vec![self.and()?];
        while self.keyword(Kw::Or) {
            self.next();
            parts.push(self.and()?);
        }
        Ok(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Expr::Or(parts)
        })
    }

    fn and(&mut self) -> Result<Expr, Error> {
        let mut parts = vec![self.primary()?];
        while self.keyword(Kw::And) {
            self.next();
            parts.push(self.primary()?);
        }
        Ok(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Expr::And(parts)
        })
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        match self.peek().cloned() {
            Some(Token {
                tok: Tok::Open, at, ..
            }) => {
                self.next();
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err(Error {
                        line: at.line,
                        column: at.column,
                        // lsf's words: Shopify publishes no bound.
                        problem: format!("parentheses nest more than {MAX_DEPTH} deep."),
                    });
                }
                let inner = self.or()?;
                self.expect(&Tok::Close, at)?;
                self.depth -= 1;
                Ok(inner)
            }
            Some(Token {
                tok: Tok::Ident(_), ..
            }) => self.clause().map(Expr::Clause),
            Some(token) => Err(token.unexpected(token.at)),
            None => Err(self.ended(self.end)),
        }
    }

    fn clause(&mut self) -> Result<Clause, Error> {
        let Some(Token {
            tok: Tok::Ident(name),
            at,
            ..
        }) = self.next()
        else {
            return Err(self.ended(self.end));
        };
        self.clauses += 1;
        if self.clauses > MAX_CLAUSES {
            return Err(Error {
                line: at.line,
                column: at.column,
                // lsf's words for the "Too many clauses (>10)" of shopify.dev's reference.
                problem: format!("too many clauses: a segment query has {MAX_CLAUSES} at most."),
            });
        }
        let Some(filter) = filters::find(&name) else {
            return Err(Error {
                line: at.line,
                column: at.column,
                problem: format!("'{name}' filter cannot be found."),
            });
        };
        let mut argument = None;
        if filter.kind == Kind::Anniversary {
            self.expect(&Tok::Open, at)?;
            match self.next() {
                Some(Token {
                    tok: Tok::Value(Value::Text(path)),
                    ..
                }) => argument = Some(path),
                Some(token) => return Err(token.unexpected(at)),
                None => return Err(self.ended(at)),
            }
            self.expect(&Tok::Close, at)?;
        }
        let test = self.test(filter, at)?;
        check(filter, &test, at)?;
        Ok(Clause {
            attribute: filter.query_name.to_string(),
            argument,
            test,
        })
    }

    fn value(&mut self, clause: At) -> Result<Value, Error> {
        match self.next() {
            Some(Token {
                tok: Tok::Value(value),
                ..
            }) => Ok(value),
            Some(token) => Err(token.unexpected(clause)),
            None => Err(self.ended(clause)),
        }
    }

    fn test(&mut self, filter: &Filter, at: At) -> Result<Test, Error> {
        let Some(token) = self.next() else {
            return Err(self.ended(at));
        };
        match token.tok {
            Tok::Op(op) => Ok(Test::Compare(op, self.value(at)?)),
            Tok::Keyword(Kw::Between, _) => {
                let from = self.value(at)?;
                if !self.keyword(Kw::And) {
                    return match self.next() {
                        Some(token) => Err(token.unexpected(at)),
                        None => Err(self.ended(at)),
                    };
                }
                self.next();
                Ok(Test::Between(from, self.value(at)?))
            }
            Tok::Keyword(Kw::Contains, _) => Ok(Test::Contains(false, self.value(at)?)),
            // `MATCHES` after a filter that is not a function is Shopify's quoted error.
            Tok::Keyword(Kw::Matches, _) if filter.kind != Kind::Function => {
                Err(token.unexpected(at))
            }
            Tok::Keyword(Kw::Matches, _) => Ok(Test::Matches(false, self.params(filter, at)?)),
            Tok::Keyword(Kw::Not, _) => match self.next() {
                Some(Token {
                    tok: Tok::Keyword(Kw::Contains, _),
                    ..
                }) => Ok(Test::Contains(true, self.value(at)?)),
                Some(
                    token @ Token {
                        tok: Tok::Keyword(Kw::Matches, _),
                        ..
                    },
                ) if filter.kind != Kind::Function => Err(token.unexpected(at)),
                Some(Token {
                    tok: Tok::Keyword(Kw::Matches, _),
                    ..
                }) => Ok(Test::Matches(true, self.params(filter, at)?)),
                Some(token) => Err(token.unexpected(at)),
                None => Err(self.ended(at)),
            },
            Tok::Keyword(Kw::Is, _) => {
                let negated = self.keyword(Kw::Not);
                if negated {
                    self.next();
                }
                match self.next() {
                    Some(Token {
                        tok: Tok::Keyword(Kw::Null, _),
                        ..
                    }) => Ok(Test::Null(negated)),
                    Some(token) => Err(token.unexpected(at)),
                    None => Err(self.ended(at)),
                }
            }
            _ => Err(token.unexpected(at)),
        }
    }

    /// The parameters of `MATCHES (…)`.
    fn params(&mut self, filter: &Filter, at: At) -> Result<Vec<Param>, Error> {
        self.expect(&Tok::Open, at)?;
        let mut params = Vec::new();
        if matches!(
            self.peek(),
            Some(Token {
                tok: Tok::Close,
                ..
            })
        ) {
            self.next();
            return Ok(params);
        }
        loop {
            let name = match self.next() {
                Some(Token {
                    tok: Tok::Ident(name),
                    ..
                }) => name,
                Some(token) => return Err(token.unexpected(at)),
                None => return Err(self.ended(at)),
            };
            let Some(definition) = filter.param(&name) else {
                return Err(Error {
                    line: at.line,
                    column: at.column,
                    problem: format!("'{name}' is not a parameter of '{}'.", filter.query_name),
                });
            };
            let test = match self.next() {
                Some(Token {
                    tok: Tok::Op(op), ..
                }) => {
                    // `coordinates = (45.4, -75.6)`.
                    if definition.kind == ParamKind::Coordinates {
                        ParamTest::Compare(op, self.coordinates(at)?)
                    } else {
                        ParamTest::Compare(op, self.value(at)?)
                    }
                }
                Some(Token {
                    tok: Tok::Keyword(Kw::Between, _),
                    ..
                }) => {
                    let from = self.value(at)?;
                    if !self.keyword(Kw::And) {
                        return match self.next() {
                            Some(token) => Err(token.unexpected(at)),
                            None => Err(self.ended(at)),
                        };
                    }
                    self.next();
                    ParamTest::Between(from, self.value(at)?)
                }
                Some(Token {
                    tok: Tok::Keyword(Kw::In, _),
                    ..
                }) => ParamTest::In(false, self.list(at)?),
                Some(Token {
                    tok: Tok::Keyword(Kw::Contains, _),
                    ..
                }) => ParamTest::Contains(false, self.value(at)?),
                Some(Token {
                    tok: Tok::Keyword(Kw::Not, _),
                    ..
                }) => match self.next() {
                    Some(Token {
                        tok: Tok::Keyword(Kw::In, _),
                        ..
                    }) => ParamTest::In(true, self.list(at)?),
                    Some(Token {
                        tok: Tok::Keyword(Kw::Contains, _),
                        ..
                    }) => ParamTest::Contains(true, self.value(at)?),
                    Some(token) => return Err(token.unexpected(at)),
                    None => return Err(self.ended(at)),
                },
                Some(token) => return Err(token.unexpected(at)),
                None => return Err(self.ended(at)),
            };
            check_param(filter, definition, &test, at)?;
            params.push(Param { name, test });
            match self.next() {
                Some(Token {
                    tok: Tok::Comma, ..
                }) => continue,
                Some(Token {
                    tok: Tok::Close, ..
                }) => break,
                Some(token) => return Err(token.unexpected(at)),
                None => return Err(self.ended(at)),
            }
        }
        Ok(params)
    }

    /// `(1, 2, 3)`, the values of `IN`. At most 500, as shopify.dev says of IDs.
    fn list(&mut self, at: At) -> Result<Vec<Value>, Error> {
        self.expect(&Tok::Open, at)?;
        let mut values = vec![self.value(at)?];
        loop {
            match self.next() {
                Some(Token {
                    tok: Tok::Comma, ..
                }) => values.push(self.value(at)?),
                Some(Token {
                    tok: Tok::Close, ..
                }) => break,
                Some(token) => return Err(token.unexpected(at)),
                None => return Err(self.ended(at)),
            }
        }
        if values.len() > 500 {
            return Err(Error {
                line: at.line,
                column: at.column,
                // lsf's words for shopify.dev's "A list can contain up to 500 IDs".
                problem: "a list holds 500 values at most.".into(),
            });
        }
        Ok(values)
    }

    /// `(latitude, longitude)`.
    fn coordinates(&mut self, at: At) -> Result<Value, Error> {
        self.expect(&Tok::Open, at)?;
        let latitude = self.value(at)?;
        self.expect(&Tok::Comma, at)?;
        let longitude = self.value(at)?;
        self.expect(&Tok::Close, at)?;
        match (latitude.as_f64(), longitude.as_f64()) {
            (Some(lat), Some(lng)) => Ok(Value::Coordinates(lat, lng)),
            _ => Err(Error {
                line: at.line,
                column: at.column,
                problem: "coordinates are two numbers: (latitude, longitude).".into(),
            }),
        }
    }
}

/// The error of a value a filter does not take. lsf's words.
fn bad_value(value: &Value, name: &str, at: At) -> Error {
    Error {
        line: at.line,
        column: at.column,
        problem: format!("'{}' is not a valid value for '{name}'.", value.written()),
    }
}

/// The error of an operator a filter does not take. lsf's words.
fn bad_operator(operator: &str, name: &str, at: At) -> Error {
    Error {
        line: at.line,
        column: at.column,
        problem: format!("'{operator}' is not an operator of '{name}'."),
    }
}

fn test_operator(test: &Test) -> String {
    match test {
        Test::Compare(op, _) => op.text().to_string(),
        Test::Between(..) => "BETWEEN".into(),
        Test::Contains(false, _) => "CONTAINS".into(),
        Test::Contains(true, _) => "NOT CONTAINS".into(),
        Test::Null(false) => "IS NULL".into(),
        Test::Null(true) => "IS NOT NULL".into(),
        Test::Matches(false, _) => "MATCHES".into(),
        Test::Matches(true, _) => "NOT MATCHES".into(),
    }
}

/// Whether a value is one of a kind: a date, a number (an integer for an integer filter), text.
fn value_fits(kind: &Kind, value: &Value) -> bool {
    match kind {
        Kind::Date | Kind::Anniversary => matches!(value, Value::Date(_)),
        Kind::Float => value.as_f64().is_some(),
        Kind::Integer => value.as_i64().is_some(),
        Kind::Enum(values) => match value {
            Value::Text(text) => values.is_empty() || values.contains(&text.as_str()),
            _ => false,
        },
        Kind::Text | Kind::TextList | Kind::EnumList => matches!(value, Value::Text(_)),
        Kind::IdList => value.as_i64().is_some_and(|id| id >= 0),
        Kind::Boolean => matches!(value, Value::Bool(_)),
        Kind::Function => false,
    }
}

/// Checks a clause against its filter: the operators its type takes, the type of its values.
fn check(filter: &Filter, test: &Test, at: At) -> Result<(), Error> {
    let name = filter.query_name;
    let operator_ok = match (&filter.kind, test) {
        (Kind::Function, Test::Matches(..)) => true,
        (Kind::Function, _) => false,
        (_, Test::Matches(..)) => false,
        (_, Test::Null(_)) => filter.nullable,
        (Kind::TextList | Kind::EnumList | Kind::IdList, Test::Contains(..)) => true,
        (Kind::TextList | Kind::EnumList | Kind::IdList, _) => false,
        (_, Test::Contains(..)) => false,
        (Kind::Date | Kind::Float | Kind::Integer | Kind::Anniversary, _) => true,
        (Kind::Enum(_) | Kind::Text | Kind::Boolean, Test::Compare(Op::Eq | Op::Ne, _)) => true,
        (Kind::Enum(_) | Kind::Text | Kind::Boolean, _) => false,
    };
    if !operator_ok {
        return Err(bad_operator(&test_operator(test), name, at));
    }
    let values: Vec<&Value> = match test {
        Test::Compare(_, value) | Test::Contains(_, value) => vec![value],
        Test::Between(from, to) => vec![from, to],
        Test::Null(_) | Test::Matches(..) => Vec::new(),
    };
    for value in values {
        if !value_fits(&filter.kind, value) {
            return Err(bad_value(value, name, at));
        }
    }
    Ok(())
}

/// Checks a parameter of a function against its definition.
fn check_param(
    filter: &Filter,
    definition: &filters::ParamDef,
    test: &ParamTest,
    at: At,
) -> Result<(), Error> {
    let name = format!("{}.{}", filter.query_name, definition.name);
    let fits = |value: &Value| match definition.kind {
        ParamKind::Id => value.as_i64().is_some_and(|id| id >= 0),
        ParamKind::Text => matches!(value, Value::Text(_)),
        ParamKind::Date => matches!(value, Value::Date(_)),
        ParamKind::Number => value.as_f64().is_some(),
        ParamKind::Coordinates => matches!(value, Value::Coordinates(..)),
    };
    let operator_ok = matches!(
        (definition.kind, test),
        (
            ParamKind::Id,
            ParamTest::Compare(Op::Eq | Op::Ne, _) | ParamTest::In(..)
        ) | (
            ParamKind::Text,
            ParamTest::Compare(Op::Eq | Op::Ne, _) | ParamTest::Contains(..)
        ) | (
            ParamKind::Date | ParamKind::Number,
            ParamTest::Compare(..) | ParamTest::Between(..)
        ) | (ParamKind::Coordinates, ParamTest::Compare(Op::Eq, _))
    );
    if !operator_ok {
        let operator = match test {
            ParamTest::Compare(op, _) => op.text().to_string(),
            ParamTest::Between(..) => "BETWEEN".into(),
            ParamTest::In(false, _) => "IN".into(),
            ParamTest::In(true, _) => "NOT IN".into(),
            ParamTest::Contains(false, _) => "CONTAINS".into(),
            ParamTest::Contains(true, _) => "NOT CONTAINS".into(),
        };
        return Err(bad_operator(&operator, &name, at));
    }
    let values: Vec<&Value> = match test {
        ParamTest::Compare(_, value) | ParamTest::Contains(_, value) => vec![value],
        ParamTest::Between(from, to) => vec![from, to],
        ParamTest::In(_, values) => values.iter().collect(),
    };
    for value in values {
        if !fits(value) {
            return Err(bad_value(value, &name, at));
        }
    }
    Ok(())
}

impl Expr {
    /// The clauses of the tree, in order.
    pub fn clauses(&self) -> Vec<&Clause> {
        match self {
            Expr::Clause(clause) => vec![clause],
            Expr::And(parts) | Expr::Or(parts) => parts.iter().flat_map(Expr::clauses).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clause(query: &str) -> Clause {
        match parse(query).unwrap().root {
            Expr::Clause(clause) => clause,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parentheses_nest_a_hundred_deep_at_most() {
        let nested = |depth: usize| {
            format!(
                "{}number_of_orders = 1{}",
                "(".repeat(depth),
                ")".repeat(depth)
            )
        };
        assert!(parse(&nested(MAX_DEPTH)).is_ok());
        assert_eq!(
            parse(&nested(MAX_DEPTH + 1)).unwrap_err().to_string(),
            "Query Line 1 Column 100: parentheses nest more than 100 deep."
        );
        // Deep enough to overflow the stack of a request's thread before the bound.
        let error = parse(&"(".repeat(400_000)).unwrap_err();
        assert_eq!(error.problem, "parentheses nest more than 100 deep.");
    }

    #[test]
    fn reads_the_examples_of_the_reference() {
        for query in [
            "email_subscription_status = 'SUBSCRIBED'",
            "last_order_date = 2020-05-30",
            "last_order_date > -7d",
            "last_order_date >= -2w",
            "last_order_date = -1m",
            "last_order_date < -1y",
            "last_order_date BETWEEN 2021-01-01 AND today",
            "last_order_date BETWEEN 2020-01-01 AND -7d",
            "last_order_date > 2020-05-30T16:00:00",
            "last_order_date = yesterday",
            "last_order_date >= 30_days_ago",
            "customer_account_status != 'DISABLED'",
            "amount_spent BETWEEN 100 AND 1000.99",
            "products_purchased NOT MATCHES ()",
            "products_purchased MATCHES (id = 2012162031638)",
            "products_purchased MATCHES (tag = 'Sports wear')",
            "products_purchased MATCHES (id IN (2012162031638, 1012132033639))",
            "products_purchased MATCHES (id NOT IN (1012132033639))",
            "products_purchased MATCHES (id = 1012132033639, date BETWEEN -12m AND today)",
            "shopify_email.opened MATCHES (activity_id = 5240029206)",
            "shopify_email.bounced NOT MATCHES (activity_id = 5240029206, date BETWEEN -12m AND today)",
            "number_of_orders BETWEEN 100 AND 1000",
            "customer_tags CONTAINS 'vip'",
            "customer_cities NOT CONTAINS 'US-CA-LosAngeles'",
            "customer_email_domain = ''",
            "email_subscription_status = 'SUBSCRIBED' AND customer_countries CONTAINS 'CA' OR amount_spent > 999.99",
            "email_subscription_status = 'SUBSCRIBED' AND (customer_countries CONTAINS 'CA' OR amount_spent > 999.99)",
            "companies IS NOT NULL",
            "companies CONTAINS 3778915041302",
            "orders_placed MATCHES (count >= 3, date >= -6m)",
            "orders_placed MATCHES (location_id IN (1122334455,6677889900))",
            "customer_within_distance MATCHES (coordinates = (45.419190, -75.696727), distance_mi = 10 )",
            "anniversary ('metafields.facts.birth_date') BETWEEN today AND +30d",
            "store_credit_accounts MATCHES (next_expiry_date <= +7d)",
            "storefront_event.product_viewed MATCHES (id = 2012162031638, date >= -30d)",
            "storefront.product_viewed MATCHES (tag CONTAINS 'jeans')",
        ] {
            parse(query).unwrap_or_else(|error| panic!("{query}: {error}"));
        }
    }

    #[test]
    fn and_binds_tighter_than_or() {
        let query = parse(
            "email_subscription_status = 'SUBSCRIBED' AND customer_countries CONTAINS 'CA' OR amount_spent > 999.99",
        )
        .unwrap();
        let Expr::Or(parts) = query.root else {
            panic!()
        };
        assert!(matches!(parts[0], Expr::And(_)));
        assert!(matches!(parts[1], Expr::Clause(_)));
    }

    #[test]
    fn values_keep_what_they_are() {
        assert_eq!(
            clause("customer_tags CONTAINS '\\'VIP\\''").test,
            Test::Contains(false, Value::Text("'VIP'".into()))
        );
        assert_eq!(
            clause("last_order_date > -7d").test,
            Test::Compare(Op::Gt, Value::Date(DateValue::Offset(-7, Unit::Day)))
        );
        let anniversary = clause("anniversary('metafields.facts.birth_date') = today");
        assert_eq!(
            anniversary.argument.as_deref(),
            Some("metafields.facts.birth_date")
        );
    }

    #[test]
    fn errors_name_the_line_and_the_column_of_the_clause() {
        let error = |query: &str| parse(query).unwrap_err().to_string();
        // The form of an answer of Shopify quoted on community.shopify.dev.
        assert_eq!(
            error("number_of_orders = 1 AND amount_spent MATCHES (id = 1)"),
            "Query Line 1 Column 25: 'MATCHES' is unexpected."
        );
        assert_eq!(
            error("number_of_orders = 1 AND amount_spent ? 3"),
            "Query Line 1 Column 38: '?' is unexpected."
        );
        assert_eq!(
            error("total_spent > 10"),
            "Query Line 1 Column 0: 'total_spent' filter cannot be found."
        );
        assert_eq!(
            error("number_of_orders = 1.5"),
            "Query Line 1 Column 0: '1.5' is not a valid value for 'number_of_orders'."
        );
        assert_eq!(
            error("email_subscription_status = 'subscribed'"),
            "Query Line 1 Column 0: 'subscribed' is not a valid value for 'email_subscription_status'."
        );
        assert_eq!(
            error("customer_tags = 'vip'"),
            "Query Line 1 Column 0: '=' is not an operator of 'customer_tags'."
        );
        assert_eq!(
            error("amount_spent >\n  AND number_of_orders = 1"),
            "Query Line 1 Column 0: 'AND' is unexpected."
        );
        assert_eq!(
            error("number_of_orders = 1\nAND customer_tags CONTAINS"),
            "Query Line 2 Column 4: the query ends before the clause does."
        );
        let eleven = ["customer_countries CONTAINS 'CA'"; 11].join(" OR ");
        assert!(
            error(&eleven).contains("too many clauses"),
            "{}",
            error(&eleven)
        );
        assert!(error("").contains("empty"));
        assert!(error("customer_tags CONTAINS 'vip").contains("not closed"));
    }
}
