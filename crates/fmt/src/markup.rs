//! The inside of a Liquid tag or output: its tokens, and the expressions, filters and
//! arguments they make. A port of the markup parser of Shopify's `liquid-html-parser`, in the
//! tolerant mode its Prettier plugin runs it in.

use crate::ast::{Argument, Expr, Filter, Markup, RenderVariable, Span, Variable, is_space};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok {
    Id,
    String,
    Number,
    Dot,
    DotDot,
    Pipe,
    Colon,
    Comma,
    OpenRound,
    CloseRound,
    OpenSquare,
    CloseSquare,
    Dash,
    Comparison,
    Equality,
    Logical,
    EndOfString,
}

#[derive(Debug, Clone)]
pub struct MarkupToken {
    pub kind: Tok,
    pub value: String,
    pub start: usize,
    pub end: usize,
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `\d[\d_]*(\.\d[\d_]*)?` from `pos`: where the number ends.
fn number_end(bytes: &[u8], pos: usize) -> Option<usize> {
    if !bytes.get(pos)?.is_ascii_digit() {
        return None;
    }
    let digits = |mut at: usize| {
        while at < bytes.len() && (bytes[at].is_ascii_digit() || bytes[at] == b'_') {
            at += 1;
        }
        at
    };
    let mut end = digits(pos + 1);
    if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        end = digits(end + 2);
    }
    Some(end)
}

pub fn tokenize_markup(markup: &str, start_offset: usize) -> Vec<MarkupToken> {
    let bytes = markup.as_bytes();
    let mut tokens: Vec<MarkupToken> = Vec::new();
    let mut pos = 0;
    let mut last: Option<Tok> = None;
    let push = |tokens: &mut Vec<MarkupToken>, kind: Tok, start: usize, end: usize| {
        tokens.push(MarkupToken {
            kind,
            value: markup[start..end].to_string(),
            start: start + start_offset,
            end: end + start_offset,
        });
    };
    while pos < markup.len() {
        let c = markup[pos..].chars().next().unwrap();
        if is_space(c) {
            pos += c.len_utf8();
            continue;
        }
        let next = bytes.get(pos + 1).copied();
        if (c == '\'' || c == '"')
            && let Some(close) = markup[pos + 1..].find(c)
        {
            let end = pos + 1 + close + 1;
            push(&mut tokens, Tok::String, pos, end);
            last = Some(Tok::String);
            pos = end;
            continue;
        }
        if c == '-'
            && next.is_some_and(|next| next.is_ascii_digit())
            && let Some(end) = number_end(bytes, pos + 1)
        {
            push(&mut tokens, Tok::Number, pos, end);
            last = Some(Tok::Number);
            pos = end;
            continue;
        }
        if let Some(end) = number_end(bytes, pos) {
            push(&mut tokens, Tok::Number, pos, end);
            last = Some(Tok::Number);
            pos = end;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let mut end = pos + 1;
            while end < bytes.len() && (is_word(bytes[end]) || bytes[end] == b'-') {
                end += 1;
            }
            if bytes.get(end) == Some(&b'?') {
                end += 1;
            }
            let value = &markup[pos..end];
            let mut kind = Tok::Id;
            if last != Some(Tok::Dot) {
                if value == "contains" {
                    kind = Tok::Comparison;
                } else if value == "and" || value == "or" {
                    kind = Tok::Logical;
                }
            }
            push(&mut tokens, kind, pos, end);
            last = Some(kind);
            pos = end;
            continue;
        }
        if c.is_ascii() && next.is_some_and(|next| next.is_ascii()) {
            let kind = match &markup[pos..pos + 2] {
                "==" | "!=" => Some(Tok::Equality),
                ">=" | "<=" => Some(Tok::Comparison),
                ".." => Some(Tok::DotDot),
                _ => None,
            };
            if let Some(kind) = kind {
                push(&mut tokens, kind, pos, pos + 2);
                last = Some(kind);
                pos += 2;
                continue;
            }
        }
        let single = match c {
            '.' => Some(Tok::Dot),
            '|' => Some(Tok::Pipe),
            ':' => Some(Tok::Colon),
            ',' => Some(Tok::Comma),
            '(' => Some(Tok::OpenRound),
            ')' => Some(Tok::CloseRound),
            '[' => Some(Tok::OpenSquare),
            ']' => Some(Tok::CloseSquare),
            '-' => Some(Tok::Dash),
            '>' | '<' => Some(Tok::Comparison),
            '=' => Some(Tok::Equality),
            _ => None,
        };
        if let Some(kind) = single {
            push(&mut tokens, kind, pos, pos + 1);
            last = Some(kind);
        }
        pos += c.len_utf8();
    }
    tokens.push(MarkupToken {
        kind: Tok::EndOfString,
        value: String::new(),
        start: markup.len() + start_offset,
        end: markup.len() + start_offset,
    });
    tokens
}

