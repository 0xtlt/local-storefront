//! `localization`, `country`, `currency` and `shop_locale`.

use std::any::Any;

use lsf_liquid::{Object, Value};

use lsf_core::store::{Country, Language, reference};

use super::{SiteRef, hash};

pub fn currency_value(code: &str) -> Value {
    hash([
        ("iso_code", Value::from(code)),
        ("symbol", Value::from(reference::currency_symbol(code))),
        ("name", Value::from(reference::currency_name(code))),
    ])
}

/// `rtl` for the languages written right to left, `ltr` for the others.
fn direction(iso_code: &str) -> &'static str {
    let mut subtags = iso_code.split(['-', '_']);
    let language = subtags.next().unwrap_or_default().to_ascii_lowercase();
    // A script subtag, which comes right after the language, decides when there is one:
    // `sd-Deva` is written left to right, and `pa-Arab` right to left.
    if let Some(script) = subtags
        .next()
        .filter(|subtag| subtag.len() == 4 && subtag.chars().all(|c| c.is_ascii_alphabetic()))
    {
        return match script.to_ascii_lowercase().as_str() {
            "adlm" | "arab" | "hebr" | "mand" | "nkoo" | "rohg" | "samr" | "syrc" | "thaa" => "rtl",
            _ => "ltr",
        };
    }
    // Otherwise the usual script of the language. Kurdish (`ku`, Kurmanji) is written in Latin
    // letters; Sorani (`ckb`) in Arabic ones.
    match language.as_str() {
        "ar" | "arc" | "ckb" | "dv" | "fa" | "he" | "iw" | "ks" | "ps" | "sd" | "ug" | "ur"
        | "yi" => "rtl",
        _ => "ltr",
    }
}

pub fn language_value(site: &SiteRef, language: &Language) -> Value {
    hash([
        ("iso_code", Value::from(&language.iso_code)),
        ("direction", Value::str(direction(&language.iso_code))),
        ("name", Value::from(&language.name)),
        ("endonym_name", Value::from(&language.endonym_name)),
        ("primary", Value::Bool(language.primary)),
        // On the web presence of the page: `/fr-ca` under the subfolders of a market.
        ("root_url", Value::from(site.root_url_of(language))),
    ])
}

pub fn country_value(site: &SiteRef, country: &Country) -> Value {
    hash([
        ("iso_code", Value::from(&country.iso_code)),
        ("name", Value::from(&country.name)),
        ("unit_system", Value::from(&country.unit_system)),
        ("currency", currency_value(&country.currency)),
        ("popular?", Value::Bool(country.popular)),
        ("continent", Value::Nil),
        (
            "available_languages",
            available_languages(site, &country.iso_code),
        ),
        ("market", market_value()),
    ])
}

/// The languages of a country's market (all those of the store when its market lists none),
/// which `country.available_languages` and, for the visitor's country,
/// `localization.available_languages` are.
fn available_languages(site: &SiteRef, iso_code: &str) -> Value {
    Value::array(
        site.store
            .languages_of(iso_code)
            .into_iter()
            .map(|language| language_value(site, language))
            .collect(),
    )
}

fn market_value() -> Value {
    hash([
        ("id", Value::Int(1)),
        ("handle", Value::str("primary")),
        ("metafields", Value::Nil),
    ])
}

pub struct LocalizationDrop {
    pub site: SiteRef,
}

impl Object for LocalizationDrop {
    fn type_name(&self) -> &str {
        "localization"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        Some(match key {
            "available_countries" => Value::array(
                site.store
                    .countries
                    .iter()
                    .map(|country| country_value(site, country))
                    .collect(),
            ),
            // The languages of the visitor's market, as the Storefront API's
            // `Localization.availableLanguages` ("available for the active country").
            "available_languages" => available_languages(site, &site.country().iso_code),
            "country" => country_value(site, site.country()),
            "language" => language_value(site, site.language()),
            "market" => market_value(),
            _ => return None,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::direction;

    #[test]
    fn languages_written_right_to_left_say_so() {
        assert_eq!(direction("ar"), "rtl");
        assert_eq!(direction("he"), "rtl");
        assert_eq!(direction("fa-IR"), "rtl");
        assert_eq!(direction("en"), "ltr");
        assert_eq!(direction("pt-BR"), "ltr");
        assert_eq!(direction("ku"), "ltr");
        assert_eq!(direction("ks"), "rtl");
        assert_eq!(direction("sd-Deva"), "ltr");
        assert_eq!(direction("pa-Arab"), "rtl");
        assert_eq!(direction("uz-Arab-AF"), "rtl");
        assert_eq!(direction("ar-u-nu-latn"), "rtl");
        assert_eq!(direction("en-u-nu-arab"), "ltr");
    }
}
