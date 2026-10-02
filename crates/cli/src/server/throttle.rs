//! Artificial latency: answering some requests late, to see what a storefront shows while it
//! waits (a spinner on the add-to-cart button, a skeleton in the cart drawer).
//!
//! A throttle is a set of rules, `<what>=<how long>`. The most specific rule wins: one about
//! a kind of request (`cart-add`), then one about the cart as a whole (`cart`), then the one
//! about everything (`all`).

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Value as Json, json};

/// The longest a request can be delayed. Anything above is a typo more often than a wish.
const LONGEST: Duration = Duration::from_secs(60);

/// A kind of request, as a storefront makes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Page,
    Section,
    CartRead,
    CartAdd,
    CartChange,
    CartUpdate,
    CartClear,
    Search,
    Recommendations,
    Product,
    Form,
    Asset,
    Image,
}

impl Kind {
    pub const ALL: [Kind; 13] = [
        Kind::Page,
        Kind::Section,
        Kind::CartRead,
        Kind::CartAdd,
        Kind::CartChange,
        Kind::CartUpdate,
        Kind::CartClear,
        Kind::Search,
        Kind::Recommendations,
        Kind::Product,
        Kind::Form,
        Kind::Asset,
        Kind::Image,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Page => "page",
            Kind::Section => "section",
            Kind::CartRead => "cart-read",
            Kind::CartAdd => "cart-add",
            Kind::CartChange => "cart-change",
            Kind::CartUpdate => "cart-update",
            Kind::CartClear => "cart-clear",
            Kind::Search => "search",
            Kind::Recommendations => "recommendations",
            Kind::Product => "product",
            Kind::Form => "form",
            Kind::Asset => "asset",
            Kind::Image => "image",
        }
    }

    fn is_cart(self) -> bool {
        matches!(
            self,
            Kind::CartRead | Kind::CartAdd | Kind::CartChange | Kind::CartUpdate | Kind::CartClear
        )
    }
}

/// What a rule is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Scope {
    All,
    /// Every cart request.
    Cart,
    Kind(Kind),
}

impl Scope {
    fn name(self) -> &'static str {
        match self {
            Scope::All => "all",
            Scope::Cart => "cart",
            Scope::Kind(kind) => kind.name(),
        }
    }

    fn names() -> Vec<&'static str> {
        ["all", "cart"]
            .into_iter()
            .chain(Kind::ALL.iter().map(|kind| kind.name()))
            .collect()
    }

    fn parse(name: &str) -> Result<Scope, String> {
        match name {
            "all" => Ok(Scope::All),
            "cart" => Ok(Scope::Cart),
            other => Kind::ALL
                .into_iter()
                .find(|kind| kind.name() == other)
                .map(Scope::Kind)
                .ok_or_else(|| {
                    let names = Scope::names();
                    let suggestion = lsf_core::util::closest_match(other, names.iter().copied())
                        .map(|name| format!(" Did you mean \"{name}\"?"))
                        .unwrap_or_default();
                    format!(
                        "\"{other}\" is not a kind of request.{suggestion} Kinds: {}.",
                        names.join(", ")
                    )
                }),
        }
    }
}

/// `300ms`, `1.5s`, or a number of milliseconds.
fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let (number, unit) = match text {
        text if text.ends_with("ms") => (&text[..text.len() - 2], 1.0),
        text if text.ends_with('s') => (&text[..text.len() - 1], 1000.0),
        text => (text, 1.0),
    };
    let milliseconds = number
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| format!("\"{text}\" is not a duration: write it like 300ms, 1.5s or 250"))?
        * unit;
    let duration = Duration::from_millis(milliseconds.round() as u64);
    if duration > LONGEST {
        return Err(format!(
            "\"{text}\" is too long: a request can be delayed by {}s at most",
            LONGEST.as_secs()
        ));
    }
    Ok(duration)
}

/// A set of rules that comes with lsf, in milliseconds per kind.
pub struct Preset {
    pub name: &'static str,
    rules: [(Kind, u64); 13],
}

