//! Translations (`t`), locale-aware dates and unit formatting.

use std::sync::Arc;

use lsf_core::money::format_money;
use lsf_liquid::filters::escape_html;
use lsf_liquid::number::float_to_s;
use lsf_liquid::time::{Tz, strftime, timestamp_time, to_time};
use lsf_liquid::{Context, Environment, Error, FilterArgs, Result, Value};
use serde_json::Value as Json;

use super::site;
use crate::site::Site;
use crate::theme::locales::{Operands, Translations, plural_category, system_translation};

/// Replaces the `{{ name }}` placeholders of a translation with the arguments of `t`, as
/// Shopify does: given no argument at all, `t` keeps them as they are, and a placeholder that
/// has no argument makes the text Shopify's error.
fn interpolate(template: &str, named: &[(String, Value)]) -> String {
    if named.is_empty() || !template.contains("{{") {
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
        match named.iter().rev().find(|(key, _)| key == name) {
            Some((_, value)) => out.push_str(&value.to_str()),
            None => {
                return format!(
                    "I18n Error: Missing interpolation value \"{name}\" for \"{template}\""
                );
            }
        }
        rest = &rest[start + length + 2..];
    }
    out.push_str(rest);
    out
}

/// The `count` argument of `t`, when it is given and not nil.
fn count(named: &[(String, Value)]) -> Option<&Value> {
    named
        .iter()
        .rev()
        .find(|(name, _)| name == "count")
        .map(|(_, count)| count)
        .filter(|count| !count.is_nil())
}

/// The plural form of `count`: `zero` for 0 when there is one, in every language, else the
/// CLDR category of the locale, else `other`. `None` when the form is missing.
fn plural_form<'a>(
    forms: &'a serde_json::Map<String, Json>,
    locale: &str,
    count: &Value,
) -> Option<&'a str> {
    let operands = match count {
        Value::Int(count) => Some(Operands::from(*count)),
        Value::Float(count) => Operands::parse(&float_to_s(*count)),
        // Not a number, a text (`'1'`) included: Shopify takes `other`.
        _ => None,
    };
    let key = match operands {
        Some(operands) if operands.n == 0.0 && forms.contains_key("zero") => "zero",
        Some(operands) => plural_category(locale, &operands),
        None => "other",
    };
    forms
        .get(key)
        .or_else(|| forms.get("other"))
        .and_then(Json::as_str)
}

/// What `t` prints for a key in the first of `locales` that has it: a text, the plural form of
/// its `count`, or, for a group of keys (plural forms without a `count` too), the group as Ruby
/// prints a hash (`{"one"=>"{{ count }} item", ...}`), as Shopify does. `None` when no locale
/// has the key, or when the plural form of `count` is missing.
fn translate_in(
    locales: &[(String, Arc<Translations>)],
    key: &str,
    named: &[(String, Value)],
) -> Option<String> {
    let (locale, node) = locales
        .iter()
        .find_map(|(locale, translations)| translations.lookup(key).map(|node| (locale, node)))?;
    match (node, count(named)) {
        (Json::String(text), _) => Some(interpolate(text, named)),
        (Json::Object(forms), Some(count)) => {
            plural_form(forms, locale, count).map(|text| interpolate(text, named))
        }
        (group, _) => Some(Value::from(group).to_str().into_owned()),
    }
}

/// Looks a key up in the request's locale, then in the theme's default locale, then in
/// Shopify's built-in translations. Returns the raw (unescaped) text.
pub fn translate(site: &Site, key: &str, named: &[(String, Value)]) -> Option<String> {
    if let Some(text) = translate_in(site.translations(), key, named) {
        return Some(text);
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
    // As Ruby's I18n reads it: `.a..b` is `a.b`.
    let key = input
        .to_str()
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(".");
    let translated = match crate::app_extensions::render::extension_of(ctx) {
        // A key of a theme app extension's Liquid: looked up in its own `locales/`, never in
        // the theme's.
        Some(extension) => translate_in(
            &extension.translations(&site.request.locale),
            &key,
            &args.named,
        ),
        None => translate(site, &key, &args.named),
    };
    match translated {
        Some(text) => {
            // Translations are escaped unless their key ends in `_html`.
            let is_html = key.ends_with("_html") || key.ends_with(".html");
            Ok(Value::from(if is_html { text } else { escape_html(&text) }))
        }
        None => {
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

/// The time `date` and `time_tag` read: what Liquid reads, and a Unix timestamp with a fraction
/// (`1772323200.9`, or as a text), which a storefront reads and the reference gem leaves as it
/// is.
fn storefront_time(input: &Value, ctx: &Context) -> Option<chrono::DateTime<Tz>> {
    let decimal = |text: &str| {
        text.split_once('.').is_some_and(|(whole, fraction)| {
            [whole, fraction]
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
    };
    match input {
        Value::Float(seconds) => timestamp_time(*seconds, ctx),
        Value::Str(text) if decimal(text) => timestamp_time(text.parse().ok()?, ctx),
        _ => to_time(input, ctx),
    }
}

/// `date` and `time_tag` without a format print the locale's `default` format, as
/// `format: 'default'` does.
const DEFAULT_FORMAT: &str = "default";

fn date(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let pattern = match (args.named("format"), args.get(0)) {
        (None, Some(format)) => format.to_str().into_owned(),
        (name, _) => {
            let name = name.map_or(DEFAULT_FORMAT.into(), Value::to_str);
            date_pattern(site(ctx)?, &name).ok_or_else(|| {
                Error::argument(format!(
                    "date format '{name}' is not defined in the locale file"
                ))
            })?
        }
    };
    if pattern.is_empty() {
        return Ok(input.clone());
    }
    match storefront_time(input, ctx) {
        Some(time) => strftime(&time, &pattern)
            .map(Value::from)
            .map_err(Error::argument),
        None => Ok(input.clone()),
    }
}

fn time_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let Some(time) = storefront_time(input, ctx) else {
        return Ok(input.clone());
    };
    let pattern = match (args.named("format"), args.get(0)) {
        (None, Some(format)) => format.to_str().into_owned(),
        (name, _) => {
            let name = name.map_or(DEFAULT_FORMAT.into(), Value::to_str);
            date_pattern(site(ctx)?, &name).unwrap_or_else(|| "%B %-d, %Y".to_string())
        }
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
    let grams = lsf_liquid::number::to_number(input).to_f64();
    let unit = args
        .get(0)
        .map(|unit| unit.to_str().into_owned())
        .unwrap_or_else(|| "kg".to_string());
    let amount = crate::drops::product::weight_value(grams, &unit);
    Ok(Value::from(format!("{} {unit}", amount.to_str())))
}

fn unit_price_with_measurement(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    // The price is either cents or an already formatted money string.
    let price = match input {
        Value::Str(text) => text.to_string(),
        other => {
            let cents = lsf_liquid::number::to_number(other).to_f64().round() as i64;
            let site = site(ctx)?;
            let (format, _) = super::money::money_formats(&site.store.shop, Some(site.currency()));
            format_money(cents, &format)
        }
    };
    let measurement = args.at(0);
    let unit = measurement.get("reference_unit").to_str().into_owned();
    let reference = measurement.get("reference_value");
    let prefix = match lsf_liquid::number::to_number(&reference).to_f64() {
        value if value == 1.0 || value == 0.0 => String::new(),
        value => compact_number(value),
    };
    Ok(Value::from(format!("{price}/{prefix}{unit}")))
}

fn pluralize(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    let count = lsf_liquid::number::to_number(input).to_f64();
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
