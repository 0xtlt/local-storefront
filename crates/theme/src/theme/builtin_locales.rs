//! Shopify's own storefront strings in the languages other than English, as a store gives them
//! on a page of that language (`/fr/`, `/de/`).
//!
//! Shopify publishes these strings in English only (`data/shopify_system_translations.json`).
//! The tables of `data/locales/` were read on a development store with each language
//! published (2026-11-16), through a theme that prints them: what `t` gives for every key of the
//! English table, the named formats of `date` with their month, day and AM/PM names (every
//! pattern renders back the 199 dates it was read from), `country.continent`, the default
//! policy titles and the labels of the filters Shopify makes. What was not read there (another
//! continent, the `Brand` filter) stays in English. A theme's own translation of a key still
//! comes first.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value as Json;

/// The languages there is a table for, by the locale code Shopify gives them.
const TABLES: [(&str, &str); 9] = [
    ("de", include_str!("../../data/locales/de.json")),
    ("es", include_str!("../../data/locales/es.json")),
    ("fr", include_str!("../../data/locales/fr.json")),
    ("it", include_str!("../../data/locales/it.json")),
    ("ja", include_str!("../../data/locales/ja.json")),
    ("ko", include_str!("../../data/locales/ko.json")),
    ("nl", include_str!("../../data/locales/nl.json")),
    ("pt-BR", include_str!("../../data/locales/pt-BR.json")),
    ("zh-CN", include_str!("../../data/locales/zh-CN.json")),
];

/// Shopify's strings in one language.
#[derive(Debug, Default)]
pub struct Table {
    system: HashMap<String, String>,
    date_formats: HashMap<String, String>,
    months: Vec<String>,
    short_months: Vec<String>,
    /// Monday first, as chrono numbers them.
    weekdays: Vec<String>,
    /// What `%p` gives, before and after noon, when the language has its own words (`오전`).
    meridian: Option<[String; 2]>,
    continents: HashMap<String, String>,
    policies: HashMap<String, String>,
    filters: HashMap<String, String>,
}

