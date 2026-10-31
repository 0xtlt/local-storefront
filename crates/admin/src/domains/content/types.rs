//! The metafield types of 2026-10: their names, categories and the validations a definition of
//! each may have (`metafieldDefinitionTypes`), and how a value of each is checked and kept
//! (`metafieldsSet`, the fields of a metaobject).
//!
//! Sources, read in 2026-10:
//!
//! - the types, their categories and supported validations: the example response of the
//!   `metafieldDefinitionTypes` query on shopify.dev (51 types, in its order), completed from
//!   "List of data types" (<https://shopify.dev/docs/apps/build/metafields/list-of-data-types>:
//!   `id`, `article_reference`, the measurement types and their units) and "List of validation
//!   options" (<https://shopify.dev/docs/apps/build/metafields/list-of-validation-options>);
//! - the value formats: the samples of "List of data types" (`{"value": 25.0, "unit":
//!   "centimeters"}`, `{"amount": "5.99", "currency_code": "CAD"}`, `{"value": "3.5",
//!   "scale_min": "1.0", "scale_max": "5.0"}`, a URL of `https`, `http`, `mailto`, `sms` or `tel`).
//!
//! What a value is kept as is what the seed keeps (`content::seed_metafield`): `value` the
//! string Admin returns (JSON compact for the types that hold JSON, references as GIDs, `money`
//! with a string amount), `jsonValue` from [`super::json_value`], `reference` or `references`.
//! Store sync projects those back to the store data.
//!
//! Messages: "Value must be an integer." is Shopify's (quoted in a community answer,
//! community.shopify.com/t/update-order-metafield-n-error-for-value-330-n/390199); Shopify does
//! not publish the others, so they are lsf's, in the same form.

use chrono::{DateTime, NaiveDate, NaiveDateTime};
use serde_json::{Value, json};

use crate::state::{ShopState, gid_type, parse_gid};

/// The types `metafieldDefinitionTypes` lists, in its order: Shopify's example first, then those
/// the example leaves out.
pub const TYPES: &[&str] = &[
    "boolean",
    "color",
    "date_time",
    "date",
    "dimension",
    "json",
    "language",
    "list.color",
    "list.date_time",
    "list.date",
    "list.dimension",
    "list.number_decimal",
    "list.number_integer",
    "list.rating",
    "list.single_line_text_field",
    "list.url",
    "list.volume",
    "list.weight",
    "money",
    "multi_line_text_field",
    "number_decimal",
    "number_integer",
    "rating",
    "rich_text_field",
    "single_line_text_field",
    "url",
    "link",
    "list.link",
    "volume",
    "weight",
    "company_reference",
    "list.company_reference",
    "customer_reference",
    "list.customer_reference",
    "product_reference",
    "list.product_reference",
    "collection_reference",
    "list.collection_reference",
    "variant_reference",
    "list.variant_reference",
    "file_reference",
    "list.file_reference",
    "product_taxonomy_value_reference",
    "list.product_taxonomy_value_reference",
    "metaobject_reference",
    "list.metaobject_reference",
    "mixed_reference",
    "list.mixed_reference",
    "page_reference",
    "list.page_reference",
    "order_reference",
    // Not in the example.
    "list.order_reference",
    "article_reference",
    "list.article_reference",
    "id",
    "antenna_gain",
    "list.antenna_gain",
    "area",
    "list.area",
    "battery_charge_capacity",
    "list.battery_charge_capacity",
    "battery_energy_capacity",
    "list.battery_energy_capacity",
    "capacitance",
    "list.capacitance",
    "concentration",
    "list.concentration",
    "data_storage_capacity",
    "list.data_storage_capacity",
    "data_transfer_rate",
    "list.data_transfer_rate",
    "display_density",
    "list.display_density",
    "distance",
    "list.distance",
    "duration",
    "list.duration",
    "electric_current",
    "list.electric_current",
    "electrical_resistance",
    "list.electrical_resistance",
    "energy",
    "list.energy",
    "frequency",
    "list.frequency",
    "illuminance",
    "list.illuminance",
    "inductance",
    "list.inductance",
    "luminous_flux",
    "list.luminous_flux",
    "mass_flow_rate",
    "list.mass_flow_rate",
    "power",
    "list.power",
    "pressure",
    "list.pressure",
    "resolution",
    "list.resolution",
    "rotational_speed",
    "list.rotational_speed",
    "sound_level",
    "list.sound_level",
    "speed",
    "list.speed",
    "temperature",
    "list.temperature",
    "thermal_power",
    "list.thermal_power",
    "voltage",
    "list.voltage",
    "volumetric_flow_rate",
    "list.volumetric_flow_rate",
];

