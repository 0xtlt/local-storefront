//! `schema`, `stylesheet`, `javascript`, `style` and `layout`.

use lsf_liquid::{BlockBody, Context, Error, Expr, Parser, Result, Tag, TagToken};

use crate::render::state::RenderState;
use crate::theme::Layout;

/// A tag whose body is consumed without being rendered.
struct Silent;

impl Tag for Silent {
    fn render(&self, _ctx: &mut Context, _out: &mut String) -> Result<()> {
        Ok(())
    }

    fn blank(&self) -> bool {
        true
    }
}

fn parse_raw(parser: &mut Parser<'_, '_>, name: &str) -> Result<Box<dyn Tag>> {
    parser.parse_raw_body(name)?;
    Ok(Box::new(Silent))
}

/// `{% schema %}`: read when the file is loaded, never rendered.
pub(super) fn parse_schema(
    parser: &mut Parser<'_, '_>,
    _token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    parse_raw(parser, "schema")
}

/// `{% stylesheet %}`: bundled into `compiled_assets/styles.css`.
pub(super) fn parse_stylesheet(
    parser: &mut Parser<'_, '_>,
    _token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    parse_raw(parser, "stylesheet")
}

/// `{% javascript %}`: bundled into `compiled_assets/scripts.js`.
pub(super) fn parse_javascript(
    parser: &mut Parser<'_, '_>,
    _token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    parse_raw(parser, "javascript")
}

struct Style {
    body: BlockBody,
}

impl Tag for Style {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        out.push_str("<style data-shopify>");
        self.body.render(ctx, out);
        out.push_str("</style>");
        Ok(())
    }
}

pub(super) fn parse_style(
    parser: &mut Parser<'_, '_>,
    _token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    Ok(Box::new(Style {
        body: parser.parse_block("style")?,
    }))
}

struct LayoutTag {
    /// `None` for `{% layout none %}`.
    name: Option<Expr>,
}

impl Tag for LayoutTag {
    fn render(&self, ctx: &mut Context, _out: &mut String) -> Result<()> {
        let layout = match &self.name {
            None => Layout::None,
            Some(expr) => match expr.evaluate(ctx)? {
                lsf_liquid::Value::Str(name) if !name.is_empty() => Layout::Named(name.to_string()),
                _ => Layout::None,
            },
        };
        RenderState::of(ctx)?.set_layout(layout);
        Ok(())
    }

    fn blank(&self) -> bool {
        true
    }
}

pub(super) fn parse_layout(
    _parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    let markup = token.markup.trim();
    if markup.is_empty() {
        return Err(Error::syntax(
            "Syntax error in tag 'layout' - Valid syntax: layout 'name' or layout none",
        ));
    }
    Ok(Box::new(LayoutTag {
        name: (markup != "none").then(|| Expr::parse(markup)),
    }))
}

/// `{% render block %}`: Shopify's form for rendering an app block held in a variable. App
/// blocks come from installed apps, which do not exist outside Shopify, so nothing is rendered.
struct RenderObject {
    object: Expr,
}

impl Tag for RenderObject {
    fn render(&self, ctx: &mut Context, _out: &mut String) -> Result<()> {
        self.object.evaluate(ctx)?;
        Ok(())
    }
}

/// `render`, extended with the variable form standard Liquid rejects.
pub(super) fn parse_render(
    parser: &mut Parser<'_, '_>,
    token: &TagToken<'_>,
) -> Result<Box<dyn Tag>> {
    let markup = lsf_liquid::markup::strip(token.markup);
    let starts_with_name = markup
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_');
    if starts_with_name && lsf_liquid::markup::quoted_fragment(markup, 0) == Some(markup.len()) {
        return Ok(Box::new(RenderObject {
            object: Expr::parse(markup),
        }));
    }
    lsf_liquid::tags::parse_render(parser, token)
}
