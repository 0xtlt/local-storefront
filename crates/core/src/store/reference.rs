//! Reference data: currencies, countries and languages.
//!
//! Only used to fill in defaults (a country's name, a currency's symbol, a money format). The
//! countries are every one Shopify lists; the other tables cover the common cases and fall back
//! gracefully for the rest.

use super::collate;
use super::countries::{COUNTRIES, CountryData};

pub struct Currency {
    pub code: &'static str,
    pub symbol: &'static str,
    pub name: &'static str,
    /// Shopify's default "HTML without currency" format.
    pub money_format: &'static str,
    /// Shopify's default "HTML with currency" format.
    pub money_with_currency_format: &'static str,
}

const CURRENCIES: &[Currency] = &[
    Currency {
        code: "USD",
        symbol: "$",
        name: "United States Dollar",
        money_format: "${{amount}}",
        money_with_currency_format: "${{amount}} USD",
    },
    Currency {
        code: "EUR",
        symbol: "€",
        name: "Euro",
        money_format: "€{{amount_with_comma_separator}}",
        money_with_currency_format: "€{{amount_with_comma_separator}} EUR",
    },
    Currency {
        code: "GBP",
        symbol: "£",
        name: "British Pound",
        money_format: "£{{amount}}",
        money_with_currency_format: "£{{amount}} GBP",
    },
    Currency {
        code: "CAD",
        symbol: "$",
        name: "Canadian Dollar",
        money_format: "${{amount}}",
        money_with_currency_format: "${{amount}} CAD",
    },
    Currency {
        code: "AUD",
        symbol: "$",
        name: "Australian Dollar",
        money_format: "${{amount}}",
        money_with_currency_format: "${{amount}} AUD",
    },
    Currency {
        code: "NZD",
        symbol: "$",
        name: "New Zealand Dollar",
        money_format: "${{amount}}",
        money_with_currency_format: "${{amount}} NZD",
    },
    Currency {
        code: "JPY",
        symbol: "¥",
        name: "Japanese Yen",
        money_format: "¥{{amount_no_decimals}}",
        money_with_currency_format: "¥{{amount_no_decimals}} JPY",
    },
    Currency {
        code: "CHF",
        symbol: "CHF",
        name: "Swiss Franc",
        money_format: "CHF {{amount}}",
        money_with_currency_format: "CHF {{amount}}",
    },
    Currency {
        code: "SEK",
        symbol: "kr",
        name: "Swedish Krona",
        money_format: "{{amount_no_decimals}} kr",
        money_with_currency_format: "{{amount_no_decimals}} SEK",
    },
    Currency {
        code: "DKK",
        symbol: "kr.",
        name: "Danish Krone",
        money_format: "{{amount_with_comma_separator}} kr",
        money_with_currency_format: "{{amount_with_comma_separator}} DKK",
    },
    Currency {
        code: "NOK",
        symbol: "kr",
        name: "Norwegian Krone",
        money_format: "{{amount_with_comma_separator}} kr",
        money_with_currency_format: "{{amount_with_comma_separator}} NOK",
    },
    Currency {
        code: "PLN",
        symbol: "zł",
        name: "Polish Złoty",
        money_format: "{{amount_with_comma_separator}} zł",
        money_with_currency_format: "{{amount_with_comma_separator}} zł PLN",
    },
    Currency {
        code: "CZK",
        symbol: "Kč",
        name: "Czech Koruna",
        money_format: "{{amount_with_comma_separator}} Kč",
        money_with_currency_format: "{{amount_with_comma_separator}} Kč",
    },
    Currency {
        code: "HUF",
        symbol: "Ft",
        name: "Hungarian Forint",
        money_format: "{{amount_no_decimals_with_comma_separator}} Ft",
        money_with_currency_format: "{{amount_no_decimals_with_comma_separator}} Ft",
    },
    Currency {
        code: "RON",
        symbol: "Lei",
        name: "Romanian Leu",
        money_format: "{{amount_with_comma_separator}} lei",
        money_with_currency_format: "{{amount_with_comma_separator}} lei RON",
    },
    Currency {
        code: "BRL",
        symbol: "R$",
        name: "Brazilian Real",
        money_format: "R$ {{amount_with_comma_separator}}",
        money_with_currency_format: "R$ {{amount_with_comma_separator}} BRL",
    },
    Currency {
        code: "MXN",
        symbol: "$",
        name: "Mexican Peso",
        money_format: "$ {{amount}}",
        money_with_currency_format: "$ {{amount}} MXN",
    },
    Currency {
        code: "INR",
        symbol: "₹",
        name: "Indian Rupee",
        money_format: "Rs. {{amount}}",
        money_with_currency_format: "Rs. {{amount}}",
    },
    Currency {
        code: "CNY",
        symbol: "¥",
        name: "Chinese Renminbi Yuan",
        money_format: "¥{{amount}}",
        money_with_currency_format: "¥{{amount}} CNY",
    },
    Currency {
        code: "HKD",
        symbol: "$",
        name: "Hong Kong Dollar",
        money_format: "${{amount}}",
        money_with_currency_format: "HK${{amount}}",
    },
    Currency {
        code: "SGD",
        symbol: "$",
        name: "Singapore Dollar",
        money_format: "${{amount}}",
        money_with_currency_format: "${{amount}} SGD",
    },
    Currency {
        code: "KRW",
        symbol: "₩",
        name: "South Korean Won",
        money_format: "₩{{amount_no_decimals}}",
        money_with_currency_format: "₩{{amount_no_decimals}} KRW",
    },
    Currency {
        code: "TWD",
        symbol: "$",
        name: "New Taiwan Dollar",
        money_format: "${{amount}}",
        money_with_currency_format: "${{amount}} TWD",
    },
    Currency {
        code: "THB",
        symbol: "฿",
        name: "Thai Baht",
        money_format: "{{amount}} ฿",
        money_with_currency_format: "{{amount}} ฿ THB",
    },
    Currency {
        code: "MYR",
        symbol: "RM",
        name: "Malaysian Ringgit",
        money_format: "RM{{amount}} MYR",
        money_with_currency_format: "RM{{amount}} MYR",
    },
    Currency {
        code: "IDR",
        symbol: "Rp",
        name: "Indonesian Rupiah",
        money_format: "Rp {{amount_with_comma_separator}}",
        money_with_currency_format: "Rp {{amount_with_comma_separator}}",
    },
    Currency {
        code: "PHP",
        symbol: "₱",
        name: "Philippine Peso",
        money_format: "₱{{amount}}",
        money_with_currency_format: "₱{{amount}} PHP",
    },
    Currency {
        code: "VND",
        symbol: "₫",
        name: "Vietnamese Đồng",
        money_format: "{{amount_no_decimals_with_comma_separator}}₫",
        money_with_currency_format: "{{amount_no_decimals_with_comma_separator}} VND",
    },
    Currency {
        code: "ILS",
        symbol: "₪",
        name: "Israeli New Shekel",
        money_format: "{{amount}} NIS",
        money_with_currency_format: "{{amount}} NIS",
    },
    Currency {
        code: "AED",
        symbol: "د.إ",
        name: "United Arab Emirates Dirham",
        money_format: "Dhs. {{amount}}",
        money_with_currency_format: "Dhs. {{amount}} AED",
    },
    Currency {
        code: "SAR",
        symbol: "ر.س",
        name: "Saudi Riyal",
        money_format: "{{amount}} SR",
        money_with_currency_format: "{{amount}} SAR",
    },
    Currency {
        code: "TRY",
        symbol: "₺",
        name: "Turkish Lira",
        money_format: "{{amount}}TL",
        money_with_currency_format: "{{amount}}TL",
    },
    Currency {
        code: "ZAR",
        symbol: "R",
        name: "South African Rand",
        money_format: "R {{amount}}",
        money_with_currency_format: "R {{amount}} ZAR",
    },
    Currency {
        code: "RUB",
        symbol: "₽",
        name: "Russian Ruble",
        money_format: "руб{{amount_with_comma_separator}}",
        money_with_currency_format: "руб{{amount_with_comma_separator}} RUB",
    },
    Currency {
        code: "UAH",
        symbol: "₴",
        name: "Ukrainian Hryvnia",
        money_format: "₴{{amount}}",
        money_with_currency_format: "₴{{amount}} UAH",
    },
    Currency {
        code: "ARS",
        symbol: "$",
        name: "Argentine Peso",
        money_format: "${{amount_with_comma_separator}}",
        money_with_currency_format: "${{amount_with_comma_separator}} ARS",
    },
    Currency {
        code: "CLP",
        symbol: "$",
        name: "Chilean Peso",
        money_format: "${{amount_no_decimals}}",
        money_with_currency_format: "${{amount_no_decimals}} CLP",
    },
    Currency {
        code: "COP",
        symbol: "$",
        name: "Colombian Peso",
        money_format: "${{amount_with_comma_separator}}",
        money_with_currency_format: "${{amount_with_comma_separator}} COP",
    },
    Currency {
        code: "BGN",
        symbol: "лв.",
        name: "Bulgarian Lev",
        money_format: "{{amount}} лв",
        money_with_currency_format: "{{amount}} лв BGN",
    },
    Currency {
        code: "ISK",
        symbol: "kr",
        name: "Icelandic Króna",
        money_format: "{{amount_no_decimals}} kr",
        money_with_currency_format: "{{amount_no_decimals}} kr ISK",
    },
    Currency {
        code: "MAD",
        symbol: "د.م.",
        name: "Moroccan Dirham",
        money_format: "{{amount}} dh",
        money_with_currency_format: "Dh {{amount}} MAD",
    },
    Currency {
        code: "EGP",
        symbol: "ج.م",
        name: "Egyptian Pound",
        money_format: "LE {{amount}}",
        money_with_currency_format: "LE {{amount}} EGP",
    },
    Currency {
        code: "AFN",
        symbol: "؋",
        name: "Afghan Afghani",
        money_format: "{{amount}}؋",
        money_with_currency_format: "{{amount}}؋ AFN",
    },
];

