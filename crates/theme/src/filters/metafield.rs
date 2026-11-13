//! `metafield_tag` and `metafield_text`.

use lsf_core::money::format_money;
use lsf_liquid::filters::escape_html;
use lsf_liquid::time::strftime;
use lsf_liquid::{Context, Environment, Error, FilterArgs, Result, Value};
use serde_json::Value as Json;

use super::media::video_html;
use super::misc::to_script_safe_json;
use super::money::money_formats;
use super::site;
use crate::drops::media::{ImageDrop, MediaDrop, image_base_url};
use crate::drops::metafield::{MetafieldDrop, MoneyDrop, compact, is_measurement, measure};
use crate::site::Site;

/// The text of one (non-list) metafield value.
fn text_of(
    site: &Site,
    kind: &str,
    raw: &Json,
    value: &Value,
    args: &FilterArgs,
    ctx: &Context,
) -> String {
    match kind {
        "money" => match value.downcast::<MoneyDrop>() {
            Some(money) => format_money(
                money.cents,
                &money_formats(&site.store.shop, Some(&money.currency)).1,
            ),
            None => value.to_str().into_owned(),
        },
        // The rating alone, `4.5`, as Shopify's reference of `metafield_tag` shows it.
        "rating" => value.get("rating").to_str().into_owned(),
        kind if is_measurement(kind) => format!(
            "{} {}",
            measure(&value.get("value")),
            value.get("unit").to_str()
        ),
        "date" | "date_time" => {
            let pattern = if kind == "date" {
                "%B %-d, %Y"
            } else {
                "%b %-d, %Y, %-l:%M %P"
            };
            lsf_liquid::time::to_time(value, ctx)
                .and_then(|time| strftime(&time, pattern).ok())
                .unwrap_or_else(|| value.to_str().into_owned())
        }
        "json" => to_script_safe_json(raw),
        "product_reference"
        | "collection_reference"
        | "page_reference"
        | "variant_reference"
        | "article_reference" => value.get("title").to_str().into_owned(),
        "metaobject_reference" => match field(args) {
            Some(field) => value.get(&field).get("value").to_str().into_owned(),
            None => value.get("system").get("handle").to_str().into_owned(),
        },
        // Shopify prints nothing for these.
        kind if prints_nothing(kind) => String::new(),
        "link" => String::new(),
        // The file's URL, as the `src` of its `metafield_tag`.
        "file_reference" => match value.downcast::<ImageDrop>() {
            Some(image) => crate::urls::file_url(site, &image.image.src),
            None if value.downcast::<MediaDrop>().is_some() => compact(value),
            None => value.get("url").to_str().into_owned(),
        },
        "rich_text_field" => rich_text_plain(raw),
        _ => compact(&value.to_liquid_value()),
    }
}

/// The `field` parameter of a metaobject reference.
fn field(args: &FilterArgs) -> Option<String> {
    args.named("field").map(|field| field.to_str().into_owned())
}

/// The types `metafield_tag` and `metafield_text` print nothing for on Shopify.
fn prints_nothing(kind: &str) -> bool {
    matches!(
        kind,
        "mixed_reference" | "customer_reference" | "company_reference"
    )
}

/// The lists the filters support, as Shopify's reference says: "doesn't currently support list
/// metafields other than `list.single_line_text_field` and `list.metaobject_reference`".
fn is_supported_list(kind: &str) -> bool {
    matches!(kind, "single_line_text_field" | "metaobject_reference")
}

/// A URL as the text of its link, without its scheme: `example.com/a`, as Shopify prints it.
fn without_scheme(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, rest)| rest)
}

