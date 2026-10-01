//! Translations (`t`), locale-aware dates and unit formatting.

use serde_json::Value as Json;
use slt_liquid::filters::escape_html;
use slt_liquid::number::float_to_s;
use slt_liquid::time::{strftime, to_time};
use slt_liquid::{Context, Environment, Error, FilterArgs, Result, Value};

use super::money::format_money;
use super::site;
use crate::site::Site;
use crate::theme::locales::{plural_category, system_translation};

/// Replaces `{{ name }}` placeholders with the given arguments.
fn interpolate(template: &str, named: &[(String, Value)]) -> String {
    if !template.contains("{{") {
        return template.to_string();
    }
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let Some(length) = rest[start..].find("}}") else {
            break;
        };
        out.push_str(&rest[..start]);
        let name = rest[start + 2..start + length].trim();
        if let Some((_, value)) = named.iter().rev().find(|(key, _)| key == name) {
            out.push_str(&value.to_str());
        }
        rest = &rest[start + length + 2..];
    }
    out.push_str(rest);
    out
}

/// Picks the plural form of a translation node for a count.
fn plural_form<'a>(node: &'a Json, locale: &str, count: Option<i64>) -> Option<&'a str> {
    match node {
        Json::String(text) => Some(text),
        Json::Object(forms) => {
            let category = plural_category(locale, count.unwrap_or(0));
            forms
                .get(category)
                .or_else(|| forms.get("other"))
                .or_else(|| forms.values().next())
                .and_then(Json::as_str)
        }
        _ => None,
    }
}

/// Looks a key up in the request's locale, then in the theme's default locale, then in
/// Shopify's built-in translations. Returns the raw (unescaped) text.
pub fn translate(site: &Site, key: &str, named: &[(String, Value)]) -> Option<String> {
    let count = named
        .iter()
        .find(|(name, _)| name == "count")
        .and_then(|(_, value)| match value {
            Value::Int(count) => Some(*count),
            Value::Float(count) => Some(*count as i64),
            Value::Str(text) => text.parse().ok(),
            _ => None,
        });
    let locale = &site.request.locale;
    let default_locale = site.theme.default_locale();
    for candidate in [locale.as_str(), default_locale.as_str()] {
        let translations = site.theme.translations(candidate);
        if let Some(text) = translations
            .lookup(key)
            .and_then(|node| plural_form(node, candidate, count))
        {
            return Some(interpolate(text, named));
        }
    }
    // Shopify's own keys use `%{name}` placeholders.
    system_translation(key).map(|text| {
        let mut text = text.to_string();
        for (name, value) in named {
            text = text.replace(&format!("%{{{name}}}"), &value.to_str());
        }
        text
    })
}

fn t(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let site = site(ctx)?;
    let key = input.to_str();
    match translate(site, &key, &args.named) {
        Some(text) => {
            // Translations are escaped unless their key ends in `_html`.
            let is_html = key.ends_with("_html") || key.ends_with(".html");
            Ok(Value::from(if is_html { text } else { escape_html(&text) }))
        }
        None => {
            if let Some(default) = args.named("default") {
                return Ok(default.clone());
            }
            ctx.warn(format!("missing translation '{key}'"));
            Ok(Value::from(format!(
                "Translation missing: {}.{key}",
                site.request.locale
            )))
        }
    }
}

/// The `strftime` pattern of a named date format in English.
fn named_format(name: &str) -> Option<&'static str> {
    Some(match name {
        "abbreviated_date" => "%b %-d, %Y",
        "basic" => "%-m/%-d/%Y",
        "date" | "month_day_year" => "%B %-d, %Y",
        "date_at_time" => "%B %-d, %Y at %-I:%M %P",
        "default" => "%A, %B %-d, %Y at %-I:%M %P %z",
        "on_date" => "on %b %-d, %Y",
        "short" => "%-d %b %H:%M",
        "long" => "%B %d, %Y %H:%M",
        "month_and_year" => "%B %Y",
        _ => return None,
    })
}

