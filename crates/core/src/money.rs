//! Amounts as a shop's money format writes them.

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
