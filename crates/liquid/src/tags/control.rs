//! `if`, `unless`, `case` and `ifchanged`.

use std::ops::Range;

use crate::condition::{Condition, equal};
use crate::context::Context;
use crate::error::{Error, Result};
use crate::expr::Expr;
use crate::lax;
use crate::lexer::{MarkupParser, TokenKind};
use crate::parser::{Parser, TagToken};
use crate::strict2::{self, Issue, Style};
use crate::template::{BlockBody, Tag};

struct Branch {
    /// `None` for `else`.
    condition: Option<Condition>,
    body: BlockBody,
}

struct If {
    branches: Vec<Branch>,
    /// `unless`: the first branch runs when its condition is falsy.
    negate_first: bool,
    blank: bool,
}

impl Tag for If {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        for (index, branch) in self.branches.iter().enumerate() {
            let matches = match &branch.condition {
                None => true,
                Some(condition) => {
                    let truthy = condition.is_truthy(ctx)?;
                    if index == 0 && self.negate_first {
                        !truthy
                    } else {
                        truthy
                    }
                }
            };
            if matches {
                branch.body.render(ctx, out);
                return Ok(());
            }
        }
        Ok(())
    }

    fn blank(&self) -> bool {
        self.blank
    }
}

fn parse_conditional(
    parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
    negate_first: bool,
) -> Result<Box<dyn Tag>> {
    let block_name = token.name;
    let delimiter = format!("end{block_name}");
    let mut branches = vec![Branch {
        condition: Some(Condition::parse(token.markup)?),
        body: BlockBody::new(),
    }];
    let mut blank = true;
    loop {
        let current = branches.last_mut().expect("at least one branch");
        let end = parser.parse_body(&mut current.body)?;
        blank = blank && current.body.is_blank();
        match end {
            None => return Err(Parser::tag_never_closed(block_name)),
            Some(end) if end.name == delimiter => break,
            Some(end) if end.name == "elsif" => {
                parser.audit_delimiter(&end);
                branches.push(Branch {
                    condition: Some(Condition::parse(&end.markup)?),
                    body: BlockBody::new(),
                })
            }
            Some(end) if end.name == "else" => branches.push(Branch {
                condition: None,
                body: BlockBody::new(),
            }),
            Some(end) => return Err(Parser::unknown_tag_error(&end.name, block_name, &delimiter)),
        }
    }
    if blank {
        for branch in &mut branches {
            branch.body.remove_blank_strings();
        }
    }
    Ok(Box::new(If {
        branches,
        negate_first,
        blank,
    }))
}

/// The markup of `if`, `elsif` and `unless`.
pub(super) fn check_condition(markup: &str, style: &Style) -> Option<Issue> {
    strict2::check(
        markup,
        style,
        |markup| Condition::parse(markup).ok(),
        Condition::strict2_parse,
        strict2::condition,
        false,
    )
    .map(|issue| issue.with_intent(strict2::intent::condition(markup, style)))
}

pub(super) fn parse_if(parser: &mut Parser<'_, '_>, token: &TagToken<'_>) -> Result<Box<dyn Tag>> {
    parse_conditional(parser, token, false)
}

pub(super) fn parse_unless(
    parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    parse_conditional(parser, token, true)
}

enum CaseBranch {
    When { values: Vec<Expr>, body: BlockBody },
    Else { body: BlockBody },
}

struct Case {
    left: Expr,
    branches: Vec<CaseBranch>,
    blank: bool,
}