/// The pattern for `date: format: 'name'`: a custom format from the locale file's
/// `date_formats`, or one of Shopify's named formats.
pub(super) fn date_pattern(site: &Site, name: &str) -> Option<String> {
    translate(site, &format!("date_formats.{name}"), &[])
        .or_else(|| named_format(name).map(str::to_string))
}

fn date(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let pattern = match args.named("format") {
        Some(name) => {
            let name = name.to_str();
            date_pattern(site(ctx)?, &name).ok_or_else(|| {
                Error::argument(format!(
                    "date format '{name}' is not defined in the locale file"
                ))
            })?
        }
        None => match args.get(0) {
            Some(format) => format.to_str().into_owned(),
            None => return Err(Error::wrong_arity(1, "2")),
        },
    };
    if pattern.is_empty() {
        return Ok(input.clone());
    }
    match to_time(input, ctx) {
        Some(time) => strftime(&time, &pattern)
            .map(Value::from)
            .map_err(Error::argument),
        None => Ok(input.clone()),
    }
}

fn time_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let Some(time) = to_time(input, ctx) else {
        return Ok(input.clone());
    };
    let pattern = match (args.named("format"), args.get(0)) {
        (Some(name), _) => {
            date_pattern(site(ctx)?, &name.to_str()).unwrap_or_else(|| "%B %-d, %Y".to_string())
        }
        (None, Some(format)) => format.to_str().into_owned(),
        (None, None) => "%B %-d, %Y at %-I:%M %P".to_string(),
    };
    let text = strftime(&time, &pattern).map_err(Error::argument)?;
    let datetime = match args.named("datetime") {
        Some(format) => strftime(&time, &format.to_str()).map_err(Error::argument)?,
        None => time
            .with_timezone(&chrono::Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
    };
    Ok(Value::from(format!(
        "<time datetime=\"{datetime}\">{text}</time>"
    )))
}

/// Prints a number without a trailing `.0`.
fn compact_number(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    let text = float_to_s(rounded);
    text.strip_suffix(".0").map(str::to_string).unwrap_or(text)
}

fn weight_with_unit(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let grams = slt_liquid::number::to_number(input).to_f64();
    let unit = args
        .get(0)
        .map(|unit| unit.to_str().into_owned())
        .unwrap_or_else(|| "kg".to_string());
    let amount = match unit.as_str() {
        "g" => grams,
        "oz" => grams / 28.349_523_125,
        "lb" => grams / 453.592_37,
        _ => grams / 1000.0,
    };
    Ok(Value::from(format!("{} {unit}", compact_number(amount))))
}

fn unit_price_with_measurement(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    // The price is either cents or an already formatted money string.
    let price = match input {
        Value::Str(text) => text.to_string(),
        other => {
            let cents = slt_liquid::number::to_number(other).to_f64().round() as i64;
            format_money(cents, &site(ctx)?.store.shop.money_format)
        }
    };
    let measurement = args.at(0);
    let unit = measurement.get("reference_unit").to_str().into_owned();
    let reference = measurement.get("reference_value");
    let prefix = match slt_liquid::number::to_number(&reference).to_f64() {
        value if value == 1.0 || value == 0.0 => String::new(),
        value => compact_number(value),
    };
    Ok(Value::from(format!("{price}/{prefix}{unit}")))
}

fn pluralize(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let count = slt_liquid::number::to_number(input).to_f64();
    Ok(if count == 1.0 { args.at(0) } else { args.at(1) })
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("t", t);
    env.register_filter("translate", t);
    env.register_filter("date", date);
    env.register_filter("time_tag", time_tag);
    env.register_filter("weight_with_unit", weight_with_unit);
    env.register_filter("unit_price_with_measurement", unit_price_with_measurement);
    env.register_filter("pluralize", pluralize);
}
