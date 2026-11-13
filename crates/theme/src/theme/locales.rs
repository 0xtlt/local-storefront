//! Storefront translations (`locales/*.json`).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value as Json;

/// The translations of one locale, flattened to `dotted.key` → node.
#[derive(Debug, Default)]
pub struct Translations {
    root: Json,
}

impl Translations {
    pub fn new(root: Json) -> Self {
        Translations { root }
    }

    /// The node at a dotted key: a string, or an object of plural forms.
    pub fn lookup(&self, key: &str) -> Option<&Json> {
        let mut node = &self.root;
        for part in key.split('.') {
            node = node.as_object()?.get(part)?;
        }
        Some(node)
    }
}

/// The English text of the translation keys every Shopify store has (`shopify.*`).
pub fn system_translation(key: &str) -> Option<&'static str> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../data/shopify_system_translations.json"))
                .unwrap_or_default()
        })
        .get(key)
        .map(String::as_str)
}

/// A number as CLDR's plural rules read it: `n` its absolute value, `i` its integer digits, `v`
/// the number of its visible fraction digits, `f` those digits and `t` them without trailing
/// zeros (`1.50`: `i` 1, `v` 2, `f` 50, `t` 5).
/// https://unicode.org/reports/tr35/tr35-numbers.html#Operands
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Operands {
    pub n: f64,
    pub i: u64,
    pub v: u32,
    pub f: u64,
    pub t: u64,
}

impl From<i64> for Operands {
    fn from(count: i64) -> Operands {
        let i = count.unsigned_abs();
        Operands {
            n: i as f64,
            i,
            v: 0,
            f: 0,
            t: 0,
        }
    }
}

impl Operands {
    /// The operands of a number as it is written (`-1.50`), `None` for what is not a decimal
    /// number.
    pub fn parse(text: &str) -> Option<Operands> {
        let digits = text.strip_prefix('-').unwrap_or(text);
        let (integer, fraction) = digits.split_once('.').unwrap_or((digits, ""));
        let all_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
        if integer.is_empty() || !all_digits(integer) || !all_digits(fraction) {
            return None;
        }
        let trimmed = fraction.trim_end_matches('0');
        Some(Operands {
            n: digits.parse().ok()?,
            i: integer.parse().ok()?,
            v: fraction.len() as u32,
            f: if fraction.is_empty() {
                0
            } else {
                fraction.parse().ok()?
            },
            t: if trimmed.is_empty() {
                0
            } else {
                trimmed.parse().ok()?
            },
        })
    }

    /// Whether `n` is a whole number whose remainder by `modulo` is in `range`, as
    /// `n % 100 = 11..19` reads in a rule.
    fn n_mod(&self, modulo: u64, range: std::ops::RangeInclusive<u64>) -> bool {
        self.n.fract() == 0.0 && range.contains(&((self.n as u64) % modulo))
    }
}

