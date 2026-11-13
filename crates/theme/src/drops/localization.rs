//! `localization`, `country`, `currency` and `shop_locale`.

use std::any::Any;

use lsf_liquid::{Object, Value};

use lsf_core::store::{Country, Language, Metafields, reference};

use super::metafield::MetafieldsDrop;
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

/// The name of a country on the page: the one Shopify gives it in the page's language
/// (`États-Unis` on `/fr/`), unless the data names the country otherwise. A language lsf has no
/// names in keeps the English one.
pub fn country_name(site: &SiteRef, iso_code: &str, name: &str) -> String {
    let named_by_default = name.is_empty()
        || name.eq_ignore_ascii_case(iso_code)
        || reference::country_name(iso_code) == Some(name)
        || reference::country_name_in(iso_code, "en") == Some(name);
    match reference::country_name_in(iso_code, &site.language().iso_code) {
        Some(localized) if named_by_default => localized.to_string(),
        _ => name.to_string(),
    }
}

pub fn country_value(site: &SiteRef, country: &Country) -> Value {
    hash([
        ("iso_code", Value::from(&country.iso_code)),
        (
            "name",
            Value::from(country_name(site, &country.iso_code, &country.name)),
        ),
        ("unit_system", Value::from(&country.unit_system)),
        ("currency", currency_value(&country.currency)),
        ("popular?", Value::Bool(country.popular)),
        (
            "continent",
            reference::country_continent(&country.iso_code).map_or(Value::Nil, Value::str),
        ),
        (
            "available_languages",
            available_languages(site, &country.iso_code),
        ),
        ("market", market_value(site, &country.iso_code)),
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

/// The market that sells to a country ([`lsf_core::Store::market_for`]): its handle, the id the
/// Admin API gives it, and its metafields, as Shopify's `market` object has them.
fn market_value(site: &SiteRef, iso_code: &str) -> Value {
    let market = site.store.market_for(iso_code);
    let metafields = match market.declared {
        Some(declared) => MetafieldsDrop::value(site, &declared.metafields),
        None => MetafieldsDrop::value(site, &Metafields::default()),
    };
    hash([
        ("id", Value::Int(market.id as i64)),
        ("handle", Value::from(market.handle)),
        ("metafields", metafields),
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
            // By their names on the page, as Shopify lists them: `Belgique`, `Canada`,
            // `États-Unis` on `/fr/`.
            "available_countries" => {
                let mut countries: Vec<(String, &Country)> = site
                    .store
                    .countries
                    .iter()
                    .map(|country| {
                        (
                            country_name(site, &country.iso_code, &country.name),
                            country,
                        )
                    })
                    .collect();
                reference::sort_by_name(&mut countries, |(name, _)| name);
                Value::array(
                    countries
                        .into_iter()
                        .map(|(_, country)| country_value(site, country))
                        .collect(),
                )
            }
            // The languages of the visitor's market, as the Storefront API's
            // `Localization.availableLanguages` ("available for the active country").
            "available_languages" => available_languages(site, &site.country().iso_code),
            "country" => country_value(site, site.country()),
            "language" => language_value(site, site.language()),
            // The market of the visitor's country, as Shopify's "the market that applies to
            // the buyer's country".
            "market" => market_value(site, &site.country().iso_code),
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