/// The presets, by name.
///
/// `simulated` follows what Shopify's own Horizon demo store answers in, timed from a fast
/// connection in October 2026: about 70 ms for a page or a section (served from the edge
/// cache), 150 ms for the cart, 200 to 300 ms for search, recommendations and product data,
/// and well under 100 ms for assets and images. Requests that write (cart changes, forms)
/// were not timed against someone's store: their values are estimates, set above the reads.
pub const PRESETS: [Preset; 2] = [
    // A Shopify storefront on a good connection.
    Preset {
        name: "simulated",
        rules: [
            (Kind::Page, 80),
            (Kind::Section, 80),
            (Kind::CartRead, 150),
            (Kind::CartAdd, 300),
            (Kind::CartChange, 300),
            (Kind::CartUpdate, 300),
            (Kind::CartClear, 300),
            (Kind::Search, 250),
            (Kind::Recommendations, 250),
            (Kind::Product, 200),
            (Kind::Form, 400),
            (Kind::Asset, 30),
            (Kind::Image, 40),
        ],
    },
    // The same storefront on a slow mobile connection.
    Preset {
        name: "slow",
        rules: [
            (Kind::Page, 800),
            (Kind::Section, 600),
            (Kind::CartRead, 700),
            (Kind::CartAdd, 1200),
            (Kind::CartChange, 1200),
            (Kind::CartUpdate, 1200),
            (Kind::CartClear, 1200),
            (Kind::Search, 1000),
            (Kind::Recommendations, 1000),
            (Kind::Product, 800),
            (Kind::Form, 1500),
            (Kind::Asset, 300),
            (Kind::Image, 500),
        ],
    },
];

/// The word that removes every rule, to lift the server's throttle for a session.
const NONE: &str = "none";

fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.name == name)
}

fn unknown_word(word: &str) -> String {
    let names: Vec<&str> = PRESETS
        .iter()
        .map(|preset| preset.name)
        .chain([NONE])
        .collect();
    let suggestion = lsf_core::util::closest_match(word, names.iter().copied())
        .map(|name| format!(" Did you mean \"{name}\"?"))
        .unwrap_or_default();
    format!(
        "\"{word}\" is neither a duration (300ms, 1.5s, 250) nor a preset.{suggestion} Presets: {}.",
        names.join(", ")
    )
}

/// How long each kind of request is held before it is answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Throttle {
    rules: HashMap<Scope, Duration>,
}

impl Throttle {
    /// Whether no request is delayed.
    pub fn is_empty(&self) -> bool {
        self.rules.values().all(Duration::is_zero)
    }