/// The units of each measurement type ("List of data types"), as the value writes them.
const UNITS: &[(&str, &[&str])] = &[
    ("antenna_gain", &["decibels_isotropic", "decibels_dipole"]),
    (
        "area",
        &[
            "square_centimeters",
            "square_feet",
            "square_inches",
            "square_meters",
            "square_yards",
        ],
    ),
    ("battery_charge_capacity", &["milliamp_hours"]),
    ("battery_energy_capacity", &["watt_hours"]),
    (
        "capacitance",
        &["picofarads", "nanofarads", "microfarads", "farads"],
    ),
    (
        "concentration",
        &["milligrams_per_gram", "milligrams_per_milliliter"],
    ),
    (
        "data_storage_capacity",
        &["bytes", "kilobytes", "megabytes", "gigabytes", "terabytes"],
    ),
    (
        "data_transfer_rate",
        &[
            "bits_per_second",
            "kilobits_per_second",
            "megabits_per_second",
            "gigabits_per_second",
        ],
    ),
    (
        "dimension",
        &[
            "inches",
            "feet",
            "yards",
            "millimeters",
            "centimeters",
            "meters",
        ],
    ),
    ("display_density", &["pixels_per_inch", "dots_per_inch"]),
    (
        "distance",
        &[
            "millimeters",
            "centimeters",
            "meters",
            "kilometers",
            "inches",
            "feet",
            "yards",
            "miles",
        ],
    ),
    (
        "duration",
        &[
            "nanoseconds",
            "microseconds",
            "milliseconds",
            "seconds",
            "minutes",
            "hours",
            "days",
            "months",
            "years",
        ],
    ),
    (
        "electric_current",
        &["milliamperes", "amperes", "kiloamperes"],
    ),
    ("electrical_resistance", &["ohms", "kiloohms"]),
    (
        "energy",
        &["joules", "calories", "kilojoules", "kilocalories"],
    ),
    (
        "frequency",
        &["hertz", "kilohertz", "megahertz", "gigahertz"],
    ),
    ("illuminance", &["lux", "foot_candles"]),
    ("inductance", &["microhenries", "millihenries", "henries"]),
    ("luminous_flux", &["lumens"]),
    (
        "mass_flow_rate",
        &[
            "grams_per_day",
            "grams_per_hour",
            "grams_per_minute",
            "grams_per_second",
            "ounces_per_day",
            "ounces_per_hour",
            "ounces_per_minute",
            "ounces_per_second",
            "pounds_per_day",
            "pounds_per_hour",
            "pounds_per_minute",
            "pounds_per_second",
            "kilograms_per_day",
            "kilograms_per_hour",
            "kilograms_per_minute",
            "kilograms_per_second",
            "tons_per_day",
            "tons_per_hour",
            "tons_per_minute",
            "tons_per_second",
            "tonnes_per_day",
            "tonnes_per_hour",
            "tonnes_per_minute",
            "tonnes_per_second",
        ],
    ),
    ("power", &["milliwatts", "watts", "horsepower", "kilowatts"]),
    ("pressure", &["pounds_per_square_inch", "bars"]),
    ("resolution", &["pixels", "megapixels"]),
    ("rotational_speed", &["revolutions_per_minute"]),
    ("sound_level", &["decibels"]),
    (
        "speed",
        &[
            "kilometers_per_hour",
            "feet_per_second",
            "miles_per_hour",
            "meters_per_second",
        ],
    ),
    ("temperature", &["celsius", "fahrenheit", "kelvin"]),
    (
        "thermal_power",
        &[
            "british_thermal_units_per_hour",
            "kilowatts",
            "tons_of_refrigeration",
        ],
    ),
    ("voltage", &["volts"]),
    (
        "volume",
        &[
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
        ],
    ),
    (
        "volumetric_flow_rate",
        &[
            "liters_per_hour",
            "liters_per_minute",
            "liters_per_second",
            "gallons_per_hour",
            "gallons_per_minute",
            "gallons_per_second",
            "cubic_feet_per_hour",
            "cubic_feet_per_minute",
            "cubic_feet_per_second",
            "cubic_meters_per_hour",
            "cubic_meters_per_minute",
            "cubic_meters_per_second",
        ],
    ),
    ("weight", &["ounces", "pounds", "grams", "kilograms"]),
];

/// The short units of weights, volumes and dimensions, which the validation examples of
/// shopify.dev write (`{"unit": "g", "value": 10}`), with the factor to the first full unit of
/// their kind (grams, milliliters, millimeters) and the full unit's name.
const SHORT_UNITS: &[(&str, &str, f64)] = &[
    ("g", "grams", 1.0),
    ("kg", "kilograms", 1000.0),
    ("oz", "ounces", 28.349_523_125),
    ("lb", "pounds", 453.592_37),
    ("ml", "milliliters", 1.0),
    ("cl", "centiliters", 10.0),
    ("l", "liters", 1000.0),
    ("m3", "cubic_meters", 1_000_000.0),
    ("us_fl_oz", "us_fluid_ounces", 29.573_529_562_5),
    ("us_pt", "us_pints", 473.176_473),
    ("us_qt", "us_quarts", 946.352_946),
    ("us_gal", "us_gallons", 3_785.411_784),
    ("imp_fl_oz", "imperial_fluid_ounces", 28.413_062_5),
    ("imp_pt", "imperial_pints", 568.261_25),
    ("imp_qt", "imperial_quarts", 1_136.522_5),
    ("imp_gal", "imperial_gallons", 4_546.09),
    ("mm", "millimeters", 1.0),
    ("cm", "centimeters", 10.0),
    ("m", "meters", 1000.0),
    ("in", "inches", 25.4),
    ("ft", "feet", 304.8),
    ("yd", "yards", 914.4),
];

/// The schemes a `url` value may have.
const URL_SCHEMES: &[&str] = &["https", "http", "mailto", "sms", "tel"];

/// The largest `number_integer` (±9,007,199,254,740,991, "List of data types").
const MAX_INTEGER: i64 = 9_007_199_254_740_991;

/// Whether `kind` is a metafield type of 2026-10.
pub fn is_type(kind: &str) -> bool {
    TYPES.contains(&kind)
}

/// The type of a list's items: `list.weight` is `weight`.
pub fn base(kind: &str) -> &str {
    kind.strip_prefix("list.").unwrap_or(kind)
}

pub fn is_list(kind: &str) -> bool {
    kind.starts_with("list.")
}

pub fn is_reference(kind: &str) -> bool {
    base(kind).ends_with("_reference")
}

/// The units of a measurement type, `None` for another type.
fn units(kind: &str) -> Option<&'static [&'static str]> {
    UNITS
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, units)| *units)
}

