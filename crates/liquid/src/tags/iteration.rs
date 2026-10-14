//! `for`, `break`, `continue`, `cycle` and `tablerow`.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::context::{Context, Interrupt};
use crate::error::{Error, Result};
use crate::expr::Expr;
use crate::lax;
use crate::lexer::{MarkupParser, TokenKind};
use crate::number::{string_to_i, to_integer};
use crate::parser::{Parser, TagToken};
use crate::strict2::{self, Issue, Style};
use crate::template::{BlockBody, Tag};
use crate::value::{Object, Value};

/// The `forloop` object.
pub struct ForLoop {
    name: String,
    length: usize,
    index: AtomicUsize,
    parent: Value,
}

impl ForLoop {
    pub fn new(name: impl Into<String>, length: usize, parent: Value) -> Self {
        ForLoop {
            name: name.into(),
            length,
            index: AtomicUsize::new(0),
            parent,
        }
    }

    pub fn increment(&self) {
        self.index.fetch_add(1, Ordering::Relaxed);
    }
}

impl Object for ForLoop {
    fn type_name(&self) -> &str {
        "forloop"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let index = self.index.load(Ordering::Relaxed) as i64;
        let length = self.length as i64;
        Some(match key {
            "length" => Value::Int(length),
            "parentloop" => self.parent.clone(),
            "name" => Value::str(&self.name),
            "index" => Value::Int(index + 1),
            "index0" => Value::Int(index),
            "rindex" => Value::Int(length - index),
            "rindex0" => Value::Int(length - index - 1),
            "first" => Value::Bool(index == 0),
            "last" => Value::Bool(index == length - 1),
            _ => return None,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The items a `for` or `tablerow` loop iterates over, with `offset`/`limit` applied
/// (Ruby Liquid's `slice_collection_for_iteration`).
pub fn collection_items(collection: &Value, from: i64, to: Option<i64>) -> Vec<Value> {
    fn slice<'a>(
        items: impl Iterator<Item = Value> + 'a,
        from: i64,
        to: Option<i64>,
    ) -> Vec<Value> {
        let mut out = Vec::new();
        for (index, item) in items.enumerate() {
            let index = index as i64;
            if to.is_some_and(|to| to <= index) {
                break;
            }
            if from <= index {
                out.push(item);
            }
        }
        out
    }
    match collection {
        Value::Array(items) => slice(items.iter().cloned(), from, to),
        Value::Range(start, end) => {
            if to.is_some_and(|to| to <= from) {
                return Vec::new();
            }
            slice((*start..=*end).map(Value::Int), from, to)
        }
        Value::Hash(map) => slice(
            map.iter()
                .map(|(key, value)| Value::array(vec![Value::str(key), value.clone()])),
            from,
            to,
        ),
        // A string is a single item, and `offset`/`limit` do not apply to it.
        Value::Str(s) if s.is_empty() => Vec::new(),
        Value::Str(_) => vec![collection.clone()],
        Value::Object(object) => match object.items() {
            Some(items) => slice(items.iter().cloned(), from, to),
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

#[derive(PartialEq)]
enum Offset {
    None,
    Continue,
    Expr(Expr),
}

struct For {
    variable_name: String,
    collection: Expr,
    /// `variable-collection`, the key `offset: continue` resumes from.
    name: String,
    reversed: bool,
    limit: Option<Expr>,
    offset: Offset,
    body: BlockBody,
    else_body: Option<BlockBody>,
    blank: bool,
}

impl For {
    fn segment(&self, ctx: &mut Context) -> Result<Vec<Value>> {
        let from = match &self.offset {
            Offset::None => 0,
            Offset::Continue => ctx.for_offset(&self.name) as i64,
            Offset::Expr(expr) => match expr.evaluate(ctx)? {
                Value::Nil => 0,
                value => to_integer(&value)?,
            },
        };
        let collection = self.collection.evaluate(ctx)?;
        let to = match &self.limit {
            None => None,
            Some(expr) => match expr.evaluate(ctx)? {
                Value::Nil => None,
                value => Some(to_integer(&value)? + from),
            },
        };
        let mut segment = collection_items(&collection, from, to);
        if self.reversed {
            segment.reverse();
        }
        ctx.set_for_offset(&self.name, (from.max(0) as usize) + segment.len());
        Ok(segment)
    }
}

impl Tag for For {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let segment = self.segment(ctx)?;
        if segment.is_empty() {
            if let Some(else_body) = &self.else_body {
                else_body.render(ctx, out);
            }
            return Ok(());
        }
        let parent = ctx.for_stack().last().cloned().unwrap_or(Value::Nil);
        let forloop = Arc::new(ForLoop::new(self.name.clone(), segment.len(), parent));
        let forloop_value = Value::Object(forloop.clone());
        ctx.push_scope()?;
        ctx.for_stack().push(forloop_value.clone());
        ctx.set("forloop", forloop_value);
        for item in segment {
            ctx.set(self.variable_name.clone(), item);
            self.body.render(ctx, out);
            forloop.increment();
            if let Some(interrupt) = ctx.pop_interrupt()
                && interrupt == Interrupt::Break
            {
                break;
            }
        }
        ctx.for_stack().pop();
        ctx.pop_scope();
        Ok(())
    }

    fn blank(&self) -> bool {
        self.blank
    }
}

#[derive(PartialEq)]
struct ForMarkup {
    variable_name: String,
    collection_markup: String,
    reversed: bool,
    limit: Option<Expr>,
    offset: Offset,
}

fn parse_offset(markup: &str) -> Offset {
    if markup == "continue" {
        Offset::Continue
    } else {
        Offset::Expr(Expr::parse(markup))
    }
}

fn strict_parse_for(mut parser: MarkupParser<'_>) -> Result<ForMarkup> {
    let variable_name = parser.consume(TokenKind::Id)?.to_string();
    if !parser.id("in") {
        return Err(Error::syntax("For loops require an 'in' clause"));
    }
    let collection_markup = parser.expression()?;
    let reversed = parser.id("reversed");
    let mut limit = None;
    let mut offset = Offset::None;
    while parser.look(TokenKind::Comma) || parser.look(TokenKind::Id) {
        parser.consume_if(TokenKind::Comma);
        let is_limit = parser.id("limit");
        if !is_limit && !parser.id("offset") {
            return Err(Error::syntax(
                "Invalid attribute in for loop. Valid attributes are limit and offset",
            ));
        }
        parser.consume(TokenKind::Colon)?;
        let expression = parser.expression()?;
        if is_limit {
            limit = Some(Expr::parse(&expression));
        } else {
            offset = parse_offset(&expression);
        }
    }
    parser.consume(TokenKind::EndOfString)?;
    Ok(ForMarkup {
        variable_name,
        collection_markup,
        reversed,
        limit,
        offset,
    })
}

/// `Syntax = /\A(VariableSegment+)\s+in\s+(QuotedFragment+)\s*(reversed)?/`.
fn lax_parse_for(markup: &str) -> Result<ForMarkup> {
    let syntax_error =
        || Error::syntax("Syntax Error in 'for loop' - Valid syntax: for [item] in [collection]");
    let name_len = markup
        .char_indices()
        .find(|(_, c)| !(lax::is_word(*c) || *c == '-'))
        .map_or(markup.len(), |(i, _)| i);
    if name_len == 0 {
        return Err(syntax_error());
    }
    let after_name = lax::require_space(markup, name_len).ok_or_else(syntax_error)?;
    let rest = markup[after_name..]
        .strip_prefix("in")
        .ok_or_else(syntax_error)?;
    let in_end = markup.len() - rest.len();
    let collection_start = lax::require_space(markup, in_end).ok_or_else(syntax_error)?;
    let collection_end =
        lax::quoted_fragments(markup, collection_start).ok_or_else(syntax_error)?;
    let reversed = markup[lax::skip_space(markup, collection_end)..].starts_with("reversed");
    let mut limit = None;
    let mut offset = Offset::None;
    for (key, value) in lax::scan_tag_attributes(markup) {
        match key {
            "limit" => limit = Some(Expr::parse(value)),
            "offset" => offset = parse_offset(value),
            _ => {}
        }
    }
    Ok(ForMarkup {
        variable_name: markup[..name_len].to_string(),
        collection_markup: markup[collection_start..collection_end].to_string(),
        reversed,
        limit,
        offset,
    })
}

fn for_markup(markup: &str) -> Result<ForMarkup> {
    MarkupParser::new(markup)
        .and_then(strict_parse_for)
        .or_else(|_| lax_parse_for(markup))
}

pub(super) fn check_for(markup: &str, style: &Style) -> Option<Issue> {
    strict2::check(
        markup,
        style,
        |markup| for_markup(markup).ok(),
        |markup| strict_parse_for(MarkupParser::strict2(markup)?),
        |markup, style| {
            // The loop is named after the markup of its collection (`offset: continue` resumes
            // by that name): keep it when the strict parser reads it as it is.
            let collection = match MarkupParser::strict2(&markup.collection_markup)
                .and_then(|mut parser| Ok((parser.expression()?, parser.at_end())))
            {
                Ok((normalized, true)) if normalized == markup.collection_markup => normalized,
                _ => strict2::expr(&Expr::parse(&markup.collection_markup), style)?,
            };
            let mut out = format!(
                "{} in {collection}",
                strict2::identifier(&markup.variable_name)?
            );
            if markup.reversed {
                out.push_str(" reversed");
            }
            if let Some(limit) = &markup.limit {
                out.push_str(&format!(" limit: {}", strict2::expr(limit, style)?));
            }
            match &markup.offset {
                Offset::None => {}
                Offset::Continue => out.push_str(" offset: continue"),
                Offset::Expr(offset) => {
                    out.push_str(&format!(" offset: {}", strict2::expr(offset, style)?))
                }
            }
            Ok(out)
        },
        false,
    )
}

pub(super) fn parse_for(parser: &mut Parser<'_, '_>, token: &TagToken<'_>) -> Result<Box<dyn Tag>> {
    let markup = for_markup(token.markup)?;
    let mut body = BlockBody::new();
    let mut else_body: Option<BlockBody> = None;
    let mut blank = true;
    loop {
        let in_else = else_body.is_some();
        let current = else_body.as_mut().unwrap_or(&mut body);
        let end = parser.parse_body(current)?;
        blank = blank && current.is_blank();
        match end {
            None => return Err(Parser::tag_never_closed("for")),
            Some(end) if end.name == "endfor" => break,
            Some(end) if end.name == "else" => {
                else_body = Some(BlockBody::new());
                // The reference only parses one `else` body: a second `else` ends the tag
                // there, leaving `endfor` to the enclosing block.
                if in_else {
                    break;
                }
            }
            Some(end) => return Err(Parser::unknown_tag_error(&end.name, "for", "endfor")),
        }
    }
    if blank {
        body.remove_blank_strings();
        if let Some(else_body) = &mut else_body {
            else_body.remove_blank_strings();
        }
    }
    Ok(Box::new(For {
        name: format!("{}-{}", markup.variable_name, markup.collection_markup),
        collection: Expr::parse(&markup.collection_markup),
        variable_name: markup.variable_name,
        reversed: markup.reversed,
        limit: markup.limit,
        offset: markup.offset,
        body,
        else_body,
        blank,
    }))
}

struct InterruptTag(Interrupt);

impl Tag for InterruptTag {
    fn render(&self, ctx: &mut Context, _out: &mut String) -> Result<()> {
        ctx.push_interrupt(self.0);
        Ok(())
    }
}

pub(super) fn parse_break(
    _parser: &mut Parser<'_, '_>,
    _token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    Ok(Box::new(InterruptTag(Interrupt::Break)))
}

pub(super) fn parse_continue(
    _parser: &mut Parser<'_, '_>,
    _token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    Ok(Box::new(InterruptTag(Interrupt::Continue)))
}

enum CycleName {
    Expr(Expr),
    /// Unnamed cycles are keyed by their values.
    Key(String),
}

struct Cycle {
    name: CycleName,
    variables: Vec<Expr>,
}

impl Tag for Cycle {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let key = match &self.name {
            CycleName::Key(key) => key.clone(),
            CycleName::Expr(expr) => expr.evaluate(ctx)?.inspect(),
        };
        let iteration = *ctx.cycle_index(&key);
        if let Some(expr) = self.variables.get(iteration) {
            let value = expr.evaluate(ctx)?;
            match &value {
                Value::Array(items) => {
                    for item in items.iter() {
                        out.push_str(&item.to_str());
                    }
                }
                other => out.push_str(&other.to_str()),
            }
        }
        let next = iteration + 1;
        *ctx.cycle_index(&key) = if next >= self.variables.len() {
            0
        } else {
            next
        };
        Ok(())
    }
}

static CYCLE_IDS: AtomicU64 = AtomicU64::new(0);

fn cycle_variables(markup: &str) -> Vec<Expr> {
    markup
        .split(',')
        .filter_map(|part| {
            let (start, end) = lax::find_quoted_fragment(part, 0)?;
            Some(Expr::parse(&part[start..end]))
        })
        .collect()
}

fn cycle_syntax_error() -> Error {
    Error::syntax("Syntax Error in 'cycle' - Valid syntax: cycle [name :] var [, var2, var3 ...]")
}

/// The name of a cycle, when it has one, and its values.
fn cycle_markup(markup: &str) -> Result<(Option<Expr>, Vec<Expr>)> {
    let fragment_end = lax::quoted_fragment(markup, 0).ok_or_else(cycle_syntax_error)?;
    // NamedSyntax = /\A(QuotedFragment)\s*\:\s*(.*)/om, where the fragment may give back a
    // trailing colon it matched greedily.
    let candidates = [fragment_end, fragment_end.saturating_sub(1)];
    for end in candidates {
        if end == 0 || !markup.is_char_boundary(end) {
            continue;
        }
        if end != fragment_end && !markup[end..].starts_with(':') {
            continue;
        }
        let colon = lax::skip_space(markup, end);
        if markup[colon..].starts_with(':') {
            return Ok((
                Some(Expr::parse(&markup[..end])),
                cycle_variables(&markup[lax::skip_space(markup, colon + 1)..]),
            ));
        }
    }
    Ok((None, cycle_variables(markup)))
}

fn strict2_cycle(markup: &str) -> Result<(Option<Expr>, Vec<Expr>)> {
    let mut parser = MarkupParser::strict2(markup)?;
    if parser.at_end() {
        return Err(cycle_syntax_error());
    }
    let first = Expr::parse(&parser.expression()?);
    let (name, mut variables) = if parser.consume_if(TokenKind::Colon).is_some() {
        (Some(first), vec![Expr::parse(&parser.expression()?)])
    } else {
        (None, vec![first])
    };
    while parser.consume_if(TokenKind::Comma).is_some() {
        if parser.at_end() {
            break;
        }
        variables.push(Expr::parse(&parser.expression()?));
    }
    parser.consume(TokenKind::EndOfString)?;
    Ok((name, variables))
}

pub(super) fn check_cycle(markup: &str, style: &Style) -> Option<Issue> {
    strict2::check(
        markup,
        style,
        |markup| cycle_markup(markup).ok(),
        strict2_cycle,
        |(name, variables), style| {
            let values = variables
                .iter()
                .map(|value| strict2::expr(value, style))
                .collect::<std::result::Result<Vec<_>, _>>()?
                .join(", ");
            Ok(match name {
                Some(name) => format!("{}: {values}", strict2::expr(name, style)?),
                None => values,
            })
        },
        false,
    )
}

pub(super) fn parse_cycle(
    _parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    let (name, variables) = cycle_markup(token.markup)?;
    if let Some(name) = name {
        return Ok(Box::new(Cycle {
            name: CycleName::Expr(name),
            variables,
        }));
    }
    // Two unnamed cycles share their position only when they list the same literals.
    let all_literals = variables.iter().all(|v| matches!(v, Expr::Literal(_)));
    let key = if all_literals {
        let values: Vec<Value> = variables
            .iter()
            .map(|v| match v {
                Expr::Literal(value) => value.clone(),
                _ => Value::Nil,
            })
            .collect();
        Value::array(values).inspect()
    } else {
        format!("cycle:{}", CYCLE_IDS.fetch_add(1, Ordering::Relaxed))
    };
    Ok(Box::new(Cycle {
        name: CycleName::Key(key),
        variables,
    }))
}

/// The `tablerowloop` object.
struct TableRowLoop {
    length: usize,
    cols: i64,
    index: AtomicUsize,
}

impl TableRowLoop {
    fn col(&self) -> i64 {
        if self.cols <= 0 {
            self.index.load(Ordering::Relaxed) as i64 + 1
        } else {
            (self.index.load(Ordering::Relaxed) as i64 % self.cols) + 1
        }
    }

    fn row(&self) -> i64 {
        if self.cols <= 0 {
            1
        } else {
            (self.index.load(Ordering::Relaxed) as i64 / self.cols) + 1
        }
    }
}

impl Object for TableRowLoop {
    fn type_name(&self) -> &str {
        "tablerowloop"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let index = self.index.load(Ordering::Relaxed) as i64;
        let length = self.length as i64;
        Some(match key {
            "length" => Value::Int(length),
            "col" => Value::Int(self.col()),
            "row" => Value::Int(self.row()),
            "index" => Value::Int(index + 1),
            "index0" => Value::Int(index),
            "col0" => Value::Int(self.col() - 1),
            "rindex" => Value::Int(length - index),
            "rindex0" => Value::Int(length - index - 1),
            "first" => Value::Bool(index == 0),
            "last" => Value::Bool(index == length - 1),
            "col_first" => Value::Bool(self.col() == 1),
            "col_last" => Value::Bool(self.col() == self.cols),
            _ => return None,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct TableRow {
    variable_name: String,
    collection: Expr,
    cols: Option<Expr>,
    limit: Option<Expr>,
    offset: Option<Expr>,
    body: BlockBody,
}

/// `value.to_i` as `tablerow` applies it to its attributes.
fn lenient_integer(value: &Value) -> Result<i64> {
    match value {
        Value::Nil => Ok(0),
        Value::Int(i) => Ok(*i),
        Value::Float(f) => Ok(*f as i64),
        Value::Str(s) => Ok(string_to_i(s)),
        _ => Err(Error::argument("invalid integer")),
    }
}

impl Tag for TableRow {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let collection = self.collection.evaluate(ctx)?;
        if !collection.is_truthy() {
            return Ok(());
        }
        let from = match &self.offset {
            Some(expr) => lenient_integer(&expr.evaluate(ctx)?)?,
            None => 0,
        };
        let to = match &self.limit {
            Some(expr) => Some(from + lenient_integer(&expr.evaluate(ctx)?)?),
            None => None,
        };
        let items = collection_items(&collection, from, to);
        let length = items.len();
        let cols = match &self.cols {
            Some(expr) => lenient_integer(&expr.evaluate(ctx)?)?,
            None => length as i64,
        };
        out.push_str("<tr class=\"row1\">\n");
        let tablerowloop = Arc::new(TableRowLoop {
            length,
            cols,
            index: AtomicUsize::new(0),
        });
        ctx.push_scope()?;
        ctx.set("tablerowloop", Value::Object(tablerowloop.clone()));
        for item in items {
            ctx.set(self.variable_name.clone(), item);
            out.push_str(&format!("<td class=\"col{}\">", tablerowloop.col()));
            self.body.render(ctx, out);
            out.push_str("</td>");
            if let Some(interrupt) = ctx.pop_interrupt()
                && interrupt == Interrupt::Break
            {
                break;
            }
            let index = tablerowloop.index.load(Ordering::Relaxed);
            let is_last = index + 1 == length;
            if tablerowloop.col() == cols && !is_last {
                out.push_str(&format!(
                    "</tr>\n<tr class=\"row{}\">",
                    tablerowloop.row() + 1
                ));
            }
            tablerowloop.index.fetch_add(1, Ordering::Relaxed);
        }
        ctx.pop_scope();
        out.push_str("</tr>\n");
        Ok(())
    }

    fn blank(&self) -> bool {
        self.body.is_blank()
    }
}

/// `Syntax = /(\w+)\s+in\s+(QuotedFragment+)/`, searched anywhere in the markup.
#[derive(PartialEq)]
struct TableRowMarkup {
    variable_name: String,
    collection: Expr,
    cols: Option<Expr>,
    limit: Option<Expr>,
    offset: Option<Expr>,
}

/// `Syntax = /(\w+)\s+in\s+(#{QuotedFragment}+)/o` and the attributes found anywhere.
fn tablerow_markup(markup: &str) -> Result<TableRowMarkup> {
    let syntax_error = || {
        Error::syntax(
            "Syntax Error in 'table_row loop' - Valid syntax: table_row [item] in [collection] cols=3",
        )
    };
    let mut found = None;
    let mut pos = 0;
    while pos < markup.len() {
        let c = markup[pos..].chars().next().ok_or_else(syntax_error)?;
        if lax::is_word(c) {
            let name_len = markup[pos..]
                .char_indices()
                .find(|(_, c)| !lax::is_word(*c))
                .map_or(markup.len() - pos, |(i, _)| i);
            let name_end = pos + name_len;
            if let Some(after_name) = lax::require_space(markup, name_end)
                && let Some(rest) = markup[after_name..].strip_prefix("in")
            {
                let in_end = markup.len() - rest.len();
                if let Some(collection_start) = lax::require_space(markup, in_end)
                    && let Some(collection_end) = lax::quoted_fragments(markup, collection_start)
                {
                    found = Some((
                        &markup[pos..name_end],
                        &markup[collection_start..collection_end],
                    ));
                    break;
                }
            }
            pos = name_end;
        } else {
            pos += c.len_utf8();
        }
    }
    let (variable_name, collection_markup) = found.ok_or_else(syntax_error)?;
    let mut tag = TableRowMarkup {
        variable_name: variable_name.to_string(),
        collection: Expr::parse(collection_markup),
        cols: None,
        limit: None,
        offset: None,
    };
    for (key, value) in lax::scan_tag_attributes(markup) {
        match key {
            "cols" => tag.cols = Some(Expr::parse(value)),
            "limit" => tag.limit = Some(Expr::parse(value)),
            "offset" => tag.offset = Some(Expr::parse(value)),
            _ => {}
        }
    }
    Ok(tag)
}

fn strict2_tablerow(markup: &str) -> Result<TableRowMarkup> {
    let mut parser = MarkupParser::strict2(markup)?;
    let variable_name = parser.consume(TokenKind::Id)?.to_string();
    if !parser.id("in") {
        return Err(Error::syntax("For loops require an 'in' clause"));
    }
    let mut tag = TableRowMarkup {
        variable_name,
        collection: Expr::parse(&parser.expression()?),
        cols: None,
        limit: None,
        offset: None,
    };
    parser.consume_if(TokenKind::Comma);
    while parser.look(TokenKind::Id) {
        let key = parser.consume_any();
        let slot = match key {
            "cols" => Some(&mut tag.cols),
            "limit" => Some(&mut tag.limit),
            "offset" => Some(&mut tag.offset),
            // Accepted, and not used.
            "range" => None,
            _ => {
                return Err(Error::syntax(format!(
                    "Invalid attribute '{key}' in tablerow loop. Valid attributes are cols, limit, offset, and range"
                )));
            }
        };
        parser.consume(TokenKind::Colon)?;
        let value = Expr::parse(&parser.expression()?);
        if let Some(slot) = slot {
            *slot = Some(value);
        }
        parser.consume_if(TokenKind::Comma);
    }
    parser.consume(TokenKind::EndOfString)?;
    Ok(tag)
}

pub(super) fn check_tablerow(markup: &str, style: &Style) -> Option<Issue> {
    strict2::check(
        markup,
        style,
        |markup| tablerow_markup(markup).ok(),
        strict2_tablerow,
        |tag, style| {
            let mut out = format!(
                "{} in {}",
                strict2::identifier(&tag.variable_name)?,
                strict2::expr(&tag.collection, style)?
            );
            for (key, value) in [
                ("cols", &tag.cols),
                ("limit", &tag.limit),
                ("offset", &tag.offset),
            ] {
                if let Some(value) = value {
                    out.push_str(&format!(" {key}: {}", strict2::expr(value, style)?));
                }
            }
            Ok(out)
        },
        false,
    )
}

pub(super) fn parse_tablerow(
    parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    let markup = tablerow_markup(token.markup)?;
    Ok(Box::new(TableRow {
        variable_name: markup.variable_name,
        collection: markup.collection,
        cols: markup.cols,
        limit: markup.limit,
        offset: markup.offset,
        body: parser.parse_block("tablerow")?,
    }))
}
