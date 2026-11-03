//! `{% partial 'name' %}content{% endpartial %}`: a region that JavaScript can refresh.

use lsf_liquid::lexer::{MarkupParser, TokenKind};
use lsf_liquid::template::BlockBody;
use lsf_liquid::{Context, Expr, Parser, Result, Tag, TagToken};

/// The comments around a partial, which `@shopify/partial-rendering` looks for. Shopify has
/// not published its own markup yet: these are lsf's.
pub const START_MARKER: &str = "shopify-partial";
pub const END_MARKER: &str = "/shopify-partial";

struct Partial {
    name: Expr,
    body: BlockBody,
}

impl Tag for Partial {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let name = self.name.evaluate(ctx)?.to_str().into_owned();
        // A comment cannot hold `--`, and the name is read back between double quotes: the
        // runtime turns these entities back into the name.
        let name = name
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace("--", "-&#45;");
        out.push_str(&format!("<!--{START_MARKER} name=\"{name}\"-->"));
        self.body.render(ctx, out);
        out.push_str(&format!("<!--{END_MARKER}-->"));
        Ok(())
    }
}

pub(super) fn parse(parser: &mut Parser<'_, '_>, token: &TagToken<'_>) -> Result<Box<dyn Tag>> {
    let mut markup = MarkupParser::new(token.markup)?;
    let name = Expr::parse(&markup.expression()?);
    markup.consume(TokenKind::EndOfString)?;
    Ok(Box::new(Partial {
        name,
        body: parser.parse_block("partial")?,
    }))
}