/// Rich text is stored either as HTML or as Shopify's rich text JSON tree.
fn rich_text_html(raw: &Json) -> String {
    fn node(value: &Json, out: &mut String) {
        let children = |out: &mut String| {
            for child in value
                .get("children")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
            {
                node(child, out);
            }
        };
        match value.get("type").and_then(Json::as_str) {
            Some("root") => children(out),
            Some("paragraph") => {
                out.push_str("<p>");
                children(out);
                out.push_str("</p>");
            }
            Some("heading") => {
                let level = value.get("level").and_then(Json::as_u64).unwrap_or(2);
                out.push_str(&format!("<h{level}>"));
                children(out);
                out.push_str(&format!("</h{level}>"));
            }
            Some("list") => {
                let tag = if value.get("listType").and_then(Json::as_str) == Some("ordered") {
                    "ol"
                } else {
                    "ul"
                };
                out.push_str(&format!("<{tag}>"));
                children(out);
                out.push_str(&format!("</{tag}>"));
            }
            Some("list-item") => {
                out.push_str("<li>");
                children(out);
                out.push_str("</li>");
            }
            Some("link") => {
                // `href`, then `target` and `title` when the link has them, as Shopify writes it.
                out.push_str("<a");
                for attribute in ["url", "target", "title"] {
                    if let Some(text) = value.get(attribute).and_then(Json::as_str) {
                        let name = if attribute == "url" {
                            "href"
                        } else {
                            attribute
                        };
                        out.push_str(&format!(" {name}=\"{}\"", escape_html(text)));
                    }
                }
                out.push('>');
                children(out);
                out.push_str("</a>");
            }
            Some("text") => {
                let mut text = escape_html(
                    value
                        .get("value")
                        .and_then(Json::as_str)
                        .unwrap_or_default(),
                );
                if value.get("bold").and_then(Json::as_bool) == Some(true) {
                    text = format!("<strong>{text}</strong>");
                }
                if value.get("italic").and_then(Json::as_bool) == Some(true) {
                    text = format!("<em>{text}</em>");
                }
                out.push_str(&text);
            }
            _ => children(out),
        }
    }
    match raw {
        Json::String(html) => html.clone(),
        tree => {
            let mut out = String::new();
            node(tree, &mut out);
            out
        }
    }
}

/// Rich text as text: the text of each block (a paragraph, a heading, a list item) on a line of
/// its own, as Shopify gives it.
fn rich_text_plain(raw: &Json) -> String {
    fn strip_tags(html: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for c in html.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                c if !in_tag => out.push(c),
                _ => {}
            }
        }
        out
    }
    fn blocks(value: &Json, out: &mut Vec<String>) {
        match value.get("type").and_then(Json::as_str) {
            Some("root" | "list") => {
                for child in value
                    .get("children")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                {
                    blocks(child, out);
                }
            }
            _ => out.push(strip_tags(&rich_text_html(value))),
        }
    }
    match raw {
        Json::String(html) => strip_tags(html),
        tree => {
            let mut out = Vec::new();
            blocks(tree, &mut out);
            out.join("\n")
        }
    }
}

fn single_tag(
    site: &std::sync::Arc<Site>,
    kind: &str,
    raw: &Json,
    value: &Value,
    args: &FilterArgs,
    ctx: &Context,
    element: &str,
) -> String {
    let class = format!("metafield-{kind}");
    let text = || escape_html(&text_of(site, kind, raw, value, args, ctx));
    match kind {
        kind if prints_nothing(kind) => String::new(),
        "language" => String::new(),
        // The field of the entry, which can only be a `single_line_text_field`.
        "metaobject_reference" => format!(
            "<{element} class=\"metafield-single_line_text_field\">{}</{element}>",
            text()
        ),
        "link" => format!(
            "<a href=\"{}\" class=\"metafield-url\">{}</a>",
            escape_html(&value.get("url").to_str()),
            escape_html(&value.get("text").to_str())
        ),
        "product_reference" | "collection_reference" | "page_reference" | "variant_reference" => {
            format!(
                "<a href=\"{}\" class=\"{class}\">{}</a>",
                escape_html(&value.get("url").to_str()),
                text()
            )
        }
        "url" => {
            let url = value.to_str();
            format!(
                "<a href=\"{}\" class=\"{class}\">{}</a>",
                escape_html(&url),
                escape_html(without_scheme(&url))
            )
        }
        // `datetime="<the metafield value>"`, as Shopify's reference of `metafield_tag` has it.
        "date" | "date_time" => {
            let datetime = lsf_liquid::time::to_time(value, ctx)
                .map(|_| escape_html(&value.to_str()))
                .unwrap_or_default();
            format!(
                "<time datetime=\"{datetime}\" class=\"{class}\">{}</time>",
                text()
            )
        }
        "json" => format!(
            "<script type=\"application/json\" class=\"{class}\">{}</script>",
            to_script_safe_json(raw)
        ),
        "multi_line_text_field" => format!(
            "<{element} class=\"{class}\">{}</{element}>",
            text().replace('\n', "<br />\n")
        ),
        "rich_text_field" => format!("<div class=\"{class}\">{}</div>", rich_text_html(raw)),
        kind if is_measurement(kind) => format!(
            "<{element} class=\"{class}\"><span class=\"{class}_value\">{} </span><span class=\"{class}_unit\">{}</span></{element}>",
            escape_html(&measure(&value.get("value"))),
            escape_html(&value.get("unit").to_str())
        ),
        "file_reference" => {
            if let Some(image) = value.downcast::<ImageDrop>() {
                format!(
                    "<img src=\"{}\" loading=\"lazy\" class=\"{class}\">",
                    escape_html(&image_base_url(site, &image.image))
                )
            } else if let Some(media) = value.downcast::<MediaDrop>() {
                // As `video_tag` prints it, without the class.
                video_html(site, media, &FilterArgs::default(), false)
            } else {
                format!(
                    "<a href=\"{}\" class=\"{class}\">{}</a>",
                    escape_html(&value.get("url").to_str()),
                    escape_html(&value.get("url").to_str())
                )
            }
        }
        _ => format!("<{element} class=\"{class}\">{}</{element}>", text()),
    }
}

