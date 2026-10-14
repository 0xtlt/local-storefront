//! Numbers, as Prettier prints them in JavaScript, JSON and CSS.

/// Prettier's `printNumber`: lowercase, no `+` nor leading zeros in the exponent, a digit
/// before the dot, no zeros at the end of the decimals.
pub(crate) fn print_number(raw: &str) -> String {
    if raw.len() == 1 {
        return raw.to_string();
    }
    let lower = raw.to_ascii_lowercase();
    let (sign, unsigned) = match lower.strip_prefix(['+', '-']) {
        Some(unsigned) => (&lower[..1], unsigned),
        None => ("", lower.as_str()),
    };
    let (mantissa, exponent) = match unsigned.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (unsigned, None),
    };
    let well_formed = !mantissa.is_empty()
        && mantissa
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        && mantissa.matches('.').count() <= 1
        && exponent.is_none_or(|exponent| {
            let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        });
    if !well_formed {
        return lower;
    }

    let mut printed = String::from(sign);
    let (whole, decimals) = match mantissa.split_once('.') {
        Some((whole, decimals)) => (whole, Some(decimals)),
        None => (mantissa, None),
    };
    printed.push_str(if whole.is_empty() { "0" } else { whole });
    if let Some(decimals) = decimals {
        // `1.50` → `1.5`, `1.00` → `1.0`, `1.` → `1`.
        let kept = decimals.trim_end_matches('0');
        let kept = if kept.is_empty() && !decimals.is_empty() {
            "0"
        } else {
            kept
        };
        if !kept.is_empty() {
            printed.push('.');
            printed.push_str(kept);
        }
    }
    if let Some(exponent) = exponent {
        let (exponent_sign, digits) = match exponent.strip_prefix('-') {
            Some(digits) => ("-", digits),
            None => ("", exponent.strip_prefix('+').unwrap_or(exponent)),
        };
        let digits = digits.trim_start_matches('0');
        // `1e0`: no exponent.
        if !digits.is_empty() {
            printed.push('e');
            printed.push_str(exponent_sign);
            printed.push_str(digits);
        }
    }
    printed
}

/// The number as CSS prints it: the same, without a `.0`.
pub(crate) fn print_css_number(raw: &str) -> String {
    let printed = print_number(raw);
    match printed.split_once(".0") {
        Some((whole, rest)) if rest.is_empty() || rest.starts_with('e') => format!("{whole}{rest}"),
        _ => printed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_numbers_as_prettier_does() {
        for (raw, printed, css) in [
            ("1.0", "1.0", "1"),
            ("1.50", "1.5", "1.5"),
            ("1.00", "1.0", "1"),
            ("1e+10", "1e10", "1e10"),
            ("1E5", "1e5", "1e5"),
            ("1e-05", "1e-5", "1e-5"),
            ("1e0", "1", "1"),
            (".5", "0.5", "0.5"),
            ("-.5", "-0.5", "-0.5"),
            ("+.50", "+0.5", "+0.5"),
            ("5.", "5", "5"),
            ("010", "010", "010"),
            ("0", "0", "0"),
            ("1.0e3", "1.0e3", "1e3"),
        ] {
            assert_eq!(print_number(raw), printed, "{raw}");
            assert_eq!(print_css_number(raw), css, "{raw} in CSS");
        }
    }
}
