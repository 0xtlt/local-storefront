//! The types of metafields, and what a value of each type must look like.
//!
//! A typed metafield of the data (`{"type": "number_integer", "value": 2}`) is checked against
//! its type when the store is built: a value Shopify would refuse is an error, with what the
//! type takes. References are only collected here: they are resolved once every entity they can
//! name is built.
//! https://shopify.dev/docs/apps/build/custom-data/metafields/list-of-data-types

use serde_json::Value as Json;

/// The measurements: a value and a unit.
pub const MEASUREMENTS: [&str; 32] = [
    "antenna_gain",
    "area",
    "battery_charge_capacity",
    "battery_energy_capacity",
    "capacitance",
    "concentration",
    "data_storage_capacity",
    "data_transfer_rate",
    "dimension",
    "display_density",
    "distance",
    "duration",
    "electric_current",
    "electrical_resistance",
    "energy",
    "frequency",
    "illuminance",
    "inductance",
    "luminous_flux",
    "mass_flow_rate",
    "power",
    "pressure",
    "resolution",
    "rotational_speed",
    "sound_level",
    "speed",
    "temperature",
    "thermal_power",
    "voltage",
    "volume",
    "volumetric_flow_rate",
    "weight",
];

/// The other types of Shopify, which can also be lists (`list.<type>`).
const TYPES: [&str; 27] = [
    "article_reference",
    "boolean",
    "collection_reference",
    "color",
    "company_reference",
    "customer_reference",
    "date",
    "date_time",
    "file_reference",
    "id",
    "json",
    "language",
    "link",
    "metaobject_reference",
    "mixed_reference",
    "money",
    "multi_line_text_field",
    "number_decimal",
    "number_integer",
    "page_reference",
    "product_reference",
    "product_taxonomy_value_reference",
    "rating",
    "rich_text_field",
    "single_line_text_field",
    "url",
    "variant_reference",
];

/// The types that have no list: one value only.
const SINGLE_ONLY: [&str; 5] = ["boolean", "json", "language", "money", "rich_text_field"];

/// Whether a type is a measurement (`weight`, `dimension`, …).
pub fn is_measurement(kind: &str) -> bool {
    MEASUREMENTS.contains(&kind)
}

/// The units Shopify takes for the measurements whose units are few: the names its docs list
/// (`kilograms`), and the short forms (`kg`) older data and lsf's own examples use.
fn units(kind: &str) -> Option<&'static [&'static str]> {
    match kind {
        "weight" => Some(&[
            "grams",
            "kilograms",
            "ounces",
            "pounds",
            "g",
            "kg",
            "lb",
            "oz",
        ]),
        "dimension" => Some(&[
            "millimeters",
            "centimeters",
            "meters",
            "inches",
            "feet",
            "yards",
            "mm",
            "cm",
            "m",
            "in",
            "ft",
            "yd",
        ]),
        "volume" => Some(&[
            "milliliters",
            "centiliters",
            "liters",
            "cubic_meters",
            "us_fluid_ounces",
            "us_pints",
            "us_quarts",
            "us_gallons",
            "imperial_fluid_ounces",
            "imperial_pints",
            "imperial_quarts",
            "imperial_gallons",
            "ml",
            "cl",
            "l",
            "m3",
            "us_fl_oz",
            "us_pt",
            "us_qt",
            "us_gal",
            "imp_fl_oz",
            "imp_pt",
            "imp_qt",
            "imp_gal",
        ]),
        _ => None,
    }
}

/// Every type name, lists included, for "did you mean" hints.
pub fn all_types() -> Vec<String> {
    let mut all: Vec<String> = TYPES
        .iter()
        .chain(MEASUREMENTS.iter())
        .map(|kind| (*kind).to_string())
        .collect();
    for kind in TYPES.iter().chain(MEASUREMENTS.iter()) {
        if !SINGLE_ONLY.contains(kind) {
            all.push(format!("list.{kind}"));
        }
    }
    all
}

