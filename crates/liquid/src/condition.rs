//! Conditions used by `if`, `unless` and `case`.

use std::cmp::Ordering;

use crate::context::Context;
use crate::error::{Error, Result};
use crate::expr::Expr;
use crate::lax;
use crate::lexer::{MarkupParser, TokenKind};
use crate::number::float_to_s;
use crate::value::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    And,
    Or,
}

#[derive(Clone, Debug)]
pub struct Condition {
    pub left: Expr,
    pub operator: Option<String>,
    pub right: Option<Expr>,
    pub child: Option<(Relation, Box<Condition>)>,
}

impl Condition {
    pub fn new(left: Expr, operator: Option<String>, right: Option<Expr>) -> Condition {
        Condition {
            left,
            operator,
            right,
            child: None,
        }
    }

    /// Parses the markup of an `if`/`elsif`/`unless` tag.
    pub fn parse(markup: &str) -> Result<Condition> {
        Self::strict_parse(markup).or_else(|_| Self::lax_parse(markup))
    }

    fn strict_parse(markup: &str) -> Result<Condition> {
        let mut parser = MarkupParser::new(markup)?;
        let mut conditions = vec![Self::parse_comparison(&mut parser)?];
        let mut relations = Vec::new();
        loop {
            if parser.id("and") {
                relations.push(Relation::And);
            } else if parser.id("or") {
                relations.push(Relation::Or);
            } else {
                break;
            }
            conditions.push(Self::parse_comparison(&mut parser)?);
        }
        parser.consume(TokenKind::EndOfString)?;
        // `a and b or c` is evaluated right to left: `a and (b or c)`.
        let mut chain = conditions.pop().expect("at least one condition");
        while let Some(mut condition) = conditions.pop() {
            let relation = relations.pop().expect("one relation per extra condition");
            condition.child = Some((relation, Box::new(chain)));
            chain = condition;
        }
        Ok(chain)
    }

    fn parse_comparison(parser: &mut MarkupParser<'_>) -> Result<Condition> {
        let left = Expr::parse_condition_operand(&parser.expression()?);
        match parser.consume_if(TokenKind::Comparison) {
            Some(operator) => {
                let right = Expr::parse_condition_operand(&parser.expression()?);
                Ok(Condition::new(
                    left,
                    Some(operator.to_string()),
                    Some(right),
                ))
            }
            None => Ok(Condition::new(left, None, None)),
        }
    }

    fn lax_parse(markup: &str) -> Result<Condition> {
        let syntax_error =
            || Error::syntax("Syntax Error in tag 'if' - Valid syntax: if [expression]");
        let mut parts = scan_expressions_and_operators(markup);
        let last = parts.pop().ok_or_else(syntax_error)?;
        let mut condition = lax_comparison(last).ok_or_else(syntax_error)?;
        while let Some(operator) = parts.pop() {
            let relation = match lax::strip(operator) {
                "and" => Relation::And,
                "or" => Relation::Or,
                _ => return Err(syntax_error()),
            };
            let expression = parts.pop().ok_or_else(syntax_error)?;
            let mut parent = lax_comparison(expression).ok_or_else(syntax_error)?;
            parent.child = Some((relation, Box::new(condition)));
            condition = parent;
        }
        Ok(condition)
    }

    /// Evaluates the whole chain and returns the deciding value.
    pub fn evaluate(&self, ctx: &Context) -> Result<Value> {
        let mut condition = self;
        loop {
            let result = condition.interpret(ctx)?;
            match &condition.child {
                Some((Relation::Or, child)) if !result.is_truthy() => condition = child,
                Some((Relation::And, child)) if result.is_truthy() => condition = child,
                _ => return Ok(result),
            }
        }
    }

    pub fn is_truthy(&self, ctx: &Context) -> Result<bool> {
        Ok(self.evaluate(ctx)?.is_truthy())
    }

    fn interpret(&self, ctx: &Context) -> Result<Value> {
        let Some(operator) = &self.operator else {
            return self.left.evaluate(ctx);
        };
        let right_expr = self.right.as_ref().unwrap_or(&Expr::Literal(Value::Nil));
        let left = self.left.evaluate(ctx)?.to_liquid_value();
        let right = right_expr.evaluate(ctx)?.to_liquid_value();
        let result = match operator.as_str() {
            "==" => equal(&self.left, &left, right_expr, &right),
            "!=" | "<>" => !equal(&self.left, &left, right_expr, &right),
            "<" => compare(&left, &right)?.is_some_and(Ordering::is_lt),
            ">" => compare(&left, &right)?.is_some_and(Ordering::is_gt),
            "<=" => compare(&left, &right)?.is_some_and(Ordering::is_le),
            ">=" => compare(&left, &right)?.is_some_and(Ordering::is_ge),
            "contains" => contains(&left, &right),
            other => return Err(Error::argument(format!("Unknown operator {other}"))),
        };
        Ok(Value::Bool(result))
    }
}

