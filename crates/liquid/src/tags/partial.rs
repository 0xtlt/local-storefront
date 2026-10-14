//! `render` and `include`.

use std::sync::Arc;

use crate::context::Context;
use crate::error::{Error, ErrorKind, Result};
use crate::expr::Expr;
use crate::lax;
use crate::lexer::{MarkupParser, TokenKind};
use crate::parser::{Parser, TagToken};
use crate::strict2::{self, Issue, Style};
use crate::tags::iteration::ForLoop;
use crate::template::Tag;
use crate::value::Value;

/// The parsed markup shared by `render` and `include`:
/// `'name' (with|for) expr as alias, key: value, ...`.
#[derive(PartialEq)]
pub struct PartialCall {
    pub template_name: Expr,
    pub variable: Option<Expr>,
    pub alias: Option<String>,
    pub is_for_loop: bool,
    pub attributes: Vec<(String, Expr)>,
}

/// Parses
/// `/(NAME)(\s+(with|for)\s+(QuotedFragment+))?(\s+(?:as)\s+(VariableSegment+))?/`,
/// where `NAME` is a quoted string for `render` and any fragment for `include`.
pub fn parse_partial_call(markup: &str, quoted_name_only: bool) -> Option<PartialCall> {
    let (name_start, name_end) = find_template_name(markup, quoted_name_only)?;
    let mut call = PartialCall {
        template_name: Expr::parse(&markup[name_start..name_end]),
        variable: None,
        alias: None,
        is_for_loop: false,
        attributes: Vec::new(),
    };
    let mut pos = name_end;
    if let Some(keyword_start) = lax::require_space(markup, pos) {
        let rest = &markup[keyword_start..];
        let keyword = ["with", "for"]
            .into_iter()
            .find(|keyword| rest.starts_with(keyword));
        if let Some(keyword) = keyword
            && let Some(variable_start) = lax::require_space(markup, keyword_start + keyword.len())
            && let Some(variable_end) = lax::quoted_fragments(markup, variable_start)
        {
            call.variable = Some(Expr::parse(&markup[variable_start..variable_end]));
            call.is_for_loop = keyword == "for";
            pos = variable_end;
        }
    }
    if let Some(as_start) = lax::require_space(markup, pos)
        && let Some(rest) = markup[as_start..].strip_prefix("as")
    {
        let after_as = markup.len() - rest.len();
        if let Some(alias_start) = lax::require_space(markup, after_as) {
            let alias_len = markup[alias_start..]
                .char_indices()
                .find(|(_, c)| !(lax::is_word(*c) || *c == '-'))
                .map_or(markup.len() - alias_start, |(i, _)| i);
            if alias_len > 0 {
                call.alias = Some(markup[alias_start..alias_start + alias_len].to_string());
            }
        }
    }
    for (key, value) in lax::scan_tag_attributes(markup) {
        let expr = Expr::parse(value);
        match call
            .attributes
            .iter_mut()
            .find(|(existing, _)| existing == key)
        {
            Some(slot) => slot.1 = expr,
            None => call.attributes.push((key.to_string(), expr)),
        }
    }
    Some(call)
}

/// The markup of `render` (`quoted_name_only`) or `include` as Shopify's strict parser reads
/// it: `name [(with|for) expr] [as alias] [,] (key: value [,])*`.
pub fn strict2_partial_call(markup: &str, quoted_name_only: bool) -> Result<PartialCall> {
    let mut parser = MarkupParser::strict2(markup)?;
    let template_name = if quoted_name_only {
        Expr::parse(parser.consume(TokenKind::String)?)
    } else {
        Expr::parse(&parser.expression()?)
    };
    let is_for_loop = parser.id("for");
    let variable = if is_for_loop || parser.id("with") {
        Some(Expr::parse(&parser.expression()?))
    } else {
        None
    };
    let alias = if parser.id("as") {
        Some(parser.consume(TokenKind::Id)?.to_string())
    } else {
        None
    };
    parser.consume_if(TokenKind::Comma);
    let mut attributes: Vec<(String, Expr)> = Vec::new();
    while parser.look(TokenKind::Id) {
        let key = parser.consume_any();
        parser.consume(TokenKind::Colon)?;
        let value = Expr::parse(&parser.expression()?);
        match attributes.iter_mut().find(|(existing, _)| existing == key) {
            Some(slot) => slot.1 = value,
            None => attributes.push((key.to_string(), value)),
        }
        parser.consume_if(TokenKind::Comma);
    }
    parser.consume(TokenKind::EndOfString)?;
    Ok(PartialCall {
        template_name,
        variable,
        alias,
        is_for_loop,
        attributes,
    })
}

/// The markup of a partial call, written for the strict parser.
pub fn print_partial_call(call: &PartialCall, style: &Style) -> strict2::Printed {
    let mut out = strict2::expr(&call.template_name, style)?;
    if let Some(variable) = &call.variable {
        out.push_str(if call.is_for_loop { " for " } else { " with " });
        out.push_str(&strict2::expr(variable, style)?);
    }
    if let Some(alias) = &call.alias {
        out.push_str(" as ");
        out.push_str(&strict2::identifier(alias)?);
    }
    out.push_str(&strict2::arguments(&call.attributes, style)?);
    Ok(out)
}

pub fn check_render(markup: &str, style: &Style) -> Option<Issue> {
    strict2::check(
        markup,
        style,
        |markup| {
            parse_partial_call(markup, true)
                .filter(|call| call.template_name.as_literal_str().is_some())
        },
        |markup| strict2_partial_call(markup, true),
        print_partial_call,
        false,
    )
}

