//! `{% block 'name', parameter: value %}content{% endblock %}`: renders `blocks/name.liquid`
//! from a template or a layout.

use lsf_liquid::lexer::{MarkupParser, TokenKind};
use lsf_liquid::template::BlockBody;
use lsf_liquid::{Context, Expr, Parser, Result, Tag, TagToken, Value};

use crate::render::section::render_called_block;

/// The value of a parameter: an expression, or a literal array, which only this tag accepts.
enum Argument {
    Expr(Expr),
    List(Vec<Expr>),
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
}

struct BlockTag {
    name: Expr,
    arguments: Vec<(String, Argument)>,
    body: BlockBody,
}

impl Tag for BlockTag {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let kind = self.name.evaluate(ctx)?.to_str().into_owned();
        // The body is rendered where it is written: it sees the caller's variables, not the
        // block's.
        let mut content = String::new();
        self.body.render(ctx, &mut content);
        let mut parameters = Vec::with_capacity(self.arguments.len() + 1);
        for (key, argument) in &self.arguments {
            parameters.push((key.clone(), argument.evaluate(ctx)?));
        }
        parameters.push(("content".to_string(), Value::from(content)));
        out.push_str(&render_called_block(ctx, &kind, &parameters)?);
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
        let argument = if markup.consume_if(TokenKind::OpenSquare).is_some() {
            let mut items = Vec::new();
            while markup.consume_if(TokenKind::CloseSquare).is_none() {
                items.push(Expr::parse(&markup.expression()?));
                if markup.consume_if(TokenKind::Comma).is_none() {
                    markup.consume(TokenKind::CloseSquare)?;
                    break;
                }
            }
            Argument::List(items)
        } else {
            Argument::Expr(Expr::parse(&markup.expression()?))
        };
        arguments.push((key, argument));
    }
    markup.consume(TokenKind::EndOfString)?;
    Ok(Box::new(BlockTag {
        name,
        arguments,
        body: parser.parse_block("block")?,
    }))
}