impl Table {
    fn parse(source: &str) -> Table {
        let json: Json = serde_json::from_str(source).unwrap_or_default();
        let map = |key: &str| -> HashMap<String, String> {
            json.get(key)
                .and_then(Json::as_object)
                .map(|object| {
                    object
                        .iter()
                        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default()
        };
        let list = |key: &str| -> Vec<String> {
            json.get(key)
                .and_then(Json::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let meridian = list("meridian");
        Table {
            system: map("system"),
            date_formats: map("date_formats"),
            months: list("months"),
            short_months: list("short_months"),
            weekdays: list("weekdays"),
            meridian: (meridian.len() == 2).then(|| [meridian[0].clone(), meridian[1].clone()]),
            continents: map("continents"),
            policies: map("policies"),
            filters: map("filters"),
        }
    }

    /// The text of one of Shopify's keys (`shopify.collections.sorting.manual`).
    pub fn system_translation(&self, key: &str) -> SystemText<'_> {
        match self.system.get(key) {
            Some(text) => SystemText::Translated(text),
            None if super::locales::system_translation(key).is_some() => SystemText::Missing,
            None => SystemText::Unknown,
        }
    }

    /// The `strftime` pattern of a named date format, `None` when Shopify does not have it in
    /// this language.
    pub fn named_format(&self, name: &str) -> Option<&str> {
        self.date_formats.get(name).map(String::as_str)
    }

    /// `pattern` with this language's names in place of `%B`, `%b`, `%A` and, when it has its
    /// own words for it, `%p`: `month` 1 to 12, `weekday` 0 for Monday.
    pub fn names(&self, pattern: &str, month: u32, weekday: u32, afternoon: bool) -> String {
        let month = month.clamp(1, 12) as usize - 1;
        let pick = |names: &[String], index: usize| names.get(index).cloned().unwrap_or_default();
        let mut out = String::with_capacity(pattern.len());
        let mut chars = pattern.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('B') => out.push_str(&pick(&self.months, month)),
                Some('b') => out.push_str(&pick(&self.short_months, month)),
                Some('A') => out.push_str(&pick(&self.weekdays, weekday as usize)),
                Some('p') if self.meridian.is_some() => {
                    let meridian = self.meridian.as_ref().expect("checked");
                    out.push_str(&meridian[usize::from(afternoon)]);
                }
                Some(other) => {
                    out.push('%');
                    out.push(other);
                }
                None => out.push('%'),
            }
        }
        out
    }

    /// A continent as `country.continent` gives it, by its English name.
    pub fn continent(&self, name: &str) -> Option<&str> {
        self.continents.get(name).map(String::as_str)
    }

    /// The default title of a store policy, by handle.
    pub fn policy_title(&self, handle: &str) -> Option<&str> {
        self.policies.get(handle).map(String::as_str)
    }

    /// The label of a storefront filter Shopify makes, by its English label.
    pub fn filter_label(&self, label: &str) -> Option<&str> {
        self.filters.get(label).map(String::as_str)
    }
}

/// What a page has for one of Shopify's keys.
pub enum SystemText<'t> {
    /// Its text in the language, with its `%{name}` placeholders.
    Translated(&'t str),
    /// Shopify has the key in English and not in the language: `Translation missing`.
    Missing,
    /// Not one of Shopify's keys.
    Unknown,
}

/// The table of a page's language: its own (`pt-BR`, `zh-CN`), else that of its language
/// (`fr-CA` has French, `de-CH` German). `zh-TW` and `pt-PT` have none: Simplified Chinese
/// and Brazilian Portuguese are not theirs. `None` for English and for a language without one.
pub fn for_locale(locale: &str) -> Option<&'static Table> {
    static PARSED: OnceLock<Vec<(&'static str, Table)>> = OnceLock::new();
    let tables = PARSED.get_or_init(|| {
        TABLES
            .iter()
            .map(|(code, source)| (*code, Table::parse(source)))
            .collect()
    });
    let find = |code: &str| {
        tables
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(code))
            .map(|(_, table)| table)
    };
    find(locale).or_else(|| {
        let base = locale.split('-').next()?;
        (base != locale && !matches!(base.to_ascii_lowercase().as_str(), "zh" | "pt"))
            .then(|| find(base))
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_page_has_its_language() {
        assert!(for_locale("en").is_none());
        assert!(for_locale("fr").is_some() && for_locale("fr-CA").is_some());
        assert!(for_locale("pt-BR").is_some() && for_locale("pt-PT").is_none());
        assert!(for_locale("zh-CN").is_some() && for_locale("zh-TW").is_none());
        assert!(for_locale("de-CH").is_some() && for_locale("sv").is_none());
        for (code, _) in TABLES {
            let table = for_locale(code).expect("a table");
            assert_eq!(table.system.len(), 263, "{code}");
            assert_eq!(table.months.len(), 12, "{code}");
            assert_eq!(table.short_months.len(), 12, "{code}");
            assert_eq!(table.weekdays.len(), 7, "{code}");
        }
    }

    #[test]
    fn names_are_put_in_and_other_directives_kept() {
        let french = for_locale("fr").unwrap();
        assert_eq!(
            french.names("%A %-d %B %Y à %H:%M", 3, 3, true),
            "jeudi %-d mars %Y à %H:%M"
        );
        assert_eq!(
            french.names("%b|%%|%B|%p", 1, 0, false),
            "janv.|%%|janvier|%p"
        );
        let korean = for_locale("ko").unwrap();
        assert_eq!(korean.names("%p %-I:%M", 1, 0, true), "오후 %-I:%M");
    }

    #[test]
    fn the_tables_have_shopifys_strings() {
        let german = for_locale("de").unwrap();
        assert!(matches!(
            german.system_translation("shopify.collections.sorting.manual"),
            SystemText::Translated("Ausgewählt")
        ));
        assert!(matches!(
            german.system_translation("nope"),
            SystemText::Unknown
        ));
        assert_eq!(german.named_format("month_day_year"), None);
    }
}