/// `==` with the special handling of the `blank` and `empty` keywords.
pub fn equal(left_expr: &Expr, left: &Value, right_expr: &Expr, right: &Value) -> bool {
    match (left_expr, right_expr) {
        // A keyword compared with a keyword: neither side responds to `blank?`/`empty?`.
        (Expr::Blank | Expr::Empty, Expr::Blank | Expr::Empty) => false,
        (Expr::Blank, _) => right.is_blank(),
        (Expr::Empty, _) => right.is_empty().unwrap_or(false),
        (_, Expr::Blank) => left.is_blank(),
        (_, Expr::Empty) => left.is_empty().unwrap_or(false),
        _ => left.ruby_eq(right),
    }
}

/// `<`, `>`, `<=`, `>=`: `Ok(None)` when one side does not support ordering (the condition is
/// then false) and an error when both do but are of incompatible types.
fn compare(left: &Value, right: &Value) -> Result<Option<Ordering>> {
    let orderable =
        |value: &Value| matches!(value, Value::Int(_) | Value::Float(_) | Value::Str(_));
    if !orderable(left) || !orderable(right) {
        return Ok(None);
    }
    match left.ruby_cmp(right) {
        Some(ordering) => Ok(Some(ordering)),
        None => {
            let right_description = match right {
                Value::Int(i) => i.to_string(),
                Value::Float(f) => float_to_s(*f),
                other => other.class_name().to_string(),
            };
            Err(Error::argument(format!(
                "comparison of {} with {} failed",
                left.class_name(),
                right_description
            )))
        }
    }
}

pub fn contains(left: &Value, right: &Value) -> bool {
    if left.is_nil()
        || right.is_nil()
        || matches!(left, Value::Bool(false))
        || matches!(right, Value::Bool(false))
    {
        return false;
    }
    match left {
        Value::Str(haystack) => haystack.contains(&*right.to_str()),
        Value::Array(items) => items.iter().any(|item| item.ruby_eq(right)),
        Value::Hash(map) => right.as_str().is_some_and(|key| map.contains_key(key)),
        Value::Range(from, to) => match right {
            Value::Int(i) => from <= i && i <= to,
            Value::Float(f) => (*from as f64) <= *f && *f <= (*to as f64),
            _ => false,
        },
        Value::Object(object) => object
            .items()
            .is_some_and(|items| items.iter().any(|item| item.ruby_eq(right))),
        _ => false,
    }
}

/// `markup.scan(ExpressionsAndOperators)`: alternating expressions and `and`/`or` operators.
fn scan_expressions_and_operators(markup: &str) -> Vec<&str> {
    // The reference pattern is
    // /(?:\b(?:\s?and\s?|\s?or\s?)\b|(?:\s*(?!\b(?:\s?and\s?|\s?or\s?)\b)(?:QuotedFragment|\S+)\s*)+)/
    // In practice it yields the boolean operators as their own matches and everything between
    // them as expressions; we split on whitespace-delimited `and` / `or` outside quotes.
    let mut out = Vec::new();
    let mut expression_start: Option<usize> = None;
    let mut pos = 0;
    while pos < markup.len() {
        let after_space = lax::skip_space(markup, pos);
        if after_space >= markup.len() {
            break;
        }
        let word_end = lax::quoted_fragment(markup, after_space).unwrap_or_else(|| {
            after_space
                + markup[after_space..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8)
        });
        let word = &markup[after_space..word_end];
        if word == "and" || word == "or" {
            if let Some(start) = expression_start.take() {
                out.push(&markup[start..pos]);
            }
            out.push(word);
        } else if expression_start.is_none() {
            expression_start = Some(after_space);
        }
        pos = word_end;
    }
    if let Some(start) = expression_start {
        out.push(&markup[start..]);
    }
    out
}

/// `Syntax = /(QuotedFragment)\s*([=!<>a-z_]+)?\s*(QuotedFragment)?/`.
fn lax_comparison(markup: &str) -> Option<Condition> {
    let (start, end) = lax::find_quoted_fragment(markup, 0)?;
    let left = Expr::parse_condition_operand(&markup[start..end]);
    let operator_start = lax::skip_space(markup, end);
    let operator_len = markup[operator_start..]
        .bytes()
        .take_while(|b| matches!(b, b'=' | b'!' | b'<' | b'>' | b'_') || b.is_ascii_lowercase())
        .count();
    if operator_len == 0 {
        return Some(Condition::new(left, None, None));
    }
    let operator = &markup[operator_start..operator_start + operator_len];
    let right_start = lax::skip_space(markup, operator_start + operator_len);
    let right = lax::quoted_fragment(markup, right_start)
        .map(|right_end| Expr::parse_condition_operand(&markup[right_start..right_end]))
        .unwrap_or(Expr::Literal(Value::Nil));
    Some(Condition::new(
        left,
        Some(operator.to_string()),
        Some(right),
    ))
}