pub fn currency(code: &str) -> Option<&'static Currency> {
    CURRENCIES
        .iter()
        .find(|currency| currency.code.eq_ignore_ascii_case(code))
}

/// Whether a currency has no minor unit (ISO 4217 exponent 0): its amounts are whole units.
pub fn has_no_decimals(code: &str) -> bool {
    [
        "BIF", "CLP", "DJF", "GNF", "ISK", "JPY", "KMF", "KRW", "PYG", "RWF", "UGX", "UYI", "VND",
        "VUV", "XAF", "XOF", "XPF",
    ]
    .iter()
    .any(|known| known.eq_ignore_ascii_case(code))
}

pub fn currency_symbol(code: &str) -> String {
    currency(code).map_or_else(|| code.to_string(), |currency| currency.symbol.to_string())
}

pub fn currency_name(code: &str) -> String {
    currency(code).map_or_else(|| code.to_string(), |currency| currency.name.to_string())
}

pub fn default_money_format(code: &str) -> String {
    currency(code).map_or_else(
        || format!("{{{{amount}}}} {code}"),
        |currency| currency.money_format.to_string(),
    )
}

pub fn default_money_with_currency_format(code: &str) -> String {
    currency(code).map_or_else(
        || format!("{{{{amount}}}} {code}"),
        |currency| currency.money_with_currency_format.to_string(),
    )
}

