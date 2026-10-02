//! JSON, hashing and string helpers specific to Shopify.

use hmac::{Hmac, KeyInit, Mac};
use lsf_liquid::{Context, Environment, FilterArgs, Result, Value};
use md5::Md5;
use serde_json::{Value as Json, json};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use super::site;
use crate::drops::cart::{CartDrop, cart_token, line_key, resolve_lines};
use crate::drops::collection::CollectionDrop;
use crate::drops::content::ArticleDrop;
use crate::drops::product::ProductDrop;
use crate::util;

/// Serializes JSON the way Shopify's `json` filter does: compact, with the characters that are
/// unsafe inside a `<script>` element escaped.
pub fn to_script_safe_json(value: &Json) -> String {
    let raw = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    let mut out = String::with_capacity(raw.len() + 16);
    for c in raw.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '/' => out.push_str("\\/"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

fn json_filter(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(Value::from(to_script_safe_json(&input.to_json())))
}

fn handleize(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(Value::from(util::handleize(&input.to_str())))
}

/// `variable-name` → `VariableName`.
fn camelize(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let text = input.to_str();
    let mut out = String::with_capacity(text.len());
    let mut upper = true;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if upper {
                out.extend(c.to_uppercase());
                upper = false;
            } else {
                out.push(c);
            }
        } else {
            upper = true;
        }
    }
    Ok(Value::from(out))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn md5(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(Value::from(hex(&Md5::digest(input.to_str().as_bytes()))))
}

fn sha1(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(Value::from(hex(&Sha1::digest(input.to_str().as_bytes()))))
}

fn sha256(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(Value::from(hex(&Sha256::digest(input.to_str().as_bytes()))))
}

fn hmac_sha1(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let mut mac = Hmac::<Sha1>::new_from_slice(args.at(0).to_str().as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(input.to_str().as_bytes());
    Ok(Value::from(hex(&mac.finalize().into_bytes())))
}

fn hmac_sha256(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let mut mac = Hmac::<Sha256>::new_from_slice(args.at(0).to_str().as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(input.to_str().as_bytes());
    Ok(Value::from(hex(&mac.finalize().into_bytes())))
}

fn blake3(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(Value::from(
        ::blake3::hash(input.to_str().as_bytes())
            .to_hex()
            .to_string(),
    ))
}

fn money_string(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents.abs() % 100)
}

/// schema.org structured data for a product or an article.
fn structured_data(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let site = site(ctx)?;
    let origin = site.request.origin();
    if let Some(drop) = input.downcast::<ProductDrop>() {
        let product = drop.product();
        let url = drop.url();
        let image = product.images().next().map(|image| {
            format!(
                "{}:{}&width=1920",
                site.request.scheme,
                crate::drops::media::image_base_url(site, image)
            )
        });
        let offer = |variant: &crate::store::Variant| {
            json!({
                "@id": format!("{url}?variant={}#offer", variant.id),
                "@type": "Offer",
                "availability": if variant.available { "http://schema.org/InStock" } else { "http://schema.org/OutOfStock" },
                "price": money_string(variant.price),
                "priceCurrency": site.currency(),
                "url": format!("{origin}{url}?variant={}", variant.id),
            })
        };
        let data = if product.variants.len() == 1 {
            json!({
                "@context": "http://schema.org/",
                "@id": format!("{url}#product"),
                "@type": "Product",
                "brand": { "@type": "Brand", "name": product.vendor },
                "category": product.product_type,
                "description": plain_text(&product.description),
                "image": image,
                "name": product.title,
                "offers": offer(&product.variants[0]),
                "url": format!("{origin}{url}"),
            })
        } else {
            json!({
                "@context": "http://schema.org/",
                "@id": format!("{url}#product"),
                "@type": "ProductGroup",
                "brand": { "@type": "Brand", "name": product.vendor },
                "category": product.product_type,
                "description": plain_text(&product.description),
                "hasVariant": product.variants.iter().map(|variant| json!({
                    "@id": format!("{url}?variant={}#variant", variant.id),
                    "@type": "Product",
                    "image": image,
                    "name": format!("{} - {}", product.title, variant.title),
                    "offers": offer(variant),
                    "sku": variant.sku,
                })).collect::<Vec<_>>(),
                "name": product.title,
                "productGroupID": product.id.to_string(),
                "url": format!("{origin}{url}"),
            })
        };
        return Ok(Value::from(to_script_safe_json(&data)));
    }
    if let Some(drop) = input.downcast::<ArticleDrop>() {
        let get = |key: &str| lsf_liquid::Object::get(drop, key).unwrap_or(Value::Nil);
        let data = json!({
            "@context": "http://schema.org/",
            "@id": format!("{}#article", drop.url()),
            "@type": "Article",
            "mainEntityOfPage": { "@type": "WebPage", "@id": format!("{origin}{}", drop.url()) },
            "articleBody": plain_text(&get("content").to_str()),
            "headline": get("title").to_str(),
            "description": plain_text(&get("excerpt_or_content").to_str()),
            "datePublished": get("published_at").to_json(),
            "dateModified": get("updated_at").to_json(),
            "author": { "@type": "Person", "name": get("author").to_str() },
            "publisher": { "@type": "Organization", "name": site.store.shop.name },
        });
        return Ok(Value::from(to_script_safe_json(&data)));
    }
    Ok(Value::empty_string())
}

/// Plain text from HTML, for structured data.
fn plain_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
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

/// `format_code`: a gift card code in groups of four, `WCGX-7X97-G74J-GDGC`.
fn format_code(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let code: Vec<char> = input.to_str().chars().filter(|c| *c != '-').collect();
    let groups: Vec<String> = code.chunks(4).map(|group| group.iter().collect()).collect();
    Ok(Value::from(groups.join("-")))
}

/// A price as standard events carry it: a decimal amount and its currency.
fn event_price(cents: i64, currency: &str) -> Json {
    json!({
        "amount": format!("{}.{:02}", cents / 100, (cents % 100).abs()),
        "currencyCode": currency,
    })
}

/// `standard_event_data`: the payload of a standard storefront event about a product, a
/// collection or the cart, for the `view-event-payload` attribute of a view event element.
fn standard_event_data(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let site = site(ctx)?;
    let currency = site.currency();
    let context = args
        .named("context")
        .filter(|context| !context.is_nil())
        .map_or(Json::Null, |context| json!(context.to_str()));
    let mut data = serde_json::Map::new();

    if let Some(drop) = input.downcast::<ProductDrop>() {
        let product = drop.product();
        let selected = drop
            .selected_variant()
            .map(|index| &product.variants[index]);
        let options_of = |variant: &crate::store::Variant| -> Vec<Json> {
            product
                .options
                .iter()
                .zip(&variant.options)
                .map(|(option, value)| json!({"name": option.name, "value": value}))
                .collect()
        };
        data.insert(
            "product".to_string(),
            json!({
                "id": product.id,
                "title": product.title,
                "handle": product.handle,
                "selectedVariant": selected.map(|variant| json!({
                    "id": variant.id,
                    "title": variant.title,
                    "availableForSale": variant.available,
                    "price": event_price(variant.price, currency),
                    "selectedOptions": options_of(variant),
                })),
            }),
        );
        data.insert("context".to_string(), context);
        data.insert(
            "selectedOptions".to_string(),
            Json::Array(selected.map(options_of).unwrap_or_default()),
        );
    } else if let Some(drop) = input.downcast::<CollectionDrop>() {
        let collection = drop.collection();
        data.insert(
            "collection".to_string(),
            json!({
                // The catalog (`collections.all`) is not a collection of the store.
                "id": if collection.handle == "all" { Json::Null } else { json!(collection.id) },
                "handle": collection.handle,
                "productsCount": drop.product_indexes().len(),
            }),
        );
    } else if input.downcast::<CartDrop>().is_some() {
        data.insert("context".to_string(), context);
        let cart = if site.session.has_cart() {
            let lines = resolve_lines(site);
            let total: i64 = lines.iter().map(|line| line.line_price()).sum();
            json!({
                "id": cart_token(site),
                "totalQuantity": lines.iter().map(|line| u64::from(line.line.quantity)).sum::<u64>(),
                "cost": {"totalAmount": event_price(total, currency)},
                "lines": lines.iter().map(|line| json!({
                    "id": line_key(line.line),
                    "quantity": line.line.quantity,
                    "cost": {"totalAmount": event_price(line.line_price(), currency)},
                })).collect::<Vec<_>>(),
                "discountCodes": [],
            })
        } else {
            // Shopify only creates a cart when something goes into it.
            Json::Null
        };
        data.insert("cart".to_string(), cart);
    } else {
        return Ok(Value::Nil);
    }
    Ok(Value::from(to_script_safe_json(&Json::Object(data))))
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("json", json_filter);
    env.register_filter("handleize", handleize);
    env.register_filter("handle", handleize);
    env.register_filter("camelize", camelize);
    env.register_filter("camelcase", camelize);
    env.register_filter("md5", md5);
    env.register_filter("sha1", sha1);
    env.register_filter("sha256", sha256);
    env.register_filter("hmac_sha1", hmac_sha1);
    env.register_filter("hmac_sha256", hmac_sha256);
    env.register_filter("blake3", blake3);
    env.register_filter("format_code", format_code);
    env.register_filter("structured_data", structured_data);
    env.register_filter("standard_event_data", standard_event_data);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_json_for_scripts() {
        let escaped = to_script_safe_json(&json!({"url": "//a/b?x=1&y=2", "html": "<b>"}));
        let expected = [
            "{\"url\":\"",
            "\\",
            "/",
            "\\",
            "/a",
            "\\",
            "/b?x=1",
            "\\",
            "u0026y=2\",\"html\":\"",
            "\\",
            "u003cb",
            "\\",
            "u003e\"}",
        ]
        .concat();
        assert_eq!(escaped, expected);
    }
}