/// Why a markup is kept as written instead of being parsed.
pub type MarkupError = String;
type Result<T> = std::result::Result<T, MarkupError>;

pub struct MarkupParser<'s> {
    tokens: Vec<MarkupToken>,
    source: &'s str,
    p: usize,
}

const LITERALS: [&str; 6] = ["nil", "null", "true", "false", "blank", "empty"];

impl<'s> MarkupParser<'s> {
    /// A parser of `markup`, which starts at `offset` in `source`.
    pub fn new(markup: &str, offset: usize, source: &'s str) -> MarkupParser<'s> {
        MarkupParser {
            tokens: tokenize_markup(markup, offset),
            source,
            p: 0,
        }
    }

    fn peek(&self) -> &MarkupToken {
        &self.tokens[self.p]
    }

    fn look(&self, kind: Tok, ahead: usize) -> bool {
        self.tokens
            .get(self.p + ahead)
            .is_some_and(|token| token.kind == kind)
    }

    fn consume(&mut self, kind: Tok) -> Result<MarkupToken> {
        let token = self.peek().clone();
        if token.kind != kind {
            return Err(format!(
                "Expected {kind:?} but found {:?} ({})",
                token.kind, token.value
            ));
        }
        self.p += 1;
        Ok(token)
    }

    fn consume_optional(&mut self, kind: Tok) -> Option<MarkupToken> {
        if self.peek().kind != kind {
            return None;
        }
        self.p += 1;
        Some(self.tokens[self.p - 1].clone())
    }

    /// Consumes the identifier `keyword`, if that is what comes next.
    fn id(&mut self, keyword: &str) -> bool {
        let token = self.peek();
        if token.kind != Tok::Id || token.value != keyword {
            return false;
        }
        self.p += 1;
        true
    }

    pub fn is_at_end(&self) -> bool {
        self.peek().kind == Tok::EndOfString
    }

    fn eos_start(&self) -> usize {
        self.tokens.last().unwrap().start
    }

    fn at_argument_start(&self) -> bool {
        matches!(
            self.peek().kind,
            Tok::String | Tok::Number | Tok::Id | Tok::OpenSquare | Tok::OpenRound
        )
    }

    fn skip_to_next_pipe(&mut self) {
        while !self.is_at_end() && !self.look(Tok::Pipe, 0) {
            self.p += 1;
        }
    }

    fn unexpected<T>(&self) -> Result<T> {
        let token = self.peek();
        Err(format!(
            "Expected expression but found {:?} ({})",
            token.kind, token.value
        ))
    }

    pub fn value_expression(&mut self) -> Result<Expr> {
        let token = self.peek().clone();
        let span = Span::new(token.start, token.end);
        match token.kind {
            Tok::String => {
                self.p += 1;
                Ok(Expr::String {
                    value: token.value[1..token.value.len() - 1].to_string(),
                    span,
                })
            }
            Tok::Number => {
                self.p += 1;
                Ok(Expr::Number {
                    value: token.value,
                    span,
                })
            }
            Tok::Id => {
                if LITERALS.contains(&token.value.as_str())
                    && !self.look(Tok::Dot, 1)
                    && !self.look(Tok::OpenSquare, 1)
                {
                    self.p += 1;
                    return Ok(Expr::Literal {
                        keyword: token.value,
                        span,
                    });
                }
                self.variable_lookup()
            }
            Tok::OpenSquare => self.variable_lookup(),
            Tok::OpenRound => {
                self.p += 1;
                let start = self.value_expression()?;
                self.consume(Tok::DotDot)?;
                let end = self.value_expression()?;
                let close = self.consume(Tok::CloseRound)?;
                Ok(Expr::Range {
                    start: Box::new(start),
                    end: Box::new(end),
                    span: Span::new(token.start, close.end),
                })
            }
            _ => self.unexpected(),
        }
    }

    pub fn variable_lookup(&mut self) -> Result<Expr> {
        let current = self.peek().clone();
        let (name, start) = match current.kind {
            Tok::Id => {
                self.p += 1;
                (Some(current.value), current.start)
            }
            Tok::OpenSquare => (None, current.start),
            _ => {
                return Err(format!(
                    "Expected identifier or [ but found {:?} ({})",
                    current.kind, current.value
                ));
            }
        };
        let mut lookups = Vec::new();
        let mut end = if name.is_some() { current.end } else { start };
        while !self.is_at_end() {
            if self.consume_optional(Tok::Dot).is_some() {
                let property = self.consume(Tok::Id)?;
                end = property.end;
                lookups.push(Expr::String {
                    value: property.value,
                    span: Span::new(property.start, property.end),
                });
            } else if self.look(Tok::OpenSquare, 0) {
                self.p += 1;
                let inner = self.value_expression()?;
                let close = self.consume(Tok::CloseSquare)?;
                end = close.end;
                lookups.push(inner);
            } else {
                break;
            }
        }
        Ok(Expr::Lookup {
            name,
            lookups,
            span: Span::new(start, end),
        })
    }

    fn comparison(&mut self) -> Result<Expr> {
        let left = self.value_expression()?;
        let token = self.peek().clone();
        if token.kind != Tok::Comparison && token.kind != Tok::Equality {
            return Ok(left);
        }
        if !matches!(
            token.value.as_str(),
            "==" | "!=" | "<" | ">" | "<=" | ">=" | "contains"
        ) {
            return Err(format!("Unknown operator {}", token.value));
        }
        self.p += 1;
        let right = self.value_expression()?;
        let span = Span::new(left.span().start, right.span().end);
        Ok(Expr::Comparison {
            comparator: token.value,
            left: Box::new(left),
            right: Box::new(right),
            span,
        })
    }

    fn logical_expr(&mut self) -> Result<Expr> {
        let left = self.comparison()?;
        let token = self.peek().clone();
        if token.kind != Tok::Logical {
            return Ok(left);
        }
        self.p += 1;
        let right = self.logical_expr()?;
        let span = Span::new(left.span().start, right.span().end);
        Ok(Expr::Logical {
            relation: token.value,
            left: Box::new(left),
            right: Box::new(right),
            span,
        })
    }

    pub fn conditional_expression(&mut self) -> Result<Expr> {
        self.logical_expr()
    }

    /// A value, or a condition used as one.
    fn expression(&mut self) -> Result<Expr> {
        let expression = self.logical_expr()?;
        Ok(match expression {
            Expr::Comparison { span, .. } | Expr::Logical { span, .. } => Expr::Boolean {
                condition: Box::new(expression),
                span,
            },
            other => other,
        })
    }

    pub fn liquid_variable(&mut self) -> Result<Variable> {
        let expression = self.expression()?;
        let filters = self.filters()?;
        let start = expression.span().start;
        let end = filters
            .last()
            .map_or(expression.span().end, |filter| filter.span.end);
        Ok(Variable {
            raw_source: self.source[start..end].to_string(),
            expression,
            filters,
        })
    }

    fn filters(&mut self) -> Result<Vec<Filter>> {
        let mut filters = Vec::new();
        while self.consume_optional(Tok::Pipe).is_some() {
            if !self.look(Tok::Id, 0) {
                self.skip_to_next_pipe();
                continue;
            }
            filters.push(self.filter()?);
        }
        Ok(filters)
    }

    fn filter(&mut self) -> Result<Filter> {
        let name = self.consume(Tok::Id)?;
        let mut args = Vec::new();
        let mut end = name.end;
        if let Some(colon) = self.consume_optional(Tok::Colon) {
            end = colon.end;
            let at_boundary = self.is_at_end() || self.look(Tok::Pipe, 0);
            let only_whitespace = self.source[colon.end..self.peek().start]
                .chars()
                .all(is_space);
            let empty_arguments = at_boundary && only_whitespace;
            if !empty_arguments && self.at_argument_start() {
                args = self.arguments()?;
            }
            if let Some(last) = args.last() {
                end = last.span().end;
            }
        }
        Ok(Filter {
            name: name.value,
            args,
            span: Span::new(name.start, end),
        })
    }

    fn argument(&mut self) -> Result<Argument> {
        if self.look(Tok::Id, 0) {
            let mut ahead = 1;
            while self.look(Tok::Dot, ahead) && self.look(Tok::Id, ahead + 1) {
                ahead += 2;
            }
            if self.look(Tok::Colon, ahead) {
                return self.named_argument();
            }
        }
        Ok(Argument::Positional(self.value_expression()?))
    }

    /// `name: value`, where the name may be dotted.
    fn argument_name(&mut self) -> Result<(String, usize)> {
        let token = self.consume(Tok::Id)?;
        let mut name = token.value;
        while self.consume_optional(Tok::Dot).is_some() {
            let segment = self.consume(Tok::Id)?;
            name.push('.');
            name.push_str(&segment.value);
        }
        self.consume(Tok::Colon)?;
        Ok((name, token.start))
    }

    fn named_argument(&mut self) -> Result<Argument> {
        let (name, start) = self.argument_name()?;
        let value = self.value_expression()?;
        let span = Span::new(start, value.span().end);
        Ok(Argument::Named { name, value, span })
    }

    fn arguments(&mut self) -> Result<Vec<Argument>> {
        let mut args = vec![self.argument()?];
        while self.consume_optional(Tok::Comma).is_some() {
            if !self.at_argument_start() {
                break;
            }
            args.push(self.argument()?);
        }
        Ok(args)
    }

    fn named_arguments(&mut self) -> Result<Vec<Argument>> {
        let mut args = vec![self.named_argument()?];
        while self.consume_optional(Tok::Comma).is_some() {
            if !self.look(Tok::Id, 0) {
                break;
            }
            args.push(self.named_argument()?);
        }
        Ok(args)
    }

    /// The named arguments of `block` and `section`, whose values may be arrays.
    fn block_named_arguments(&mut self) -> Result<Vec<Argument>> {
        let mut args = vec![self.block_named_argument()?];
        while self.consume_optional(Tok::Comma).is_some() {
            if !self.look(Tok::Id, 0) {
                break;
            }
            args.push(self.block_named_argument()?);
        }
        Ok(args)
    }

    fn block_named_argument(&mut self) -> Result<Argument> {
        let (name, start) = self.argument_name()?;
        let value = if self.look(Tok::OpenSquare, 0) {
            let open = self.consume(Tok::OpenSquare)?;
            let mut elements = Vec::new();
            while !self.look(Tok::CloseSquare, 0) {
                if self.look(Tok::OpenSquare, 0) {
                    return Err("nested arrays are not supported in block array literal".into());
                }
                elements.push(self.value_expression()?);
                if !self.look(Tok::CloseSquare, 0) {
                    self.consume(Tok::Comma)?;
                }
            }
            let close = self.consume(Tok::CloseSquare)?;
            Expr::Array {
                elements,
                span: Span::new(open.start, close.end),
            }
        } else {
            self.value_expression()?
        };
        let span = Span::new(start, value.span().end);
        Ok(Argument::Named { name, value, span })
    }

    fn string_expression(&mut self, what: &str) -> Result<Expr> {
        let expression = self.value_expression()?;
        if !expression.is_string() {
            return Err(format!("Expected string for {what}"));
        }
        Ok(expression)
    }

    /// The values of a `when`: `a, b or c`.
    pub fn when_values(&mut self) -> Result<Vec<Expr>> {
        let mut values = vec![self.value_expression()?];
        loop {
            if self.consume_optional(Tok::Comma).is_some() {
                values.push(self.value_expression()?);
                continue;
            }
            let token = self.peek();
            if token.kind == Tok::Logical && token.value == "or" {
                self.p += 1;
                values.push(self.value_expression()?);
                continue;
            }
            break;
        }
        // `{% when 'a': %}`: a colon glued to the last string is ignored.
        let colon = self.peek();
        let last = values.last().unwrap();
        if colon.kind == Tok::Colon
            && self.look(Tok::EndOfString, 1)
            && last.is_string()
            && last.span().end == colon.start
        {
            self.p += 1;
        }
        Ok(values)
    }

    fn render_markup(&mut self, any_template_name: bool) -> Result<Markup> {
        let snippet = self.value_expression()?;
        if !any_template_name && !matches!(snippet, Expr::String { .. } | Expr::Lookup { .. }) {
            return Err("Expected string or variable lookup for snippet".into());
        }
        let kind = if self.id("for") {
            Some("for")
        } else if self.id("with") {
            Some("with")
        } else {
            None
        };
        let variable = match kind {
            Some(kind) => Some(RenderVariable {
                kind: kind.to_string(),
                name: self.value_expression()?,
            }),
            None => None,
        };
        let alias = if self.id("as") {
            Some(self.consume(Tok::Id)?.value)
        } else {
            None
        };
        let mut args = Vec::new();
        self.consume_optional(Tok::Comma);
        while self.look(Tok::Id, 0) && self.look(Tok::Colon, 1) {
            args.push(self.named_argument()?);
            self.consume_optional(Tok::Comma);
        }
        Ok(Markup::Render {
            snippet,
            variable,
            alias,
            args,
        })
    }

    /// The markup of the tag `name`, for the tags whose markup is parsed. `liquid` is not one
    /// of them: its markup is a list of tags, which the document parser reads.
    pub fn tag_markup(&mut self, name: &str) -> Result<Markup> {
        match name {
            "assign" => {
                let name = self.consume(Tok::Id)?.value;
                if self.consume(Tok::Equality)?.value != "=" {
                    return Err("Expected '=' in assign tag".into());
                }
                Ok(Markup::Assign {
                    name,
                    value: self.liquid_variable()?,
                })
            }
            "block" => {
                if self.is_at_end() {
                    return Err("in 'block' - missing block type".into());
                }
                let name = self.string_expression("block type")?;
                if let Expr::String { value, .. } = &name {
                    let bytes = value.strip_prefix('_').unwrap_or(value).as_bytes();
                    let valid = bytes.first().is_some_and(u8::is_ascii_alphanumeric)
                        && bytes[1..]
                            .iter()
                            .all(|byte| is_word(*byte) || *byte == b'-');
                    if !valid {
                        return Err(format!("in 'block' - '{value}' is not a valid block type"));
                    }
                }
                let mut args = Vec::new();
                if self.consume_optional(Tok::Comma).is_some() && !self.is_at_end() {
                    args = self.block_named_arguments()?;
                }
                if !self.is_at_end() {
                    return Err("Unexpected token in 'block' tag".into());
                }
                Ok(Markup::Block { name, args })
            }
            "break" | "continue" => {
                if !self.is_at_end() {
                    return Err(format!("'{name}' tag does not accept markup"));
                }
                Ok(Markup::Raw(String::new()))
            }
            "capture" | "increment" | "decrement" => {
                Ok(Markup::Expression(self.variable_lookup()?))
            }
            "case" | "layout" => Ok(Markup::Expression(self.value_expression()?)),
            "content_for" => {
                let content_for_type = self.string_expression("content_for type")?;
                let mut args = Vec::new();
                self.consume_optional(Tok::Comma);
                while self.look(Tok::Id, 0) {
                    match self.named_argument() {
                        Ok(argument) => args.push(argument),
                        Err(_) => break,
                    }
                    self.consume_optional(Tok::Comma);
                }
                Ok(Markup::ContentFor {
                    content_for_type,
                    args,
                })
            }
            "cycle" => {
                let first = self.value_expression()?;
                let mut group_name = None;
                let mut args = Vec::new();
                if self.consume_optional(Tok::Colon).is_some() {
                    group_name = Some(first);
                    args.push(self.value_expression()?);
                } else {
                    args.push(first);
                }
                while self.consume_optional(Tok::Comma).is_some() {
                    args.push(self.value_expression()?);
                }
                Ok(Markup::Cycle { group_name, args })
            }
            "echo" => Ok(Markup::Variable(self.liquid_variable()?)),
            "for" | "tablerow" => {
                let variable_name = self.consume(Tok::Id)?.value;
                if !self.id("in") {
                    return Err("Expected 'in'".into());
                }
                let collection = self.value_expression()?;
                let reversed = self.id("reversed");
                let mut args = Vec::new();
                while !self.is_at_end() {
                    self.consume_optional(Tok::Comma);
                    args.push(self.named_argument()?);
                }
                Ok(Markup::For {
                    variable_name,
                    collection,
                    reversed,
                    args,
                })
            }
            "form" => Ok(Markup::Arguments(self.arguments()?)),
            "if" | "unless" | "elsif" => Ok(Markup::Condition(self.conditional_expression()?)),
            "ifchanged" => Ok(Markup::Nothing),
            "include" => self.render_markup(true),
            "render" => self.render_markup(false),
            "paginate" => {
                let collection = self.value_expression()?;
                if !self.id("by") {
                    return Err("Expected 'by' in paginate tag".into());
                }
                let page_size = self.value_expression()?;
                let mut args = Vec::new();
                if self.consume_optional(Tok::Comma).is_some() || self.peek().kind == Tok::Id {
                    args = self.named_arguments()?;
                }
                Ok(Markup::Paginate {
                    collection,
                    page_size,
                    args,
                })
            }
            "partial" => Ok(Markup::Expression(self.string_expression("partial name")?)),
            "sections" => Ok(Markup::Expression(self.string_expression("sections name")?)),
            "section" => {
                let name = self.string_expression("section name")?;
                let mut args = Vec::new();
                if self.consume_optional(Tok::Comma).is_some() {
                    args = self.block_named_arguments()?;
                }
                Ok(Markup::Section { name, args })
            }
            "when" => Ok(Markup::Expressions(self.when_values()?)),
            _ => Err(format!("unknown tag '{name}'")),
        }
    }

    /// The end of the last token, which is where a parsed markup stops.
    pub fn end(&self) -> usize {
        self.eos_start()
    }
}

/// Parses the markup of `name`, or says why it is kept as written.
pub fn parse_tag_markup(name: &str, markup: &str, offset: usize, source: &str) -> Result<Markup> {
    let mut parser = MarkupParser::new(markup, offset, source);
    let parsed = parser.tag_markup(name)?;
    if !parser.is_at_end() {
        return Err("unexpected tokens after markup".into());
    }
    Ok(parsed)
}

/// Parses the markup of `{{ ... }}`.
pub fn parse_variable(markup: &str, offset: usize, source: &str) -> Result<Variable> {
    let mut parser = MarkupParser::new(markup, offset, source);
    let variable = parser.liquid_variable()?;
    if !parser.is_at_end() {
        return Err("unexpected tokens after markup".into());
    }
    Ok(variable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(markup: &str) -> Vec<Tok> {
        tokenize_markup(markup, 0)
            .into_iter()
            .map(|token| token.kind)
            .collect()
    }

    #[test]
    fn tokenizes_operators_and_numbers() {
        use Tok::*;
        assert_eq!(
            kinds("a.b contains -1.5 and (1..c)"),
            vec![
                Id,
                Dot,
                Id,
                Comparison,
                Number,
                Logical,
                OpenRound,
                Number,
                DotDot,
                Id,
                CloseRound,
                EndOfString
            ]
        );
        // After a dot, `contains` and `and` are properties.
        assert_eq!(kinds("a.contains"), vec![Id, Dot, Id, EndOfString]);
    }

    #[test]
    fn parses_filters_with_named_arguments() {
        let source = "product.title | append: 'x', y: 1 | upcase";
        let variable = parse_variable(source, 0, source).unwrap();
        assert_eq!(variable.filters.len(), 2);
        assert_eq!(variable.filters[0].args.len(), 2);
        assert!(variable.filters[0].args[1].is_named());
        assert_eq!(variable.raw_source, source);
    }

    #[test]
    fn keeps_what_it_does_not_understand() {
        assert!(parse_tag_markup("assign", " x == 1 ", 0, " x == 1 ").is_err());
        assert!(parse_tag_markup("if", " a = b ", 0, " a = b ").is_err());
        assert!(parse_tag_markup("for", " x in y limit: 2 ", 0, " x in y limit: 2 ").is_ok());
    }
}