/// The officially assigned codes of ISO 3166-1 alpha-2, in alphabetical order.
const ISO_COUNTRY_CODES: [&str; 249] = [
    "AD", "AE", "AF", "AG", "AI", "AL", "AM", "AO", "AQ", "AR", "AS", "AT", "AU", "AW", "AX", "AZ",
    "BA", "BB", "BD", "BE", "BF", "BG", "BH", "BI", "BJ", "BL", "BM", "BN", "BO", "BQ", "BR", "BS",
    "BT", "BV", "BW", "BY", "BZ", "CA", "CC", "CD", "CF", "CG", "CH", "CI", "CK", "CL", "CM", "CN",
    "CO", "CR", "CU", "CV", "CW", "CX", "CY", "CZ", "DE", "DJ", "DK", "DM", "DO", "DZ", "EC", "EE",
    "EG", "EH", "ER", "ES", "ET", "FI", "FJ", "FK", "FM", "FO", "FR", "GA", "GB", "GD", "GE", "GF",
    "GG", "GH", "GI", "GL", "GM", "GN", "GP", "GQ", "GR", "GS", "GT", "GU", "GW", "GY", "HK", "HM",
    "HN", "HR", "HT", "HU", "ID", "IE", "IL", "IM", "IN", "IO", "IQ", "IR", "IS", "IT", "JE", "JM",
    "JO", "JP", "KE", "KG", "KH", "KI", "KM", "KN", "KP", "KR", "KW", "KY", "KZ", "LA", "LB", "LC",
    "LI", "LK", "LR", "LS", "LT", "LU", "LV", "LY", "MA", "MC", "MD", "ME", "MF", "MG", "MH", "MK",
    "ML", "MM", "MN", "MO", "MP", "MQ", "MR", "MS", "MT", "MU", "MV", "MW", "MX", "MY", "MZ", "NA",
    "NC", "NE", "NF", "NG", "NI", "NL", "NO", "NP", "NR", "NU", "NZ", "OM", "PA", "PE", "PF", "PG",
    "PH", "PK", "PL", "PM", "PN", "PR", "PS", "PT", "PW", "PY", "QA", "RE", "RO", "RS", "RU", "RW",
    "SA", "SB", "SC", "SD", "SE", "SG", "SH", "SI", "SJ", "SK", "SL", "SM", "SN", "SO", "SR", "SS",
    "ST", "SV", "SX", "SY", "SZ", "TC", "TD", "TF", "TG", "TH", "TJ", "TK", "TL", "TM", "TN", "TO",
    "TR", "TT", "TV", "TW", "TZ", "UA", "UG", "UM", "US", "UY", "UZ", "VA", "VC", "VE", "VG", "VI",
    "VN", "VU", "WF", "WS", "YE", "YT", "ZA", "ZM", "ZW",
];