/// The category Shopify files a type under (`metafieldDefinitionTypes`).
pub fn category(kind: &str) -> Option<&'static str> {
    let base = base(kind);
    Some(match base {
        "single_line_text_field" | "multi_line_text_field" | "rich_text_field" | "id" => "TEXT",
        "number_integer" | "number_decimal" => "NUMBER",
        "boolean" => "TRUE_FALSE",
        "date" | "date_time" => "DATE_TIME",
        "color" => "COLOR",
        "url" => "URL",
        "json" => "JSON",
        "money" => "MONEY",
        "rating" => "RATING",
        "link" => "LINK",
        "language" => "LANGUAGE",
        base if base.ends_with("_reference") => "REFERENCE",
        base if units(base).is_some() => "MEASUREMENT",
        _ => return None,
    })
}

/// The validations a definition of the type may have: `(name, type of its value)`.
pub fn supported_validations(kind: &str) -> Vec<(&'static str, String)> {
    let base = base(kind);
    let mut validations: Vec<(&'static str, String)> = match base {
        "single_line_text_field" => vec![
            ("min", "number_integer".into()),
            ("max", "number_integer".into()),
            ("regex", "single_line_text_field".into()),
            ("choices", "list.single_line_text_field".into()),
        ],
        "multi_line_text_field" | "id" => vec![
            ("min", "number_integer".into()),
            ("max", "number_integer".into()),
            ("regex", "single_line_text_field".into()),
        ],
        "number_integer" => vec![
            ("min", "number_integer".into()),
            ("max", "number_integer".into()),
        ],
        "number_decimal" => vec![
            ("min", "number_decimal".into()),
            ("max", "number_decimal".into()),
            ("max_precision", "number_integer".into()),
        ],
        "date" | "date_time" => vec![("min", base.into()), ("max", base.into())],
        "json" => vec![("schema", "json".into())],
        "url" | "link" => vec![("allowed_domains", "list.single_line_text_field".into())],
        "rating" => vec![
            ("scale_min", "number_decimal".into()),
            ("scale_max", "number_decimal".into()),
        ],
        "file_reference" => vec![("file_type_options", "list.single_line_text_field".into())],
        "metaobject_reference" => {
            vec![("metaobject_definition_id", "single_line_text_field".into())]
        }
        "mixed_reference" => vec![(
            "metaobject_definition_ids",
            "list.single_line_text_field".into(),
        )],
        "product_taxonomy_value_reference" => vec![(
            "product_taxonomy_attribute_handle",
            "single_line_text_field".into(),
        )],
        base if units(base).is_some() => vec![("min", base.into()), ("max", base.into())],
        _ => Vec::new(),
    };
    if is_list(kind) {
        validations.push(("list.min", "number_integer".into()));
        validations.push(("list.max", "number_integer".into()));
    }
    validations
}

/// Whether a definition of the type can change to another type: the basic types that are not
/// lists, as Shopify's example says (`language`, `money`, `rich_text_field` and the references
/// cannot).
fn supports_migrations(kind: &str) -> bool {
    !is_list(kind)
        && !is_reference(kind)
        && !matches!(kind, "language" | "money" | "rich_text_field" | "id")
}

/// The deprecated `valueType` of a type.
fn value_type(kind: &str) -> &'static str {
    match kind {
        "boolean" => "BOOLEAN",
        "number_integer" => "INTEGER",
        "single_line_text_field"
        | "multi_line_text_field"
        | "color"
        | "date"
        | "date_time"
        | "url"
        | "number_decimal"
        | "language"
        | "id" => "STRING",
        kind if is_reference(kind) && !is_list(kind) => "STRING",
        _ => "JSON_STRING",
    }
}

/// A `MetafieldDefinitionType`, as a record holds it inline (`MetafieldDefinition.type`,
/// `MetaobjectFieldDefinition.type`) and `metafieldDefinitionTypes` lists it.
pub fn definition_type(kind: &str) -> Value {
    let mut definition = json!({
        "name": kind,
        "supportedValidations": supported_validations(kind)
            .into_iter()
            .map(|(name, kind)| json!({"name": name, "type": kind}))
            .collect::<Vec<_>>(),
        "supportsDefinitionMigrations": supports_migrations(kind),
        "valueType": value_type(kind),
    });
    if let Some(category) = category(kind) {
        definition["category"] = json!(category);
    }
    definition
}

/// A value checked and kept as the Admin API returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct Checked {
    /// `value`: the string Admin returns.
    pub value: String,
    /// `jsonValue`.
    pub json: Value,
    /// The GID of a single reference (`reference`).
    pub reference: Option<String>,
    /// The GIDs of a list of references (`references`).
    pub references: Option<Vec<String>>,
}

/// Why a value is refused: the message and the code of `MetafieldsSetUserErrorCode` (or
/// `MetaobjectUserErrorCode`, whose codes are the same names for these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid {
    pub message: String,
    pub code: &'static str,
}

impl Invalid {
    fn value(message: impl Into<String>) -> Invalid {
        Invalid {
            message: message.into(),
            code: "INVALID_VALUE",
        }
    }
}

/// What a value is checked against: the shop (references must name records of it, `money` is
/// in its currency) and the validations of the definition, if any (`{name, value}`).
pub struct Context<'a> {
    pub state: &'a ShopState,
    pub currency: &'a str,
    pub validations: &'a [Value],
}

impl Context<'_> {
    /// The value of a validation of the definition.
    fn validation(&self, name: &str) -> Option<&str> {
        self.validations.iter().find_map(|validation| {
            (validation.get("name")?.as_str()? == name)
                .then(|| validation.get("value")?.as_str())
                .flatten()
        })
    }
}

