//! `metafield_tag` and `metafield_text`.

use serde_json::Value as Json;
use slt_liquid::filters::escape_html;
use slt_liquid::number::float_to_s;
use slt_liquid::time::strftime;
use slt_liquid::{Context, Environment, FilterArgs, Result, Value};

use super::misc::to_script_safe_json;
use super::money::format_money;
use super::site;
use crate::drops::media::{ImageDrop, image_base_url};
use crate::drops::metafield::MetafieldDrop;
use crate::site::Site;

fn compact(value: &Value) -> String {
    match value {
        Value::Float(f) => {
            let text = float_to_s(*f);
            text.strip_suffix(".0").map(str::to_string).unwrap_or(text)
        }
        other => other.to_str().into_owned(),
    }
}

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
        "money" => match value {
            Value::Int(cents) => format_money(*cents, &site.store.shop.money_with_currency_format),
            other => other.to_str().into_owned(),
        },
        "rating" => format!(
            "{} / {}",
            float_to_s(number(&value.get("rating"))),
            float_to_s(number(&value.get("scale_max")))
        ),
        "weight" | "volume" | "dimension" => format!(
            "{} {}",
            compact(&value.get("value")),
            value.get("unit").to_str()
        ),
        "date" | "date_time" => {
            let pattern = if kind == "date" {
                "%B %-d, %Y"
            } else {
                "%b %-d, %Y, %-l:%M %P"
            };
            slt_liquid::time::to_time(value, ctx)
                .and_then(|time| strftime(&time, pattern).ok())
                .unwrap_or_else(|| value.to_str().into_owned())
        }
        "json" => to_script_safe_json(raw),
        "product_reference" | "collection_reference" | "page_reference" | "variant_reference" => {
            value.get("title").to_str().into_owned()
        }
        "metaobject_reference" | "mixed_reference" => {
            let field = args.named("field").map(|field| field.to_str().into_owned());
            match field {
                Some(field) => value.get(&field).get("value").to_str().into_owned(),
                None => value.get("system").get("handle").to_str().into_owned(),
            }
        }
        "rich_text_field" => rich_text_plain(raw),
        _ => compact(value),
    }
}

fn number(value: &Value) -> f64 {
    slt_liquid::number::to_number(value).to_f64()
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
                out.push_str(&format!(
                    "<a href=\"{}\">",
                    escape_html(value.get("url").and_then(Json::as_str).unwrap_or_default())
                ));
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

fn rich_text_plain(raw: &Json) -> String {
    let html = rich_text_html(raw);
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
        "product_reference" | "collection_reference" | "page_reference" => {
            format!(
                "<a href=\"{}\" class=\"{class}\">{}</a>",
                escape_html(&value.get("url").to_str()),
                text()
            )
        }
        "url" => format!(
            "<a href=\"{0}\" class=\"{class}\">{0}</a>",
            escape_html(&value.to_str())
        ),
        "date" | "date_time" => {
            let datetime = slt_liquid::time::to_time(value, ctx)
                .map(|time| {
                    if kind == "date" {
                        time.format("%Y-%m-%d").to_string()
                    } else {
                        time.with_timezone(&chrono::Utc)
                            .format("%Y-%m-%dT%H:%M:%SZ")
                            .to_string()
                    }
                })
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
        "weight" | "volume" | "dimension" => format!(
            "<{element} class=\"{class}\"><span class=\"{class}_value\">{} </span><span class=\"{class}_unit\">{}</span></{element}>",
            escape_html(&compact(&value.get("value"))),
            escape_html(&value.get("unit").to_str())
        ),
        "file_reference" => match value.downcast::<ImageDrop>() {
            Some(image) => format!(
                "<img src=\"{}\" loading=\"lazy\" class=\"{class}\">",
                escape_html(&image_base_url(site, &image.image))
            ),
            None => format!(
                "<a href=\"{}\" class=\"{class}\">{}</a>",
                escape_html(&value.get("url").to_str()),
                escape_html(&value.get("url").to_str())
            ),
        },
        _ => format!("<{element} class=\"{class}\">{}</{element}>", text()),
    }
}

fn metafield_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let Some(drop) = input.downcast::<MetafieldDrop>() else {
        return Ok(Value::empty_string());
    };
    let site = site(ctx)?;
    let kind = drop.base_kind();
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
    Ok(Value::from(format!(
        "<{list} class=\"metafield-{kind}-array\">{items}</{list}>"
    )))
}

fn metafield_text(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let Some(drop) = input.downcast::<MetafieldDrop>() else {
        return Ok(Value::empty_string());
    };
    let site = site(ctx)?;
    let kind = drop.base_kind();
    let value = drop.typed_value();
    if !drop.is_list() {
        return Ok(Value::from(text_of(
            site,
            kind,
            &drop.field.value,
            &value,
            args,
            ctx,
        )));
    }
    let raw_items = drop.field.value.as_array().cloned().unwrap_or_default();
    let texts: Vec<String> = value
        .items()
        .unwrap_or_default()
        .iter()
        .zip(raw_items.iter())
        .map(|(item, raw)| text_of(site, kind, raw, item, args, ctx))
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
