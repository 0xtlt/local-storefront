//! `assign`, `capture`, `increment`, `decrement` and `echo`.

use crate::context::Context;
use crate::error::{Error, Result};
use crate::lax;
use crate::lexer::{MarkupParser, TokenKind};
use crate::parser::{Parser, TagToken};
use crate::strict2::{self, Issue, Style};
use crate::template::{BlockBody, Tag};
use crate::value::Value;
use crate::variable::Variable;

/// The characters `VariableSignature = /\(?[\w\-\.\[\]]\)?/` is made of.
fn is_signature_char(c: char) -> bool {
    is_signature_core(c) || matches!(c, '(' | ')')
}

fn is_signature_core(c: char) -> bool {
    lax::is_word(c) || matches!(c, '-' | '.' | '[' | ']')
}

/// Where the longest run of `VariableSignature`s that ends `text` starts: a parenthesis only
/// counts next to the character it wraps.
fn signature_start(text: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_signature_char(*c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let len = chars.len();
    let core = |i: usize| chars.get(i).is_some_and(|(_, c)| is_signature_core(*c));
    let is = |i: usize, c: char| chars.get(i).is_some_and(|(_, found)| *found == c);
    // `ends[i]`: the characters from `i` on are a sequence of signatures.
    let mut ends = vec![false; len + 1];
    ends[len] = true;
    for i in (0..len).rev() {
        let open = usize::from(is(i, '('));
        ends[i] =
            core(i + open) && (ends[i + open + 1] || (is(i + open + 1, ')') && ends[i + open + 2]));
    }
    (0..len).find(|i| ends[*i]).map(|i| chars[i].0)
}

struct Assign {
    to: String,
    from: Variable,
}

impl Tag for Assign {
    fn render(&self, ctx: &mut Context, _out: &mut String) -> Result<()> {
        let value = self.from.evaluate(ctx)?;
        ctx.assign(self.to.clone(), value);
        Ok(())
    }

    fn blank(&self) -> bool {
        true
    }
}

/// The variable and the value of `assign`: `Syntax = /(#{VariableSignature}+)\s*=\s*(.*)\s*/om`.
fn assign_parts(markup: &str) -> Result<(&str, &str)> {
    for (eq, _) in markup.match_indices('=') {
        let before = markup[..eq].trim_end_matches(crate::number::is_ruby_space);
        if let Some(start) = signature_start(before) {
            return Ok((&before[start..], &markup[lax::skip_space(markup, eq + 1)..]));
        }
    }
    Err(Error::syntax(
        "Syntax Error in 'assign' - Valid syntax: assign [var] = [source]",
    ))
}

pub(super) fn parse_assign(
    _parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    let (to, from) = assign_parts(token.markup)?;
    Ok(Box::new(Assign {
        to: to.to_string(),
        from: Variable::parse(from),
    }))
}

/// A variable name alone, as the strict parser reads `capture`, `increment` and the left of
/// `assign`.
fn strict2_name(markup: &str) -> Result<String> {
    let mut parser = MarkupParser::strict2(lax::strip(markup))?;
    let name = parser.consume(TokenKind::Id)?.to_string();
    parser.consume(TokenKind::EndOfString)?;
    Ok(name)
}

pub(super) fn check_assign(markup: &str, style: &Style) -> Option<Issue> {
    strict2::check(
        markup,
        style,
        |markup| {
            let (to, from) = assign_parts(markup).ok()?;
            Some((to.to_string(), Variable::parse(from)))
        },
        |markup| {
            let (to, from) = assign_parts(markup)?;
            Ok((strict2_name(to)?, Variable::strict2_parse(from)?))
        },
        |(to, from), style| {
            Ok(format!(
                "{} = {}",
                strict2::identifier(to)?,
                strict2::variable(from, style)?
            ))
        },
        // The value is parsed like an output: the lax parser reads it today.
        true,
    )
}

struct Capture {
    to: String,
    body: BlockBody,
}

impl Tag for Capture {
    fn render(&self, ctx: &mut Context, _out: &mut String) -> Result<()> {
        let captured = self.body.render_to_string(ctx);
        ctx.assign(self.to.clone(), Value::from(captured));
        Ok(())
    }

    fn blank(&self) -> bool {
        true
    }
}

/// `Syntax = /(#{VariableSignature}+)/`: the first run of signature characters.
fn capture_target(markup: &str) -> Result<&str> {
    let start = markup
        .char_indices()
        .find(|(_, c)| is_signature_char(*c))
        .map(|(i, _)| i)
        .ok_or_else(|| Error::syntax("Syntax Error in 'capture' - Valid syntax: capture [var]"))?;
    let len = markup[start..]
        .char_indices()
        .find(|(_, c)| !is_signature_char(*c))
        .map_or(markup.len() - start, |(i, _)| i);
    Ok(&markup[start..start + len])
}

pub(super) fn parse_capture(
    parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    let to = capture_target(token.markup)?.to_string();
    let body = parser.parse_block("capture")?;
    Ok(Box::new(Capture { to, body }))
}

pub(super) fn check_capture(markup: &str, style: &Style) -> Option<Issue> {
    strict2::check(
        markup,
        style,
        |markup| capture_target(markup).ok().map(str::to_string),
        strict2_name,
        |to, _| strict2::identifier(to),
        false,
    )
}

/// `increment` and `decrement` count in the variable named by their whole markup.
pub(super) fn check_counter(markup: &str, style: &Style) -> Option<Issue> {
    strict2::check(
        markup,
        style,
        |markup| Some(lax::strip(markup).to_string()),
        strict2_name,
        |name, _| strict2::identifier(name),
        false,
    )
}

struct Counter {
    name: String,
    delta: i64,
}

impl Tag for Counter {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let counter = ctx.counter(&self.name);
        let current = counter.as_i64().unwrap_or(0);
        if self.delta > 0 {
            // `increment` prints the value, then adds one.
            out.push_str(&current.to_string());
            *counter = Value::Int(current + 1);
        } else {
            // `decrement` subtracts one, then prints.
            *counter = Value::Int(current - 1);
            out.push_str(&(current - 1).to_string());
        }
        Ok(())
    }
}

pub(super) fn parse_increment(
    _parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    Ok(Box::new(Counter {
        name: lax::strip(token.markup).to_string(),
        delta: 1,
    }))
}

pub(super) fn parse_decrement(
    _parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    Ok(Box::new(Counter {
        name: lax::strip(token.markup).to_string(),
        delta: -1,
    }))
}

struct Echo {
    variable: Variable,
}

impl Tag for Echo {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        self.variable.evaluate(ctx)?.render_to(out);
        Ok(())
    }
}

pub(super) fn parse_echo(
    _parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    Ok(Box::new(Echo {
        variable: Variable::parse(token.markup),
    }))
}