/// The CLDR plural category of a number in a locale: `zero`, `one`, `two`, `few`, `many` or
/// `other`. Covers the cardinal rules of the languages Shopify themes ship with, decimals
/// included. https://www.unicode.org/cldr/charts/latest/supplemental/language_plural_rules.html
pub fn plural_category(locale: &str, number: &Operands) -> &'static str {
    let language = locale
        .split(['-', '_'])
        .next()
        .unwrap_or(locale)
        .to_ascii_lowercase();
    let &Operands { n, i, v, f, t } = number;
    let (i10, i100) = (i % 10, i % 100);
    // `e = 0 and i != 0 and i % 1000000 = 0 and v = 0`: a million, two millions...
    let millions = i != 0 && i % 1_000_000 == 0 && v == 0;
    let whole = n.fract() == 0.0;
    match language.as_str() {
        // No plural forms.
        "ja" | "zh" | "ko" | "vi" | "th" | "id" | "ms" => "other",
        // `i = 0,1`, and a `many` for millions.
        "fr" => {
            if i <= 1 {
                "one"
            } else if millions {
                "many"
            } else {
                "other"
            }
        }
        "pt" if !locale.eq_ignore_ascii_case("pt-PT") => {
            if i <= 1 {
                "one"
            } else if millions {
                "many"
            } else {
                "other"
            }
        }
        "pt" | "it" | "ca" => {
            if i == 1 && v == 0 {
                "one"
            } else if millions {
                "many"
            } else {
                "other"
            }
        }
        "es" => {
            if n == 1.0 {
                "one"
            } else if millions {
                "many"
            } else {
                "other"
            }
        }
        "hy" => {
            if i <= 1 {
                "one"
            } else {
                "other"
            }
        }
        "hi" | "bn" | "fa" | "gu" | "kn" | "zu" | "am" => {
            if i == 0 || n == 1.0 {
                "one"
            } else {
                "other"
            }
        }
        "da" => {
            if n == 1.0 || (t != 0 && i <= 1) {
                "one"
            } else {
                "other"
            }
        }
        "nb" | "no" | "el" | "hu" | "tr" | "bg" => {
            if n == 1.0 {
                "one"
            } else {
                "other"
            }
        }
        "ru" | "uk" | "be" => {
            // Russian and Ukrainian read the digits (`1.0` is `other`), Belarusian the value.
            if (language == "be" && !whole) || (language != "be" && v != 0) {
                "other"
            } else if i10 == 1 && i100 != 11 {
                "one"
            } else if (2..=4).contains(&i10) && !(12..=14).contains(&i100) {
                "few"
            } else {
                "many"
            }
        }
        "pl" => {
            if i == 1 && v == 0 {
                "one"
            } else if v != 0 {
                "other"
            } else if (2..=4).contains(&i10) && !(12..=14).contains(&i100) {
                "few"
            } else {
                "many"
            }
        }
        "cs" | "sk" => {
            if v != 0 {
                "many"
            } else if i == 1 {
                "one"
            } else if (2..=4).contains(&i) {
                "few"
            } else {
                "other"
            }
        }
        "hr" | "sr" | "bs" => {
            // The integer digits of a whole number, the fraction digits of a decimal.
            let (ten, hundred) = if v == 0 {
                (i10, i100)
            } else {
                (f % 10, f % 100)
            };
            if ten == 1 && hundred != 11 {
                "one"
            } else if (2..=4).contains(&ten) && !(12..=14).contains(&hundred) {
                "few"
            } else {
                "other"
            }
        }
        "sl" => {
            if v != 0 {
                "few"
            } else if i100 == 1 {
                "one"
            } else if i100 == 2 {
                "two"
            } else if (3..=4).contains(&i100) {
                "few"
            } else {
                "other"
            }
        }
        "lt" => {
            if f != 0 {
                "many"
            } else if number.n_mod(10, 1..=1) && !number.n_mod(100, 11..=19) {
                "one"
            } else if number.n_mod(10, 2..=9) && !number.n_mod(100, 11..=19) {
                "few"
            } else {
                "other"
            }
        }
        "lv" => {
            let (f10, f100) = (f % 10, f % 100);
            if number.n_mod(10, 0..=0)
                || number.n_mod(100, 11..=19)
                || (v == 2 && (11..=19).contains(&f100))
            {
                "zero"
            } else if (number.n_mod(10, 1..=1) && !number.n_mod(100, 11..=11))
                || (v == 2 && f10 == 1 && f100 != 11)
                || (v != 2 && f10 == 1)
            {
                "one"
            } else {
                "other"
            }
        }
        "ro" => {
            if i == 1 && v == 0 {
                "one"
            } else if v != 0 || n == 0.0 || (n != 1.0 && number.n_mod(100, 1..=19)) {
                "few"
            } else {
                "other"
            }
        }
        "ar" => {
            if n == 0.0 {
                "zero"
            } else if n == 1.0 {
                "one"
            } else if n == 2.0 {
                "two"
            } else if number.n_mod(100, 3..=10) {
                "few"
            } else if number.n_mod(100, 11..=99) {
                "many"
            } else {
                "other"
            }
        }
        "he" | "iw" => {
            if (i == 1 && v == 0) || (i == 0 && v != 0) {
                "one"
            } else if i == 2 && v == 0 {
                "two"
            } else {
                "other"
            }
        }
        "ga" => {
            if n == 1.0 {
                "one"
            } else if n == 2.0 {
                "two"
            } else if whole && (3.0..=6.0).contains(&n) {
                "few"
            } else if whole && (7.0..=10.0).contains(&n) {
                "many"
            } else {
                "other"
            }
        }
        "cy" => {
            if n == 0.0 {
                "zero"
            } else if n == 1.0 {
                "one"
            } else if n == 2.0 {
                "two"
            } else if n == 3.0 {
                "few"
            } else if n == 6.0 {
                "many"
            } else {
                "other"
            }
        }
        // English, German, Dutch, Swedish, Finnish, Estonian...: `i = 1 and v = 0`.
        _ => {
            if i == 1 && v == 0 {
                "one"
            } else {
                "other"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_dotted_keys() {
        let translations = Translations::new(
            serde_json::json!({"a": {"b": "c", "n": {"one": "1", "other": "n"}}}),
        );
        assert_eq!(translations.lookup("a.b").and_then(Json::as_str), Some("c"));
        assert!(translations.lookup("a.n").is_some_and(Json::is_object));
        assert!(translations.lookup("a.x").is_none());
        assert!(translations.lookup("a.b.c").is_none());
    }

    #[test]
    fn picks_plural_categories() {
        let category = |locale: &str, number: &str| {
            plural_category(locale, &Operands::parse(number).expect("a number"))
        };
        assert_eq!(category("en", "1"), "one");
        assert_eq!(category("en", "0"), "other");
        // Issue #111: decimals and the `many` of millions, as CLDR's rules have them.
        assert_eq!(category("en", "1.5"), "other");
        assert_eq!(category("en", "1.0"), "other");
        assert_eq!(category("fr", "0"), "one");
        assert_eq!(category("fr", "1.5"), "one");
        assert_eq!(category("fr", "1000000"), "many");
        assert_eq!(category("en", "1000000"), "other");
        assert_eq!(category("es", "2000000"), "many");
        assert_eq!(category("ru", "22"), "few");
        assert_eq!(category("ru", "11"), "many");
        assert_eq!(category("ru", "1.5"), "other");
        assert_eq!(category("pl", "5"), "many");
        assert_eq!(category("cs", "1.5"), "many");
        assert_eq!(category("hr", "0.1"), "one");
        assert_eq!(category("ar", "103"), "few");
        assert_eq!(category("ja", "1"), "other");
        assert_eq!(category("pt-BR", "0"), "one");
        assert_eq!(category("pt-PT", "0"), "other");
        assert_eq!(plural_category("en", &Operands::from(-1)), "one");
    }

    #[test]
    fn reads_the_operands_of_a_number() {
        let operands = Operands::parse("-1.50").expect("a number");
        assert_eq!((operands.n, operands.i, operands.v), (1.5, 1, 2));
        assert_eq!((operands.f, operands.t), (50, 5));
        assert_eq!(Operands::parse("1.0e+20"), None);
        assert_eq!(Operands::parse("x"), None);
    }

    #[test]
    fn knows_system_translations() {
        assert_eq!(system_translation("shopify.pagination.next"), Some("Next"));
        assert_eq!(system_translation("shopify.nope"), None);
    }
}