/// Codes that are not officially assigned by ISO 3166-1 and that Shopify takes as countries of an
/// address: Ascension Island, Tristan da Cunha, Kosovo.
const SHOPIFY_COUNTRY_CODES: [&str; 3] = ["AC", "TA", "XK"];

/// Whether `code` (upper case) is a country of an address: an ISO 3166-1 alpha-2 code, or one of
/// the few others Shopify takes. `ZZ` (unknown region) and the other user-assigned codes are not.
pub fn is_country_code(code: &str) -> bool {
    ISO_COUNTRY_CODES.binary_search(&code).is_ok() || SHOPIFY_COUNTRY_CODES.contains(&code)
}

/// Every country Shopify lists, by ISO code, in the order Shopify lists them: by English name,
/// accents aside (`Åland Islands` comes after `Afghanistan`).
pub fn country_codes() -> Vec<&'static str> {
    let mut countries: Vec<&CountryData> = COUNTRIES.iter().collect();
    countries.sort_by_cached_key(|(_, name, _, _)| collate::sort_key(name));
    countries.into_iter().map(|(code, _, _, _)| *code).collect()
}

/// The English names of every country Shopify lists, in the order of [`country_codes`].
pub fn country_names() -> Vec<&'static str> {
    country_codes()
        .into_iter()
        .filter_map(country_name)
        .collect()
}

fn country(code: &str) -> Option<&'static CountryData> {
    let code = code.to_ascii_uppercase();
    COUNTRIES
        .binary_search_by(|(iso, _, _, _)| (*iso).cmp(code.as_str()))
        .ok()
        .map(|index| &COUNTRIES[index])
}

/// The English name Shopify gives a country: `Côte d’Ivoire`, `Hong Kong SAR`.
pub fn country_name(code: &str) -> Option<&'static str> {
    country(code).map(|(_, name, _, _)| *name)
}

pub fn country_currency(code: &str) -> Option<&'static str> {
    country(code).map(|(_, _, currency, _)| *currency)
}

/// The continent of a country, as Liquid's `country.continent` gives it: `Africa`, `Asia`,
/// `Central America`, `Europe`, `North America`, `Oceania` or `South America`.
pub fn country_continent(code: &str) -> Option<&'static str> {
    country(code).map(|(_, _, _, continent)| *continent)
}

pub fn country_code_from_name(name: &str) -> Option<&'static str> {
    COUNTRIES
        .iter()
        .find(|(_, country, _, _)| country.eq_ignore_ascii_case(name))
        .map(|(iso, _, _, _)| *iso)
}