    /// Parses rules written like on the command line: `500ms` (everything),
    /// `cart=500ms,cart-add=1s,image=0`, or a preset and what to change in it
    /// (`simulated,cart-add=2s`). Rules apply in order: a later one replaces an earlier one.
    pub fn parse<'a>(specs: impl IntoIterator<Item = &'a str>) -> Result<Throttle, String> {
        let mut throttle = Throttle::default();
        for rule in specs.into_iter().flat_map(|spec| spec.split(',')) {
            let rule = rule.trim();
            if rule.is_empty() {
                continue;
            }
            match rule.split_once('=') {
                Some((name, duration)) => {
                    throttle
                        .rules
                        .insert(Scope::parse(name.trim())?, parse_duration(duration)?);
                }
                None => throttle.apply_word(rule)?,
            }
        }
        Ok(throttle)
    }

    /// Applies a rule that stands on its own: a preset, `none`, or a duration for everything.
    fn apply_word(&mut self, word: &str) -> Result<(), String> {
        if word == NONE {
            self.rules.clear();
        } else if let Some(preset) = preset(word) {
            // A preset says everything: nothing before it survives.
            self.rules.clear();
            for (kind, milliseconds) in preset.rules {
                self.rules
                    .insert(Scope::Kind(kind), Duration::from_millis(milliseconds));
            }
        } else if word.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '-') {
            self.rules.insert(Scope::All, parse_duration(word)?);
        } else {
            return Err(unknown_word(word));
        }
        Ok(())
    }

    /// Parses the `throttle` of a session: a string like on the command line, a number of
    /// milliseconds for everything, or an object of rules.
    pub fn from_json(value: &Json) -> Result<Throttle, String> {
        let duration = |value: &Json| match value {
            Json::Number(number) => parse_duration(&number.to_string()),
            Json::String(text) => parse_duration(text),
            other => Err(format!(
                "{other} is not a duration: use a number of milliseconds or a string like \"300ms\""
            )),
        };
        match value {
            Json::String(text) => Throttle::parse([text.as_str()]),
            Json::Number(_) => Ok(Throttle {
                rules: HashMap::from([(Scope::All, duration(value)?)]),
            }),
            Json::Object(map) => {
                let mut throttle = Throttle::default();
                // The preset first, whatever the order of the keys: the rules adjust it.
                if let Some(name) = map.get("preset") {
                    match name.as_str() {
                        Some(name) if name == NONE || preset(name).is_some() => {
                            throttle.apply_word(name)?
                        }
                        Some(name) => return Err(unknown_word(name)),
                        None => return Err(format!("{name} is not the name of a preset")),
                    }
                }
                for (name, value) in map {
                    if name != "preset" {
                        throttle.rules.insert(Scope::parse(name)?, duration(value)?);
                    }
                }
                Ok(throttle)
            }
            other => Err(format!(
                "{other} is not a throttle: use a duration, or an object such as {{\"cart-add\": \"1s\"}}"
            )),
        }
    }

    /// How long a request of this kind waits.
    pub fn delay(&self, kind: Kind) -> Duration {
        self.rules
            .get(&Scope::Kind(kind))
            .or_else(|| {
                kind.is_cart()
                    .then(|| self.rules.get(&Scope::Cart))
                    .flatten()
            })
            .or_else(|| self.rules.get(&Scope::All))
            .copied()
            .unwrap_or_default()
    }

    /// The rules as JSON, in milliseconds: `{"all": 300, "cart-add": 1000}`.
    pub fn to_json(&self) -> Json {
        let mut rules: Vec<(&Scope, &Duration)> = self.rules.iter().collect();
        rules
            .sort_by_key(|(scope, _)| Scope::names().iter().position(|name| *name == scope.name()));
        Json::Object(
            rules
                .into_iter()
                .map(|(scope, duration)| (scope.name().to_string(), json!(duration.as_millis())))
                .collect(),
        )
    }
}

impl std::fmt::Display for Throttle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let rules: Vec<String> = self
            .to_json()
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, milliseconds)| format!("{name}={milliseconds}ms"))
            .collect();
        f.write_str(&rules.join(", "))
    }
}

