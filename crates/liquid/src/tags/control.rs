//! `if`, `unless`, `case` and `ifchanged`.

use crate::condition::{Condition, equal};
use crate::context::Context;
use crate::error::{Error, Result};
use crate::expr::Expr;
use crate::lax;
use crate::parser::{Parser, TagToken};
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
            Some(end) if end.name == "elsif" => branches.push(Branch {
                condition: Some(Condition::parse(&end.markup)?),
                body: BlockBody::new(),
            }),
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

pub(super) fn parse_case(
    parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    let (start, end) = lax::find_quoted_fragment(token.markup, 0)
        .ok_or_else(|| Error::syntax("Syntax Error in 'case' - Valid syntax: case [condition]"))?;
    let left = Expr::parse(&token.markup[start..end]);
    let mut branches: Vec<CaseBranch> = Vec::new();
    // Whatever sits between `case` and the first `when` is parsed but never rendered.
    let mut preamble = BlockBody::new();
    let mut blank = true;
    loop {
        let body = match branches.last_mut() {
            Some(CaseBranch::When { body, .. } | CaseBranch::Else { body }) => body,
            None => &mut preamble,
        };
        let end = parser.parse_body(body)?;
        blank = blank && body.is_blank();
        match end {
            None => return Err(Parser::tag_never_closed("case")),
            Some(end) if end.name == "endcase" => break,
            Some(end) if end.name == "when" => branches.push(CaseBranch::When {
                values: parse_when(&end.markup)?,
                body: BlockBody::new(),
            }),
            Some(end) if end.name == "else" => {
                if !lax::is_blank(&end.markup) {
                    return Err(Error::syntax(
                        "Syntax Error in tag 'case' - Valid else condition: {% else %} (no parameters) ",
                    ));
                }
                branches.push(CaseBranch::Else {
                    body: BlockBody::new(),
                });
            }
            Some(end) => return Err(Parser::unknown_tag_error(&end.name, "case", "endcase")),
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