/// The provinces of a country, as `(code, name, label)` sorted by name, as Shopify lists them:
/// empty for a country Shopify lists none for. The name is what an address holds and a form
/// sends (`Hokkaidō`), the label what a selector shows (`Hokkaido`).
pub fn provinces(country: &str) -> &'static [super::provinces::Province] {
    super::provinces::PROVINCES
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(country))
        .map_or(&[], |(_, provinces)| provinces)
}

/// The province of a country with this code, name or label (any case): `(code, name)`.
pub fn province(country: &str, code_or_name: &str) -> Option<(&'static str, &'static str)> {
    let wanted = code_or_name.trim();
    let all = provinces(country);
    all.iter()
        .find(|(code, _, _)| code.eq_ignore_ascii_case(wanted))
        .or_else(|| {
            all.iter()
                .find(|(_, name, _)| name.eq_ignore_ascii_case(wanted))
        })
        .or_else(|| {
            all.iter()
                .find(|(_, _, label)| label.eq_ignore_ascii_case(wanted))
        })
        .map(|(code, name, _)| (*code, *name))
}

/// Countries that use imperial units.
pub fn unit_system(country: &str) -> &'static str {
    if ["US", "LR", "MM"]
        .iter()
        .any(|code| code.eq_ignore_ascii_case(country))
    {
        "imperial"
    } else {
        "metric"
    }
}

/// `(locale, English name, endonym)`.
const LANGUAGES: &[(&str, &str, &str)] = &[
    ("en", "English", "English"),
    ("fr", "French", "français"),
    ("de", "German", "Deutsch"),
    ("es", "Spanish", "Español"),
    ("it", "Italian", "Italiano"),
    ("nl", "Dutch", "Nederlands"),
    ("pt", "Portuguese", "português"),
    ("pt-BR", "Portuguese (Brazil)", "português (Brasil)"),
    ("pt-PT", "Portuguese (Portugal)", "português (Portugal)"),
    ("da", "Danish", "Dansk"),
    ("sv", "Swedish", "svenska"),
    ("nb", "Norwegian (Bokmål)", "norsk bokmål"),
    ("fi", "Finnish", "Suomi"),
    ("pl", "Polish", "Polski"),
    ("cs", "Czech", "čeština"),
    ("sk", "Slovak", "slovenčina"),
    ("sl", "Slovenian", "slovenščina"),
    ("hu", "Hungarian", "magyar"),
    ("ro", "Romanian", "română"),
    ("bg", "Bulgarian", "български"),
    ("hr", "Croatian", "hrvatski"),
    ("el", "Greek", "Ελληνικά"),
    ("tr", "Turkish", "Türkçe"),
    ("ru", "Russian", "русский"),
    ("uk", "Ukrainian", "українська"),
    ("lt", "Lithuanian", "lietuvių"),
    ("lv", "Latvian", "latviešu"),
    ("et", "Estonian", "eesti"),
    ("ja", "Japanese", "日本語"),
    ("ko", "Korean", "한국어"),
    ("zh-CN", "Chinese (Simplified)", "简体中文"),
    ("zh-TW", "Chinese (Traditional)", "繁體中文"),
    ("th", "Thai", "ภาษาไทย"),
    ("vi", "Vietnamese", "Tiếng Việt"),
    ("id", "Indonesian", "Indonesia"),
    ("ms", "Malay", "Melayu"),
    ("hi", "Hindi", "हिन्दी"),
    ("ar", "Arabic", "العربية"),
    ("he", "Hebrew", "עברית"),
];

fn language(code: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    LANGUAGES
        .iter()
        .find(|(iso, _, _)| iso.eq_ignore_ascii_case(code))
        .or_else(|| {
            let base = code.split('-').next().unwrap_or(code);
            LANGUAGES
                .iter()
                .find(|(iso, _, _)| iso.eq_ignore_ascii_case(base))
        })
}

pub fn language_name(code: &str) -> String {
    language(code).map_or_else(|| code.to_string(), |(_, name, _)| (*name).to_string())
}

pub fn language_endonym(code: &str) -> String {
    language(code).map_or_else(
        || code.to_string(),
        |(_, _, endonym)| (*endonym).to_string(),
    )
}

