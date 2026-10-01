//! Reference data: currencies, countries and languages.
//!
//! Only used to fill in defaults (a country's name, a currency's symbol, a money format), so
//! the tables cover the common cases and fall back gracefully for the rest.

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

/// `(ISO code, English name, currency)`.
const COUNTRIES: &[(&str, &str, &str)] = &[
    ("AE", "United Arab Emirates", "AED"),
    ("AF", "Afghanistan", "AFN"),
    ("AR", "Argentina", "ARS"),
    ("AT", "Austria", "EUR"),
    ("AU", "Australia", "AUD"),
    ("BE", "Belgium", "EUR"),
    ("BG", "Bulgaria", "BGN"),
    ("BR", "Brazil", "BRL"),
    ("CA", "Canada", "CAD"),
    ("CH", "Switzerland", "CHF"),
    ("CL", "Chile", "CLP"),
    ("CN", "China", "CNY"),
    ("CO", "Colombia", "COP"),
    ("CY", "Cyprus", "EUR"),
    ("CZ", "Czechia", "CZK"),
    ("DE", "Germany", "EUR"),
    ("DK", "Denmark", "DKK"),
    ("EE", "Estonia", "EUR"),
    ("EG", "Egypt", "EGP"),
    ("ES", "Spain", "EUR"),
    ("FI", "Finland", "EUR"),
    ("FR", "France", "EUR"),
    ("GB", "United Kingdom", "GBP"),
    ("GR", "Greece", "EUR"),
    ("HK", "Hong Kong SAR", "HKD"),
    ("HR", "Croatia", "EUR"),
    ("HU", "Hungary", "HUF"),
    ("ID", "Indonesia", "IDR"),
    ("IE", "Ireland", "EUR"),
    ("IL", "Israel", "ILS"),
    ("IN", "India", "INR"),
    ("IS", "Iceland", "ISK"),
    ("IT", "Italy", "EUR"),
    ("JP", "Japan", "JPY"),
    ("KR", "South Korea", "KRW"),
    ("LT", "Lithuania", "EUR"),
    ("LU", "Luxembourg", "EUR"),
    ("LV", "Latvia", "EUR"),
    ("MA", "Morocco", "MAD"),
    ("MC", "Monaco", "EUR"),
    ("MT", "Malta", "EUR"),
    ("MX", "Mexico", "MXN"),
    ("MY", "Malaysia", "MYR"),
    ("NL", "Netherlands", "EUR"),
    ("NO", "Norway", "NOK"),
    ("NZ", "New Zealand", "NZD"),
    ("PH", "Philippines", "PHP"),
    ("PL", "Poland", "PLN"),
    ("PT", "Portugal", "EUR"),
    ("RO", "Romania", "RON"),
    ("RU", "Russia", "RUB"),
    ("SA", "Saudi Arabia", "SAR"),
    ("SE", "Sweden", "SEK"),
    ("SG", "Singapore", "SGD"),
    ("SI", "Slovenia", "EUR"),
    ("SK", "Slovakia", "EUR"),
    ("TH", "Thailand", "THB"),
    ("TR", "Türkiye", "TRY"),
    ("TW", "Taiwan", "TWD"),
    ("UA", "Ukraine", "UAH"),
    ("US", "United States", "USD"),
    ("VN", "Vietnam", "VND"),
    ("ZA", "South Africa", "ZAR"),
];

/// The English names of every country known here, in alphabetical order.
pub fn country_names() -> Vec<&'static str> {
    let mut names: Vec<&str> = COUNTRIES.iter().map(|(_, name, _)| *name).collect();
    names.sort_unstable();
    names
}

pub fn country_name(code: &str) -> Option<&'static str> {
    COUNTRIES
        .iter()
        .find(|(iso, _, _)| iso.eq_ignore_ascii_case(code))
        .map(|(_, name, _)| *name)
}

pub fn country_currency(code: &str) -> Option<&'static str> {
    COUNTRIES
        .iter()
        .find(|(iso, _, _)| iso.eq_ignore_ascii_case(code))
        .map(|(_, _, currency)| *currency)
}

pub fn country_code_from_name(name: &str) -> Option<&'static str> {
    COUNTRIES
        .iter()
        .find(|(_, country, _)| country.eq_ignore_ascii_case(name))
        .map(|(iso, _, _)| *iso)
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