fn metafield_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let Some(drop) = input.downcast::<MetafieldDrop>() else {
        return Ok(Value::empty_string());
    };
    let site = site(ctx)?;
    let kind = drop.base_kind();
    if kind == "metaobject_reference" && field(args).is_none() {
        return Err(Error::argument(format!(
            "metafield_tag requires a 'field' parameter for metafields of type '{}'",
            drop.field.kind
        )));
    }
    if drop.is_list() && !is_supported_list(kind) {
        return Ok(Value::empty_string());
    }
    let value = drop.typed_value();
    if !drop.is_list() {
        return Ok(Value::from(single_tag(
            site,
            kind,
            &drop.field.value,
            &value,
            args,
            ctx,
            "span",
        )));
    }
    let list = if args
        .named("list_format")
        .is_some_and(|format| format.to_str() == "ordered")
    {
        "ol"
    } else {
        "ul"
    };
    let raw_items = drop.field.value.as_array().cloned().unwrap_or_default();
    let items: String = value
        .items()
        .unwrap_or_default()
        .iter()
        .zip(raw_items.iter())
        .map(|(item, raw)| single_tag(site, kind, raw, item, args, ctx, "li"))
        .collect();
    // A list of metaobjects lists their field, a `single_line_text_field`.
    let class = if kind == "metaobject_reference" {
        "single_line_text_field"
    } else {
        kind
    };
    Ok(Value::from(format!(
        "<{list} class=\"metafield-{class}-array\">{items}</{list}>"
    )))
}

/// The types whose `metafield_text` is text a merchant wrote, which Shopify HTML-escapes: the
/// text fields, and the `field` of a metaobject, which can only be a `single_line_text_field`.
fn is_text(kind: &str) -> bool {
    matches!(
        kind,
        "single_line_text_field"
            | "multi_line_text_field"
            | "metaobject_reference"
            | "mixed_reference"
    )
}

fn metafield_text(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let Some(drop) = input.downcast::<MetafieldDrop>() else {
        return Ok(Value::empty_string());
    };
    let site = site(ctx)?;
    let kind = drop.base_kind();
    if drop.is_list() && !is_supported_list(kind) {
        return Ok(Value::empty_string());
    }
    let value = drop.typed_value();
    // Text is HTML-escaped, as `metafield_tag` escapes it: `Tom &amp; &quot;Jerry&quot;`.
    let text = |raw: &Json, value: &Value| {
        let text = text_of(site, kind, raw, value, args, ctx);
        if is_text(kind) {
            escape_html(&text)
        } else {
            text
        }
    };
    if !drop.is_list() {
        return Ok(Value::from(text(&drop.field.value, &value)));
    }
    let raw_items = drop.field.value.as_array().cloned().unwrap_or_default();
    let texts: Vec<String> = value
        .items()
        .unwrap_or_default()
        .iter()
        .zip(raw_items.iter())
        .map(|(item, raw)| text(raw, item))
        .collect();
    // An English sentence: "a", "a and b", "a, b, and c".
    Ok(Value::from(match texts.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [init @ .., last] => format!("{}, and {last}", init.join(", ")),
    }))
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("metafield_tag", metafield_tag);
    env.register_filter("metafield_text", metafield_text);
}