/// Whether a language is written right to left.
pub fn is_rtl(code: &str) -> bool {
    let base = code.split('-').next().unwrap_or(code).to_ascii_lowercase();
    matches!(base.as_str(), "ar" | "he" | "fa" | "ur")
}

/// The standard titles of the store policies, by handle.
pub fn policy_title(handle: &str) -> &'static str {
    match handle {
        "privacy-policy" => "Privacy policy",
        "refund-policy" => "Refund policy",
        "shipping-policy" => "Shipping policy",
        "terms-of-service" => "Terms of service",
        "subscription-policy" => "Cancellation policy",
        _ => "Policy",
    }
}

/// The name Shopify gives a country on a page in a language: `États-Unis` in French,
/// `Vereinigte Staaten` in German. A regional language (`fr-CA`) has the names of its language
/// when it has none of its own. `None` for a language lsf has no names in and for a country
/// Shopify does not list.
pub fn country_name_in(code: &str, language: &str) -> Option<&'static str> {
    use std::collections::HashMap;
    use std::sync::OnceLock;

    use super::country_names::{CODES, NAMES};

    static TABLES: OnceLock<HashMap<String, Vec<&'static str>>> = OnceLock::new();
    let tables = TABLES.get_or_init(|| {
        NAMES
            .iter()
            .map(|(language, names)| (language.to_ascii_lowercase(), names.lines().collect()))
            .collect()
    });
    let index = CODES
        .binary_search(&code.to_ascii_uppercase().as_str())
        .ok()?;
    let language = language.to_ascii_lowercase().replace('_', "-");
    let names = tables.get(&language).or_else(|| {
        let base = language.split('-').next().unwrap_or_default();
        tables.get(base)
    })?;
    names.get(index).copied()
}

