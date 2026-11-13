//! Everything a render needs to know that does not change while it runs: the theme, the store,
//! the request and the visitor's session.

use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use lsf_core::session::Session;
use lsf_core::store::Store;

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
    /// The prefix of every storefront URL in this language: empty, `/fr`, or `/fr-ca` under the
    /// subfolders of a market.
    pub root: String,
    /// The handle of the market whose subfolders the URL is under (`/fr-ca/...`): `None` at
    /// `/`, where the primary market and the markets without a subfolder are served.
    pub market: Option<String>,
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
            market: None,
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

    /// The parameters of the request that the URLs Liquid builds from it keep: all but those
    /// that render a section alone (`section_id`, `sections`), since those URLs lead to the page.
    pub fn page_query(&self) -> impl Iterator<Item = (&str, &str)> {
        self.query
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "section_id" | "sections" | "sections[]"))
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// A URL of the current page with these parameters, written as Shopify writes those of
    /// `paginate` and of filters: `page` first when there is one, then the others sorted by
    /// name, each name's values in their order (`/collections/all?page=2&sort_by=price-ascending`).
    pub fn page_url<'a>(
        &self,
        page: Option<usize>,
        params: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> String {
        self.page_url_with("page", page, params)
    }

    /// [`Request::page_url`] with the page in `param`, a list's own (`page_b72aba52`).
    pub fn page_url_with<'a>(
        &self,
        param: &str,
        page: Option<usize>,
        params: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> String {
        use lsf_core::urls::encode_component;
        let mut params: Vec<(&str, &str)> = params.into_iter().collect();
        params.sort_by_key(|(key, _)| *key);
        let pairs: Vec<String> = page
            .map(|page| format!("{param}={page}"))
            .into_iter()
            .chain(params.iter().map(|(key, value)| {
                format!("{}={}", encode_component(key), encode_component(value))
            }))
            .collect();
        let path = self.localized(&self.path);
        if pairs.is_empty() {
            path
        } else {
            format!("{path}?{}", pairs.join("&"))
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
                    lsf_core::urls::encode_component(key),
                    lsf_core::urls::encode_component(value)
                )
            })
            .collect();
        format!("{path}?{}", query.join("&"))
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
    /// The handles of the menus lsf made up that the render used, in the order it first did.
    default_menus: Mutex<Vec<String>>,
    /// The page `{% paginate %}` shows of each list that has a page parameter of its own, by
    /// that parameter: a list rebuilt at each lookup (a metafield's) is paged all the same.
    paginate_windows: Mutex<Vec<(String, PaginateWindow)>>,
}

/// The items a paginated list shows: `(offset, size)`, or all of them outside `paginate`.
pub(crate) type PaginateWindow = Arc<Mutex<Option<(usize, usize)>>>;

impl Site {
    pub fn new(
        theme: Arc<Theme>,
        store: Arc<Store>,
        request: Request,
        session: Session,
        now: DateTime<Utc>,
    ) -> Site {
        // Prices are in the currency of the visitor's country, at its market's rate or fixed
        // prices, as Shopify presents a storefront to a market.
        let market = request
            .market
            .as_deref()
            .and_then(|handle| store.market_named(handle));
        let country = store
            .country_at(session.country.as_deref(), market)
            .iso_code
            .clone();
        let store = store.presented_for(&country);
        Site {
            theme,
            store,
            request,
            session,
            now,
            translations: OnceLock::new(),
            schema_translations: OnceLock::new(),
            default_menus: Mutex::new(Vec::new()),
            paginate_windows: Mutex::new(Vec::new()),
        }
    }

    /// The window of the lists of a page parameter, shared by every lookup of them.
    pub(crate) fn paginate_window(&self, param: &str) -> PaginateWindow {
        let mut windows = self.paginate_windows.lock().expect("windows poisoned");
        if let Some((_, window)) = windows.iter().find(|(known, _)| known == param) {
            return window.clone();
        }
        let window = PaginateWindow::default();
        windows.push((param.to_string(), window.clone()));
        window
    }

    /// Notes that the render uses a menu lsf made up ([`lsf_core::store::Menu::supplied_by_lsf`]).
    pub(crate) fn note_default_menu(&self, handle: &str) {
        let mut handles = self.default_menus.lock().expect("menus poisoned");
        if !handles.iter().any(|known| known == handle) {
            handles.push(handle.to_string());
        }
    }

    /// The handles of the menus lsf made up that the render used, in the order it first did.
    pub fn default_menus(&self) -> Vec<String> {
        self.default_menus.lock().expect("menus poisoned").clone()
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

    /// The country the visitor shops in: the one they selected, when the web presence of the
    /// URL serves it, else the presence's default ([`lsf_core::Store::country_at`]): the
    /// store's default country at `/`, the one the checkout opens in.
    pub fn country(&self) -> &lsf_core::store::Country {
        self.store
            .country_at(self.session.country.as_deref(), self.market())
    }

    /// The market whose subfolders the page is under (`/fr-ca/...`), if it is.
    pub fn market(&self) -> Option<&lsf_core::store::Market> {
        self.request
            .market
            .as_deref()
            .and_then(|handle| self.store.market_named(handle))
    }

    /// The prefix of a language's URLs on the web presence of the page: `/fr`, or `/fr-ca`
    /// under the subfolders of a market. `/` for the primary language at the root.
    pub fn root_url_of(&self, language: &lsf_core::store::Language) -> String {
        let root = self.store.locale_root(language, self.market());
        if root.is_empty() {
            "/".to_string()
        } else {
            root
        }
    }

    /// The currency prices are displayed in: the one of the visitor's country.
    pub fn currency(&self) -> &str {
        self.store.currency()
    }

    pub fn language(&self) -> &lsf_core::store::Language {
        self.store
            .language(&self.request.locale)
            .unwrap_or_else(|| self.store.primary_language())
    }

    pub fn customer(&self) -> Option<&lsf_core::store::Customer> {
        self.session
            .customer_id
            .and_then(|id| self.store.customer_by_id(id))
    }
}
