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

/// The CLDR plural category of `count` in a locale: `zero`, `one`, `two`, `few`, `many` or
/// `other`. Covers the cardinal rules of the languages Shopify themes ship with; counts are
/// integers.
pub fn plural_category(locale: &str, count: i64) -> &'static str {
    let language = locale
        .split(['-', '_'])
        .next()
        .unwrap_or(locale)
        .to_ascii_lowercase();
    let n = count.unsigned_abs();
    let (n10, n100) = (n % 10, n % 100);
    match language.as_str() {
        // No plural forms.
        "ja" | "zh" | "ko" | "vi" | "th" | "id" | "ms" => "other",
        // 0 and 1 are singular.
        "fr" | "hi" | "bn" | "fa" | "gu" | "kn" | "zu" | "am" | "hy" => {
            if n <= 1 {
                "one"
            } else {
                "other"
            }
        }
        "pt" => {
            if locale.eq_ignore_ascii_case("pt-PT") {
                if n == 1 { "one" } else { "other" }
            } else if n <= 1 {
                "one"
            } else {
                "other"
            }
        }
        "ru" | "uk" | "be" => {
            if n10 == 1 && n100 != 11 {
                "one"
            } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
                "few"
            } else {
                "many"
            }
        }
        "pl" => {
            if n == 1 {
                "one"
            } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
                "few"
            } else {
                "many"
            }
        }
        "cs" | "sk" => match n {
            1 => "one",
            2..=4 => "few",
            _ => "other",
        },
        "hr" | "sr" | "bs" => {
            if n10 == 1 && n100 != 11 {
                "one"
            } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
                "few"
            } else {
                "other"
            }
        }
        "sl" => match n100 {
            1 => "one",
            2 => "two",
            3 | 4 => "few",
            _ => "other",
        },
        "lt" => {
            if n10 == 1 && !(11..=19).contains(&n100) {
                "one"
            } else if (2..=9).contains(&n10) && !(11..=19).contains(&n100) {
                "few"
            } else {
                "other"
            }
        }
        "lv" => {
            if n10 == 0 || (11..=19).contains(&n100) {
                "zero"
            } else if n10 == 1 && n100 != 11 {
                "one"
            } else {
                "other"
            }
        }
        "ro" => {
            if n == 1 {
                "one"
            } else if n == 0 || (1..=19).contains(&n100) {
                "few"
            } else {
                "other"
            }
        }
        "ar" => match n {
            0 => "zero",
            1 => "one",
            2 => "two",
            _ if (3..=10).contains(&n100) => "few",
            _ if (11..=99).contains(&n100) => "many",
            _ => "other",
        },
        "he" | "iw" => match n {
            1 => "one",
            2 => "two",
            _ => "other",
        },
        "ga" => match n {
            1 => "one",
            2 => "two",
            3..=6 => "few",
            7..=10 => "many",
            _ => "other",
        },
        "cy" => match n {
            0 => "zero",
            1 => "one",
            2 => "two",
            3 => "few",
            6 => "many",
            _ => "other",
        },
        _ => {
            if n == 1 {
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
        assert_eq!(plural_category("en", 1), "one");
        assert_eq!(plural_category("en", 0), "other");
        assert_eq!(plural_category("fr", 0), "one");
        assert_eq!(plural_category("ru", 22), "few");
        assert_eq!(plural_category("ru", 11), "many");
        assert_eq!(plural_category("ja", 1), "other");
        assert_eq!(plural_category("pt-BR", 0), "one");
        assert_eq!(plural_category("pt-PT", 0), "other");
    }

    #[test]
    fn knows_system_translations() {
        assert_eq!(system_translation("shopify.pagination.next"), Some("Next"));
        assert_eq!(system_translation("shopify.nope"), None);
    }
}