/// Puts names in the order Shopify lists them in: by name, accents aside (`Belgique`, `Canada`,
/// `États-Unis`).
pub fn sort_by_name<T>(items: &mut [T], name: impl Fn(&T) -> &str) {
    items.sort_by_cached_key(|item| super::collate::sort_key(name(item)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_country_code_is_one_of_iso_3166_or_of_shopify() {
        assert!(ISO_COUNTRY_CODES.is_sorted());
        for code in ["FR", "US", "GB", "XK", "AC", "TA", "AX"] {
            assert!(is_country_code(code), "{code}");
        }
        for code in ["ZZ", "XX", "AA", "QM", "EU", "UK", "fr", "FRA", ""] {
            assert!(!is_country_code(code), "{code}");
        }
        // Every country Shopify lists is one.
        for (code, _, _, _) in COUNTRIES {
            assert!(is_country_code(code), "{code}");
        }
    }

    #[test]
    fn every_country_shopify_lists_has_a_name_a_currency_and_a_continent() {
        assert!(COUNTRIES.is_sorted_by_key(|(code, _, _, _)| *code));
        assert_eq!(COUNTRIES.len(), 237);
        assert_eq!(country_codes().len(), 237);
        for (code, name) in [
            ("AD", "Andorra"),
            ("AL", "Albania"),
            ("KE", "Kenya"),
            ("QA", "Qatar"),
            ("uy", "Uruguay"),
            ("HK", "Hong Kong SAR"),
            ("TR", "Türkiye"),
            ("XK", "Kosovo"),
        ] {
            assert_eq!(country_name(code), Some(name), "{code}");
        }
        assert_eq!(country_code_from_name("kenya"), Some("KE"));
        assert_eq!(country_currency("KE"), Some("KES"));
        assert_eq!(country_continent("BE"), Some("Europe"));
        assert_eq!(country_continent("CA"), Some("North America"));
        assert_eq!(country_continent("US"), Some("North America"));
        assert_eq!(country_continent("MX"), Some("North America"));
        assert_eq!(country_continent("GT"), Some("Central America"));
        assert_eq!(country_continent("BR"), Some("South America"));
        assert_eq!(country_continent("JP"), Some("Asia"));
        assert_eq!(country_continent("NZ"), Some("Oceania"));
        // Countries Shopify does not sell to, and codes that are none.
        for code in ["CU", "IR", "KP", "SY", "BV", "HM", "ZZ", ""] {
            assert_eq!(country_name(code), None, "{code}");
        }
        // The documented continents only.
        for (_, _, _, continent) in COUNTRIES {
            assert!(
                [
                    "Africa",
                    "Asia",
                    "Central America",
                    "Europe",
                    "North America",
                    "Oceania",
                    "South America"
                ]
                .contains(&continent),
                "{continent}"
            );
        }
        // By name, accents aside.
        assert_eq!(
            &country_names()[..3],
            ["Afghanistan", "Åland Islands", "Albania"]
        );
    }

    #[test]
    fn countries_have_the_provinces_shopify_lists() {
        assert_eq!(provinces("CA").len(), 13);
        assert_eq!(provinces("CA")[0], ("AB", "Alberta", "Alberta"));
        assert_eq!(provinces("us").len(), 62);
        // Sorted by name, which a selector shows by its label.
        assert_eq!(provinces("JP")[0], ("JP-23", "Aichi", "Aichi"));
        assert!(provinces("JP").contains(&("JP-13", "Tōkyō", "Tokyo")));
        assert!(provinces("US").contains(&("DC", "District of Columbia", "Washington DC")));
        assert!(provinces("US").contains(&("VI", "Virgin Islands", "U.S. Virgin Islands")));
        // The regions of the United Kingdom; France and Türkiye have none.
        assert_eq!(provinces("GB")[1], ("ENG", "England", "England"));
        assert!(provinces("FR").is_empty());
        assert!(provinces("TR").is_empty());
        assert!(provinces("").is_empty());
        assert_eq!(province("US", "or"), Some(("OR", "Oregon")));
        assert_eq!(province("US", "oregon"), Some(("OR", "Oregon")));
        assert_eq!(province("JP", "Tokyo"), Some(("JP-13", "Tōkyō")));
        assert_eq!(province("US", "Atlantis"), None);
        for (country, provinces) in super::super::provinces::PROVINCES {
            assert!(is_country_code(country), "{country}");
            assert!(!provinces.is_empty(), "{country}");
            let names: Vec<&str> = provinces.iter().map(|(_, name, _)| *name).collect();
            assert!(names.is_sorted(), "{country} is not sorted by name");
            let mut codes: Vec<&str> = provinces.iter().map(|(code, _, _)| *code).collect();
            codes.sort_unstable();
            codes.dedup();
            assert_eq!(codes.len(), provinces.len(), "{country} has a code twice");
        }
    }

    #[test]
    fn countries_have_the_names_shopify_gives_them_in_each_language() {
        assert_eq!(country_name_in("US", "fr"), Some("États-Unis"));
        assert_eq!(country_name_in("us", "FR-ca"), Some("États-Unis"));
        assert_eq!(country_name_in("BE", "de"), Some("Belgien"));
        assert_eq!(country_name_in("CH", "de"), Some("Schweiz"));
        assert_eq!(country_name_in("US", "de"), Some("Vereinigte Staaten"));
        assert_eq!(country_name_in("US", "en"), Some("United States"));
        assert_eq!(country_name_in("HK", "en"), Some("Hong Kong SAR"));
        // `pt-BR` reads `pt`, `pt-PT` has its own, `nb` those of `no`.
        assert_eq!(country_name_in("CZ", "pt-BR"), Some("Tchéquia"));
        assert_eq!(country_name_in("CZ", "pt-PT"), Some("Chéquia"));
        assert_eq!(country_name_in("US", "nb"), Some("USA"));
        assert_eq!(country_name_in("US", "zh-CN"), Some("美国"));
        assert_eq!(country_name_in("US", "zh-TW"), Some("美國"));
        assert_eq!(country_name_in("US", "xx"), None);
        assert_eq!(country_name_in("CU", "fr"), None);
        assert_eq!(country_name_in("ZZ", "fr"), None);
        // Every language lsf knows has a name for every country.
        for (language, _, _) in LANGUAGES {
            for code in super::super::country_names::CODES {
                assert!(
                    country_name_in(code, language).is_some(),
                    "{code} in {language}"
                );
            }
        }
        let mut names = ["États-Unis", "Canada", "Belgique"];
        sort_by_name(&mut names, |name| name);
        assert_eq!(names, ["Belgique", "Canada", "États-Unis"]);
    }
}