/// Checks a value of `kind` given as Shopify takes it (a string, JSON for the types that hold
/// JSON and for lists), and answers it as it is kept.
pub fn check(kind: &str, raw: &str, cx: &Context<'_>) -> Result<Checked, Invalid> {
    if let Some(base) = kind.strip_prefix("list.") {
        let items = match serde_json::from_str::<Value>(raw) {
            Ok(Value::Array(items)) => items,
            Ok(_) => return Err(Invalid::value("Value must be an array.")),
            Err(error) => return Err(invalid_json(raw, &error)),
        };
        let mut kept = Vec::with_capacity(items.len());
        for item in &items {
            kept.push(check_item(base, Item::Json(item), cx)?);
        }
        check_count(items.len(), cx)?;
        let references = is_reference(base).then(|| {
            kept.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        });
        let array = Value::Array(kept);
        let value = array.to_string();
        return Ok(Checked {
            json: super::json_value(kind, &value),
            value,
            reference: None,
            references,
        });
    }
    let kept = check_item(kind, Item::Text(raw), cx)?;
    let value = match &kept {
        Value::String(text) if !holds_json(kind) => text.clone(),
        other => other.to_string(),
    };
    Ok(Checked {
        json: super::json_value(kind, &value),
        reference: is_reference(kind).then(|| value.clone()),
        value,
        references: None,
    })
}

/// A value to check: the string of a single value, or an item of a list's JSON.
#[derive(Clone, Copy)]
enum Item<'v> {
    Text(&'v str),
    Json(&'v Value),
}

impl<'v> Item<'v> {
    /// The text of a value that is a string: the string given, or a string item of a list.
    fn text(self) -> Option<&'v str> {
        match self {
            Item::Text(text) => Some(text),
            Item::Json(Value::String(text)) => Some(text),
            Item::Json(_) => None,
        }
    }

    /// The text of a number: a string, or a number item of a list.
    fn number_text(self) -> Option<String> {
        match self {
            Item::Text(text) => Some(text.to_string()),
            Item::Json(Value::String(text)) => Some(text.clone()),
            Item::Json(Value::Number(number)) => Some(number.to_string()),
            Item::Json(_) => None,
        }
    }

    /// The JSON of a value that holds JSON: parsed from the string given, or the item of a list.
    fn json(self) -> Result<Value, Invalid> {
        match self {
            Item::Text(text) => {
                serde_json::from_str(text).map_err(|error| invalid_json(text, &error))
            }
            Item::Json(value) => Ok(value.clone()),
        }
    }
}

/// Ruby's JSON parser's message, which Shopify's says after "Value is invalid JSON: " (lsf's
/// wording for the part after it).
fn invalid_json(raw: &str, error: &serde_json::Error) -> Invalid {
    let rest: String = raw.chars().take(40).collect();
    Invalid::value(format!(
        "Value is invalid JSON: unexpected token at '{rest}' ({error})."
    ))
}

/// Checks one value (or one item of a list) of a type that is not a list, and answers it as it
/// is kept: a string for the types written as strings, JSON for the others.
fn check_item(kind: &str, item: Item<'_>, cx: &Context<'_>) -> Result<Value, Invalid> {
    match kind {
        "single_line_text_field" | "id" => {
            let text = text_of(item, "Value must be a string.")?;
            if text.contains(['\n', '\r']) {
                return Err(Invalid::value("Value must be a single line text string."));
            }
            check_text(text, cx)?;
            if let Some(choices) = cx.validation("choices") {
                let choices: Vec<String> = serde_json::from_str(choices).unwrap_or_default();
                if !choices.iter().any(|choice| choice == text) {
                    return Err(Invalid::value(format!(
                        "Value does not exist in provided choices: {}.",
                        choices.join(", ")
                    )));
                }
            }
            Ok(json!(text))
        }
        "multi_line_text_field" => {
            let text = text_of(item, "Value must be a string.")?;
            check_text(text, cx)?;
            Ok(json!(text))
        }
        "number_integer" => {
            let text = item
                .number_text()
                .ok_or_else(|| Invalid::value("Value must be an integer."))?;
            let number =
                parse_integer(&text).ok_or_else(|| Invalid::value("Value must be an integer."))?;
            if number.abs() > MAX_INTEGER {
                return Err(Invalid::value(format!(
                    "Value must be an integer between -{MAX_INTEGER} and {MAX_INTEGER}."
                )));
            }
            check_range(number as f64, &text, cx, |bound| bound.parse::<f64>().ok())?;
            Ok(match item {
                Item::Json(Value::Number(number)) => Value::Number(number.clone()),
                Item::Json(_) => json!(number),
                Item::Text(_) => json!(text),
            })
        }
        "number_decimal" => {
            let text = item
                .number_text()
                .ok_or_else(|| Invalid::value("Value must be a decimal."))?;
            let number =
                parse_decimal(&text).ok_or_else(|| Invalid::value("Value must be a decimal."))?;
            check_range(number, &text, cx, |bound| bound.parse::<f64>().ok())?;
            if let Some(precision) = cx
                .validation("max_precision")
                .and_then(|p| p.parse::<usize>().ok())
                && text
                    .split_once('.')
                    .is_some_and(|(_, fraction)| fraction.len() > precision)
            {
                return Err(Invalid::value(format!(
                    "Value must have {precision} or fewer digits after the decimal point."
                )));
            }
            Ok(match item {
                Item::Json(Value::Number(number)) => Value::Number(number.clone()),
                _ => json!(text),
            })
        }
        "boolean" => match item {
            Item::Text("true") | Item::Json(Value::Bool(true)) => Ok(json!("true")),
            Item::Text("false") | Item::Json(Value::Bool(false)) => Ok(json!("false")),
            _ => Err(Invalid::value("Value must be true or false.")),
        },
        "date" => {
            let text = text_of(item, "Value must be in YYYY-MM-DD format.")?;
            let date = NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map_err(|_| Invalid::value("Value must be in YYYY-MM-DD format."))?;
            let bound = |text: &str| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok();
            check_dates(date, cx, bound)?;
            Ok(json!(text))
        }
        "date_time" => {
            let text = text_of(item, "Value must be in YYYY-MM-DDTHH:MM:SS format.")?;
            let at = parse_date_time(text)
                .ok_or_else(|| Invalid::value("Value must be in YYYY-MM-DDTHH:MM:SS format."))?;
            check_dates(at, cx, parse_date_time)?;
            Ok(json!(text))
        }
        "color" => {
            let text = text_of(item, "Value must be a hex color code.")?;
            let valid = text.len() == 7
                && text.starts_with('#')
                && text[1..].chars().all(|c| c.is_ascii_hexdigit());
            if !valid {
                return Err(Invalid::value("Value must be a hex color code."));
            }
            Ok(json!(text))
        }
        "url" => {
            let text = text_of(item, "Value must be a valid URL.")?;
            check_url(text, cx)?;
            Ok(json!(text))
        }
        "language" => {
            let text = text_of(item, "Value must be a valid language code.")?;
            if !is_locale(text) {
                return Err(Invalid::value("Value must be a valid language code."));
            }
            Ok(json!(text))
        }
        "json" => item.json(),
        "rich_text_field" => {
            let value = item.json()?;
            let root = value.get("type").and_then(Value::as_str) == Some("root")
                && value.get("children").is_some_and(Value::is_array);
            if !root {
                return Err(Invalid::value(
                    "Value is not the correct format: the root node must be {\"type\": \"root\", \"children\": [...]}.",
                ));
            }
            Ok(value)
        }
        "link" => {
            let value = item.json()?;
            let (Some(text), Some(url)) = (
                value.get("text").and_then(Value::as_str),
                value.get("url").and_then(Value::as_str),
            ) else {
                return Err(Invalid::value(
                    "Value must be an object with a text and a url.",
                ));
            };
            check_url(url, cx)?;
            Ok(json!({"text": text, "url": url}))
        }
        "money" => check_money(item, cx),
        "rating" => check_rating(item, cx),
        kind if units(kind).is_some() => check_measurement(kind, item, cx),
        kind if kind.ends_with("_reference") => check_reference(kind, item, cx),
        _ => Err(Invalid {
            message: format!("Type '{kind}' is not a valid type."),
            code: "INVALID_TYPE",
        }),
    }
}

