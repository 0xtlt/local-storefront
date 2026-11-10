//! The values records hold, written as the Admin API writes them: money, dates, images,
//! addresses, ids that lsf-core does not give.

use chrono::{DateTime, Utc};
use lsf_core::store::{Address, Image};
use serde_json::{Map, Value, json};

/// The bound of a money input, in cents: an amount must be below 10^13 in the currency's unit
/// (ten trillion), either sign.
///
/// Shopify publishes no limit on its `Money` and `Decimal` amounts: shopify.dev documents none
/// for the scalars or for the inputs that take them. This one is lsf's, chosen so that what an
/// order, a refund or a balance adds or multiplies of such amounts stays far within the 64 bits
/// of cents lsf counts in.
pub const MAX_MONEY_CENTS: i64 = 1_000_000_000_000_000;

/// [`MAX_MONEY_CENTS`] in the currency's unit, as the message of an amount past it writes it.
pub const MAX_MONEY_TEXT: &str = "10000000000000";

/// Whether an amount in cents is within [`MAX_MONEY_CENTS`], either sign.
pub fn money_in_bounds(cents: i64) -> bool {
    cents
        .checked_abs()
        .is_some_and(|cents| cents < MAX_MONEY_CENTS)
}

/// The message of an amount past [`MAX_MONEY_CENTS`] (`"Price must be less than
/// 10000000000000"`): lsf's, in the wording of Rails' numericality validation.
pub fn money_too_large(name: &str) -> String {
    format!("{name} must be less than {MAX_MONEY_TEXT}")
}

/// A decimal amount (`"19.99"`, `"20"`, `"0.125"`) in cents, rounded half away from zero.
/// `None` when it is not a number.
pub fn parse_cents(text: &str) -> Option<i64> {
    let text = text.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (units, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if (units.is_empty() && fraction.is_empty())
        || !units
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let units: i64 = if units.is_empty() {
        0
    } else {
        units.parse().ok()?
    };
    let digit = |index: usize| {
        fraction
            .as_bytes()
            .get(index)
            .map_or(0, |byte| i64::from(byte - b'0'))
    };
    // `None` past what 64 bits of cents hold, rather than an overflow.
    let cents = units
        .checked_mul(100)?
        .checked_add(digit(0) * 10 + digit(1) + i64::from(digit(2) >= 5))?;
    Some(if negative { -cents } else { cents })
}

/// The cents of an amount as the records and the inputs write it: a `MoneyBag` (its shop
/// money), a `MoneyV2` or `MoneyInput` (its `amount`), a `Money` or `Decimal` string, a number.
/// 0 when there is none.
pub fn cents(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Object(map)) => match map.get("shopMoney") {
            Some(shop) => cents(Some(shop)),
            None => cents(map.get("amount")),
        },
        Some(Value::String(text)) => parse_cents(text).unwrap_or(0),
        Some(Value::Number(number)) => number
            .as_f64()
            .map_or(0, |amount| (amount * 100.0).round() as i64),
        _ => 0,
    }
}

/// The cents of a `Money` or `Decimal` string with at most two decimals, as lsf-core reads the
/// prices of the store data: `None` for anything else (a number, a third decimal, a space). No
/// rounding, unlike [`parse_cents`].
pub fn exact_cents(value: Option<&Value>) -> Option<i64> {
    let text = value?.as_str()?;
    lsf_core::store::model::Money::Decimal(text.to_string()).cents()
}

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

/// A `DateTime` as the records and the inputs write it (RFC 3339, any offset), in UTC: `None`
/// when it is not one.
pub fn parse_date_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// [`parse_date_time`] of a field or an argument: `None` when it is missing or not a string.
pub fn date_time_of(value: Option<&Value>) -> Option<DateTime<Utc>> {
    value.and_then(Value::as_str).and_then(parse_date_time)
}

/// A string field of a record or an input: `None` when it is missing or not a string.
pub fn str_field<'r>(record: &'r Map<String, Value>, field: &str) -> Option<&'r str> {
    record.get(field).and_then(Value::as_str)
}

/// [`str_field`], `""` when it is missing or not a string.
pub fn str_field_or_empty<'r>(record: &'r Map<String, Value>, field: &str) -> &'r str {
    str_field(record, field).unwrap_or_default()
}

