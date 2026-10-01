//! `raw`, `comment`, `doc` and the inline `#` comment.

use crate::context::Context;
use crate::error::{Error, Result};
use crate::lax;
use crate::parser::{Parser, TagToken};
use crate::template::Tag;

struct Raw {
    body: String,
}

impl Tag for Raw {
    fn render(&self, _ctx: &mut Context, out: &mut String) -> Result<()> {
        out.push_str(&self.body);
        Ok(())
    }

    fn blank(&self) -> bool {
        self.body.is_empty()
    }
}

pub(super) fn parse_raw(parser: &mut Parser<'_, '_>, token: &TagToken<'_>) -> Result<Box<dyn Tag>> {
    if !lax::is_blank(token.markup) {
        return Err(Error::syntax("Syntax Error in 'raw' - Valid syntax: raw"));
    }
    Ok(Box::new(Raw {
        body: parser.parse_raw_body("raw")?,
    }))
}

/// A tag that renders nothing.
struct Silent {
    blank: bool,
}

impl Tag for Silent {
    fn render(&self, _ctx: &mut Context, _out: &mut String) -> Result<()> {
        Ok(())
    }

    fn blank(&self) -> bool {
        self.blank
    }
}

pub(super) fn parse_comment(
    parser: &mut Parser<'_, '_>,
    _token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    parser.skip_comment_body()?;
    Ok(Box::new(Silent { blank: true }))
}

pub(super) fn parse_doc(parser: &mut Parser<'_, '_>, token: &TagToken<'_>) -> Result<Box<dyn Tag>> {
    if !lax::is_blank(token.markup) {
        return Err(Error::syntax(
            "Syntax Error in 'doc' - Valid syntax: {% doc %}{% enddoc %}",
        ));
    }
    let body = parser.parse_raw_body("doc")?;
    if body.contains("{% doc %}") || body.contains("{%- doc") || body.contains("{% doc") {
        return Err(Error::syntax(
            "Syntax Error in 'doc' - Nested doc tags are not allowed",
        ));
    }
    Ok(Box::new(Silent {
        blank: body.is_empty(),
    }))
}

pub(super) fn parse_inline_comment(
    _parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    // Every line of a multi-line inline comment must start with `#`.
    let invalid = token
        .markup
        .split('\n')
        .skip(1)
        .any(|line| lax::lstrip(line).chars().next().is_some_and(|c| c != '#'));
    if invalid {
        return Err(Error::syntax(
            "Syntax error in tag '#' - Each line of comments must be prefixed by the '#' character",
        ));
    }
    Ok(Box::new(Silent { blank: true }))
}