/// Whether `kind` is a type of Shopify.
pub fn is_known(kind: &str) -> bool {
    match kind.strip_prefix("list.") {
        Some(item) => {
            (TYPES.contains(&item) || is_measurement(item)) && !SINGLE_ONLY.contains(&item)
        }
        None => TYPES.contains(&kind) || is_measurement(kind),
    }
}

/// A reference a metafield value makes to another entity of the data, resolved once the store
/// is built.
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    /// The type of the item: `product_reference`, `variant_reference`, …
    pub kind: String,
    pub value: Json,
    /// The JSON pointer of the value, below the metafield's own (`/value` or `/value/2`).
    pub pointer: String,
}

/// What is wrong with a value: a message and what to write instead.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    /// Below the metafield's own pointer: `/type`, `/value`, `/value/1`, `/value/unit`.
    pub pointer: String,
    pub code: &'static str,
    pub message: String,
    pub hint: String,
}

/// Checks a value against its type. Answers what is wrong with it, and the references it makes.
pub fn check(kind: &str, value: &Json) -> (Vec<Problem>, Vec<Reference>) {
    let mut problems = Vec::new();
    let mut references = Vec::new();
    if !is_known(kind) {
        let all = all_types();
        let hint = match crate::util::closest_match(kind, all.iter().map(String::as_str)) {
            Some(close) => format!("did you mean \"{close}\"?"),
            None => "use a type of Shopify: single_line_text_field, number_integer, boolean, \
                     product_reference, list.product_reference, ... (data-format.md#metafields-and-metaobjects)"
                .to_string(),
        };
        problems.push(Problem {
            pointer: "/type".to_string(),
            code: "unknown_metafield_type",
            message: format!("\"{kind}\" is not a metafield type"),
            hint,
        });
        return (problems, references);
    }
    match kind.strip_prefix("list.") {
        Some(item) => match value {
            Json::Array(items) => {
                for (index, item_value) in items.iter().enumerate() {
                    check_one(
                        item,
                        item_value,
                        &format!("/value/{index}"),
                        &mut problems,
                        &mut references,
                    );
                }
            }
            other => problems.push(Problem {
                pointer: "/value".to_string(),
                code: "invalid_metafield",
                message: format!("a {kind} is a list, not {}", describe(other)),
                hint: format!("write the values in a JSON array: [{}]", example(item)),
            }),
        },
        None => check_one(kind, value, "/value", &mut problems, &mut references),
    }
    (problems, references)
}

fn describe(value: &Json) -> String {
    let kind = match value {
        Json::Null => "null",
        Json::Bool(_) => "a boolean",
        Json::Number(number) if number.is_f64() => "a decimal number",
        Json::Number(_) => "an integer",
        Json::String(_) => "a string",
        Json::Array(_) => "an array",
        Json::Object(_) => "an object",
    };
    let mut text = value.to_string();
    if text.chars().count() > 40 {
        text = format!("{}…", text.chars().take(40).collect::<String>());
    }
    format!("{kind} ({text})")
}

/// A value of the type, as the data writes it.
fn example(kind: &str) -> &'static str {
    match kind {
        "number_integer" => "2",
        "number_decimal" => "\"2.5\"",
        "boolean" => "true",
        "date" => "\"2025-05-01\"",
        "date_time" => "\"2025-05-01T10:00:00Z\"",
        "color" => "\"#f4f1ea\"",
        "rating" => "{\"value\": \"4.5\", \"scale_min\": \"1\", \"scale_max\": \"5\"}",
        "money" => "{\"amount\": \"5.99\", \"currency_code\": \"USD\"}",
        "url" => "\"https://example.com\"",
        "link" => "{\"text\": \"Size guide\", \"url\": \"https://example.com\"}",
        "weight" => "{\"value\": 2.5, \"unit\": \"kilograms\"}",
        "dimension" => "{\"value\": 30, \"unit\": \"centimeters\"}",
        "volume" => "{\"value\": 500, \"unit\": \"milliliters\"}",
        "language" => "\"fr\"",
        kind if is_measurement(kind) => "{\"value\": 2, \"unit\": \"...\"}",
        "variant_reference" => "\"SKU-1\"",
        "metaobject_reference" | "mixed_reference" => "\"designer/sam\"",
        "article_reference" => "\"news/first-post\"",
        "file_reference" => "\"lookbook.jpg\"",
        "rich_text_field" => "{\"type\": \"root\", \"children\": [...]}",
        "json" => "{...}",
        _ => "\"text\"",
    }
}

