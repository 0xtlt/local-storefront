//! The order Shopify lists names in: a country selector sorts `Åland Islands` after
//! `Afghanistan`, and `Belgique`, `Canada`, `États-Unis` in French.

use icu_normalizer::DecomposingNormalizerBorrowed;
use unicode_general_category::{GeneralCategory, get_general_category};

/// What a name sorts by: the name without its accents (the combining marks of its canonical
/// decomposition), compared as it is written otherwise.
pub fn sort_key(name: &str) -> String {
    DecomposingNormalizerBorrowed::new_nfd()
        .normalize(name)
        .chars()
        .filter(|c| get_general_category(*c) != GeneralCategory::NonspacingMark)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::sort_key;

    #[test]
    fn accents_do_not_change_the_order() {
        assert_eq!(sort_key("États-Unis"), "Etats-Unis");
        assert_eq!(sort_key("Åland Islands"), "Aland Islands");
        assert_eq!(sort_key("São Tomé & Príncipe"), "Sao Tome & Principe");
        assert_eq!(sort_key("日本"), "日本");
        let mut names = ["États-Unis", "Canada", "Belgique", "Égypte", "Danemark"];
        names.sort_by_cached_key(|name| sort_key(name));
        assert_eq!(
            names,
            ["Belgique", "Canada", "Danemark", "Égypte", "États-Unis"]
        );
    }
}
