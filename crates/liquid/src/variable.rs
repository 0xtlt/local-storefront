//! `{{ expression | filter: args }}`: an expression followed by a chain of filters.

use crate::context::Context;
use crate::error::Result;
use crate::expr::Expr;
use crate::lax;
use crate::lexer::{MarkupParser, TokenKind};
use crate::value::Value;

#[derive(Clone, Debug, Default)]
pub struct Variable {
    /// `None` when the markup is empty, which evaluates to `nil`.
    pub expr: Option<Expr>,
    pub filters: Vec<FilterCall>,
}

#[derive(Clone, Debug)]
pub struct FilterCall {
    pub name: String,
    pub args: Vec<Expr>,
    pub kwargs: Vec<(String, Expr)>,
}

/// Evaluated filter arguments.
#[derive(Clone, Debug, Default)]
pub struct FilterArgs {
    pub positional: Vec<Value>,
    pub named: Vec<(String, Value)>,
}

impl Variable {
    /// Parses output markup: strictly first, then with the forgiving legacy parser.
    pub fn parse(markup: &str) -> Variable {
        Self::strict_parse(markup).unwrap_or_else(|_| Self::lax_parse(markup))
    }

    fn strict_parse(markup: &str) -> Result<Variable> {
        let mut parser = MarkupParser::new(markup)?;
        let mut variable = Variable::default();
        if parser.at_end() {
            return Ok(variable);
        }
        variable.expr = Some(Expr::parse(&parser.expression()?));
        while parser.consume_if(TokenKind::Pipe).is_some() {
            let name = parser.consume(TokenKind::Id)?;
            let mut args = Vec::new();
            if parser.consume_if(TokenKind::Colon).is_some() {
                args.push(parser.argument()?);
                while parser.consume_if(TokenKind::Comma).is_some() {
                    args.push(parser.argument()?);
                }
            }
            variable.filters.push(FilterCall::from_markup(
                name,
                args.iter().map(String::as_str),
            ));
        }
        parser.consume(TokenKind::EndOfString)?;
        Ok(variable)
    }

    fn lax_parse(markup: &str) -> Variable {
        let mut variable = Variable::default();
        let Some((start, end)) = lax::find_quoted_fragment(markup, 0) else {
            return variable;
        };
        variable.expr = Some(Expr::parse(&markup[start..end]));
        let rest = &markup[end..];
        let Some(pipe) = rest.find('|') else {
            return variable;
        };
        let filter_markup = &rest[lax::skip_space(rest, pipe + 1)..];
        for segment in scan_filter_segments(filter_markup) {
            let Some(name) = first_word(segment) else {
                continue;
            };
            variable.filters.push(FilterCall::from_markup(
                name,
                scan_filter_args(segment).into_iter(),
            ));
        }
        variable
    }

    pub fn evaluate(&self, ctx: &Context) -> Result<Value> {
        let mut value = match &self.expr {
            Some(expr) => expr.evaluate(ctx)?,
            None => Value::Nil,
        };
        for filter in &self.filters {
            let mut args = FilterArgs::default();
            for expr in &filter.args {
                args.positional.push(expr.evaluate(ctx)?);
            }
            for (key, expr) in &filter.kwargs {
                args.named.push((key.clone(), expr.evaluate(ctx)?));
            }
            value = ctx.invoke_filter(&filter.name, &ctx.detach(value), &args)?;
        }
        Ok(value)
    }
}

impl FilterCall {
    fn from_markup<'a>(name: &str, unparsed_args: impl Iterator<Item = &'a str>) -> FilterCall {
        let mut call = FilterCall {
            name: name.to_string(),
            args: Vec::new(),
            kwargs: Vec::new(),
        };
        for arg in unparsed_args {
            match lax::just_tag_attribute(arg) {
                Some((key, value)) => {
                    let expr = Expr::parse(value);
                    match call.kwargs.iter_mut().find(|(existing, _)| existing == key) {
                        Some(slot) => slot.1 = expr,
                        None => call.kwargs.push((key.to_string(), expr)),
                    }
                }
                None => call.args.push(Expr::parse(arg)),
            }
        }
        call
    }
}

impl FilterArgs {
    pub fn get(&self, index: usize) -> Option<&Value> {
        self.positional.get(index)
    }

    /// The positional argument at `index`, or `nil`.
    pub fn at(&self, index: usize) -> Value {
        self.positional.get(index).cloned().unwrap_or(Value::Nil)
    }

    pub fn named(&self, key: &str) -> Option<&Value> {
        self.named
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.positional.len()
    }

    pub fn is_empty(&self) -> bool {
        self.positional.is_empty() && self.named.is_empty()
    }
}

/// `markup.scan(/(?:\s+|#{QuotedFragment}|,)+/)`: the chunks between pipes.
fn scan_filter_segments(markup: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < markup.len() {
        let mut end = pos;
        loop {
            let after_space = lax::skip_space(markup, end);
            if after_space > end {
                end = after_space;
            } else if let Some(next) = lax::quoted_fragment(markup, end) {
                end = next;
            } else if markup[end..].starts_with(',') {
                end += 1;
            } else {
                break;
            }
        }
        if end > pos {
            out.push(&markup[pos..end]);
            pos = end;
        } else {
            pos += markup[pos..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

/// The first `\w+` run in the segment.
fn first_word(segment: &str) -> Option<&str> {
    let start = segment.char_indices().find(|(_, c)| lax::is_word(*c))?.0;
    let len = segment[start..]
        .char_indices()
        .find(|(_, c)| !lax::is_word(*c))
        .map_or(segment.len() - start, |(i, _)| i);
    Some(&segment[start..start + len])
}

/// `segment.scan(/(?:[:,])\s*((?:\w+\s*\:\s*)?#{QuotedFragment})/)`.
fn scan_filter_args(segment: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(offset) = segment[pos..].find([':', ',']) {
        let separator = pos + offset;
        let start = lax::skip_space(segment, separator + 1);
        match keyword_argument(segment, start).or_else(|| lax::quoted_fragment(segment, start)) {
            Some(end) => {
                out.push(&segment[start..end]);
                pos = end;
            }
            None => pos = separator + 1,
        }
    }
    out
}

/// `\w+\s*:\s*#{QuotedFragment}` anchored at `pos`.
fn keyword_argument(s: &str, pos: usize) -> Option<usize> {
    let mut end = pos;
    for c in s[pos..].chars() {
        if lax::is_word(c) {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    if end == pos {
        return None;
    }
    let colon = lax::skip_space(s, end);
    if !s[colon..].starts_with(':') {
        return None;
    }
    lax::quoted_fragment(s, lax::skip_space(s, colon + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_filters_strictly() {
        let variable = Variable::parse(" a.b | f: 1, k: 'v' | g ");
        assert!(variable.expr.is_some());
        assert_eq!(variable.filters.len(), 2);
        assert_eq!(variable.filters[0].name, "f");
        assert_eq!(variable.filters[0].args.len(), 1);
        assert_eq!(variable.filters[0].kwargs[0].0, "k");
        assert_eq!(variable.filters[1].name, "g");
    }

    #[test]
    fn falls_back_to_lax_parsing() {
        // `=` is not a valid token for the strict lexer.
        let variable = Variable::parse("a = b | append: 'x', y | upcase");
        assert!(variable.expr.is_some());
        assert_eq!(variable.filters.len(), 2);
        assert_eq!(variable.filters[0].args.len(), 2);
    }

    #[test]
    fn empty_markup_is_nil() {
        assert!(Variable::parse("   ").expr.is_none());
    }
}