fn text_of<'v>(item: Item<'v>, message: &str) -> Result<&'v str, Invalid> {
    item.text().ok_or_else(|| Invalid::value(message))
}

/// An integer as Shopify writes one: digits, an optional minus sign, nothing else.
fn parse_integer(text: &str) -> Option<i64> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().or_else(|| {
        // Past i64: refused as out of range by the caller.
        Some(if text.starts_with('-') {
            i64::MIN + 1
        } else {
            i64::MAX
        })
    })
}

/// A decimal in the range of `number_decimal` (±9999999999999.999999999).
fn parse_decimal(text: &str) -> Option<f64> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let valid = !whole.is_empty()
        && whole.len() <= 13
        && fraction.len() <= 9
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && !(digits.contains('.') && fraction.is_empty());
    valid.then(|| text.parse().ok()).flatten()
}

/// A `date_time`: ISO 8601 with or without a zone (without one, GMT).
fn parse_date_time(text: &str) -> Option<NaiveDateTime> {
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Some(at.naive_utc());
    }
    [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
    ]
    .iter()
    .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
}

/// The length validations of a text (`min`, `max`, in characters) and its `regex`.
fn check_text(text: &str, cx: &Context<'_>) -> Result<(), Invalid> {
    let length = text.chars().count();
    if let Some(min) = cx
        .validation("min")
        .and_then(|min| min.parse::<usize>().ok())
        && length < min
    {
        return Err(Invalid {
            message: format!("Value is too short (minimum is {min} characters)."),
            code: "TOO_SHORT",
        });
    }
    if let Some(max) = cx
        .validation("max")
        .and_then(|max| max.parse::<usize>().ok())
        && length > max
    {
        return Err(Invalid {
            message: format!("Value is too long (maximum is {max} characters)."),
            code: "TOO_LONG",
        });
    }
    if let Some(pattern) = cx.validation("regex")
        && let Ok(regex) = regex::Regex::new(pattern)
        && !regex.is_match(text)
    {
        return Err(Invalid::value(format!(
            "Value does not match the regular expression {pattern}."
        )));
    }
    Ok(())
}

/// The `min` and `max` validations of a number.
fn check_range(
    number: f64,
    text: &str,
    cx: &Context<'_>,
    bound: impl Fn(&str) -> Option<f64>,
) -> Result<(), Invalid> {
    if let Some(min) = cx.validation("min")
        && let Some(minimum) = bound(min)
        && number < minimum
    {
        return Err(Invalid::value(format!(
            "Value {text} must be greater than or equal to {min}."
        )));
    }
    if let Some(max) = cx.validation("max")
        && let Some(maximum) = bound(max)
        && number > maximum
    {
        return Err(Invalid {
            message: format!("Value {text} must be less than or equal to {max}."),
            code: "LESS_THAN_OR_EQUAL_TO",
        });
    }
    Ok(())
}

/// The `min` and `max` validations of a date or a date and time.
fn check_dates<T: PartialOrd>(
    value: T,
    cx: &Context<'_>,
    bound: impl Fn(&str) -> Option<T>,
) -> Result<(), Invalid> {
    if let Some(min) = cx.validation("min")
        && bound(min).is_some_and(|minimum| value < minimum)
    {
        return Err(Invalid::value(format!("Value must be on or after {min}.")));
    }
    if let Some(max) = cx.validation("max")
        && bound(max).is_some_and(|maximum| value > maximum)
    {
        return Err(Invalid::value(format!("Value must be on or before {max}.")));
    }
    Ok(())
}

/// The `list.min` and `list.max` validations of a list.
fn check_count(count: usize, cx: &Context<'_>) -> Result<(), Invalid> {
    if let Some(min) = cx
        .validation("list.min")
        .and_then(|min| min.parse::<usize>().ok())
        && count < min
    {
        return Err(Invalid::value(format!(
            "Value must have at least {min} items."
        )));
    }
    if let Some(max) = cx
        .validation("list.max")
        .and_then(|max| max.parse::<usize>().ok())
        && count > max
    {
        return Err(Invalid::value(format!(
            "Value must have at most {max} items."
        )));
    }
    Ok(())
}

