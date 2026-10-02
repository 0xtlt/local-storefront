//! `{% content_for 'blocks' %}` and `{% content_for 'block', type: ..., id: ... %}`.

use std::sync::Arc;

use lsf_liquid::lexer::{MarkupParser, TokenKind};
use lsf_liquid::{Context, Error, Expr, Hash, Parser, Result, Tag, TagToken, Value};

use crate::render::section::{CONTAINER_VARIABLE, Container, render_block};
use crate::theme::BlockInstance;

struct ContentFor {
    /// `blocks` or `block`.
    target: String,
    /// `key: value` arguments. Keys starting with `closest.` set the closest resources.
    arguments: Vec<(String, Expr)>,
}

impl Tag for ContentFor {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let container_value = ctx.find_variable(CONTAINER_VARIABLE);
        let Some(container) = container_value.downcast::<Container>() else {
            return Err(Error::standard(
                "content_for can only be used in a section or block file",
            ));
        };

        let mut closest: Hash = (*container.closest).clone();
        let mut variables: Vec<(String, Value)> = Vec::new();
        let mut kind = None;
        let mut id = None;
        for (key, expr) in &self.arguments {
            let value = expr.evaluate(ctx)?;
            match key.strip_prefix("closest.") {
                Some(resource) => {
                    closest.insert(resource.to_string(), value);
                }
                None if self.target == "block" && key == "type" => {
                    kind = Some(value.to_str().into_owned())
                }
                None if self.target == "block" && key == "id" => {
                    id = Some(value.to_str().into_owned())
                }
                None => variables.push((key.clone(), value)),
            }
        }
        let closest = Arc::new(closest);

        match self.target.as_str() {
            "blocks" => {
                for key in &container.block_order {
                    let Some(instance) = container.blocks.get(key).filter(|block| !block.disabled)
                    else {
                        continue;
                    };
                    match render_block(ctx, container, key, instance, &closest, &variables) {
                        Ok(html) => out.push_str(&html),
                        Err(error) => out.push_str(&ctx.handle_error(error, 1)),
                    }
                }
                Ok(())
            }
            "block" => {
                let (Some(kind), Some(id)) = (kind, id) else {
                    return Err(Error::syntax(
                        "Syntax error in tag 'content_for' - a static block needs both `type` and `id`",
                    ));
                };
                // A static block uses its stored settings when the template has some, and the
                // defaults of its schema otherwise.
                let stored = container.blocks.get(&id).filter(|block| block.kind == kind);
                if stored.is_some_and(|block| block.disabled) {
                    return Ok(());
                }
                let default = BlockInstance {
                    kind,
                    is_static: true,
                    ..BlockInstance::default()
                };
                out.push_str(&render_block(
                    ctx,
                    container,
                    &id,
                    stored.unwrap_or(&default),
                    &closest,
                    &variables,
                )?);
                Ok(())
            }
            other => Err(Error::syntax(format!(
                "Syntax error in tag 'content_for' - '{other}' is not a valid content_for type. Valid types: blocks, block"
            ))),
        }
    }
}

pub(super) fn parse(_parser: &mut Parser<'_, '_>, token: &TagToken<'_>) -> Result<Box<dyn Tag>> {
    let mut markup = MarkupParser::new(token.markup)?;
    let target = Expr::parse(markup.consume(TokenKind::String)?)
        .as_literal_str()
        .unwrap_or_default()
        .to_string();
    let mut arguments = Vec::new();
    loop {
        markup.consume_if(TokenKind::Comma);
        if !markup.look(TokenKind::Id) {
            break;
        }
        let mut key = markup.consume(TokenKind::Id)?.to_string();
        while markup.consume_if(TokenKind::Dot).is_some() {
            key.push('.');
            key.push_str(markup.consume(TokenKind::Id)?);
        }
        markup.consume(TokenKind::Colon)?;
        arguments.push((key, Expr::parse(&markup.expression()?)));
    }
    markup.consume(TokenKind::EndOfString)?;
    Ok(Box::new(ContentFor { target, arguments }))
}
