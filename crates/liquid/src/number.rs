//! Number coercion and arithmetic with the semantics of Ruby Liquid.
//!
//! Ruby Liquid converts floats to `BigDecimal` before doing arithmetic, which is why
//! `0.1 | plus: 0.2` prints `0.3`. We reproduce that with a decimal type and fall back to binary
//! floating point only when the decimal range is exceeded.

use std::str::FromStr;

use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::{Decimal, RoundingStrategy};

use crate::error::Error;
use crate::value::Value;

/// A number after `Utils.to_number`: either an integer or a decimal.
#[derive(Clone, Copy, Debug)]
pub enum Number {
    Int(i64),
    Dec(Decimal),
    /// A float that does not fit a decimal (very large, very small, NaN or infinite).
    Float(f64),
}

impl Number {
    pub fn to_f64(self) -> f64 {
        match self {
            Number::Int(i) => i as f64,
            Number::Dec(d) => decimal_to_f64(d),
            Number::Float(f) => f,
        }
    }

    /// Converts back to a Liquid value: integers stay integers, everything else is a float.
    pub fn into_value(self) -> Value {
        match self {
            Number::Int(i) => Value::Int(i),
            other => Value::Float(other.to_f64()),
        }
    }

    fn is_zero(self) -> bool {
        match self {
            Number::Int(i) => i == 0,
            Number::Dec(d) => d.is_zero(),
            Number::Float(f) => f == 0.0,
        }
    }

    fn as_decimal(self) -> Option<Decimal> {
        match self {
            Number::Int(i) => Some(Decimal::from(i)),
            Number::Dec(d) => Some(d),
            Number::Float(_) => None,
        }
    }

    pub fn partial_cmp(self, other: Number) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Number::Int(a), Number::Int(b)) => Some(a.cmp(&b)),
            (a, b) => match (a.as_decimal(), b.as_decimal()) {
                (Some(x), Some(y)) => Some(x.cmp(&y)),
                _ => a.to_f64().partial_cmp(&b.to_f64()),
            },
        }
    }
}

fn float_to_number(f: f64) -> Number {
    if !f.is_finite() {
        return Number::Float(f);
    }
    // `BigDecimal(float.to_s)`: go through the shortest round-trip representation.
    match Decimal::from_str(&format!("{f}")) {
        Ok(d) => Number::Dec(d),
        Err(_) => match Decimal::from_f64(f) {
            Some(d) if d.to_f64() == Some(f) => Number::Dec(d),
            _ => Number::Float(f),
        },
    }
}

/// Ruby Liquid's `Utils.to_number`.
pub fn to_number(value: &Value) -> Number {
    match value {
        Value::Int(i) => Number::Int(*i),
        Value::Float(f) => float_to_number(*f),
        Value::Str(s) => {
            let trimmed = s.trim_matches(is_ruby_space);
            if is_decimal_literal(trimmed) {
                match Decimal::from_str(trimmed) {
                    Ok(d) => Number::Dec(d),
                    Err(_) => Number::Float(trimmed.parse().unwrap_or(0.0)),
                }
            } else {
                Number::Int(string_to_i(s))
            }
        }
        Value::Object(object) => match object.to_value() {
            Some(scalar @ (Value::Int(_) | Value::Float(_))) => to_number(&scalar),
            _ => Number::Int(0),
        },
        _ => Number::Int(0),
    }
}

/// Matches `/\A-?\d+\.\d+\z/`.
fn is_decimal_literal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    match s.split_once('.') {
        Some((int, frac)) => {
            !int.is_empty()
                && !frac.is_empty()
                && int.bytes().all(|b| b.is_ascii_digit())
                && frac.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

/// The whitespace characters Ruby's `strip` and `\s` recognise.
pub fn is_ruby_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{0b}' | '\u{0c}')
}

/// Ruby's `String#to_i`: parses a leading integer and ignores the rest.
pub fn string_to_i(s: &str) -> i64 {
    let s = s.trim_start_matches(is_ruby_space);
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut value: i64 = 0;
    let mut last_was_digit = false;
    for b in digits.bytes() {
        if b.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add(i64::from(b - b'0'));
            last_was_digit = true;
        } else if b == b'_' && last_was_digit {
            last_was_digit = false;
        } else {
            break;
        }
    }
    if negative { -value } else { value }
}

/// Ruby Liquid's `Utils.to_integer`: strict, raises `invalid integer` otherwise.
pub fn to_integer(value: &Value) -> Result<i64, Error> {
    if let Value::Int(i) = value {
        return Ok(*i);
    }
    parse_strict_integer(&value.to_str()).ok_or_else(|| Error::argument("invalid integer"))
}