/// A URL of one of the schemes Shopify takes, on one of the `allowed_domains` when the
/// definition has them.
fn check_url(text: &str, cx: &Context<'_>) -> Result<(), Invalid> {
    let message = || {
        Invalid::value(format!(
            "Value must be a valid URL with one of the following schemes: {}.",
            URL_SCHEMES.join(", ")
        ))
    };
    let (scheme, rest) = text.split_once(':').ok_or_else(message)?;
    let scheme = scheme.to_ascii_lowercase();
    if !URL_SCHEMES.contains(&scheme.as_str())
        || rest.is_empty()
        || text.contains(char::is_whitespace)
    {
        return Err(message());
    }
    let host = match scheme.as_str() {
        "http" | "https" => {
            let host = rest
                .strip_prefix("//")
                .and_then(|rest| rest.split(['/', '?', '#']).next())
                .map(|authority| authority.rsplit('@').next().unwrap_or(authority))
                .map(|host| host.split(':').next().unwrap_or(host))
                .filter(|host| !host.is_empty())
                .ok_or_else(message)?;
            Some(host.to_ascii_lowercase())
        }
        _ => None,
    };
    if let Some(domains) = cx.validation("allowed_domains") {
        let domains: Vec<String> = serde_json::from_str(domains).unwrap_or_default();
        let allowed = host.as_deref().is_some_and(|host| {
            domains.iter().any(|domain| {
                let domain = domain.to_ascii_lowercase();
                host == domain || host.ends_with(&format!(".{domain}"))
            })
        });
        if !domains.is_empty() && !allowed {
            return Err(Invalid::value(format!(
                "Value must be a URL on one of the allowed domains: {}.",
                domains.join(", ")
            )));
        }
    }
    Ok(())
}

/// A locale: `en`, `pt-BR`, `zh-Hant`.
fn is_locale(text: &str) -> bool {
    let mut parts = text.split('-');
    let language = parts.next().unwrap_or_default();
    let region = parts.next();
    parts.next().is_none()
        && (2..=3).contains(&language.len())
        && language.bytes().all(|b| b.is_ascii_lowercase())
        && region.is_none_or(|region| {
            (region.len() == 2 && region.bytes().all(|b| b.is_ascii_uppercase()))
                || (region.len() == 4 && region.bytes().all(|b| b.is_ascii_alphabetic()))
        })
}

/// `{"amount", "currency_code"}`: the amount a decimal, the currency the shop's ("A numeric
/// amount, with a currency code that matches the store's currency"). Kept with the amount as a
/// string.
fn check_money(item: Item<'_>, cx: &Context<'_>) -> Result<Value, Invalid> {
    let value = item.json()?;
    let amount = match value.get("amount") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Number(number)) => Some(number.to_string()),
        _ => None,
    };
    let amount = amount
        .filter(|amount| parse_decimal(amount).is_some())
        .ok_or_else(|| {
            Invalid::value("Value must be an object with an amount and a currency_code.")
        })?;
    let currency = value
        .get("currency_code")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            Invalid::value("Value must be an object with an amount and a currency_code.")
        })?;
    if !currency.eq_ignore_ascii_case(cx.currency) {
        return Err(Invalid::value(format!(
            "Value currency code must be the shop's currency ({}).",
            cx.currency
        )));
    }
    Ok(json!({"amount": amount, "currency_code": currency.to_ascii_uppercase()}))
}

/// `{"value", "scale_min", "scale_max"}`: the value within the scale, the scale the
/// definition's when it has one. Kept with strings, as the sample writes them.
fn check_rating(item: Item<'_>, cx: &Context<'_>) -> Result<Value, Invalid> {
    let value = item.json()?;
    let number = |field: &str| -> Option<(String, f64)> {
        let text = match value.get(field)? {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            _ => return None,
        };
        let number = parse_decimal(&text)?;
        Some((text, number))
    };
    let message = "Value must be an object with a value, a scale_min and a scale_max.";
    let (Some(rating), Some(min), Some(max)) =
        (number("value"), number("scale_min"), number("scale_max"))
    else {
        return Err(Invalid::value(message));
    };
    if min.1 >= max.1 {
        return Err(Invalid::value(
            "Value scale_min must be less than scale_max.",
        ));
    }
    for (name, scale) in [("scale_min", &min), ("scale_max", &max)] {
        if let Some(defined) = cx.validation(name).and_then(parse_decimal)
            && (defined - scale.1).abs() > f64::EPSILON
        {
            return Err(Invalid::value(format!(
                "Value {name} must be the definition's: {}.",
                cx.validation(name).unwrap_or_default()
            )));
        }
    }
    if rating.1 < min.1 || rating.1 > max.1 {
        return Err(Invalid::value(format!(
            "Value must be between {} and {}.",
            min.0, max.0
        )));
    }
    Ok(json!({"value": rating.0, "scale_min": min.0, "scale_max": max.0}))
}

