//! `{% paginate collection.products by 12 %}`.

use lsf_liquid::lexer::{MarkupParser, TokenKind};
use lsf_liquid::number::to_integer;
use lsf_liquid::{BlockBody, Context, Error, Expr, Hash, Parser, Result, Tag, TagToken, Value};

use crate::drops::PaginatedList;
use crate::drops::lists::Lookup;
use crate::render::state::RenderState;
use crate::site::Request;
use crate::urls::encode_component;

struct Paginate {
    collection: Expr,
    /// The variable name when the collection is a plain variable, to substitute its page.
    variable: Option<String>,
    page_size: Expr,
    window_size: Option<Expr>,
    body: BlockBody,
}

/// The URL of another page of the current listing: the current URL with `page` replaced.
pub fn paginate_url(request: &Request, page: usize) -> String {
    let mut pairs: Vec<String> = request
        .query
        .iter()
        .filter(|(key, _)| key != "page")
        .map(|(key, value)| format!("{}={}", encode_component(key), encode_component(value)))
        .collect();
    pairs.push(format!("page={page}"));
    format!("{}?{}", request.localized(&request.path), pairs.join("&"))
}

fn part(title: Value, url: Option<String>) -> Value {
    let mut part = Hash::new();
    part.insert("title".to_string(), title);
    if let Some(url) = &url {
        part.insert("url".to_string(), Value::from(url));
    }
    part.insert("is_link".to_string(), Value::Bool(url.is_some()));
    Value::hash(part)
}

impl Tag for Paginate {
    fn render(&self, ctx: &mut Context, out: &mut String) -> Result<()> {
        let collection = self.collection.evaluate(ctx)?;
        let page_size = to_integer(&self.page_size.evaluate(ctx)?)?.clamp(1, 250) as usize;
        let window_size = match &self.window_size {
            Some(expr) => to_integer(&expr.evaluate(ctx)?)?.max(1) as usize,
            None => 3,
        };
        let state = RenderState::of(ctx)?;
        let request = &state.site.request;
        let current_page = request
            .param("page")
            .and_then(|page| page.parse::<usize>().ok())
            .filter(|page| *page >= 1)
            .unwrap_or(1);

        // `collections`, `pages` and `blogs` paginate through their underlying list.
        let collection = match collection.downcast::<Lookup>().and_then(Lookup::paginated) {
            Some(list) => list,
            None => collection,
        };
        let list = collection.downcast::<PaginatedList>();
        let total = match (list, &collection) {
            (Some(list), _) => list.total(),
            (None, Value::Array(items)) => items.len(),
            (None, Value::Nil) => {
                return Err(Error::argument("Array '' is not paginateable."));
            }
            (None, other) => other.items().map_or(0, |items| items.len()),
        };
        let page_count = total.div_ceil(page_size).max(1);
        let offset = (current_page - 1) * page_size;

        // The page numbers shown: the first, the last, and a window around the current one.
        let mut parts = Vec::new();
        if page_count > 1 {
            let mut gap = false;
            for page in 1..=page_count {
                if page == current_page {
                    parts.push(part(Value::from(page), None));
                } else if page == 1 || page == page_count {
                    parts.push(part(Value::from(page), Some(paginate_url(request, page))));
                } else if page + window_size <= current_page || page >= current_page + window_size {
                    if gap {
                        continue;
                    }
                    parts.push(part(Value::str("&hellip;"), None));
                    gap = true;
                    continue;
                } else {
                    parts.push(part(Value::from(page), Some(paginate_url(request, page))));
                }
                gap = false;
            }
        }

        let mut paginate = Hash::new();
        paginate.insert("page_size".to_string(), Value::from(page_size));
        paginate.insert("current_page".to_string(), Value::from(current_page));
        paginate.insert("current_offset".to_string(), Value::from(offset));
        paginate.insert("items".to_string(), Value::from(total));
        paginate.insert("pages".to_string(), Value::from(page_count));
        paginate.insert("page_param".to_string(), Value::str("page"));
        paginate.insert("parts".to_string(), Value::array(parts));
        if current_page > 1 {
            paginate.insert(
                "previous".to_string(),
                part(
                    Value::str("&laquo; Previous"),
                    Some(paginate_url(request, current_page - 1)),
                ),
            );
        }
        if current_page < page_count {
            paginate.insert(
                "next".to_string(),
                part(
                    Value::str("Next &raquo;"),
                    Some(paginate_url(request, current_page + 1)),
                ),
            );
        }

        ctx.push_scope()?;
        ctx.set("paginate", Value::hash(paginate));
        match list {
            Some(list) => list.set_window(Some((offset, page_size))),
            None => {
                // A plain array: expose its current page under the same variable name.
                if let (Some(name), Value::Array(items)) = (&self.variable, &collection) {
                    let end = (offset + page_size).min(items.len());
                    let page: Vec<Value> =
                        items.get(offset.min(end)..end).unwrap_or_default().to_vec();
                    ctx.set(name.clone(), Value::array(page));
                }
            }
        }
        self.body.render(ctx, out);
        if let Some(list) = list {
            list.set_window(None);
        }
        ctx.pop_scope();
        Ok(())
    }
}

pub(super) fn parse(parser: &mut Parser<'_, '_>, token: &TagToken<'_>) -> Result<Box<dyn Tag>> {
    let syntax_error = || {
        Error::syntax(
            "Syntax error in tag 'paginate' - Valid syntax: paginate [collection] by number",
        )
    };
    let mut markup = MarkupParser::new(token.markup)?;
    let collection_markup = markup.expression()?;
    if !markup.id("by") {
        return Err(syntax_error());
    }
    let page_size = Expr::parse(&markup.expression()?);
    let mut window_size = None;
    while markup.consume_if(TokenKind::Comma).is_some() || markup.look(TokenKind::Id) {
        let key = markup.consume(TokenKind::Id)?;
        markup.consume(TokenKind::Colon)?;
        let value = Expr::parse(&markup.expression()?);
        if key == "window_size" {
            window_size = Some(value);
        }
    }
    markup.consume(TokenKind::EndOfString)?;
    let is_plain_variable = !collection_markup.contains(['.', '[']);
    Ok(Box::new(Paginate {
        collection: Expr::parse(&collection_markup),
        variable: is_plain_variable.then_some(collection_markup),
        page_size,
        window_size,
        body: parser.parse_block("paginate")?,
    }))
}