/// The kind of a request, from its method, its path without the locale prefix, and whether
/// it asks for sections. `None` for what is never delayed: the control API, which tests use
/// to set things up.
pub fn classify(method: &str, path: &str, wants_sections: bool) -> Option<Kind> {
    if path.starts_with("/__lsf") {
        return None;
    }
    if let Some(rest) = path.strip_prefix("/cdn/") {
        let extension = rest
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let image = matches!(
            extension.as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "bmp" | "svg"
        ) && [
            "shop/files/",
            "shop/products/",
            "shop/collections/",
            "shop/articles/",
        ]
        .iter()
        .any(|folder| rest.starts_with(folder));
        return Some(if image { Kind::Image } else { Kind::Asset });
    }
    let trimmed = path.trim_matches('/');
    let segments: Vec<&str> = trimmed.split('/').collect();
    let post = method.eq_ignore_ascii_case("POST");
    Some(match segments.as_slice() {
        ["cart.js" | "cart.json"] => Kind::CartRead,
        ["cart", "add" | "add.js"] => Kind::CartAdd,
        ["cart", "change" | "change.js"] => Kind::CartChange,
        ["cart", "update" | "update.js"] => Kind::CartUpdate,
        ["cart", "clear" | "clear.js"] => Kind::CartClear,
        // The cart form posts its quantities.
        ["cart"] if post => Kind::CartUpdate,
        ["search", "suggest" | "suggest.json"] => Kind::Search,
        ["recommendations", "products" | "products.json"] => Kind::Recommendations,
        ["products.json"] | ["collections", _, "products.json"] => Kind::Product,
        ["products", name] if name.ends_with(".js") || name.ends_with(".json") => Kind::Product,
        _ if post => Kind::Form,
        _ if wants_sections => Kind::Section,
        _ => Kind::Page,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn the_most_specific_rule_wins() {
        let throttle = Throttle::parse(["300ms", "cart=500ms,cart-add=1.5s", "image=0"]).unwrap();
        assert_eq!(throttle.delay(Kind::Page), MS(300));
        assert_eq!(throttle.delay(Kind::Section), MS(300));
        assert_eq!(throttle.delay(Kind::CartRead), MS(500));
        assert_eq!(throttle.delay(Kind::CartChange), MS(500));
        assert_eq!(throttle.delay(Kind::CartAdd), MS(1500));
        // A rule of zero exempts a kind from the rule about everything.
        assert_eq!(throttle.delay(Kind::Image), MS(0));
        assert_eq!(
            throttle.to_string(),
            "all=300ms, cart=500ms, cart-add=1500ms, image=0ms"
        );

        let only_add = Throttle::parse(["cart-add=800"]).unwrap();
        assert_eq!(only_add.delay(Kind::CartAdd), MS(800));
        assert_eq!(only_add.delay(Kind::CartRead), MS(0));
        assert!(Throttle::default().is_empty());
        assert!(Throttle::parse(["0"]).unwrap().is_empty());
        assert!(!only_add.is_empty());
    }

    #[test]
    fn presets_are_rules_that_can_be_adjusted() {
        let simulated = Throttle::parse(["simulated"]).unwrap();
        assert_eq!(simulated.delay(Kind::Page), MS(80));
        assert_eq!(simulated.delay(Kind::CartRead), MS(150));
        assert_eq!(simulated.delay(Kind::CartAdd), MS(300));
        // Every kind has a value, and reads are faster than writes.
        for kind in Kind::ALL {
            assert!(!simulated.delay(kind).is_zero(), "{}", kind.name());
            assert!(
                Throttle::parse(["slow"]).unwrap().delay(kind) > simulated.delay(kind),
                "{}",
                kind.name()
            );
        }

        let adjusted = Throttle::parse(["simulated,cart-add=2s,image=0"]).unwrap();
        assert_eq!(adjusted.delay(Kind::CartAdd), MS(2000));
        assert_eq!(adjusted.delay(Kind::Image), MS(0));
        assert_eq!(adjusted.delay(Kind::Page), MS(80));
        // Rules apply in order.
        assert_eq!(
            Throttle::parse(["cart-add=2s", "simulated"])
                .unwrap()
                .delay(Kind::CartAdd),
            MS(300)
        );
        assert!(Throttle::parse(["simulated,none"]).unwrap().is_empty());

        let from_json = Throttle::from_json(&json!({"cart-add": "2s", "preset": "slow"})).unwrap();
        assert_eq!(from_json.delay(Kind::CartAdd), MS(2000));
        assert_eq!(from_json.delay(Kind::Page), MS(800));
        assert_eq!(Throttle::from_json(&json!("simulated")).unwrap(), simulated);
        assert!(Throttle::from_json(&json!("none")).unwrap().is_empty());

        let unknown = Throttle::parse(["simulted"]).unwrap_err();
        assert!(unknown.contains("Did you mean \"simulated\"?"), "{unknown}");
        assert!(
            unknown.contains("Presets: simulated, slow, none."),
            "{unknown}"
        );
        assert!(Throttle::from_json(&json!({"preset": "fast"})).is_err());
    }

    /// The README gives the values of the presets: it must say what the code does.
    #[test]
    fn the_readme_lists_the_presets_as_they_are() {
        let readme = include_str!("../../../../README.md");
        let value = |name: &str, kind: Kind| {
            let preset = preset(name).expect("a preset");
            let (_, milliseconds) = preset.rules.iter().find(|(k, _)| *k == kind).unwrap();
            *milliseconds
        };
        for kind in Kind::ALL {
            let row = readme
                .lines()
                .find(|line| line.starts_with(&format!("| `{}` |", kind.name())))
                .unwrap_or_else(|| panic!("the README has no row for `{}`", kind.name()));
            let expected = format!("| {} | {} |", value("simulated", kind), value("slow", kind));
            assert!(
                row.ends_with(&expected),
                "README: {row}\nexpected to end with {expected}"
            );
        }
        for preset in &PRESETS {
            assert!(
                readme.contains(&format!("| `{}` |", preset.name)),
                "{}",
                preset.name
            );
        }
    }

    #[test]
    fn mistakes_are_explained() {
        let unknown = Throttle::parse(["cart-ad=1s"]).unwrap_err();
        assert!(unknown.contains("Did you mean \"cart-add\"?"), "{unknown}");
        assert!(unknown.contains("recommendations"), "{unknown}");
        assert!(
            Throttle::parse(["cart=soon"])
                .unwrap_err()
                .contains("is not a duration")
        );
        assert!(Throttle::parse(["90s"]).unwrap_err().contains("too long"));
        assert!(Throttle::parse(["-5"]).is_err());
    }

    #[test]
    fn sessions_describe_their_throttle_in_json() {
        assert_eq!(
            Throttle::from_json(&json!("cart-add=1s"))
                .unwrap()
                .delay(Kind::CartAdd),
            MS(1000)
        );
        assert_eq!(
            Throttle::from_json(&json!(250)).unwrap().delay(Kind::Page),
            MS(250)
        );
        let rules = Throttle::from_json(&json!({"cart": "400ms", "section": 120})).unwrap();
        assert_eq!(rules.delay(Kind::CartClear), MS(400));
        assert_eq!(rules.delay(Kind::Section), MS(120));
        assert_eq!(rules.to_json(), json!({"cart": 400, "section": 120}));
        assert!(Throttle::from_json(&json!({"panier": 100})).is_err());
        assert!(Throttle::from_json(&json!(true)).is_err());
    }

    #[test]
    fn requests_are_told_apart() {
        let kind = |method, path, sections| classify(method, path, sections);
        assert_eq!(kind("GET", "/", false), Some(Kind::Page));
        assert_eq!(kind("GET", "/products/mug", false), Some(Kind::Page));
        assert_eq!(kind("GET", "/cart", false), Some(Kind::Page));
        assert_eq!(kind("GET", "/products/mug", true), Some(Kind::Section));
        assert_eq!(kind("GET", "/cart.js", false), Some(Kind::CartRead));
        // Sections bundled with a cart request do not make it a section request.
        assert_eq!(kind("POST", "/cart/add.js", true), Some(Kind::CartAdd));
        assert_eq!(kind("POST", "/cart/add", false), Some(Kind::CartAdd));
        assert_eq!(
            kind("POST", "/cart/change.js", false),
            Some(Kind::CartChange)
        );
        assert_eq!(
            kind("POST", "/cart/update.js", false),
            Some(Kind::CartUpdate)
        );
        assert_eq!(kind("POST", "/cart", false), Some(Kind::CartUpdate));
        assert_eq!(kind("POST", "/cart/clear.js", false), Some(Kind::CartClear));
        assert_eq!(kind("GET", "/search/suggest", true), Some(Kind::Search));
        assert_eq!(kind("GET", "/search", false), Some(Kind::Page));
        assert_eq!(
            kind("GET", "/recommendations/products", true),
            Some(Kind::Recommendations)
        );
        assert_eq!(kind("GET", "/products/mug.js", false), Some(Kind::Product));
        assert_eq!(
            kind("GET", "/collections/all/products.json", false),
            Some(Kind::Product)
        );
        assert_eq!(kind("POST", "/contact", false), Some(Kind::Form));
        assert_eq!(kind("POST", "/account/login", false), Some(Kind::Form));
        assert_eq!(
            kind("GET", "/cdn/shop/t/1/assets/theme.css", false),
            Some(Kind::Asset)
        );
        assert_eq!(
            kind("GET", "/cdn/shop/files/products/mug.jpg", false),
            Some(Kind::Image)
        );
        assert_eq!(
            kind("GET", "/cdn/shop/files/guide.pdf", false),
            Some(Kind::Asset)
        );
        assert_eq!(kind("PUT", "/__lsf/session", false), None);
    }
}
