//! Filters that build URLs and links.

use lsf_liquid::filters::escape_html;
use lsf_liquid::{Context, Environment, FilterArgs, Result, Value};

use super::{html_attributes, site};
use crate::drops::collection::CollectionDrop;
use crate::render::page::Resource;
use crate::render::state::RenderState;
use crate::urls;
use crate::util::handleize;

/// Percent-encodes everything but unreserved characters, with `%20` for spaces.
fn encode_query_value(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn asset_url(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    Ok(Value::from(urls::asset_url(site(ctx)?, &input.to_str())))
}

fn size_argument(args: &FilterArgs) -> String {
    args.get(0)
        .map(|size| size.to_str().into_owned())
        .unwrap_or_else(|| "small".to_string())
}

fn asset_img_url(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let url = urls::asset_url(site(ctx)?, &input.to_str());
    Ok(Value::from(urls::with_size_suffix(
        &url,
        &size_argument(args),
    )))
}

fn file_url(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    Ok(Value::from(urls::file_url(site(ctx)?, &input.to_str())))
}

fn file_img_url(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let url = urls::file_url(site(ctx)?, &input.to_str());
    Ok(Value::from(urls::with_size_suffix(
        &url,
        &size_argument(args),
    )))
}

fn global_asset_url(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    Ok(Value::from(format!(
        "{}/cdn/s/global/{}",
        urls::cdn_origin(site(ctx)?),
        input.to_str()
    )))
}

fn shopify_asset_url(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    Ok(Value::from(format!(
        "{}/cdn/shopifycloud/storefront/assets/themes_support/{}",
        urls::cdn_origin(site(ctx)?),
        input.to_str()
    )))
}

fn payment_type_img_url(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    Ok(Value::from(format!(
        "{}/cdn/shopifycloud/storefront/assets/payment_icons/{}.svg",
        urls::cdn_origin(site(ctx)?),
        input.to_str()
    )))
}

fn is_external(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("//")
}

fn link_to(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let url = args.at(0).to_str().into_owned();
    let rel = if is_external(&url) {
        " rel=\"nofollow\""
    } else {
        ""
    };
    let text = input.to_str();
    Ok(Value::from(if args.named.is_empty() {
        let title = args
            .get(1)
            .map(|title| title.to_str().into_owned())
            .unwrap_or_default();
        format!(
            "<a href=\"{}\" title=\"{}\"{rel}>{text}</a>",
            escape_html(&url),
            escape_html(&title)
        )
    } else {
        format!(
            "<a{} href=\"{}\"{rel}>{text}</a>",
            html_attributes(&args.named, &[]),
            escape_html(&url)
        )
    }))
}

fn type_url(ctx: &Context, kind: &str, value: &str) -> Result<String> {
    Ok(site(ctx)?.request.localized(&format!(
        "/collections/{kind}?q={}",
        encode_query_value(value)
    )))
}

fn url_for_type(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    Ok(Value::from(type_url(ctx, "types", &input.to_str())?))
}

fn url_for_vendor(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    Ok(Value::from(type_url(ctx, "vendors", &input.to_str())?))
}

fn link_to_kind(input: &Value, args: &FilterArgs, ctx: &Context, kind: &str) -> Result<Value> {
    let text = input.to_str();
    Ok(Value::from(format!(
        "<a{} href=\"{}\" title=\"{}\">{text}</a>",
        html_attributes(&args.named, &[]),
        type_url(ctx, kind, &text)?,
        escape_html(&text)
    )))
}

fn link_to_type(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    link_to_kind(input, args, ctx, "types")
}

fn link_to_vendor(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    link_to_kind(input, args, ctx, "vendors")
}

/// `product.url | within: collection` → `/collections/<handle>/products/<handle>`.
fn within(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let url = input.to_str();
    let collection = args.at(0);
    let Some(collection) = collection.downcast::<CollectionDrop>() else {
        return Ok(input.clone());
    };
    Ok(match url.find("/products/") {
        Some(index) => Value::from(format!("{}{}", collection.url(), &url[index..])),
        None => input.clone(),
    })
}

fn sort_by(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let url = input.to_str();
    let separator = if url.contains('?') { '&' } else { '?' };
    Ok(Value::from(format!(
        "{url}{separator}sort_by={}",
        args.at(0).to_str()
    )))
}

/// The listing the page is about and the tags it is narrowed to: `(base URL, tags, noun)`.
fn tag_scope(ctx: &Context) -> Result<(String, Vec<String>, &'static str)> {
    let state = RenderState::of(ctx)?;
    let request = &state.site.request;
    Ok(match &state.page.resource {
        Resource::Collection { collection, scope } => (
            request.localized(&format!(
                "/collections/{}",
                state.site.store.collections[*collection].handle
            )),
            scope.tags.clone(),
            "products",
        ),
        Resource::Blog { blog, tags } => (
            request.localized(&format!(
                "/blogs/{}/tagged",
                state.site.store.blogs[*blog].handle
            )),
            tags.clone(),
            "articles",
        ),
        _ => (request.localized(&request.path), Vec::new(), "products"),
    })
}

fn tag_link(
    input: &Value,
    ctx: &Context,
    tags: Vec<String>,
    base: &str,
    title: String,
) -> Result<Value> {
    let _ = ctx;
    let handles: Vec<String> = tags.iter().map(|tag| handleize(tag)).collect();
    let href = if handles.is_empty() {
        base.trim_end_matches("/tagged").to_string()
    } else {
        format!("{base}/{}", handles.join("+"))
    };
    Ok(Value::from(format!(
        "<a href=\"{}\" title=\"{}\">{}</a>",
        escape_html(&href),
        escape_html(&title),
        input.to_str()
    )))
}

fn link_to_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let (base, _, noun) = tag_scope(ctx)?;
    let tag = args.at(0).to_str().into_owned();
    let title = format!("Show {noun} matching tag {tag}");
    tag_link(input, ctx, vec![tag], &base, title)
}

