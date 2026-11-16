//! Shopify's own storefront strings in French, for the pages of a French locale (`/fr/`).
//!
//! Shopify publishes these strings in English only (`data/shopify_system_translations.json`).
//! The French ones here are those read on a development store with French published
//! (2026-11-12 and 2026-11-13, issues #313 and #316); a string that was not read stays in
//! English, rather than being guessed. A theme's own translation of a key still comes first.

/// Whether a page in `language` (`fr`, `fr-CA`) gets the French strings.
pub fn applies(language: &str) -> bool {
    language
        .split('-')
        .next()
        .is_some_and(|base| base.eq_ignore_ascii_case("fr"))
}

/// The French text of one of Shopify's keys (`shopify.collections.sorting.manual`), with its
/// `%{name}` placeholders, when it was read on a store.
pub fn system_translation(key: &str) -> Option<&'static str> {
    Some(match key {
        "shopify.collections.sorting.manual" => "En vedette",
        "shopify.collections.sorting.most_relevant" => "Le plus pertinent",
        "shopify.collections.sorting.best_selling" => "Meilleures ventes",
        "shopify.filters.availability.value.in_stock" => "En stock",
        "shopify.filters.availability.value.out_of_stock" => "En rupture de stock",
        "shopify.page_titles.search_results.one" => {
            "Recherche : %{count} résultat trouvé pour « %{terms} »"
        }
        "shopify.page_titles.search_results.other" => {
            "Recherche : %{count} résultats trouvés pour « %{terms} »"
        }
        _ => return None,
    })
}

/// The label of a storefront filter Shopify makes (`Availability`, `Price`).
pub fn filter_label(label: &str) -> Option<&'static str> {
    Some(match label {
        "Availability" => "Disponibilité",
        "Price" => "Prix",
        _ => return None,
    })
}

/// A continent as `country.continent` gives it: `Amérique du Nord` for `North America`.
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

/// The default title of a store policy, by handle.
pub fn policy_title(handle: &str) -> Option<&'static str> {
    Some(match handle {
        "privacy-policy" => "Politique de confidentialité",
        _ => return None,
    })
}

/// The `strftime` pattern of a named date format, with `%B` and `%b` for the French month
/// names [`month_names`] puts in.
pub fn named_format(name: &str) -> Option<&'static str> {
    Some(match name {
        "abbreviated_date" => "%-d %b %Y",
        "date_at_time" => "%-d %B %Y à %H:%M",
        "on_date" => "le %-d %b %Y",
        "basic" => "%d/%m/%Y",
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

/// `pattern` with the French name of `month` (1 to 12) in place of `%B` and `%b`.
pub fn month_names(pattern: &str, month: u32) -> String {
    let index = (month.clamp(1, 12) - 1) as usize;
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('B') => out.push_str(MONTHS[index]),
            Some('b') => out.push_str(SHORT_MONTHS[index]),
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
    fn months_are_put_in_and_other_directives_kept() {
        assert_eq!(month_names("%-d %B %Y à %H:%M", 3), "%-d mars %Y à %H:%M");
        assert_eq!(month_names("%b|%%|%B", 1), "janv.|%%|janvier");
        assert!(applies("fr") && applies("fr-CA") && !applies("en") && !applies("fy"));
    }
}
