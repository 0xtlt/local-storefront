//! Shopify's own storefront strings in French, for the pages of a French locale (`/fr/`).
//!
//! Shopify publishes these strings in English only (`data/shopify_system_translations.json`).
//! The French ones were read on Shopify's own demo store with French published, through the
//! renderer of shopify.dev's Liquid playground (`POST https://shopify.dev/liquid/render` with
//! `"path": "/fr"`), on 2026-11-16: `data/shopify_system_translations.fr.json` is what `t` gave
//! for every key of the English table. A key Shopify has no French for would print
//! `Translation missing: fr.<key>`, as it does there. A theme's own translation of a key still
//! comes first.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Whether a page in `language` (`fr`, `fr-CA`) gets the French strings.
pub fn applies(language: &str) -> bool {
    language
        .split('-')
        .next()
        .is_some_and(|base| base.eq_ignore_ascii_case("fr"))
}

/// What a French page has for one of Shopify's keys (`shopify.collections.sorting.manual`).
pub enum SystemText {
    /// Its French text, with its `%{name}` placeholders.
    French(&'static str),
    /// Shopify has the key in English and not in French: `Translation missing`.
    Missing,
    /// Not one of Shopify's keys.
    Unknown,
}

pub fn system_translation(key: &str) -> SystemText {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../data/shopify_system_translations.fr.json"
        ))
        .unwrap_or_default()
    });
    match table.get(key) {
        Some(text) => SystemText::French(text),
        None if super::locales::system_translation(key).is_some() => SystemText::Missing,
        None => SystemText::Unknown,
    }
}

/// The label of a storefront filter Shopify makes (`Availability`, `Price`).
pub fn filter_label(label: &str) -> Option<&'static str> {
    Some(match label {
        "Availability" => "Disponibilité",
        "Price" => "Prix",
        _ => return None,
    })
}

/// A continent as `country.continent` gives it: `Amérique du Nord` for `North America`. The
/// demo store lists countries of Asia, Europe, North America and Oceania; the others are the
/// French names of the same CLDR regions.
pub fn continent(name: &str) -> Option<&'static str> {
    Some(match name {
        "Africa" => "Afrique",
        "Asia" => "Asie",
        "Central America" => "Amérique centrale",
        "Europe" => "Europe",
        "North America" => "Amérique du Nord",
        "Oceania" => "Océanie",
        "South America" => "Amérique du Sud",
        _ => return None,
    })
}

/// The default title of a store policy, by handle, with Shopify's typographic apostrophe.
/// The demo store has no subscription policy: its title stays in English.
pub fn policy_title(handle: &str) -> Option<&'static str> {
    Some(match handle {
        "privacy-policy" => "Politique de confidentialité",
        "refund-policy" => "Politique de remboursement",
        "shipping-policy" => "Politique d’expédition",
        "terms-of-service" => "Conditions d’utilisation",
        _ => return None,
    })
}

/// The `strftime` pattern of a named date format, with `%B`, `%b` and `%A` for the French
/// names [`names`] puts in.
pub fn named_format(name: &str) -> Option<&'static str> {
    Some(match name {
        "abbreviated_date" => "%-d %b %Y",
        "basic" => "%d/%m/%Y",
        "date" => "%-d %B %Y",
        "date_at_time" => "%-d %B %Y à %H:%M",
        "default" => "%A %-d %B %Y à %H:%M %z",
        "on_date" => "le %-d %b %Y",
        "short" => "%-d %b %H:%M",
        "long" => "%B %-d, %Y %H:%M",
        "month_day_year" => "%B %d, %Y",
        _ => return None,
    })
}

const MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

const SHORT_MONTHS: [&str; 12] = [
    "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
    "déc.",
];

/// Monday first, as chrono numbers them.
const WEEKDAYS: [&str; 7] = [
    "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
];

/// `pattern` with the French name of `month` (1 to 12) in place of `%B` and `%b`, and of
/// `weekday` (0 for Monday) in place of `%A`.
pub fn names(pattern: &str, month: u32, weekday: u32) -> String {
    let month = (month.clamp(1, 12) - 1) as usize;
    let weekday = weekday.min(6) as usize;
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('B') => out.push_str(MONTHS[month]),
            Some('b') => out.push_str(SHORT_MONTHS[month]),
            Some('A') => out.push_str(WEEKDAYS[weekday]),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_put_in_and_other_directives_kept() {
        assert_eq!(
            names("%A %-d %B %Y à %H:%M", 3, 3),
            "jeudi %-d mars %Y à %H:%M"
        );
        assert_eq!(names("%b|%%|%B", 1, 0), "janv.|%%|janvier");
        assert!(applies("fr") && applies("fr-CA") && !applies("en") && !applies("fy"));
    }

    #[test]
    fn the_french_table_has_shopifys_strings() {
        assert!(matches!(
            system_translation("shopify.collections.sorting.manual"),
            SystemText::French("En vedette")
        ));
        // A key the live store has in no language is missing from both tables.
        assert!(matches!(
            system_translation("shopify.sentence.words_connector"),
            SystemText::Unknown
        ));
        assert!(matches!(system_translation("nope"), SystemText::Unknown));
    }
}
