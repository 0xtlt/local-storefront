//! `money` and its variants.

use lsf_liquid::number::{Number, to_number};
use lsf_liquid::{Context, Environment, FilterArgs, Result, Value};

use lsf_core::money::format_money;
use lsf_core::store::{Shop, reference};

use super::site;
use crate::drops::metafield::MoneyDrop;

/// The amount in cents a money filter receives. `None` for `nil`, which prints nothing.
fn cents(input: &Value) -> Option<i64> {
    match input {
        Value::Nil => None,
        Value::Str(text) if text.trim().is_empty() => None,
        other => Some(match to_number(other) {
            Number::Int(cents) => cents,
            other => other.to_f64().round() as i64,
        }),
    }
}

/// The formats an amount is printed with, without and with its currency: the shop's, or
/// Shopify's defaults for another currency (a market's, or that of a money metafield written
/// in another one), which the shop's settings do not change.
pub fn money_formats(shop: &Shop, currency: Option<&str>) -> (String, String) {
    match currency.filter(|code| !code.eq_ignore_ascii_case(&shop.currency)) {
        Some(code) => reference::currency(code).map_or_else(
            || {
                let format = format!("{{{{amount}}}} {}", code.to_uppercase());
                (format.clone(), format)
            },
            |currency| {
                (
                    currency.money_format.to_string(),
                    currency.money_with_currency_format.to_string(),
                )
            },
        ),
        None => (
            shop.money_format.clone(),
            shop.money_with_currency_format.clone(),
        ),
    }
}

/// The formats of [`money_formats`] for a value: a money metafield brings its currency, the
/// others are in the visitor's.
fn formats_of(input: &Value, ctx: &Context) -> Result<(String, String)> {
    let site = site(ctx)?;
    let currency = input
        .downcast::<MoneyDrop>()
        .map_or_else(|| site.currency(), |money| money.currency.as_str());
    Ok(money_formats(&site.store.shop, Some(currency)))
}

fn with_format(
    input: &Value,
    ctx: &Context,
    pick: impl Fn((String, String)) -> String,
) -> Result<Value> {
    let Some(cents) = cents(input) else {
        return Ok(Value::empty_string());
    };
    let format = pick(formats_of(input, ctx)?);
    Ok(Value::from(format_money(cents, &format)))
}

fn money(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    with_format(input, ctx, |(plain, _)| plain)
}

fn money_with_currency(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    with_format(input, ctx, |(_, with_currency)| with_currency)
}

/// The first `{{ ... }}` placeholder of a money format, i.e. the format minus its symbols.
fn amount_placeholder(format: &str) -> String {
    match (format.find("{{"), format.find("}}")) {
        (Some(start), Some(end)) if end > start => format[start..end + 2].to_string(),
        _ => "{{amount}}".to_string(),
    }
}

fn money_without_currency(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    with_format(input, ctx, |(plain, _)| amount_placeholder(&plain))
}

fn money_without_trailing_zeros(input: &Value, _args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let Some(cents) = cents(input) else {
        return Ok(Value::empty_string());
    };
    let (format, _) = formats_of(input, ctx)?;
    let format = &format;
    let formatted = format_money(cents, format);
    if cents % 100 != 0 {
        return Ok(Value::from(formatted));
    }
    // Drop the zero decimals, whichever separator the format uses.
    let placeholder = amount_placeholder(format);
    let amount = format_money(cents, &placeholder);
    let trimmed = amount
        .strip_suffix(".00")
        .or_else(|| amount.strip_suffix(",00"))
        .unwrap_or(&amount);
    Ok(Value::from(formatted.replacen(&amount, trimmed, 1)))
}

fn money_amount(input: &Value, _args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    Ok(match cents(input) {
        Some(cents) => Value::Float(cents as f64 / 100.0),
        None => Value::Nil,
    })
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("money", money);
    env.register_filter("money_with_currency", money_with_currency);
    env.register_filter("money_without_currency", money_without_currency);
    env.register_filter("money_without_trailing_zeros", money_without_trailing_zeros);
    env.register_filter("money_amount", money_amount);
}