pub(super) fn check_include(markup: &str, style: &Style) -> Option<Issue> {
    strict2::check(
        markup,
        style,
        |markup| parse_partial_call(markup, false),
        |markup| strict2_partial_call(markup, false),
        print_partial_call,
        false,
    )
}

fn find_template_name(markup: &str, quoted_only: bool) -> Option<(usize, usize)> {
    if !quoted_only {
        let (start, _) = lax::find_quoted_fragment(markup, 0)?;
        return Some((start, lax::quoted_fragments(markup, start)?));
    }
    // `QuotedString+`: one or more adjacent quoted strings.
    let mut pos = 0;
    while pos < markup.len() {
        let c = markup[pos..].chars().next()?;
        if c == '"' || c == '\'' {
            let mut end = pos;
            while let Some(quote) = markup[end..]
                .chars()
                .next()
                .filter(|c| *c == '"' || *c == '\'')
            {
                match markup[end + 1..].find(quote) {
                    Some(close) => end = end + 1 + close + 1,
                    None => break,
                }
            }
            if end > pos {
                return Some((pos, end));
            }
        }
        pos += c.len_utf8();
    }
    None
}

/// The name a partial's `with`/`for` value is bound to: the alias, or the last path segment.
fn context_variable_name<'a>(alias: &'a Option<String>, template_name: &'a str) -> &'a str {
    match alias {
        Some(alias) => alias,
        None => template_name.rsplit('/').next().unwrap_or(template_name),
    }
}

struct Render {
    template_name: String,
    call: PartialCall,
}

impl Tag for Render {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let partial = ctx.load_partial(&self.template_name)?;
        let variable_name = context_variable_name(&self.call.alias, &self.template_name);
        let variable = match &self.call.variable {
            Some(expr) => ctx.detach(expr.evaluate(ctx)?),
            None => Value::Nil,
        };
        let mut render_one =
            |ctx: &Context, value: Value, forloop: Option<&Arc<ForLoop>>| -> Result<()> {
                let mut inner = ctx.isolated()?;
                inner.set_include_disabled(true);
                if let Some(forloop) = forloop {
                    inner.set("forloop", Value::Object(forloop.clone()));
                }
                for (key, expr) in &self.call.attributes {
                    inner.set(key.clone(), ctx.detach(expr.evaluate(ctx)?));
                }
                if !value.is_nil() {
                    inner.set(variable_name, value);
                }
                partial.render_to(&mut inner, out);
                if let Some(forloop) = forloop {
                    forloop.increment();
                }
                Ok(())
            };
        let iterable = self.call.is_for_loop
            && match &variable {
                Value::Array(_) | Value::Hash(_) | Value::Range(..) => true,
                Value::Object(object) => object.items().is_some(),
                _ => false,
            };
        if iterable {
            let items = crate::tags::collection_items(&variable, 0, None);
            let forloop = Arc::new(ForLoop::new(
                self.template_name.clone(),
                items.len(),
                Value::Nil,
            ));
            for item in items {
                render_one(ctx, item, Some(&forloop))?;
            }
        } else {
            render_one(ctx, variable, None)?;
        }
        Ok(())
    }
}

pub fn parse_render(_parser: &mut Parser<'_, '_>, token: &TagToken<'_>) -> Result<Box<dyn Tag>> {
    let syntax_error =
        || Error::syntax("Syntax error in tag 'render' - Template name must be a quoted string");
    let call = parse_partial_call(token.markup, true).ok_or_else(syntax_error)?;
    let template_name = call
        .template_name
        .as_literal_str()
        .ok_or_else(syntax_error)?
        .to_string();
    Ok(Box::new(Render {
        template_name,
        call,
    }))
}

struct Include {
    call: PartialCall,
}

impl Tag for Include {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        if ctx.include_disabled() {
            return Err(Error::new(
                ErrorKind::Disabled,
                "include usage is not allowed in this context",
            ));
        }
        let Value::Str(template_name) = self.call.template_name.evaluate(ctx)? else {
            return Err(Error::argument(
                "Argument error in tag 'include' - Illegal template name",
            ));
        };
        let partial = ctx.load_partial(&template_name)?;
        let variable_name = context_variable_name(&self.call.alias, &template_name).to_string();
        let variable = match &self.call.variable {
            Some(expr) => expr.evaluate(ctx)?,
            None => ctx.find_variable(&template_name),
        };
        ctx.push_scope()?;
        let result = (|| -> Result<()> {
            for (key, expr) in &self.call.attributes {
                let value = expr.evaluate(ctx)?;
                ctx.set(key.clone(), value);
            }
            match &variable {
                Value::Array(items) => {
                    for item in items.iter() {
                        ctx.set(variable_name.clone(), item.clone());
                        partial.render_to(ctx, out);
                    }
                }
                other => {
                    ctx.set(variable_name.clone(), other.clone());
                    partial.render_to(ctx, out);
                }
            }
            Ok(())
        })();
        ctx.pop_scope();
        result
    }
}

pub(super) fn parse_include(
    _parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    let call = parse_partial_call(token.markup, false).ok_or_else(|| {
        Error::syntax("Error in tag 'include' - Valid syntax: include '[template]' (with|for) [object|collection]")
    })?;
    Ok(Box::new(Include { call }))
}