/// A decimal written as a JSON number or a string.
fn decimal(value: &Json) -> Option<f64> {
    match value {
        Json::Number(number) => number.as_f64(),
        Json::String(text) => text.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => None,
    }
}

fn check_one(
    kind: &str,
    value: &Json,
    pointer: &str,
    problems: &mut Vec<Problem>,
    references: &mut Vec<Reference>,
) {
    let mut wrong = |message: String| {
        problems.push(Problem {
            pointer: pointer.to_string(),
            code: "invalid_metafield",
            message,
            hint: format!("a {kind} is written {}", example(kind)),
        });
    };
    let text = value.as_str();
    match kind {
        "single_line_text_field" | "multi_line_text_field" | "id" => {
            if text.is_none() {
                wrong(format!("a {kind} is a string, not {}", describe(value)));
            } else if kind == "single_line_text_field" && text.is_some_and(|t| t.contains('\n')) {
                problems.push(Problem {
                    pointer: pointer.to_string(),
                    code: "invalid_metafield",
                    message: "a single_line_text_field has no line break".to_string(),
                    hint: "use the type multi_line_text_field".to_string(),
                });
            }
        }
        "number_integer" => {
            if !(value.is_i64() || value.is_u64()) {
                wrong(format!(
                    "a number_integer is a whole number, not {}",
                    describe(value)
                ));
            }
        }
        "number_decimal" => {
            if decimal(value).is_none() {
                wrong(format!(
                    "a number_decimal is a number, not {}",
                    describe(value)
                ));
            }
        }
        "boolean" => {
            if !value.is_boolean() {
                wrong(format!(
                    "a boolean is true or false, not {}",
                    describe(value)
                ));
            }
        }
        "date" | "date_time" => {
            let parsed = text.and_then(|text| crate::store::date::parse(text, chrono_tz::Tz::UTC));
            let day_only = text.is_some_and(|text| text.len() == 10);
            if parsed.is_none() || (kind == "date" && !day_only) {
                wrong(format!("not a {kind}: {}", describe(value)));
            }
        }
        "language" => {
            if !text.is_some_and(is_locale_code) {
                wrong(format!(
                    "a language is a locale code (en, fr, pt-BR), not {}",
                    describe(value)
                ));
            }
        }
        "color" => {
            if text.and_then(crate::color::Color::parse).is_none() {
                wrong(format!("not a color: {}", describe(value)));
            }
        }
        "url" => {
            if !text.is_some_and(is_url) {
                wrong(format!(
                    "not a URL (https, http, mailto, sms, tel or shopify:// links): {}",
                    describe(value)
                ));
            }
        }
        "link" => {
            let url = value.get("url").and_then(Json::as_str);
            if !value.is_object() || !url.is_some_and(is_url) {
                wrong(format!(
                    "a link is an object with a \"text\" and a \"url\", not {}",
                    describe(value)
                ));
            }
        }
        "money" => {
            let amount = value.get("amount").and_then(|amount| match amount {
                Json::String(text) => crate::store::model::Money::Decimal(text.clone()).cents(),
                Json::Number(number) => number.as_f64().map(|n| (n * 100.0).round() as i64),
                _ => None,
            });
            // Without a currency, the shop's.
            let currency = value.get("currency_code");
            let currency_ok = currency.is_none_or(|code| {
                code.as_str().is_some_and(|code| {
                    code.len() == 3 && code.chars().all(|c| c.is_ascii_alphabetic())
                })
            });
            if amount.is_none() || !currency_ok {
                wrong(format!(
                    "money is an \"amount\" and a \"currency_code\" (the shop's when left out), not {}",
                    describe(value)
                ));
            }
        }
        "rating" => {
            let field = |name: &str| value.get(name).and_then(decimal);
            match (field("value"), field("scale_min"), field("scale_max")) {
                (Some(rating), Some(min), Some(max)) if min <= rating && rating <= max => {}
                (Some(_), Some(_), Some(_)) => {
                    wrong("the rating is outside its scale_min and scale_max".to_string())
                }
                _ => wrong(format!(
                    "a rating has a \"value\", a \"scale_min\" and a \"scale_max\", not {}",
                    describe(value)
                )),
            }
        }
        "json" => {}
        "rich_text_field" => {
            if value.get("type").and_then(Json::as_str) != Some("root") {
                wrong(format!(
                    "a rich_text_field is an object of type \"root\", not {}",
                    describe(value)
                ));
            }
        }
        kind if is_measurement(kind) => {
            let number = value.get("value").and_then(decimal);
            let unit = value.get("unit").and_then(Json::as_str);
            match (number, unit) {
                (Some(_), Some(unit)) => {
                    if let Some(units) = units(kind)
                        && !units.contains(&unit)
                    {
                        let hint = match crate::util::closest_match(unit, units.iter().copied()) {
                            Some(close) => format!("did you mean \"{close}\"?"),
                            None => format!("a {kind} is in {}", units.join(", ")),
                        };
                        problems.push(Problem {
                            pointer: format!("{pointer}/unit"),
                            code: "invalid_metafield",
                            message: format!("\"{unit}\" is not a unit of {kind}"),
                            hint,
                        });
                    }
                }
                _ => wrong(format!(
                    "a {kind} has a \"value\" and a \"unit\", not {}",
                    describe(value)
                )),
            }
        }
        "variant_reference" => {
            if !(value.is_u64() || text.is_some()) {
                wrong(format!(
                    "a variant_reference is a SKU or a variant id, not {}",
                    describe(value)
                ));
            } else {
                references.push(Reference {
                    kind: kind.to_string(),
                    value: value.clone(),
                    pointer: pointer.to_string(),
                });
            }
        }
        // A reference by its handle (or path): resolved later.
        _ => match text {
            Some(_) => references.push(Reference {
                kind: kind.to_string(),
                value: value.clone(),
                pointer: pointer.to_string(),
            }),
            None => {
                let message = format!("a {kind} is written as a string, not {}", describe(value));
                // `{"type": "faq", "handle": "size"}`: what the data writes `"faq/size"`.
                let entry = value
                    .get("type")
                    .and_then(Json::as_str)
                    .zip(value.get("handle").and_then(Json::as_str));
                match entry {
                    Some((entry_kind, handle))
                        if matches!(kind, "metaobject_reference" | "mixed_reference") =>
                    {
                        problems.push(Problem {
                            pointer: pointer.to_string(),
                            code: "invalid_metafield",
                            message,
                            hint: format!("use \"{entry_kind}/{handle}\""),
                        })
                    }
                    _ => wrong(message),
                }
            }
        },
    }
}

