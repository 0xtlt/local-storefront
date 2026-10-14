//! `localization`, `country`, `currency` and `shop_locale`.

use std::any::Any;

use lsf_liquid::{Object, Value};

use super::{SiteRef, hash};
use crate::store::{Country, Language, reference};

pub fn currency_value(code: &str) -> Value {
    hash([
        ("iso_code", Value::from(code)),
        ("symbol", Value::from(reference::currency_symbol(code))),
        ("name", Value::from(reference::currency_name(code))),
    ])
}

/// `rtl` for the languages written right to left, `ltr` for the others.
fn direction(iso_code: &str) -> &'static str {
    let language = iso_code.split(['-', '_']).next().unwrap_or_default();
    match language.to_ascii_lowercase().as_str() {
        "ar" | "arc" | "ckb" | "dv" | "fa" | "he" | "ku" | "ps" | "sd" | "ug" | "ur" | "yi" => {
            "rtl"
        }
        _ => "ltr",
    }
}

pub fn language_value(site: &SiteRef, language: &Language) -> Value {
    let _ = site;
    hash([
        ("iso_code", Value::from(&language.iso_code)),
        ("direction", Value::str(direction(&language.iso_code))),
        ("name", Value::from(&language.name)),
        ("endonym_name", Value::from(&language.endonym_name)),
        ("primary", Value::Bool(language.primary)),
        ("root_url", Value::from(&language.root_url)),
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
            Value::array(
                site.store
                    .languages
                    .iter()
                    .map(|language| language_value(site, language))
                    .collect(),
            ),
        ),
        ("market", market_value()),
    ])
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
            "available_languages" => Value::array(
                site.store
                    .languages
                    .iter()
                    .map(|language| language_value(site, language))
                    .collect(),
            ),
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
    }
}