/// Ruby's `Kernel#Integer` for strings, in base 10 with the usual prefixes.
fn parse_strict_integer(s: &str) -> Option<i64> {
    let s = s.trim_matches(is_ruby_space);
    let (negative, rest) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let lower = rest.to_ascii_lowercase();
    let (radix, digits) = if let Some(hex) = lower.strip_prefix("0x") {
        (16, hex)
    } else if let Some(bin) = lower.strip_prefix("0b") {
        (2, bin)
    } else if let Some(oct) = lower.strip_prefix("0o") {
        (8, oct)
    } else if lower.len() > 1 && lower.starts_with('0') {
        (8, &lower[1..])
    } else {
        (10, lower.as_str())
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let cleaned: String = digits.chars().filter(|c| *c != '_').collect();
    let value = i64::from_str_radix(&cleaned, radix).ok()?;
    Some(if negative { -value } else { value })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

/// Ruby Liquid's `apply_operation`.
pub fn apply(op: Op, left: &Value, right: &Value) -> Result<Value, Error> {
    let a = to_number(left);
    let b = to_number(right);
    if matches!(op, Op::Div | Op::Mod) && b.is_zero() {
        // Integer division and every modulo by zero raise; a decimal divided by zero is
        // infinite, as with Ruby's BigDecimal.
        if op == Op::Mod || matches!((a, b), (Number::Int(_), Number::Int(_))) {
            return Err(Error::zero_division());
        }
        let numerator = a.to_f64();
        return Ok(Value::Float(if numerator == 0.0 || numerator.is_nan() {
            f64::NAN
        } else if numerator > 0.0 {
            f64::INFINITY
        } else {
            f64::NEG_INFINITY
        }));
    }
    if let (Number::Int(x), Number::Int(y)) = (a, b) {
        let result = match op {
            Op::Add => x.checked_add(y),
            Op::Sub => x.checked_sub(y),
            Op::Mul => x.checked_mul(y),
            Op::Div => Some(floor_div(x, y)),
            Op::Mod => Some(floor_mod(x, y)),
        };
        if let Some(result) = result {
            return Ok(Value::Int(result));
        }
    }
    if let (Some(x), Some(y)) = (a.as_decimal(), b.as_decimal()) {
        let result = match op {
            Op::Add => x.checked_add(y),
            Op::Sub => x.checked_sub(y),
            Op::Mul => x.checked_mul(y),
            Op::Div => x.checked_div(y),
            Op::Mod => x.checked_rem(y).map(|r| {
                if !r.is_zero() && (r.is_sign_negative() != y.is_sign_negative()) {
                    r + y
                } else {
                    r
                }
            }),
        };
        if let Some(result) = result {
            return Ok(Value::Float(decimal_to_f64(result)));
        }
    }
    let (x, y) = (a.to_f64(), b.to_f64());
    Ok(Value::Float(match op {
        Op::Add => x + y,
        Op::Sub => x - y,
        Op::Mul => x * y,
        Op::Div => x / y,
        Op::Mod => x - y * (x / y).floor(),
    }))
}

/// The nearest float to a decimal. Going through the decimal's string form is correctly
/// rounded, unlike dividing the mantissa by a power of ten.
fn decimal_to_f64(d: Decimal) -> f64 {
    d.to_string()
        .parse()
        .unwrap_or_else(|_| d.to_f64().unwrap_or(0.0))
}

fn floor_div(a: i64, b: i64) -> i64 {
    let q = a.wrapping_div(b);
    if a.wrapping_rem(b) != 0 && ((a < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

fn floor_mod(a: i64, b: i64) -> i64 {
    let r = a.wrapping_rem(b);
    if r != 0 && ((r < 0) != (b < 0)) {
        r + b
    } else {
        r
    }
}

/// `round` with Ruby's half-away-from-zero rule.
pub fn round(value: &Value, digits: &Value) -> Value {
    let digits_number = to_number(digits);
    let digits_i = match digits_number {
        Number::Int(i) => i,
        other => other.to_f64() as i64,
    };
    let is_zero_digits = digits_number.is_zero();
    match to_number(value) {
        Number::Int(i) => {
            if digits_i >= 0 {
                Value::Int(i)
            } else {
                let factor = 10i64.checked_pow(digits_i.unsigned_abs() as u32);
                match factor {
                    Some(factor) => {
                        let half = factor / 2;
                        let rounded = if i >= 0 {
                            (i + half) / factor * factor
                        } else {
                            -((-i + half) / factor * factor)
                        };
                        Value::Int(rounded)
                    }
                    None => Value::Int(0),
                }
            }
        }
        Number::Dec(d) => {
            let rounded = if digits_i >= 0 {
                d.round_dp_with_strategy(
                    digits_i.min(28) as u32,
                    RoundingStrategy::MidpointAwayFromZero,
                )
            } else {
                let factor = Decimal::from(10i64.pow(digits_i.unsigned_abs().min(18) as u32));
                (d / factor).round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
                    * factor
            };
            // BigDecimal#round returns an integer when asked for zero or fewer digits.
            if digits_i <= 0 {
                Value::Int(rounded.to_i64().unwrap_or(0))
            } else {
                Value::Float(decimal_to_f64(rounded))
            }
        }
        Number::Float(f) => {
            if is_zero_digits {
                Value::Int(f.round() as i64)
            } else {
                Value::Float(f)
            }
        }
    }
}

pub fn ceil(value: &Value) -> Value {
    match to_number(value) {
        Number::Int(i) => Value::Int(i),
        Number::Dec(d) => Value::Int(d.ceil().to_i64().unwrap_or(0)),
        Number::Float(f) => Value::Int(f.ceil() as i64),
    }
}

pub fn floor(value: &Value) -> Value {
    match to_number(value) {
        Number::Int(i) => Value::Int(i),
        Number::Dec(d) => Value::Int(d.floor().to_i64().unwrap_or(0)),
        Number::Float(f) => Value::Int(f.floor() as i64),
    }
}

pub fn abs(value: &Value) -> Value {
    match to_number(value) {
        Number::Int(i) => Value::Int(i.wrapping_abs()),
        Number::Dec(d) => Value::Float(decimal_to_f64(d.abs())),
        Number::Float(f) => Value::Float(f.abs()),
    }
}

/// Ruby's `Float#to_s`: the shortest representation that round-trips, in fixed notation for
/// "reasonable" magnitudes and scientific notation otherwise.
pub fn float_to_s(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    // `{:e}` yields the shortest round-trip digits as `d.ddde<exp>`.
    let sci = format!("{:e}", f.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    // Position of the decimal point relative to the start of `digits`.
    let decpt = exponent + 1;
    let mut out = String::with_capacity(digits.len() + 8);
    if f < 0.0 {
        out.push('-');
    }
    if 0 < decpt && decpt <= 16 {
        let decpt = decpt as usize;
        if digits.len() <= decpt {
            out.push_str(&digits);
            out.extend(std::iter::repeat_n('0', decpt - digits.len()));
            out.push_str(".0");
        } else {
            out.push_str(&digits[..decpt]);
            out.push('.');
            out.push_str(&digits[decpt..]);
        }
    } else if -4 < decpt && decpt <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-decpt) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        out.push('.');
        if digits.len() > 1 {
            out.push_str(&digits[1..]);
        } else {
            out.push('0');
        }
        out.push('e');
        out.push(if exponent < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exponent.abs()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_floats_like_ruby() {
        assert_eq!(float_to_s(1.0), "1.0");
        assert_eq!(float_to_s(0.3), "0.3");
        assert_eq!(float_to_s(-12.5), "-12.5");
        assert_eq!(float_to_s(1e15), "1000000000000000.0");
        assert_eq!(float_to_s(1e16), "1.0e+16");
        assert_eq!(float_to_s(1.5e20), "1.5e+20");
        assert_eq!(float_to_s(0.0001), "0.0001");
        assert_eq!(float_to_s(0.00001), "1.0e-05");
        assert_eq!(float_to_s(123456.789), "123456.789");
    }

    #[test]
    fn integer_division_floors() {
        assert_eq!(floor_div(7, 2), 3);
        assert_eq!(floor_div(-7, 2), -4);
        assert_eq!(floor_mod(-7, 2), 1);
        assert_eq!(floor_mod(7, -2), -1);
    }

    #[test]
    fn parses_integers_like_ruby() {
        assert_eq!(string_to_i("12abc"), 12);
        assert_eq!(string_to_i("  -4 "), -4);
        assert_eq!(string_to_i("abc"), 0);
        assert_eq!(parse_strict_integer(" 42 "), Some(42));
        assert_eq!(parse_strict_integer("1.0"), None);
        assert_eq!(parse_strict_integer(""), None);
    }
}
