//! `assign`, `capture`, `increment`, `decrement` and `echo`.

use crate::context::Context;
use crate::error::{Error, Result};
use crate::lax;
use crate::parser::{Parser, TagToken};
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

/// `Syntax = /(#{VariableSignature}+)\s*=\s*(.*)\s*/om`.
pub(super) fn parse_assign(
    _parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    let markup = token.markup;
    for (eq, _) in markup.match_indices('=') {
        let before = markup[..eq].trim_end_matches(crate::number::is_ruby_space);
        if let Some(start) = signature_start(before) {
            let rest = &markup[lax::skip_space(markup, eq + 1)..];
            return Ok(Box::new(Assign {
                to: before[start..].to_string(),
                from: Variable::parse(rest),
            }));
        }
    }
    Err(Error::syntax(
        "Syntax Error in 'assign' - Valid syntax: assign [var] = [source]",
    ))
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

pub(super) fn parse_capture(
    parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    let markup = token.markup;
    let start = markup
        .char_indices()
        .find(|(_, c)| is_signature_char(*c))
        .map(|(i, _)| i)
        .ok_or_else(|| Error::syntax("Syntax Error in 'capture' - Valid syntax: capture [var]"))?;
    let len = markup[start..]
        .char_indices()
        .find(|(_, c)| !is_signature_char(*c))
        .map_or(markup.len() - start, |(i, _)| i);
    let to = markup[start..start + len].to_string();
    let body = parser.parse_block("capture")?;
    Ok(Box::new(Capture { to, body }))
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
