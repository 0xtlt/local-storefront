//! `{% block 'name', parameter: value %}content{% endblock %}`: renders `blocks/name.liquid`
//! from a template or a layout.

use lsf_liquid::lexer::{MarkupParser, TokenKind};
use lsf_liquid::template::BlockBody;
use lsf_liquid::{Context, Error, Expr, Parser, Result, Tag, TagToken, Value};

use crate::render::section::render_called_block;

/// The value of a parameter: an expression, or a literal array, which only this tag accepts.
enum Argument {
    Expr(Expr),
    List(Vec<Argument>),
}

impl Argument {
    fn evaluate(&self, ctx: &mut Context) -> Result<Value> {
        Ok(match self {
            Argument::Expr(expr) => expr.evaluate(ctx)?,
            Argument::List(items) => Value::array(
                items
                    .iter()
                    .map(|item| item.evaluate(ctx))
                    .collect::<Result<Vec<_>>>()?,
            ),
        })
    }

    /// An expression, or an array: `['a', b, ['c']]`, read up to its closing bracket.
    fn parse(markup: &mut MarkupParser<'_>) -> Result<Argument> {
        if markup.consume_if(TokenKind::OpenSquare).is_none() {
            return Ok(Argument::Expr(Expr::parse(&markup.expression()?)));
        }
        let mut items = Vec::new();
        while markup.consume_if(TokenKind::CloseSquare).is_none() {
            items.push(Argument::parse(markup)?);
            if markup.consume_if(TokenKind::Comma).is_none() {
                markup.consume(TokenKind::CloseSquare)?;
                break;
            }
        }
        if markup.look(TokenKind::Dot) || markup.look(TokenKind::OpenSquare) {
            return Err(Error::syntax(
                "Syntax error in tag 'block' - an array written in place cannot be followed by `.` or `[`",
            ));
        }
        Ok(Argument::List(items))
    }
}

struct BlockTag {
    name: Expr,
    arguments: Vec<(String, Argument)>,
    body: BlockBody,
}

impl Tag for BlockTag {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        // Shopify allows the tag in layouts and templates only.
        if let Some(file) = ctx.template_name.as_deref()
            && !file.starts_with("layout/")
            && !file.starts_with("templates/")
        {
            return Err(Error::standard(
                "the block tag can only be used in layout/ and templates/ files",
            ));
        }
        let kind = self.name.evaluate(ctx)?.to_str().into_owned();
        // The parameters are read when the tag opens, before the body runs.
        let mut parameters = Vec::with_capacity(self.arguments.len());
        for (key, argument) in &self.arguments {
            parameters.push((key.clone(), argument.evaluate(ctx)?));
        }
        out.push_str(&render_called_block(ctx, &kind, &parameters, &self.body)?);
        Ok(())
    }
}

pub(super) fn parse(parser: &mut Parser<'_, '_>, token: &TagToken<'_>) -> Result<Box<dyn Tag>> {
    let mut markup = MarkupParser::new(token.markup)?;
    let name = Expr::parse(&markup.expression()?);
    let mut arguments = Vec::new();
    while markup.consume_if(TokenKind::Comma).is_some() {
        if markup.at_end() {
            break;
        }
        let key = markup.consume(TokenKind::Id)?.to_string();
        markup.consume(TokenKind::Colon)?;
        arguments.push((key, Argument::parse(&mut markup)?));
    }
    markup.consume(TokenKind::EndOfString)?;
    Ok(Box::new(BlockTag {
        name,
        arguments,
        body: parser.parse_block("block")?,
    }))
}
