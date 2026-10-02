//! Everything a render needs to know that does not change while it runs: the theme, the store,
//! the request and the visitor's session.

use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use indexmap::IndexMap;

use crate::store::{CartLine, Store};
use crate::theme::{Theme, Translations};

/// The request being rendered.
#[derive(Clone, Debug)]
pub struct Request {
    /// `localhost:9292`: the host the visitor used, which generated URLs point back to.
    pub host: String,
    /// `http` or `https`.
    pub scheme: String,
    /// The path without the locale prefix: `/products/shirt`.
    pub path: String,
    /// Decoded query parameters, in order.
    pub query: Vec<(String, String)>,
    /// The language being served, e.g. `en`.
    pub locale: String,
    /// The prefix of every storefront URL in this language: empty, or `/fr`.
    pub root: String,
}

impl Request {
    pub fn new(host: impl Into<String>, path: impl Into<String>) -> Self {
        Request {
            host: host.into(),
            scheme: "http".to_string(),
            path: path.into(),
            query: Vec::new(),
            locale: "en".to_string(),
            root: String::new(),
        }
    }

    /// The first value of a query parameter.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Every value of a repeated query parameter.
    pub fn params<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.query
            .iter()
            .filter(move |(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// `http://localhost:9292`.
    pub fn origin(&self) -> String {
        format!("{}://{}", self.scheme, self.host)
    }

    /// Prefixes a storefront path with the locale root: `/cart` → `/fr/cart`.
    pub fn localized(&self, path: &str) -> String {
        if self.root.is_empty() {
            path.to_string()
        } else if path == "/" {
            self.root.clone()
        } else {
            format!("{}{path}", self.root)
        }
    }

    /// The path and query as requested, e.g. `/fr/search?q=mug`.
    pub fn path_with_query(&self) -> String {
        let path = self.localized(&self.path);
        if self.query.is_empty() {
            return path;
        }
        let query: Vec<String> = self
            .query
            .iter()
            .map(|(key, value)| {
                format!(
                    "{}={}",
                    crate::urls::encode_component(key),
                    crate::urls::encode_component(value)
                )
            })
            .collect();
        format!("{path}?{}", query.join("&"))
    }
}

/// The outcome of a form the visitor just submitted, shown on the next render.
#[derive(Clone, Debug, Default)]
pub struct FormResult {
    /// `contact`, `customer_login`, ...
    pub form_type: String,
    pub posted_successfully: bool,
    /// `(field, message)`.
    pub errors: Vec<(String, String)>,
    /// The submitted values, to repopulate the form.
    pub values: IndexMap<String, String>,
}

/// The visitor's state.
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub cart_lines: Vec<CartLine>,
    pub cart_note: String,
    pub cart_attributes: IndexMap<String, String>,
    /// The id of the logged-in customer.
    pub customer_id: Option<u64>,
    /// ISO code of the selected country.
    pub country: Option<String>,
    pub form_result: Option<FormResult>,
    /// For a B2B customer: the id of the company location they are buying for.
    pub company_location: Option<u64>,
    /// The token of the visitor's cart, as `/cart.js` reports it and as the `cart` cookie
    /// holds it. Empty outside a server, where carts take a token derived from the host.
    pub cart_token: String,
}

impl Session {
    /// Whether the visitor has a cart: Shopify only creates one when something goes into it.
    pub fn has_cart(&self) -> bool {
        !self.cart_lines.is_empty()
            || !self.cart_note.is_empty()
            || !self.cart_attributes.is_empty()
    }

    /// The state a new visitor starts with, as configured in the store data.
    pub fn initial(store: &Store) -> Session {
        let defaults = &store.session_defaults;
        Session {
            cart_lines: defaults.cart_lines.clone(),
            cart_note: defaults.cart_note.clone(),
            cart_attributes: defaults.cart_attributes.clone(),
            customer_id: defaults
                .customer_email
                .as_deref()
                .and_then(|who| store.customer_named(who).ok().flatten())
                .map(|customer| customer.id),
            country: defaults.country.clone(),
            form_result: None,
            company_location: defaults.company_location,
            cart_token: String::new(),
        }
    }
}

/// Whether customer accounts are hosted by Shopify rather than rendered by the theme: what
/// the store data says, and otherwise whether the theme has a login template.
pub fn hosted_accounts(theme: &Theme, store: &Store) -> bool {
    use crate::store::model::CustomerAccounts;
    match store.shop.customer_accounts {
        Some(CustomerAccounts::New) => true,
        Some(CustomerAccounts::Legacy) => false,
        None => !["json", "liquid"].iter().any(|extension| {
            theme
                .files()
                .exists(&format!("templates/customers/login.{extension}"))
        }),
    }
}

pub struct Site {
    pub theme: Arc<Theme>,
    pub store: Arc<Store>,
    pub request: Request,
    pub session: Session,
    /// The instant `'now'` resolves to for this render.
    pub now: DateTime<Utc>,
    // A page translates hundreds of keys. The theme's translations are looked up once per
    // render, because asking the theme takes locks that every other render takes too.
    translations: OnceLock<[(String, Arc<Translations>); 2]>,
    schema_translations: OnceLock<Arc<Translations>>,
}

impl Site {
    pub fn new(
        theme: Arc<Theme>,
        store: Arc<Store>,
        request: Request,
        session: Session,
        now: DateTime<Utc>,
    ) -> Site {
        Site {
            theme,
            store,
            request,
            session,
            now,
            translations: OnceLock::new(),
            schema_translations: OnceLock::new(),
        }
    }

    /// The storefront translations a key is looked up in, in order: those of the request's
    /// locale, then those of the theme's default locale. Each comes with its locale.
    pub fn translations(&self) -> &[(String, Arc<Translations>)] {
        self.translations.get_or_init(|| {
            [self.request.locale.clone(), self.theme.default_locale()].map(|locale| {
                let translations = self.theme.translations(&locale);
                (locale, translations)
            })
        })
    }

    /// Translations for the `t:` keys used in schemas, in the request's locale.
    pub fn schema_translations(&self) -> &Translations {
        self.schema_translations
            .get_or_init(|| self.theme.schema_translations(&self.request.locale))
    }

    /// The country the visitor shops in.
    pub fn country(&self) -> &crate::store::Country {
        self.session
            .country
            .as_deref()
            .and_then(|code| self.store.country(code))
            .unwrap_or(&self.store.countries[0])
    }

    /// The currency prices are displayed in.
    pub fn currency(&self) -> &str {
        &self.store.shop.currency
    }

    pub fn language(&self) -> &crate::store::Language {
        self.store
            .language(&self.request.locale)
            .unwrap_or_else(|| self.store.primary_language())
    }

    pub fn customer(&self) -> Option<&crate::store::Customer> {
        self.session
            .customer_id
            .and_then(|id| self.store.customer_by_id(id))
    }

    /// Whether customer accounts are hosted by Shopify rather than rendered by the theme.
    pub fn hosted_accounts(&self) -> bool {
        hosted_accounts(&self.theme, &self.store)
    }
}
