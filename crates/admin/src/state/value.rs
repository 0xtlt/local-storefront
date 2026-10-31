//! The values records hold, written as the Admin API writes them: money, dates, images,
//! addresses, ids that lsf-core does not give.

use chrono::{DateTime, Utc};
use lsf_core::store::{Address, Image};
use serde_json::{Map, Value, json};

/// Cents as the `Money` scalar: `"19.99"`, `"20.00"` (as `ProductVariant.price` reads).
pub fn money(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let cents = cents.unsigned_abs();
    format!("{sign}{}.{:02}", cents / 100, cents % 100)
}

/// Cents as the `Decimal` scalar: `"19.99"`, `"20.0"`, `"19.5"` (as `MoneyV2.amount` reads).
pub fn decimal(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let (whole, fraction) = (cents.unsigned_abs() / 100, cents.unsigned_abs() % 100);
    match fraction {
        0 => format!("{sign}{whole}.0"),
        f if f % 10 == 0 => format!("{sign}{whole}.{}", f / 10),
        f => format!("{sign}{whole}.{f:02}"),
    }
}

/// A `MoneyV2`.
pub fn money_v2(cents: i64, currency: &str) -> Value {
    json!({"amount": decimal(cents), "currencyCode": currency})
}

/// A `MoneyBag`: the shop and presentment amounts, the same since lsf has one currency.
pub fn money_bag(cents: i64, currency: &str) -> Value {
    json!({
        "shopMoney": money_v2(cents, currency),
        "presentmentMoney": money_v2(cents, currency),
    })
}

/// A `DateTime`: RFC 3339 in UTC, to the second, with `Z`.
pub fn date_time(date: DateTime<Utc>) -> String {
    date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// A `Date`: `2026-01-31`.
pub fn date(date: DateTime<Utc>) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// A string, or `null` when it is empty.
pub fn text(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::String(value.to_string())
    }
}

/// An optional string, `null` when missing or empty.
pub fn optional(value: Option<&str>) -> Value {
    text(value.unwrap_or_default())
}

/// A JSON object from `json!`, as a [`super::Record`] or a nested object.
pub fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// The URL of a file of `files/` as lsf serves it, on the shop's origin
/// (`https://<primary domain>/cdn/shop/files/<path>?v=<version>`, the path the storefront serves
/// files at). An absolute URL stays as it is.
pub fn file_url(origin: &str, src: &str) -> String {
    if src.starts_with("http://") || src.starts_with("https://") {
        return src.to_string();
    }
    format!(
        "{origin}/cdn/shop/files/{}?v={}",
        lsf_core::urls::encode_path(src),
        lsf_core::urls::version_of(src)
    )
}

/// An `Image`: `id` is `gid://shopify/<id_type>/<image id>` (`ImageSource` for media,
/// `CollectionImage`, `ArticleImage`).
pub fn image(origin: &str, image: &Image, id_type: &str) -> Value {
    json!({
        "id": super::gid(id_type, image.id),
        "url": file_url(origin, &image.src),
        "altText": text(&image.alt),
        "width": image.width,
        "height": image.height,
    })
}

/// The lines of an address as `formatted` lists them: street, city line, country.
pub fn formatted(address: &Address) -> Vec<String> {
    let city_line = [
        address.city.as_str(),
        address.province_code.as_str(),
        address.zip.as_str(),
    ]
    .iter()
    .filter(|part| !part.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join(" ");
    let mut lines = Vec::new();
    let names = [address.first_name.as_str(), address.last_name.as_str()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    for line in [
        names.as_str(),
        address.company.as_str(),
        address.address1.as_str(),
        address.address2.as_str(),
        city_line.as_str(),
        address.country.as_str(),
    ] {
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

/// The city, province and country of an address: `formattedArea`.
pub fn formatted_area(address: &Address) -> Value {
    let area = [
        address.city.as_str(),
        address.province_code.as_str(),
        address.country.as_str(),
    ]
    .iter()
    .filter(|part| !part.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join(", ");
    text(&area)
}

/// The full name of an address, `null` when it has none.
pub fn full_name(first: &str, last: &str) -> Value {
    let name = [first, last]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    text(&name)
}

/// A `MailingAddress` with its id: `gid://shopify/MailingAddress/<address id>?model_name=<model>`
/// as Shopify writes it (`CustomerAddress` for the address book of a customer, `Address` for
/// the addresses of an order).
pub fn mailing_address(address: &Address, model: &str) -> Value {
    json!({
        "id": format!("gid://shopify/MailingAddress/{}?model_name={model}", address.id),
        "firstName": text(&address.first_name),
        "lastName": text(&address.last_name),
        "name": full_name(&address.first_name, &address.last_name),
        "company": text(&address.company),
        "address1": text(&address.address1),
        "address2": text(&address.address2),
        "city": text(&address.city),
        "province": text(&address.province),
        "provinceCode": text(&address.province_code),
        "country": text(&address.country),
        "countryCodeV2": text(&address.country_code.to_ascii_uppercase()),
        "zip": text(&address.zip),
        "phone": text(&address.phone),
        "formatted": formatted(address),
        "formattedArea": formatted_area(address),
    })
}

/// The text of some HTML, as `description` and `bodySummary` give it: the tags dropped, the
/// common entities decoded, the white space collapsed.
pub fn plain_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' if in_tag => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A name for a handle or a key: `care-guide` → `Care guide`.
pub fn humanize(key: &str) -> String {
    let words = key
        .split(['-', '_', ' ', '.'])
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A stable id in the format of a UUID (version 4, variant 1) made from `key`: the ids Shopify
/// writes as UUIDs (Function ids, event ids), the same for the same key every run.
pub fn uuid(key: &str) -> String {
    let high = fnv(0xcbf2_9ce4_8422_2325, key);
    let low = fnv(0x8422_2325_cbf2_9ce4, key);
    let high = (high & 0xffff_ffff_ffff_0fff) | 0x0000_0000_0000_4000;
    let low = (low & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xffff,
        high & 0xffff,
        low >> 48,
        low & 0xffff_ffff_ffff
    )
}

fn fnv(seed: u64, key: &str) -> u64 {
    key.bytes().fold(seed, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_money_as_shopify() {
        assert_eq!(money(1999), "19.99");
        assert_eq!(money(2000), "20.00");
        assert_eq!(money(5), "0.05");
        assert_eq!(money(-150), "-1.50");
        assert_eq!(decimal(1999), "19.99");
        assert_eq!(decimal(2000), "20.0");
        assert_eq!(decimal(1950), "19.5");
        assert_eq!(decimal(0), "0.0");
    }

    #[test]
    fn writes_text() {
        assert_eq!(
            plain_text("<p>Soft &amp; warm.</p>\n<p>Wash cold.</p>"),
            "Soft & warm. Wash cold."
        );
        assert_eq!(humanize("care-guide"), "Care guide");
        assert_eq!(humanize("shoe_size"), "Shoe size");
        let id = uuid("function/discount");
        assert_eq!(id, uuid("function/discount"));
        assert_ne!(id, uuid("function/other"));
        assert_eq!(&id[14..15], "4");
        assert_eq!(id.len(), 36);
    }
}
