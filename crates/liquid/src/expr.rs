//! Expressions: literals, variable lookups and ranges.

use crate::context::Context;
use crate::error::Result;
use crate::lax;
use crate::number::{string_to_i, to_integer};
use crate::value::Value;

#[derive(Clone, Debug)]
pub enum Expr {
    Literal(Value),
    Lookup(VariableLookup),
    Range(Box<Expr>, Box<Expr>),
    /// The `blank` keyword in a condition.
    Blank,
    /// The `empty` keyword in a condition.
    Empty,
}

#[derive(Clone, Debug)]
pub struct VariableLookup {
    pub name: LookupName,
    pub lookups: Vec<Lookup>,
}

#[derive(Clone, Debug)]
pub enum LookupName {
    /// The markup contained nothing that looks like a variable.
    Missing,
    Static(String),
    /// `[expr].foo`: the name of the variable is itself computed.
    Dynamic(Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum Lookup {
    /// `.name`. `command` is set for `size`, `first` and `last`, which fall back to a method call
    /// when the object has no such key.
    Key { name: String, command: bool },
    /// `[expr]`.
    Index(Expr),
}

impl Expr {
    /// Parses expression markup. Like the reference implementation this never fails: anything
    /// that is not a literal or a range is treated as a variable lookup.
    pub fn parse(markup: &str) -> Expr {
        let markup = lax::strip(markup);
        let bytes = markup.as_bytes();
        if let (Some(first), Some(last)) = (bytes.first(), bytes.last())
            && ((*first == b'"' && *last == b'"') || (*first == b'\'' && *last == b'\''))
        {
            let inner = if markup.len() >= 2 {
                &markup[1..markup.len() - 1]
            } else {
                ""
            };
            return Expr::Literal(Value::str(inner));
        }
        match markup {
            "" | "nil" | "null" => return Expr::Literal(Value::Nil),
            "true" => return Expr::Literal(Value::Bool(true)),
            "false" => return Expr::Literal(Value::Bool(false)),
            "blank" | "empty" => return Expr::Literal(Value::empty_string()),
            _ => {}
        }
        if markup.starts_with('(')
            && markup.ends_with(')')
            && let Some((from, to)) = split_range(markup)
        {
            let from = Expr::parse(from);
            let to = Expr::parse(to);
            return match (&from, &to) {
                (Expr::Literal(a), Expr::Literal(b)) => {
                    Expr::Literal(Value::Range(literal_to_i(a), literal_to_i(b)))
                }
                _ => Expr::Range(Box::new(from), Box::new(to)),
            };
        }
        if let Some(number) = parse_number(markup) {
            return Expr::Literal(number);
        }
        Expr::Lookup(VariableLookup::parse(markup))
    }

    /// Parses an operand of a condition, where `blank` and `empty` are keywords.
    pub fn parse_condition_operand(markup: &str) -> Expr {
        match markup {
            "blank" => Expr::Blank,
            "empty" => Expr::Empty,
            _ => Expr::parse(markup),
        }
    }

    pub fn evaluate(&self, ctx: &Context) -> Result<Value> {
        match self {
            Expr::Literal(value) => Ok(value.clone()),
            Expr::Lookup(lookup) => lookup.evaluate(ctx),
            Expr::Range(from, to) => {
                let from = range_bound(&from.evaluate(ctx)?)?;
                let to = range_bound(&to.evaluate(ctx)?)?;
                Ok(Value::Range(from, to))
            }
            Expr::Blank | Expr::Empty => Ok(Value::empty_string()),
        }
    }

    /// The literal string value, when the expression is a quoted string.
    pub fn as_literal_str(&self) -> Option<&str> {
        match self {
            Expr::Literal(Value::Str(s)) => Some(s),
            _ => None,
        }
    }
}

fn literal_to_i(value: &Value) -> i64 {
    match value {
        Value::Int(i) => *i,
        Value::Float(f) => *f as i64,
        Value::Str(s) => string_to_i(s),
        _ => 0,
    }
}

fn range_bound(value: &Value) -> Result<i64> {
    match value {
        Value::Int(i) => Ok(*i),
        Value::Nil => Ok(0),
        Value::Str(s) => Ok(string_to_i(s)),
        other => to_integer(other),
    }
}

/// Matches `/\A\(\s*(?>(\S+)\s*\.\.)\s*(\S+)\s*\)\z/` and returns both bounds.
fn split_range(markup: &str) -> Option<(&str, &str)> {
    let inner = &markup[1..markup.len() - 1];
    let start = lax::skip_space(inner, 0);
    let body = &inner[start..];
    let run_len = body
        .char_indices()
        .find(|(_, c)| crate::number::is_ruby_space(*c))
        .map_or(body.len(), |(i, _)| i);
    if run_len == 0 {
        return None;
    }
    // The first bound is matched greedily, then the atomic group commits to that split.
    let after_run = lax::skip_space(body, run_len);
    let (from, rest) = if body[after_run..].starts_with("..") {
        (&body[..run_len], &body[after_run + 2..])
    } else {
        let split = (1..run_len)
            .rev()
            .find(|&i| body.is_char_boundary(i) && body[i..].starts_with(".."))?;
        (&body[..split], &body[split + 2..])
    };
    let to = lax::strip(rest);
    if to.is_empty() || to.chars().any(crate::number::is_ruby_space) {
        return None;
    }
    Some((from, to))
}

/// A port of `Expression.parse_number`, including its tolerance for things like `1.2.3`.
fn parse_number(markup: &str) -> Option<Value> {
    let unsigned = markup.strip_prefix('-').unwrap_or(markup);
    if unsigned.is_empty() {
        return None;
    }
    if unsigned.bytes().all(|b| b.is_ascii_digit()) {
        return match markup.parse::<i64>() {
            Ok(i) => Some(Value::Int(i)),
            Err(_) => markup.parse::<f64>().ok().map(Value::Float),
        };
    }
    if !unsigned.as_bytes()[0].is_ascii_digit()
        || !unsigned.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        return None;
    }
    // Ruby's `String#to_f` reads the longest valid float prefix.
    let int_len = unsigned.bytes().take_while(u8::is_ascii_digit).count();
    let mut end = int_len;
    let rest = &unsigned[int_len..];
    if let Some(frac) = rest.strip_prefix('.') {
        let frac_len = frac.bytes().take_while(u8::is_ascii_digit).count();
        if frac_len > 0 {
            end += 1 + frac_len;
        }
    }
    let sign_len = markup.len() - unsigned.len();
    markup[..sign_len + end]
        .parse::<f64>()
        .ok()
        .map(Value::Float)
}

impl VariableLookup {
    pub fn parse(markup: &str) -> VariableLookup {
        let mut parts = lax::scan_variable_parts(markup).into_iter();
        let name = match parts.next() {
            None => LookupName::Missing,
            Some(part) => match bracket_inner(part) {
                Some(inner) => LookupName::Dynamic(Box::new(Expr::parse(inner))),
                None => LookupName::Static(part.to_string()),
            },
        };
        let lookups = parts
            .map(|part| match bracket_inner(part) {
                Some(inner) => Lookup::Index(Expr::parse(inner)),
                None => Lookup::Key {
                    name: part.to_string(),
                    command: matches!(part, "size" | "first" | "last"),
                },
            })
            .collect();
        VariableLookup { name, lookups }
    }