/// `{"value", "unit"}`: a number and one of the units of the type (or a short unit of a weight,
/// a volume or a dimension, written in full). Kept with the value first.
fn check_measurement(kind: &str, item: Item<'_>, cx: &Context<'_>) -> Result<Value, Invalid> {
    let value = item.json()?;
    let number = match value.get("value") {
        Some(Value::Number(number)) => Some(Value::Number(number.clone())),
        Some(Value::String(text)) if parse_decimal(text).is_some() => {
            serde_json::from_str::<Value>(text).ok()
        }
        _ => None,
    };
    let units = units(kind).unwrap_or_default();
    let unit = value
        .get("unit")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
        .and_then(|unit| full_unit(&unit, units));
    let (Some(number), Some(unit)) = (number, unit) else {
        return Err(Invalid::value(format!(
            "Value must be an object with a value and a unit, one of: {}.",
            units.join(", ")
        )));
    };
    let amount = number.as_f64().unwrap_or_default();
    for (name, below) in [("min", true), ("max", false)] {
        let Some(bound) = cx.validation(name) else {
            continue;
        };
        let Some((limit, limit_unit)) = serde_json::from_str::<Value>(bound).ok().and_then(|b| {
            let value = b
                .get("value")?
                .as_f64()
                .or_else(|| b.get("value")?.as_str()?.parse().ok())?;
            Some((
                value,
                full_unit(&b.get("unit")?.as_str()?.to_ascii_lowercase(), units)?,
            ))
        }) else {
            continue;
        };
        let (Some(here), Some(there)) = (factor(&unit), factor(&limit_unit)) else {
            continue;
        };
        let (value, limit_value) = (amount * here, limit * there);
        let out = if below {
            value < limit_value
        } else {
            value > limit_value
        };
        if out {
            return Err(Invalid::value(if below {
                format!("Value must be greater than or equal to {bound}.")
            } else {
                format!("Value must be less than or equal to {bound}.")
            }));
        }
    }
    Ok(json!({"value": number, "unit": unit}))
}

/// The full name of a unit of `units`: itself, or the full name of a short unit.
fn full_unit(unit: &str, units: &[&str]) -> Option<String> {
    if units.contains(&unit) {
        return Some(unit.to_string());
    }
    SHORT_UNITS
        .iter()
        .find(|(short, full, _)| *short == unit && units.contains(full))
        .map(|(_, full, _)| full.to_string())
}

/// The factor of a full unit to the first unit of its kind, for comparing a value with a bound.
fn factor(unit: &str) -> Option<f64> {
    SHORT_UNITS
        .iter()
        .find(|(_, full, _)| *full == unit)
        .map(|(_, _, factor)| *factor)
}

/// The record types a reference type names.
pub fn reference_types(kind: &str) -> &'static [&'static str] {
    match base(kind) {
        "product_reference" => &["Product"],
        "variant_reference" => &["ProductVariant"],
        "collection_reference" => &["Collection"],
        "page_reference" => &["Page"],
        "article_reference" => &["Article"],
        "customer_reference" => &["Customer"],
        "company_reference" => &["Company"],
        "order_reference" => &["Order"],
        "metaobject_reference" | "mixed_reference" => &["Metaobject"],
        "file_reference" => &["GenericFile", "MediaImage", "Video", "Model3d"],
        "product_taxonomy_value_reference" => &["TaxonomyValue"],
        _ => &[],
    }
}

/// A GID of a record of the shop of a type the reference type names (and the definition's
/// validations allow). Taxonomy values are not records of the shop: their GID is taken as it is.
fn check_reference(kind: &str, item: Item<'_>, cx: &Context<'_>) -> Result<Value, Invalid> {
    let types = reference_types(kind);
    let noun = kind.trim_end_matches("_reference").replace('_', " ");
    let text = item
        .text()
        .ok_or_else(|| Invalid::value(format!("Value must be a {noun} reference.")))?;
    let type_name = gid_type(text).filter(|_| parse_gid(text).is_some());
    let Some(type_name) = type_name.filter(|type_name| types.contains(type_name)) else {
        return Err(Invalid::value(format!(
            "Value must be a {} GID of the form gid://shopify/{}/<id>.",
            noun, types[0]
        )));
    };
    if type_name == "TaxonomyValue" {
        return Ok(json!(text));
    }
    let Some(record) = cx.state.get(text) else {
        return Err(Invalid::value(format!(
            "Value references non-existent resource {text}."
        )));
    };
    if kind == "file_reference"
        && let Some(options) = cx.validation("file_type_options")
    {
        let options: Vec<String> = serde_json::from_str(options).unwrap_or_default();
        let kind_of_file = match type_name {
            "MediaImage" => "Image",
            "Video" => "Video",
            _ => "GenericFile",
        };
        if !options.is_empty() && !options.iter().any(|option| option == kind_of_file) {
            return Err(Invalid::value(format!(
                "Value must be a file of one of the types: {}.",
                options.join(", ")
            )));
        }
    }
    if type_name == "Metaobject" {
        let definition = record.get("definition").and_then(Value::as_str);
        let allowed: Option<Vec<String>> = match kind {
            "metaobject_reference" => cx
                .validation("metaobject_definition_id")
                .map(|id| vec![id.to_string()]),
            _ => cx
                .validation("metaobject_definition_ids")
                .and_then(|ids| serde_json::from_str(ids).ok()),
        };
        if let Some(allowed) = allowed
            && !definition.is_some_and(|definition| allowed.iter().any(|id| id == definition))
        {
            return Err(Invalid::value(
                "Value references a metaobject of a definition the field does not allow.",
            ));
        }
    }
    Ok(json!(text))
}