fn link_to_add_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let (base, mut tags, noun) = tag_scope(ctx)?;
    let tag = args.at(0).to_str().into_owned();
    let title = format!("Narrow selection to {noun} matching tag {tag}");
    if !tags
        .iter()
        .any(|existing| handleize(existing) == handleize(&tag))
    {
        tags.push(tag);
    }
    tag_link(input, ctx, tags, &base, title)
}

fn link_to_remove_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let (base, mut tags, _) = tag_scope(ctx)?;
    let tag = args.at(0).to_str().into_owned();
    tags.retain(|existing| handleize(existing) != handleize(&tag));
    tag_link(input, ctx, tags, &base, format!("Remove tag {tag}"))
}

fn highlight_active_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let (_, tags, _) = tag_scope(ctx)?;
    let tag = input.to_str();
    let class = args
        .get(0)
        .map(|class| class.to_str().into_owned())
        .unwrap_or_else(|| "active".to_string());
    Ok(
        if tags
            .iter()
            .any(|existing| handleize(existing) == handleize(&tag))
        {
            Value::from(format!("<span class=\"{class}\">{tag}</span>"))
        } else {
            input.clone()
        },
    )
}

fn customer_link(input: &Value, ctx: &Context, path: &str, id: &str) -> Result<Value> {
    Ok(Value::from(format!(
        "<a href=\"{}\" id=\"{id}\">{}</a>",
        site(ctx)?.request.localized(path),
        input.to_str()
    )))
}

fn customer_login_link(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    customer_link(input, ctx, "/account/login", "customer_login_link")
}

fn customer_logout_link(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    customer_link(input, ctx, "/account/logout", "customer_logout_link")
}

fn customer_register_link(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    customer_link(input, ctx, "/account/register", "customer_register_link")
}

/// Escapes what is not allowed in a URL, keeping its reserved characters.
fn escape_url(input: &str, keep_ampersand: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => out.push(byte as char),
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' | b';' | b'/' | b'?'
            | b':' | b'@' | b'=' | b'+' | b'$' | b',' | b'#' => {
                out.push(byte as char);
            }
            b'&' if keep_ampersand => out.push('&'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn url_escape(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(Value::from(escape_url(&input.to_str(), true)))
}

fn url_param_escape(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(Value::from(escape_url(&input.to_str(), false)))
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("asset_url", asset_url);
    env.register_filter("asset_img_url", asset_img_url);
    env.register_filter("file_url", file_url);
    env.register_filter("file_img_url", file_img_url);
    env.register_filter("global_asset_url", global_asset_url);
    env.register_filter("shopify_asset_url", shopify_asset_url);
    env.register_filter("payment_type_img_url", payment_type_img_url);
    env.register_filter("link_to", link_to);
    env.register_filter("url_for_type", url_for_type);
    env.register_filter("url_for_vendor", url_for_vendor);
    env.register_filter("link_to_type", link_to_type);
    env.register_filter("link_to_vendor", link_to_vendor);
    env.register_filter("within", within);
    env.register_filter("sort_by", sort_by);
    env.register_filter("link_to_tag", link_to_tag);
    env.register_filter("link_to_add_tag", link_to_add_tag);
    env.register_filter("link_to_remove_tag", link_to_remove_tag);
    env.register_filter("highlight_active_tag", highlight_active_tag);
    env.register_filter("customer_login_link", customer_login_link);
    env.register_filter("customer_logout_link", customer_logout_link);
    env.register_filter("customer_register_link", customer_register_link);
    env.register_filter("url_escape", url_escape);
    env.register_filter("url_param_escape", url_param_escape);
}