impl Tag for Case {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let mut execute_else = true;
        for branch in &self.branches {
            match branch {
                CaseBranch::Else { body } => {
                    if execute_else {
                        body.render(ctx, out);
                    }
                }
                CaseBranch::When { values, body } => {
                    // Each matching value renders the body, so `when 1, 1` renders it twice.
                    for value in values {
                        let left = self.left.evaluate(ctx)?.to_liquid_value();
                        let right = value.evaluate(ctx)?.to_liquid_value();
                        if equal(&self.left, &left, value, &right) {
                            execute_else = false;
                            body.render(ctx, out);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn blank(&self) -> bool {
        self.blank
    }
}

/// `when a, b or c` as the strict parser reads it (`parse_strict2_when`).
fn strict2_when(markup: &str) -> Result<Vec<Expr>> {
    let mut parser = MarkupParser::strict2(markup)?;
    let mut values = Vec::new();
    loop {
        values.push(Expr::parse_condition_operand(&parser.expression()?));
        if !(parser.id("or") || parser.consume_if(TokenKind::Comma).is_some()) {
            break;
        }
    }
    parser.consume(TokenKind::EndOfString)?;
    Ok(values)
}

pub(super) fn check_when(markup: &str, style: &Style) -> Option<Issue> {
    strict2::check(
        markup,
        style,
        |markup| parse_when(markup).ok(),
        strict2_when,
        |values, style| {
            let printed = values
                .iter()
                .map(|value| strict2::condition_operand(value, style))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(printed.join(if style.uses_or { " or " } else { ", " }))
        },
        // Shopify reads `when` with the lax parser today.
        true,
    )
    .map(|issue| issue.with_intent(strict2::intent::when_condition(markup, style)))
}

/// `WhenSyntax = /(QuotedFragment)(?:(?:\s+or\s+|\s*\,\s*)(QuotedFragment.*))?/om`.
fn parse_when(markup: &str) -> Result<Vec<Expr>> {
    let invalid = || {
        Error::syntax(
            "Syntax Error in tag 'case' - Valid when condition: {% when [condition] [or condition2...] %}",
        )
    };
    let mut values = Vec::new();
    let mut rest = Some(markup);
    while let Some(markup) = rest {
        let (start, end) = lax::find_quoted_fragment(markup, 0).ok_or_else(invalid)?;
        values.push(Expr::parse_condition_operand(&markup[start..end]));
        let after_space = lax::skip_space(markup, end);
        let next = if after_space > end
            && markup[after_space..].starts_with("or")
            && lax::require_space(markup, after_space + 2).is_some()
        {
            lax::require_space(markup, after_space + 2)
        } else if markup[after_space..].starts_with(',') {
            Some(lax::skip_space(markup, after_space + 1))
        } else {
            None
        };
        rest = next
            .filter(|&pos| lax::quoted_fragment(markup, pos).is_some())
            .map(|pos| &markup[pos..]);
    }
    Ok(values)
}

/// The tags of a `case`, as it is parsed.
#[derive(Default)]
struct CaseShape {
    /// Each `when`: where its tag is, and its values.
    whens: Vec<(Range<usize>, Vec<Expr>)>,
    elses: Vec<Range<usize>>,
    /// Whether a `when` follows an `else`.
    when_after_else: bool,
    end: Option<Range<usize>>,
}

impl CaseShape {
    fn record(&mut self, end: &crate::parser::EndTag) {
        let Some(token) = end.token.clone() else {
            return;
        };
        match end.name.as_str() {
            "when" => {
                self.when_after_else |= !self.elses.is_empty();
                self.whens
                    .push((token, parse_when(&end.markup).unwrap_or_default()));
            }
            "else" => self.elses.push(token),
            "endcase" => self.end = Some(token),
            _ => {}
        }
    }

    /// When a `when x and y` of the `case` would be put in an `if`, but the `case` has an
    /// `else` that the code meant to run when `y` is false: the `case` as `if`/`elsif`/`else`,
    /// which does that. Only where it renders the same otherwise: one `else`, last; values that
    /// are distinct literals, since a `case` renders every `when` that matches; nothing but
    /// whitespace before the first `when`, which a `case` never renders.
    fn rewrite_as_if(
        &self,
        parser: &mut Parser<'_, '_>,
        case_token: Range<usize>,
        subject: &Expr,
        findings: &[usize],
    ) {
        let source = parser.source();
        let in_liquid_tag = parser.in_liquid_tag();
        let Some(all) = parser.findings_mut() else {
            return;
        };
        let conditions: Vec<(Range<usize>, Option<String>)> = findings
            .iter()
            .filter_map(|index| {
                let finding = all.get(*index)?;
                let condition = finding
                    .issue
                    .intent
                    .as_ref()
                    .and_then(|intent| intent.body_condition.clone());
                Some((finding.span.clone(), condition))
            })
            .collect();
        if !conditions.iter().any(|(_, condition)| condition.is_some()) {
            return;
        }
        let (Some(end), [_], false) = (&self.end, self.elses.as_slice(), self.when_after_else)
        else {
            return;
        };
        let Some((first, _)) = self.whens.first() else {
            return;
        };
        // Another finding in the `case` tag itself would be rewritten twice.
        if !lax::is_blank(&source[case_token.end.min(first.start)..first.start])
            || all.iter().any(|finding| {
                finding.span.start >= case_token.start && finding.span.end <= case_token.end
            })
        {
            return;
        }
        let mut keys = Vec::new();
        for (_, values) in &self.whens {
            for value in values {
                match strict2::literal_key(value) {
                    Some(key) if !keys.contains(&key) => keys.push(key),
                    _ => return,
                }
            }
        }
        let style = Style::default();
        let Ok(subject) = strict2::expr(subject, &style) else {
            return;
        };
        // The tag that replaces `token`, keeping its whitespace control.
        let tag = |token: &Range<usize>, markup: String| -> (Range<usize>, String) {
            let text = &source[token.clone()];
            if in_liquid_tag {
                let start = token.start + (text.len() - lax::lstrip(text).len());
                (start..token.end, markup)
            } else {
                let left = if text.starts_with("{%-") { "-" } else { "" };
                let right = if text.ends_with("-%}") { "-" } else { "" };
                (token.clone(), format!("{{%{left} {markup} {right}%}}"))
            }
        };
        let mut edits = Vec::new();
        let mut changes = Vec::new();
        for (index, (token, values)) in self.whens.iter().enumerate() {
            let mut parts = Vec::new();
            for value in values {
                let Ok(value) = strict2::condition_operand(value, &style) else {
                    return;
                };
                parts.push(format!("{subject} == {value}"));
            }
            let mut condition = parts.join(" or ");
            // The finding of this `when`, by where its markup is.
            let extra = conditions
                .iter()
                .find(|(span, _)| span.start >= token.start && span.end <= token.end)
                .and_then(|(_, condition)| condition.clone());
            if let Some(extra) = extra {
                if values.len() != 1 {
                    return;
                }
                condition = format!("{condition} and {extra}");
                changes.push(format!("`{extra}`"));
            }
            let keyword = if index == 0 { "if" } else { "elsif" };
            edits.push(tag(token, format!("{keyword} {condition}")));
        }
        // `{% case x %}` and the whitespace up to the first `when` go: the `if` takes its place.
        let (first_edit, markup) = edits.remove(0);
        let opening = if in_liquid_tag {
            let text = &source[case_token.clone()];
            case_token.start + (text.len() - lax::lstrip(text).len())
        } else {
            case_token.start
        };
        let markup = if in_liquid_tag {
            markup
        } else {
            let left = if source[case_token.clone()].starts_with("{%-") {
                "{%-"
            } else {
                "{%"
            };
            format!(
                "{left}{}",
                markup.trim_start_matches("{%-").trim_start_matches("{%")
            )
        };
        edits.insert(0, (opening..first_edit.end, markup));
        edits.push(tag(end, "endif".to_string()));
        let change = format!(
            "the `case` becomes `if`/`elsif`, so its `else` renders when {} is false",
            changes.join(", ")
        );
        for index in findings {
            if let Some(finding) = all.get_mut(*index) {
                finding.issue.intent = Some(strict2::Intent {
                    prelude: Vec::new(),
                    markup: finding.issue.fix.clone().unwrap_or_default(),
                    temporaries: Vec::new(),
                    body_condition: None,
                    block: edits.clone(),
                    change: change.clone(),
                });
            }
        }
    }
}

/// `Syntax = /(#{QuotedFragment})/`: what `case` compares.
fn case_subject(markup: &str) -> Result<Expr> {
    let (start, end) = lax::find_quoted_fragment(markup, 0)
        .ok_or_else(|| Error::syntax("Syntax Error in 'case' - Valid syntax: case [condition]"))?;
    Ok(Expr::parse(&markup[start..end]))
}

pub(super) fn check_case(markup: &str, style: &Style) -> Option<Issue> {
    strict2::check(
        markup,
        style,
        |markup| case_subject(markup).ok(),
        |markup| {
            let mut parser = MarkupParser::strict2(markup)?;
            let subject = Expr::parse(&parser.expression()?);
            parser.consume(TokenKind::EndOfString)?;
            Ok(subject)
        },
        strict2::expr,
        false,
    )
}

pub(super) fn parse_case(
    parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    let left = case_subject(token.markup)?;
    let mut branches: Vec<CaseBranch> = Vec::new();
    // Whatever sits between `case` and the first `when` is parsed but never rendered.
    let mut preamble = BlockBody::new();
    let mut blank = true;
    // The findings of the `when`s, for an intent that puts a body in an `if`.
    let mut audited: Vec<usize> = Vec::new();
    let mut open: Option<usize> = None;
    let mut has_else = false;
    // The tags of the `case`, for an intent that rewrites it as `if`/`elsif`.
    let case_token = parser.current_token();
    let mut shape = CaseShape::default();
    loop {
        let body = match branches.last_mut() {
            Some(CaseBranch::When { body, .. } | CaseBranch::Else { body }) => body,
            None => &mut preamble,
        };
        let end = parser.parse_body(body)?;
        blank = blank && body.is_blank();
        if let (Some(finding), Some(end)) = (open.take(), &end) {
            parser.close_delimiter(finding, end);
        }
        if let Some(end) = &end {
            shape.record(end);
        }
        match end {
            None => return Err(Parser::tag_never_closed("case")),
            Some(end) if end.name == "endcase" => break,
            Some(end) if end.name == "when" => {
                open = parser.audit_delimiter(&end);
                audited.extend(open);
                branches.push(CaseBranch::When {
                    values: parse_when(&end.markup)?,
                    body: BlockBody::new(),
                })
            }
            Some(end) if end.name == "else" => {
                if !lax::is_blank(&end.markup) {
                    return Err(Error::syntax(
                        "Syntax Error in tag 'case' - Valid else condition: {% else %} (no parameters) ",
                    ));
                }
                has_else = true;
                branches.push(CaseBranch::Else {
                    body: BlockBody::new(),
                });
            }
            Some(end) => return Err(Parser::unknown_tag_error(&end.name, "case", "endcase")),
        }
    }
    if has_else {
        parser.delimiters_have_else(&audited);
        if let Some(case_token) = case_token {
            shape.rewrite_as_if(parser, case_token, &left, &audited);
        }
    }
    if blank {
        for branch in &mut branches {
            match branch {
                CaseBranch::When { body, .. } | CaseBranch::Else { body } => {
                    body.remove_blank_strings()
                }
            }
        }
    }
    Ok(Box::new(Case {
        left,
        branches,
        blank,
    }))
}

struct IfChanged {
    body: BlockBody,
}

impl Tag for IfChanged {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let rendered = self.body.render_to_string(ctx);
        if ctx.ifchanged().as_deref() != Some(rendered.as_str()) {
            out.push_str(&rendered);
            *ctx.ifchanged() = Some(rendered);
        }
        Ok(())
    }

    fn blank(&self) -> bool {
        self.body.is_blank()
    }
}

pub(super) fn parse_ifchanged(
    parser: &mut Parser<'_, '_>,
    _token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    Ok(Box::new(IfChanged {
        body: parser.parse_block("ifchanged")?,
    }))
}
