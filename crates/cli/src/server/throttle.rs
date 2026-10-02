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

    /// Parses rules written like on the command line: `500ms` (everything), or
    /// `cart=500ms,cart-add=1s,image=0`.
    pub fn parse<'a>(specs: impl IntoIterator<Item = &'a str>) -> Result<Throttle, String> {
        let mut throttle = Throttle::default();
        for rule in specs.into_iter().flat_map(|spec| spec.split(',')) {
            let rule = rule.trim();
            if rule.is_empty() {
                continue;
            }
            let (scope, duration) = match rule.split_once('=') {
                Some((name, duration)) => (Scope::parse(name.trim())?, duration),
                None => (Scope::All, rule),
            };
            throttle.rules.insert(scope, parse_duration(duration)?);
        }
        Ok(throttle)
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
                for (name, value) in map {
                    throttle.rules.insert(Scope::parse(name)?, duration(value)?);
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