/// Whether a `url` metafield takes this text: Shopify takes the schemes below, and lsf the
/// paths of the storefront too.
fn is_url(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    if lower.starts_with('/') {
        return !lower.starts_with("//") || lower.len() > 2;
    }
    let Some((scheme, rest)) = lower.split_once(':') else {
        return false;
    };
    match scheme {
        "http" | "https" => rest.starts_with("//") && rest.len() > 2,
        "shopify" => rest.starts_with("//"),
        "mailto" | "sms" | "tel" => !rest.is_empty(),
        _ => false,
    }
}

/// Whether `text` is a locale code: a language (`fr`), maybe with a region or a script
/// (`pt-BR`, `zh-Hant`).
fn is_locale_code(text: &str) -> bool {
    let mut parts = text.split('-');
    let language = parts.next().unwrap_or_default();
    (2..=3).contains(&language.len())
        && language.chars().all(|c| c.is_ascii_lowercase())
        && parts.all(|part| {
            (2..=4).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn measurements_take_the_unit_names_of_shopifys_docs_and_the_short_forms() {
        for (kind, unit) in [
            ("weight", "kilograms"),
            ("weight", "kg"),
            ("dimension", "centimeters"),
            ("dimension", "cm"),
            ("volume", "liters"),
            ("volume", "us_fluid_ounces"),
            ("volume", "ml"),
        ] {
            assert!(
                problems(kind, json!({"value": 2, "unit": unit})).is_empty(),
                "{kind} {unit}"
            );
            let list = format!("list.{kind}");
            assert!(
                problems(&list, json!([{"value": 2, "unit": unit}])).is_empty(),
                "{list} {unit}"
            );
        }
        assert_eq!(
            problems("weight", json!({"value": 2, "unit": "kilogram"})),
            ["/value/unit \"kilogram\" is not a unit of weight"]
        );
    }

    #[test]
    fn a_language_is_a_locale_code() {
        for code in ["en", "fr", "pt-BR", "zh-Hant"] {
            assert!(problems("language", json!(code)).is_empty(), "{code}");
        }
        assert_eq!(problems("language", json!("French")).len(), 1);
        assert!(is_known("language"));
        assert!(!is_known("list.language"));
    }

    fn problems(kind: &str, value: Json) -> Vec<String> {
        check(kind, &value)
            .0
            .into_iter()
            .map(|problem| format!("{} {}", problem.pointer, problem.message))
            .collect()
    }

    #[test]
    fn values_follow_their_type() {
        assert!(problems("number_integer", json!(2)).is_empty());
        assert_eq!(
            problems("number_integer", json!("two")),
            ["/value a number_integer is a whole number, not a string (\"two\")"]
        );
        assert!(problems("number_decimal", json!("2.5")).is_empty());
        assert!(!problems("number_decimal", json!("2,5")).is_empty());
        assert!(!problems("boolean", json!("true")).is_empty());
        assert!(problems("date", json!("2025-05-01")).is_empty());
        assert!(!problems("date", json!("May 1, 2025")).is_empty());
        assert!(problems("date_time", json!("2025-05-01T10:00:00Z")).is_empty());
        assert!(problems("color", json!("#f4f1ea")).is_empty());
        assert!(!problems("color", json!("blurple")).is_empty());
        assert!(problems("url", json!("https://example.com")).is_empty());
        assert!(problems("url", json!("/pages/about")).is_empty());
        assert!(!problems("url", json!("example.com")).is_empty());
        assert!(problems("money", json!({"amount": "5.99", "currency_code": "EUR"})).is_empty());
        assert!(!problems("money", json!("5.99")).is_empty());
        assert!(
            problems(
                "rating",
                json!({"value": "4.5", "scale_min": "1", "scale_max": "5"})
            )
            .is_empty()
        );
        assert_eq!(
            problems(
                "rating",
                json!({"value": "6", "scale_min": "1", "scale_max": "5"})
            ),
            ["/value the rating is outside its scale_min and scale_max"]
        );
        assert!(problems("weight", json!({"value": 2.5, "unit": "kg"})).is_empty());
        assert_eq!(
            problems("weight", json!({"value": 2.5, "unit": "kgs"})),
            ["/value/unit \"kgs\" is not a unit of weight"]
        );
        assert!(problems("temperature", json!({"value": 20, "unit": "c"})).is_empty());
        assert!(problems("json", json!([1, {"a": null}])).is_empty());
        assert_eq!(
            problems("list.number_integer", json!([1, "2"])),
            ["/value/1 a number_integer is a whole number, not a string (\"2\")"]
        );
        assert!(!problems("list.product_reference", json!("a")).is_empty());
    }

    #[test]
    fn an_unknown_type_is_named_with_the_closest_one() {
        let (problems, _) = check("number_interger", &json!(2));
        assert_eq!(problems[0].code, "unknown_metafield_type");
        assert_eq!(problems[0].hint, "did you mean \"number_integer\"?");
        assert!(is_known("list.weight"));
        assert!(!is_known("list.boolean"));
    }

    #[test]
    fn references_are_collected_with_their_place() {
        let (problems, references) = check("list.product_reference", &json!(["a", "b"]));
        assert!(problems.is_empty());
        assert_eq!(references.len(), 2);
        assert_eq!(references[1].pointer, "/value/1");
        assert_eq!(references[1].value, json!("b"));
    }
}