/// [`str_field`], `None` also when it is empty.
pub fn non_empty_str_field<'r>(record: &'r Map<String, Value>, field: &str) -> Option<&'r str> {
    str_field(record, field).filter(|text| !text.is_empty())
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
    fn reads_amounts_in_cents() {
        // `parse_cents`: a decimal of any length, rounded half away from zero at the cent.
        assert_eq!(parse_cents("19.99"), Some(1999));
        assert_eq!(parse_cents("20"), Some(2000));
        assert_eq!(parse_cents(" 2.5 "), Some(250));
        assert_eq!(parse_cents("1."), Some(100));
        assert_eq!(parse_cents(".5"), Some(50));
        assert_eq!(parse_cents("0.125"), Some(13));
        assert_eq!(parse_cents("0.1249"), Some(12));
        assert_eq!(parse_cents("1.005"), Some(101));
        assert_eq!(parse_cents("-5.5"), Some(-550));
        assert_eq!(parse_cents("-0.125"), Some(-13));
        assert_eq!(parse_cents("-0.124"), Some(-12));
        assert_eq!(parse_cents("+1"), None);
        assert_eq!(parse_cents("-"), None);
        assert_eq!(parse_cents("."), None);
        assert_eq!(parse_cents("1e2"), None);
        assert_eq!(parse_cents("x"), None);
        assert_eq!(parse_cents(""), None);
        // Past what 64 bits of cents hold: `None`, not an overflow.
        assert_eq!(parse_cents("92233720368547758.07"), Some(i64::MAX));
        assert_eq!(parse_cents("-92233720368547758.07"), Some(-i64::MAX));
        assert_eq!(parse_cents("92233720368547758.09"), None);
        assert_eq!(parse_cents("92233720368547758.075"), None);
        // `cents`: any shape an amount comes in, a number rounded as a float, 0 for none.
        assert_eq!(cents(Some(&json!({"shopMoney": {"amount": "12.5"}}))), 1250);
        assert_eq!(
            cents(Some(&json!({"amount": "3", "currencyCode": "EUR"}))),
            300
        );
        assert_eq!(cents(Some(&json!("-0.125"))), -13);
        assert_eq!(cents(Some(&json!(0.125))), 13);
        assert_eq!(cents(Some(&json!(-0.125))), -13);
        assert_eq!(cents(Some(&json!(1.005))), 100);
        assert_eq!(cents(Some(&json!(7))), 700);
        assert_eq!(cents(Some(&json!("x"))), 0);
        assert_eq!(cents(Some(&json!(true))), 0);
        assert_eq!(cents(None), 0);
        // `exact_cents`: two decimals at most, as written, no rounding.
        assert_eq!(exact_cents(Some(&json!("19.99"))), Some(1999));
        assert_eq!(exact_cents(Some(&json!("19.9"))), Some(1990));
        assert_eq!(exact_cents(Some(&json!("19"))), Some(1900));
        assert_eq!(exact_cents(Some(&json!("1."))), Some(100));
        assert_eq!(exact_cents(Some(&json!("-5.5"))), Some(-550));
        assert_eq!(exact_cents(Some(&json!("-0.05"))), Some(-5));
        assert_eq!(exact_cents(Some(&json!("0.125"))), None);
        assert_eq!(exact_cents(Some(&json!(".5"))), None);
        assert_eq!(exact_cents(Some(&json!(" 1"))), None);
        assert_eq!(exact_cents(Some(&json!(19.99))), None);
        assert_eq!(exact_cents(None), None);
    }

    #[test]
    fn reads_date_times_in_utc() {
        let at = parse_date_time("2024-09-09T12:48:33-04:00").unwrap();
        assert_eq!(date_time(at), "2024-09-09T16:48:33Z");
        assert_eq!(parse_date_time(" 2024-09-09T12:48:33Z"), None);
        assert_eq!(parse_date_time("2024-09-09"), None);
        assert_eq!(
            date_time_of(Some(&json!("2024-09-09T12:48:33Z"))),
            Some(at - chrono::Duration::hours(4))
        );
        assert_eq!(date_time_of(Some(&json!(1))), None);
        assert_eq!(date_time_of(None), None);
    }

    #[test]
    fn reads_string_fields() {
        let record = object(json!({"title": "Shirt", "empty": "", "count": 2}));
        assert_eq!(str_field(&record, "title"), Some("Shirt"));
        assert_eq!(str_field(&record, "empty"), Some(""));
        assert_eq!(str_field(&record, "count"), None);
        assert_eq!(str_field_or_empty(&record, "count"), "");
        assert_eq!(str_field_or_empty(&record, "missing"), "");
        assert_eq!(non_empty_str_field(&record, "empty"), None);
        assert_eq!(non_empty_str_field(&record, "title"), Some("Shirt"));
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