    /// The dotted path when the lookup only uses static names (`a.b.c`).
    pub fn static_path(&self) -> Option<String> {
        let LookupName::Static(name) = &self.name else {
            return None;
        };
        let mut path = name.clone();
        for lookup in &self.lookups {
            match lookup {
                Lookup::Key { name, .. } => {
                    path.push('.');
                    path.push_str(name);
                }
                Lookup::Index(_) => return None,
            }
        }
        Some(path)
    }

    pub fn evaluate(&self, ctx: &Context) -> Result<Value> {
        let mut object = match &self.name {
            LookupName::Missing => return Ok(Value::Nil),
            LookupName::Static(name) => ctx.find_variable(name),
            LookupName::Dynamic(expr) => match expr.evaluate(ctx)? {
                Value::Str(name) => ctx.find_variable(&name),
                _ => Value::Nil,
            },
        };
        for lookup in &self.lookups {
            let next = match lookup {
                Lookup::Key { name, command } => lookup_named(&object, name, *command, ctx),
                Lookup::Index(expr) => match expr.evaluate(ctx)?.to_liquid_value() {
                    Value::Str(key) => lookup_named(&object, &key, false, ctx),
                    Value::Int(index) => lookup_index(&object, index),
                    _ => None,
                },
            };
            match next {
                Some(value) => object = value,
                None => return Ok(Value::Nil),
            }
        }
        Ok(object)
    }
}

fn bracket_inner(part: &str) -> Option<&str> {
    (part.len() >= 2 && part.starts_with('[') && part.ends_with(']'))
        .then(|| &part[1..part.len() - 1])
}

/// `object[key]` during a render, where the `self` of the context reads the variable `key`.
fn lookup_named(object: &Value, key: &str, command: bool, ctx: &Context) -> Option<Value> {
    match lookup_key(object, key, command) {
        Some(Value::Nil) | None if ctx.is_self(object) => Some(ctx.find_variable(key)),
        found => found,
    }
}

/// `object[key]` for a string key, with the `size`/`first`/`last` fallbacks.
pub fn lookup_key(object: &Value, key: &str, command: bool) -> Option<Value> {
    match object {
        Value::Hash(map) => match map.get(key) {
            Some(value) => Some(value.clone()),
            None if command => match key {
                "size" => Some(Value::Int(map.len() as i64)),
                "first" => map
                    .first()
                    .map(|(k, v)| Value::array(vec![Value::str(k), v.clone()])),
                _ => None,
            },
            None => None,
        },
        Value::Object(drop) => match drop.get(key) {
            Some(value) => Some(value),
            None if command => match key {
                "size" => drop.size().map(|size| Value::Int(size as i64)),
                "first" => drop
                    .items()
                    .map(|items| items.first().cloned().unwrap_or(Value::Nil)),
                "last" => drop
                    .items()
                    .map(|items| items.last().cloned().unwrap_or(Value::Nil)),
                _ => None,
            },
            // Drops answer every key; unknown ones are nil.
            None => Some(Value::Nil),
        },
        Value::Array(items) if command => match key {
            "size" => Some(Value::Int(items.len() as i64)),
            "first" => Some(items.first().cloned().unwrap_or(Value::Nil)),
            "last" => Some(items.last().cloned().unwrap_or(Value::Nil)),
            _ => None,
        },
        Value::Str(s) if command => match key {
            "size" => Some(Value::Int(s.chars().count() as i64)),
            "first" => Some(
                s.chars()
                    .next()
                    .map_or_else(Value::empty_string, |c| Value::str(c.to_string())),
            ),
            "last" => Some(
                s.chars()
                    .last()
                    .map_or_else(Value::empty_string, |c| Value::str(c.to_string())),
            ),
            _ => None,
        },
        Value::Range(from, to) if command => match key {
            "size" => Some(Value::Int(crate::value::range_len(*from, *to) as i64)),
            "first" => Some(Value::Int(*from)),
            "last" => Some(Value::Int(*to)),
            _ => None,
        },
        Value::Int(_) if command && key == "size" => Some(Value::Int(8)),
        _ => None,
    }
}

/// `object[index]` for an integer key.
pub fn lookup_index(object: &Value, index: i64) -> Option<Value> {
    match object {
        Value::Array(items) => Some(array_at(items, index)),
        // `hash[1]` on a string-keyed hash is simply nil.
        Value::Hash(_) => Some(Value::Nil),
        Value::Object(drop) => Some(match drop.index(index) {
            Some(value) => value,
            None => drop
                .items()
                .map_or(Value::Nil, |items| array_at(&items, index)),
        }),
        _ => None,
    }
}

fn array_at(items: &[Value], index: i64) -> Value {
    let len = items.len() as i64;
    let index = if index < 0 { index + len } else { index };
    if index < 0 || index >= len {
        Value::Nil
    } else {
        items[index as usize].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal(markup: &str) -> Option<Value> {
        match Expr::parse(markup) {
            Expr::Literal(value) => Some(value),
            _ => None,
        }
    }

    #[test]
    fn parses_literals() {
        assert!(matches!(literal("nil"), Some(Value::Nil)));
        assert!(matches!(literal(" 'a b' "), Some(Value::Str(s)) if &*s == "a b"));
        assert!(matches!(literal("42"), Some(Value::Int(42))));
        assert!(matches!(literal("-1.5"), Some(Value::Float(f)) if f == -1.5));
        assert!(matches!(literal("1.2.3"), Some(Value::Float(f)) if f == 1.2));
        assert!(matches!(literal("(1..3)"), Some(Value::Range(1, 3))));
        assert!(literal("a.b").is_none());
        assert!(literal("1a").is_none());
    }

    #[test]
    fn parses_lookups() {
        let Expr::Lookup(lookup) = Expr::parse("a.b['c'][d].size") else {
            panic!("expected a lookup");
        };
        assert!(matches!(&lookup.name, LookupName::Static(n) if n == "a"));
        assert_eq!(lookup.lookups.len(), 4);
        assert!(matches!(
            &lookup.lookups[3],
            Lookup::Key { command: true, .. }
        ));
        assert_eq!(
            VariableLookup::parse("a.b.c").static_path().as_deref(),
            Some("a.b.c")
        );
    }

    #[test]
    fn splits_ranges() {
        assert_eq!(split_range("(1..5)"), Some(("1", "5")));
        assert_eq!(split_range("( a.b .. c.d )"), Some(("a.b", "c.d")));
        assert_eq!(split_range("(a)"), None);
    }
}