/// Whether a value of the type is JSON (a `json` string value is `"text"`, quoted).
fn holds_json(kind: &str) -> bool {
    matches!(
        kind,
        "json" | "rich_text_field" | "link" | "money" | "rating"
    ) || units(kind).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shop() -> ShopState {
        let mut state = ShopState::new("test.myshopify.com", crate::state::Clock::System);
        state.insert(crate::state::value::object(json!({
            "__typename": "Product", "id": "gid://shopify/Product/1", "title": "Hat",
        })));
        state
    }

    fn check_with(kind: &str, raw: &str, validations: &[Value]) -> Result<Checked, Invalid> {
        let state = shop();
        let cx = Context {
            state: &state,
            currency: "EUR",
            validations,
        };
        check(kind, raw, &cx)
    }

    fn ok(kind: &str, raw: &str) -> Checked {
        check_with(kind, raw, &[]).unwrap_or_else(|error| panic!("{kind} {raw}: {error:?}"))
    }

    fn refused(kind: &str, raw: &str) -> String {
        check_with(kind, raw, &[])
            .err()
            .unwrap_or_else(|| panic!("{kind} {raw} was accepted"))
            .message
    }

    #[test]
    fn lists_every_type_once_with_a_category() {
        let mut seen = std::collections::HashSet::new();
        for kind in TYPES {
            assert!(seen.insert(kind), "{kind} twice");
            assert!(category(kind).is_some(), "{kind} has no category");
        }
        assert_eq!(UNITS.len(), 32);
        assert!(is_type("list.weight") && !is_type("list.boolean") && !is_type("integer"));
        assert_eq!(
            definition_type("list.number_integer")["supportedValidations"],
            json!([
                {"name": "min", "type": "number_integer"},
                {"name": "max", "type": "number_integer"},
                {"name": "list.min", "type": "number_integer"},
                {"name": "list.max", "type": "number_integer"},
            ])
        );
    }

    #[test]
    fn checks_and_keeps_values_as_shopify() {
        assert_eq!(ok("number_integer", "42").json, json!(42));
        assert_eq!(
            refused("number_integer", "330\n"),
            "Value must be an integer."
        );
        assert_eq!(
            refused("number_integer", "1.5"),
            "Value must be an integer."
        );
        assert_eq!(ok("number_decimal", "10.40").value, "10.40");
        assert_eq!(ok("boolean", "true").json, json!(true));
        assert!(check_with("boolean", "yes", &[]).is_err());
        assert_eq!(ok("json", "{\"a\": 1}").value, "{\"a\":1}");
        assert!(refused("json", "{nope").starts_with("Value is invalid JSON"));
        assert_eq!(
            ok("list.single_line_text_field", "[\"a\", \"b\"]").value,
            "[\"a\",\"b\"]"
        );
        assert_eq!(ok("list.number_integer", "[1, 2]").json, json!([1, 2]));
        assert!(refused("single_line_text_field", "a\nb").contains("single line"));
        assert_eq!(
            ok("money", "{\"amount\": 5.5, \"currency_code\": \"eur\"}").value,
            "{\"amount\":\"5.5\",\"currency_code\":\"EUR\"}"
        );
        assert!(
            refused("money", "{\"amount\": \"5\", \"currency_code\": \"USD\"}").contains("EUR")
        );
        assert_eq!(
            ok("weight", "{\"unit\": \"kg\", \"value\": 2.5}").value,
            "{\"value\":2.5,\"unit\":\"kilograms\"}"
        );
        assert!(check_with("weight", "{\"unit\": \"meters\", \"value\": 2}", &[]).is_err());
        assert_eq!(ok("date", "2026-10-31").value, "2026-10-31");
        assert!(check_with("date", "2026-02-30", &[]).is_err());
        assert!(check_with("date_time", "2024-01-01T12:30:00", &[]).is_ok());
        assert!(check_with("color", "#fff123", &[]).is_ok());
        assert!(check_with("color", "red", &[]).is_err());
        assert!(check_with("url", "https://shopify.com/x", &[]).is_ok());
        assert!(check_with("url", "ftp://x", &[]).is_err());
        assert!(check_with("language", "pt-BR", &[]).is_ok());
        assert!(
            check_with(
                "rating",
                "{\"value\": \"3.5\", \"scale_min\": \"1.0\", \"scale_max\": \"5.0\"}",
                &[]
            )
            .is_ok()
        );
        assert!(
            check_with(
                "rating",
                "{\"value\": \"6\", \"scale_min\": \"1.0\", \"scale_max\": \"5.0\"}",
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn checks_references_against_the_shop() {
        let product = ok("product_reference", "gid://shopify/Product/1");
        assert_eq!(
            product.reference.as_deref(),
            Some("gid://shopify/Product/1")
        );
        assert_eq!(
            refused("product_reference", "gid://shopify/Product/2"),
            "Value references non-existent resource gid://shopify/Product/2."
        );
        assert!(check_with("product_reference", "gid://shopify/Collection/1", &[]).is_err());
        let list = ok("list.product_reference", "[\"gid://shopify/Product/1\"]");
        assert_eq!(
            list.references,
            Some(vec!["gid://shopify/Product/1".to_string()])
        );
    }

    #[test]
    fn applies_the_validations_of_a_definition() {
        let validation = |name: &str, value: &str| json!({"name": name, "value": value});
        let min = [validation("min", "3")];
        assert_eq!(
            check_with("single_line_text_field", "ab", &min)
                .unwrap_err()
                .code,
            "TOO_SHORT"
        );
        let choices = [validation("choices", "[\"red\", \"blue\"]")];
        assert!(check_with("single_line_text_field", "red", &choices).is_ok());
        assert!(check_with("single_line_text_field", "green", &choices).is_err());
        let range = [validation("min", "1"), validation("max", "5")];
        assert_eq!(
            check_with("number_integer", "6", &range).unwrap_err().code,
            "LESS_THAN_OR_EQUAL_TO"
        );
        let regex = [validation("regex", "^[A-Z]+$")];
        assert!(check_with("single_line_text_field", "abc", &regex).is_err());
        let count = [validation("list.max", "1")];
        assert!(check_with("list.single_line_text_field", "[\"a\",\"b\"]", &count).is_err());
        let weight = [validation("max", "{\"unit\":\"g\",\"value\":500}")];
        assert!(check_with("weight", "{\"unit\":\"kilograms\",\"value\":1}", &weight).is_err());
        assert!(check_with("weight", "{\"unit\":\"grams\",\"value\":400}", &weight).is_ok());
        let precision = [validation("max_precision", "1")];
        assert!(check_with("number_decimal", "1.25", &precision).is_err());
    }
}
