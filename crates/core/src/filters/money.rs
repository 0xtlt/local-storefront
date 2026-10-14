//! `money` and its variants.

use lsf_liquid::number::{Number, to_number};
use lsf_liquid::{Context, Environment, FilterArgs, Result, Value};

use super::site;
use crate::drops::metafield::MoneyDrop;
use crate::store::{Shop, reference};

/// Formats a whole number with a thousands separator.
fn group(units: i64, separator: &str) -> String {
    let digits = units.abs().to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push_str(separator);
        }
        out.push(digit);
    }
    if units < 0 { format!("-{out}") } else { out }
}

fn with_decimals(cents: i64, thousands: &str, decimal: &str) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let cents = cents.abs();
    format!(
        "{sign}{}{decimal}{:02}",
        group(cents / 100, thousands),
        cents % 100
    )
}

fn without_decimals(cents: i64, thousands: &str) -> String {
    // Round half away from zero to the nearest unit.
    let units = if cents >= 0 {
        (cents + 50) / 100
    } else {
        -((-cents + 50) / 100)
    };
    group(units, thousands)
}

/// Substitutes the amount into a Shopify money format such as `${{amount}}` or
/// `{{ amount_with_comma_separator }} €`.
pub fn format_money(cents: i64, format: &str) -> String {
    let mut out = String::with_capacity(format.len() + 8);
    let mut rest = format;
    while let Some(start) = rest.find("{{") {
        let Some(length) = rest[start..].find("}}") else {
            break;
        };
        out.push_str(&rest[..start]);
        let amount = match rest[start + 2..start + length].trim() {
            "amount" => with_decimals(cents, ",", "."),
            "amount_no_decimals" => without_decimals(cents, ","),
            "amount_with_comma_separator" => with_decimals(cents, ".", ","),
            "amount_no_decimals_with_comma_separator" => without_decimals(cents, "."),
            "amount_with_apostrophe_separator" => with_decimals(cents, "'", "."),
            "amount_no_decimals_with_space_separator" => without_decimals(cents, " "),
            "amount_with_space_separator" => with_decimals(cents, " ", ","),
            "amount_with_period_and_space_separator" => with_decimals(cents, " ", "."),
            other => format!("{{{{{other}}}}}"),
        };
        out.push_str(&amount);
        rest = &rest[start + length + 2..];
    }
    out.push_str(rest);
    out
}

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
/// Shopify's defaults for the currency of a money metafield written in another one.
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

fn with_format(
    input: &Value,
    ctx: &Context,
    pick: impl Fn((String, String)) -> String,
) -> Result<Value> {
    let Some(cents) = cents(input) else {
        return Ok(Value::empty_string());
    };
    let currency = input
        .downcast::<MoneyDrop>()
        .map(|money| money.currency.as_str());
    let format = pick(money_formats(&site(ctx)?.store.shop, currency));
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
    let format = &site(ctx)?.store.shop.money_format;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_money() {
        assert_eq!(format_money(1000, "${{amount}}"), "$10.00");
        assert_eq!(
            format_money(123456789, "${{ amount }} USD"),
            "$1,234,567.89 USD"
        );
        assert_eq!(
            format_money(113465, "€{{amount_with_comma_separator}}"),
            "€1.134,65"
        );
        assert_eq!(
            format_money(113465, "{{amount_no_decimals}} kr"),
            "1,135 kr"
        );
        assert_eq!(
            format_money(113465, "{{amount_no_decimals_with_comma_separator}}"),
            "1.135"
        );
        assert_eq!(
            format_money(113465, "{{amount_with_apostrophe_separator}}"),
            "1'134.65"
        );
        assert_eq!(format_money(-550, "${{amount}}"), "$-5.50");
        assert_eq!(
            format_money(5, "<span class=money>${{amount}}</span>"),
            "<span class=money>$0.05</span>"
        );
    }
}
