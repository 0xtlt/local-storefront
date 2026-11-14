//! Storefront routes: pages, the Section Rendering API and the JSON endpoints themes call.

use std::sync::Arc;

use lsf_core::{Session, Store};
use lsf_theme::drops::collection::CollectionDrop;
use lsf_theme::drops::product::{ProductDrop, ajax_product_json, storefront_json};
use lsf_theme::drops::search::{predictive_search_value, recommendations_value};
use lsf_theme::render::page::{Page, Resource};
use lsf_theme::render::{Rendered, Target, seo};
use lsf_theme::{Request, Site};
use serde_json::{Value as Json, json};

use super::reply::{Reply, cache};
use super::{
    CART_COOKIE, CART_CURRENCY_COOKIE, Incoming, SESSION_COOKIE, ServerState, account, cart, forms,
    live_reload, timing,
};

/// A request being handled for a given visitor.
pub struct Visit<'a> {
    pub state: &'a ServerState,
    pub incoming: &'a Incoming,
    pub session_id: String,
    pub store: Arc<Store>,
    /// The request with the locale prefix resolved.
    pub request: Request,
}

impl Visit<'_> {
    /// The visitor's session as it is now (handlers may have just changed it).
    pub fn session(&self) -> Session {
        self.state
            .with_session(&self.session_id, |entry| entry.session.clone())
    }

    /// Changes the visitor's session. A change of the cart (its lines, note or attributes) is
    /// reported to the Admin API, as the storefront's cart fires `carts/create` and
    /// `carts/update` on Shopify.
    pub fn update_session(&self, change: impl FnOnce(&mut Session)) {
        if self.state.admin.is_none() {
            self.state
                .with_session(&self.session_id, |entry| change(&mut entry.session));
            return;
        }
        let changed = self.state.with_session(&self.session_id, |entry| {
            let cart = |session: &Session| {
                (
                    session.cart_lines.clone(),
                    session.cart_note.clone(),
                    session.cart_attributes.clone(),
                )
            };
            let before = cart(&entry.session);
            change(&mut entry.session);
            (cart(&entry.session) != before).then(|| entry.session.clone())
        });
        if let Some(session) = changed {
            let token = if session.cart_token.is_empty() {
                super::cart_token_of(&self.session_id)
            } else {
                session.cart_token.clone()
            };
            super::admin::record_cart(self.state, &token, &session);
        }
    }

    /// The site of the visit, its cart as the cart transforms of the app make it.
    pub fn site(&self) -> Arc<Site> {
        self.site_of(self.request.clone())
    }

    fn site_of(&self, request: Request) -> Arc<Site> {
        self.state.app.renderer.site_with(
            self.store.clone(),
            request,
            self.session(),
            |store, session| {
                super::checkout::storefront_cart_transforms(self.state, store, session)
            },
        )
    }

    /// The site for another storefront path, with the same visitor.
    pub fn site_for(&self, path_and_query: &str) -> Arc<Site> {
        let (path, query) = path_and_query
            .split_once('?')
            .unwrap_or((path_and_query, ""));
        let request = self.state.storefront_request(
            &self.store,
            self.incoming,
            path,
            super::params::query_pairs(query),
        );
        self.site_of(request)
    }

    /// The market whose subfolders the URL is under (`/fr-ca/...`), if it is.
    pub fn site_market(&self) -> Option<&lsf_core::store::Market> {
        self.request
            .market
            .as_deref()
            .and_then(|handle| self.store.market_named(handle))
    }

    /// Prefixes a storefront path with the visitor's locale root.
    pub fn localized(&self, path: &str) -> String {
        self.request.localized(path)
    }

    /// Renders sections for the Section Rendering API bundled into cart responses.
    pub fn render_sections(&self, ids: &[String], url: Option<&str>) -> Json {
        let site = match url {
            Some(url) => self.site_for(url),
            None => self.site_for(
                &self
                    .incoming
                    .referer_path()
                    .unwrap_or_else(|| "/".to_string()),
            ),
        };
        let page = lsf_theme::render::routes::resolve(&site);
        let target = Target::Sections(ids.to_vec());
        let rendered = self.state.app.renderer.render_page(&site, page, &target);
        self.state.timings.add(&rendered, &target);
        serde_json::from_str(&rendered.body).unwrap_or(Json::Null)
    }
}

/// The section ids of a `sections` parameter: a comma-separated string or an array.
pub fn section_ids(value: Option<&Json>) -> Vec<String> {
    match value {
        Some(Json::String(list)) => list
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect(),
        Some(Json::Array(items)) => items
            .iter()
            .filter_map(Json::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

pub fn handle(state: &ServerState, incoming: &Incoming) -> Reply {
    let (session_id, is_new) = state.session_id(incoming);
    let (_, store) = state.snapshot(&session_id);
    let request =
        state.storefront_request(&store, incoming, &incoming.path, incoming.query.clone());
    let visit = Visit {
        state,
        incoming,
        session_id,
        store,
        request,
    };
    let reply = match follow_the_country_of_the_url(&visit) {
        Some(moved) => moved,
        None => route(&visit),
    };
    let reply = with_cache_policy(
        &visit,
        with_cart_currency_cookie(&visit, with_cart_cookie(&visit, reply)),
    );
    if is_new {
        reply.header(
            "set-cookie",
            format!(
                "{SESSION_COOKIE}={}; Path=/; HttpOnly; SameSite=Lax",
                visit.session_id
            ),
        )
    } else {
        reply
    }
}

/// The country the URL picks, as a Shopify storefront does:
///
/// - `?country=FR` picks a country the store sells to. When the web presence of the URL serves
///   it, the visitor is in it from then on and the page is served; otherwise the answer is a
///   `301` to the same URL on the presence that serves it (`/en-be/...?country=BE`).
/// - A page under the subfolders of a market (`/en-be/...`) is in the visitor's country when the
///   market has it, else in the market's first country; a page at `/`, in the visitor's country
///   when `/` serves it, else in the shop's. The session follows, so that the cart and the
///   checkout are in the country of the last page.
fn follow_the_country_of_the_url(visit: &Visit<'_>) -> Option<Reply> {
    let store = &visit.store;
    let market = visit.site_market();
    let reads = matches!(visit.incoming.method.as_str(), "GET" | "HEAD");
    if reads
        && let Some(country) = visit
            .incoming
            .query_param("country")
            .and_then(|code| store.country(code.trim()))
    {
        let presence = store
            .market_of(&country.iso_code)
            .filter(|market| market.subfolder_suffix.is_some());
        if presence.map(|market| &market.handle) != market.map(|market| &market.handle) {
            // In the page's language when the country's market offers it, else in the market's
            // default: `/es-es?country=BE` is `/en-be?country=BE` on Shopify.
            let current = store
                .language(&visit.request.locale)
                .unwrap_or_else(|| store.primary_language());
            let language = store.language_for(current, &country.iso_code);
            let mut moved = visit.request.clone();
            moved.root = store.locale_root(language, presence);
            return Some(Reply::moved(&moved.path_with_query()));
        }
        let code = country.iso_code.clone();
        visit.update_session(|session| session.country = Some(code));
        return None;
    }
    if store
        .markets
        .iter()
        .any(|market| market.subfolder_suffix.is_some())
    {
        let selected = visit.session().country;
        let country = store.country_at(selected.as_deref(), market);
        let follows = match market {
            Some(_) => Some(country.iso_code.clone()),
            // At `/`, the visitor's choice stays, or none when it was a country `/` does not
            // serve.
            None => selected.clone().filter(|code| store.served_at_root(code)),
        };
        if follows != selected {
            visit.update_session(|session| session.country = follows);
        }
    }
    None
}

/// Says how long the response can be kept, as a storefront does: what it renders is checked
/// again every time, what is not found and redirects are not kept at all, and neither is what
/// changes the cart. Reading the cart says nothing. Every answer depends on what the client
/// accepts, since the same URL can answer with a page or with JSON.
fn with_cache_policy(visit: &Visit<'_>, reply: Reply) -> Reply {
    let reply = reply.header("vary", "Accept");
    if reply.has_header("cache-control") {
        return reply;
    }
    let path = visit.request.path.trim_matches('/');
    let policy = match (path, reply.status) {
        ("cart.js" | "cart.json", _) => return reply,
        ("cart", _) if visit.incoming.method == "POST" => cache::CART_WRITE,
        (path, _) if path.starts_with("cart/") => cache::CART_WRITE,
        (_, 404 | 300..=399) => cache::PRIVATE,
        (_, 200) => cache::RENDERED,
        _ => return reply,
    };
    reply.cached(policy)
}

/// Whether the request changes the cart, which creates it if it did not exist.
fn changes_cart(visit: &Visit<'_>) -> bool {
    let path = visit.request.path.trim_matches('/');
    visit.incoming.method == "POST" && (path == "cart" || path.starts_with("cart/"))
}

/// Gives the visitor the `cart` cookie once they have a cart, as Shopify does. Scripts tell
/// "no cart yet" from "an empty cart" by it.
fn with_cart_cookie(visit: &Visit<'_>, reply: Reply) -> Reply {
    let session = visit.session();
    if !(session.has_cart() || changes_cart(visit)) {
        return reply;
    }
    with_cookie(
        visit,
        reply,
        format!("{CART_COOKIE}={}", session.cart_token),
    )
}

/// Gives the visitor who has a cart the `cart_currency` cookie, the currency the answer is in
/// (`EUR` under `/en-be/`), as Shopify's pages and Ajax answers set it. A redirect sets none:
/// the page it leads to does.
fn with_cart_currency_cookie(visit: &Visit<'_>, reply: Reply) -> Reply {
    if (300..400).contains(&reply.status) || !(visit.session().has_cart() || changes_cart(visit)) {
        return reply;
    }
    let currency = visit.site().currency().to_string();
    with_cookie(visit, reply, format!("{CART_CURRENCY_COOKIE}={currency}"))
}

/// Sets a cookie scripts can read (`name=value`), unless the browser sent it with that value.
fn with_cookie(visit: &Visit<'_>, reply: Reply, cookie: String) -> Reply {
    let known = visit
        .incoming
        .header("cookie")
        .is_some_and(|cookies| cookies.split(';').any(|pair| pair.trim() == cookie));
    if known {
        reply
    } else {
        reply.header("set-cookie", format!("{cookie}; Path=/; SameSite=Lax"))
    }
}

fn route(visit: &Visit<'_>) -> Reply {
    let method = visit.incoming.method.as_str();
    let path = visit.request.path.trim_matches('/').to_string();
    let segments: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    let is_post = method == "POST";

    match segments.as_slice() {
        ["cart.js" | "cart.json"] => cart::show(visit),
        ["cart", "add" | "add.js"] => cart::add(visit),
        ["cart", "change" | "change.js"] => cart::change(visit),
        ["cart", "update" | "update.js"] => cart::update(visit),
        ["cart", "clear" | "clear.js"] => cart::clear(visit),
        ["cart"] if is_post => cart::submit(visit),
        ["checkout"] | ["checkouts", ..] => forms::checkout(visit),
        ["contact"] if is_post => forms::contact(visit),
        ["blogs", _, _, "comments"] if is_post => forms::comment(visit),
        ["localization"] if is_post => forms::localization(visit),
        ["password"] if is_post => forms::password(visit),
        ["account", "logout"] => forms::logout(visit),
        ["company_location", "update"] => account::switch_location(visit),
        // Shopify's own login pages, which a theme links to with `routes.storefront_login_url`.
        ["customer_authentication" | "customer_identity", ..] => account::entry(visit),
        // Customer accounts are Shopify's (new customer accounts): a single page here, whatever
        // the theme links to or posts (its login and register forms).
        ["account"] if !is_post => account::page(visit),
        ["account", ..] => account::entry(visit),
        ["products.json"] => products_json(visit, None),
        ["products", name] if name.ends_with(".js") || name.ends_with(".json") => {
            product_json(visit, name)
        }
        ["collections", handle, "products.json"] => products_json(visit, Some(handle)),
        ["variants", id] if method == "GET" || method == "HEAD" => variant(visit, id),
        ["search", "suggest.json"] => suggest_json(visit),
        ["search", "suggest"] => special_section(visit, "search"),
        ["recommendations", "products.json"] => recommendations_json(visit),
        ["recommendations", "products"] => special_section(visit, "product"),
        ["robots.txt"] => {
            let rendered = visit.state.app.renderer.render_robots(&visit.site());
            reply_from(visit, rendered, &Target::Section(String::new()))
        }
        ["agents.md" | "llms.txt" | "llms-full.txt"] => {
            match visit.state.app.renderer.render_agents(&visit.site()) {
                Some(rendered) => reply_from(visit, rendered, &Target::Section(String::new())),
                None => page(visit),
            }
        }
        ["sitemap.xml"] => Reply::new(
            200,
            "application/xml; charset=utf-8",
            seo::sitemap_index(&visit.site()),
        ),
        [name] if name.starts_with("sitemap_") && name.ends_with("_1.xml") => {
            let kind = &name["sitemap_".len()..name.len() - "_1.xml".len()];
            match seo::sitemap(&visit.site(), kind) {
                Some(xml) => Reply::new(200, "application/xml; charset=utf-8", xml),
                None => Reply::not_found(),
            }
        }
        ["favicon.ico"] => Reply::new(204, "image/x-icon", Vec::new()),
        _ if method == "GET" || method == "HEAD" => page(visit),
        _ => Reply::text(404, "Not found"),
    }
}

/// The render target the Section Rendering API parameters ask for.
fn target(incoming: &Incoming) -> Target {
    if let Some(id) = incoming.query_param("section_id") {
        return Target::Section(id.to_string());
    }
    let ids: Vec<String> = incoming
        .query
        .iter()
        .filter(|(key, _)| key == "sections" || key == "sections[]")
        .flat_map(|(_, value)| {
            value
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    if ids.is_empty() {
        Target::Page
    } else {
        Target::Sections(ids)
    }
}

fn log_problems(state: &ServerState, rendered: &Rendered) {
    if state.options.quiet {
        return;
    }
    let mut seen = Vec::new();
    for error in &rendered.errors {
        let message = error.to_string();
        if !seen.contains(&message) {
            eprintln!("    {message}");
            seen.push(message);
        }
    }
    // Why app blocks did not render, and the menus of lsf the page renders: said once, until
    // the files change.
    let mut said = super::lock(&state.notices);
    for notice in &rendered.notices {
        if said.insert(notice.clone()) {
            eprintln!("    {notice}");
        }
    }
    for note in &rendered.default_menus {
        if said.insert(note.clone()) {
            eprintln!("    note: {note}");
        }
    }
}

/// A reason (of a 404, of an app proxy that got no answer) as a header value: printable ASCII
/// only, since the URL it quotes can hold anything.
pub(super) fn header_text(reason: &str) -> String {
    reason
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() {
                c
            } else {
                '?'
            }
        })
        .collect()
}

pub(super) fn reply_from(visit: &Visit<'_>, rendered: Rendered, target: &Target) -> Reply {
    log_problems(visit.state, &rendered);
    // A missing page is easy to take for a working one: say why it is missing.
    if let Some(reason) = &rendered.not_found
        && !visit.state.options.quiet
    {
        eprintln!("    {}: {reason}", rendered.status);
    }
    visit.state.timings.add(&rendered, target);
    let timings = timing::of_render(&rendered.timings, visit.state.options.timings);
    let mut body = rendered.body;
    if visit.state.options.live_reload
        && *target == Target::Page
        && live_reload::is_html(rendered.content_type)
    {
        live_reload::inject(&mut body);
        let loaded = visit.state.loaded();
        let data_errors: Vec<String> = loaded
            .diagnostics
            .items
            .iter()
            .filter(|item| item.severity == lsf_core::diagnostics::Severity::Error)
            .map(|item| format!("{}: {}", item.file, item.message))
            .collect();
        let liquid_errors: Vec<String> = rendered.errors.iter().map(|e| e.to_string()).collect();
        if let Some(overlay) = live_reload::overlay(
            &data_errors,
            &liquid_errors,
            rendered.not_found.as_deref(),
            &rendered.default_menus,
        ) {
            live_reload::inject_markup(&mut body, &overlay);
        }
    }
    let mut reply = Reply::new(rendered.status, rendered.content_type, body.into_bytes())
        .header("x-lsf-template", rendered.template)
        .header("server-timing", timings)
        .header("content-language", visit.request.locale.clone());
    if let Some(reason) = &rendered.not_found {
        reply = reply.header("x-lsf-not-found", header_text(reason));
    }
    if !rendered.errors.is_empty() {
        reply = reply.header("x-lsf-liquid-errors", rendered.errors.len().to_string());
    }
    // An app block that renders nothing is easy to miss: every reason of the page.
    match rendered.notices.as_slice() {
        [] => {}
        [notice] => reply = reply.header("x-lsf-reason", header_text(notice)),
        notices => {
            let all = format!("{} reasons: {}", notices.len(), notices.join(" | "));
            reply = reply.header("x-lsf-reason", header_text(&all));
        }
    }
    if !rendered.preloads.is_empty() {
        reply = reply.header("link", rendered.preloads.join(", "));
    }
    reply
}

fn page(visit: &Visit<'_>) -> Reply {
    let target = target(visit.incoming);
    if target == Target::Page
        && let Some(location) = lsf_theme::render::routes::redirect(&visit.site())
    {
        return Reply::moved(&location);
    }
    let rendered = visit.state.app.renderer.render(
        visit.store.clone(),
        visit.request.clone(),
        visit.session(),
        &target,
    );
    // The outcome of a form submission is shown once.
    if target == Target::Page {
        visit.update_session(|session| session.form_result = None);
    }
    reply_from(visit, rendered, &target)
}

/// `/search/suggest` and `/recommendations/products` render a section against their own
/// objects (`predictive_search`, `recommendations`) rather than a page.
fn special_section(visit: &Visit<'_>, template: &str) -> Reply {
    let target = target(visit.incoming);
    // Without a section to render, a store answers `422` (an invalid parameter error), as HTML:
    // "Invalid parameter error: Missing section_id param" for predictive search, an empty body
    // for recommendations.
    // https://shopify.dev/docs/api/ajax/reference/predictive-search#error-responses
    if target == Target::Page {
        return Reply::html(
            422,
            if template == "search" {
                "Invalid parameter error: Missing section_id param"
            } else {
                ""
            },
        );
    }
    let site = visit.site();
    // Recommendations are rendered for the product they are about, so that the section sees
    // it as `product`, like on the product page.
    let recommended_for = visit
        .incoming
        .query_param("product_id")
        .and_then(|id| id.parse::<u64>().ok())
        .and_then(|id| site.store.product_index_by_id(id));
    let resource = match (template, recommended_for) {
        ("search", _) => Resource::Search,
        (_, Some(product)) => Resource::Product {
            product,
            collection: None,
        },
        _ => Resource::Index,
    };
    let rendered =
        visit
            .state
            .app
            .renderer
            .render_page(&site, Page::new(template, resource), &target);
    reply_from(visit, rendered, &target)
}

/// `/variants/<id>` leads to the product of the variant, with the variant selected. With the
/// Section Rendering API parameters it renders sections for the variant instead, which is how
/// themes load pickup availability.
fn variant(visit: &Visit<'_>, id: &str) -> Reply {
    let product = id
        .parse::<u64>()
        .ok()
        .and_then(|id| visit.store.variant(id))
        .map(|(product, _)| product);
    match product {
        Some(product) if target(visit.incoming) == Target::Page => {
            Reply::redirect(&visit.localized(&format!("/products/{}?variant={id}", product.handle)))
        }
        _ => page(visit),
    }
}

fn product_json(visit: &Visit<'_>, name: &str) -> Reply {
    let (handle, wrapped) = match name.strip_suffix(".json") {
        Some(handle) => (handle, true),
        None => (name.trim_end_matches(".js"), false),
    };
    let site = visit.site();
    let Some(index) = site.store.product_index_for_url(handle) else {
        return Reply::json(404, &json!({"status": 404, "error": "Not Found"}));
    };
    // `.json` is not `.js` wrapped: it has a format of its own.
    Reply::json(
        200,
        &if wrapped {
            json!({ "product": storefront_json(&site, index) })
        } else {
            ajax_product_json(&site, index)
        },
    )
}

fn products_json(visit: &Visit<'_>, collection: Option<&str>) -> Reply {
    let site = visit.site();
    let indexes: Vec<usize> = match collection {
        Some(handle) => match site.store.collection_index(handle) {
            Some(index) => match CollectionDrop::value(&site, index).downcast::<CollectionDrop>() {
                Some(drop) => drop.product_indexes(),
                None => Vec::new(),
            },
            // A store answers an unknown collection with no products, not a `404`.
            None => Vec::new(),
        },
        None => (0..site.store.products.len()).collect(),
    };
    let limit = visit
        .incoming
        .query_param("limit")
        .and_then(|limit| limit.parse::<usize>().ok())
        .unwrap_or(30)
        .clamp(1, 250);
    let page = visit
        .incoming
        .query_param("page")
        .and_then(|page| page.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let products: Vec<Json> = indexes
        .into_iter()
        .skip((page - 1) * limit)
        .take(limit)
        .map(|index| ProductDrop::value(&site, index).to_json())
        .collect();
    Reply::json(200, &json!({ "products": products }))
}

/// The `featured_image` of a suggestion: always an object, whose keys are all `null` when
/// there is no image, as on Shopify.
/// https://shopify.dev/docs/api/ajax/reference/predictive-search
fn featured_image(image: &lsf_liquid::Value) -> Json {
    if image.is_nil() {
        return json!({
            "alt": null,
            "aspect_ratio": null,
            "height": null,
            "url": null,
            "width": null,
        });
    }
    json!({
        "alt": image.get("alt").to_json(),
        "aspect_ratio": image.get("aspect_ratio").to_json(),
        "height": image.get("height").to_json(),
        "url": image.to_json(),
        "width": image.get("width").to_json(),
    })
}

/// A search result as `/search/suggest.json` describes resources.
fn suggestion(value: &lsf_liquid::Value) -> Json {
    let text = |key: &str| value.get(key).to_json();
    let mut entry = json!({
        "id": text("id"),
        "title": text("title"),
        "handle": text("handle"),
        "url": text("url"),
    });
    match value.get("object_type").to_str().as_ref() {
        "product" => {
            let price = |key: &str| {
                let cents = value.get(key).as_i64().unwrap_or(0);
                format!("{}.{:02}", cents / 100, cents % 100)
            };
            entry["available"] = text("available");
            entry["body"] = text("description");
            entry["price"] = json!(price("price"));
            entry["price_min"] = json!(price("price_min"));
            entry["price_max"] = json!(price("price_max"));
            entry["compare_at_price_min"] = json!(price("compare_at_price_min"));
            entry["compare_at_price_max"] = json!(price("compare_at_price_max"));
            entry["type"] = text("type");
            entry["vendor"] = text("vendor");
            entry["tags"] = text("tags");
            entry["image"] = value.get("featured_image").to_json();
            // Shopify lists a variant only when the query matches terms of its title alone,
            // which this search does not tell apart.
            entry["variants"] = json!([]);
            entry["featured_image"] = featured_image(&value.get("featured_image"));
        }
        "article" => {
            entry["body"] = text("content");
            entry["author"] = text("author");
            entry["image"] = value.get("image").to_json();
        }
        "page" => {
            entry["body"] = text("content");
            entry["author"] = text("author");
        }
        _ => {
            entry["body"] = text("description");
            // The collection's own image: not the first product's, which Liquid falls back to.
            entry["featured_image"] = featured_image(&value.get("image"));
            if let Some(collection) = value.downcast::<CollectionDrop>() {
                entry["published_at"] = json!(
                    collection
                        .collection()
                        .published_at
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                );
            }
        }
    }
    entry
}

/// Why Shopify refuses the parameters of `/search/suggest.json`, in the `description` of its
/// `422`: a missing or blank `q`, or a value a parameter of `resources` does not take. An
/// out-of-range `limit` is not an error: it is brought between 1 and 10.
/// https://shopify.dev/docs/api/ajax/reference/predictive-search
fn suggest_parameter_error(incoming: &Incoming) -> Option<String> {
    if incoming
        .query_param("q")
        .is_none_or(|q| q.trim().is_empty())
    {
        return Some("param is missing or the value is empty: q".to_string());
    }
    // An optional parameter left empty is its default.
    let param = |name: &str| incoming.query_param(name).filter(|value| !value.is_empty());
    let listed = |value: &str, allowed: &[&str]| {
        value
            .split(',')
            .find(|item| !allowed.contains(&item.trim()))
            .map(|item| item.trim().to_string())
    };
    if let Some(unknown) = param("resources[type]").and_then(|types| {
        listed(
            types,
            &["product", "page", "article", "collection", "query"],
        )
    }) {
        return Some(format!("Type `{unknown}` is not supported."));
    }
    if param("resources[limit]").is_some_and(|limit| limit.trim().parse::<i64>().is_err()) {
        return Some("expected String to be a integer: limit".to_string());
    }
    // `(parameter, its name in the description, the values it takes, whether it takes a
    // comma-separated list of them)`.
    let options = [
        (
            "resources[limit_scope]",
            "limit_scope",
            &["all", "each"][..],
            false,
        ),
        (
            "resources[options][unavailable_products]",
            "unavailable_products",
            &["show", "hide", "last"][..],
            false,
        ),
        (
            "resources[options][fields]",
            "fields",
            &[
                "author",
                "body",
                "product_type",
                "tag",
                "title",
                "variants.barcode",
                "variants.sku",
                "variants.title",
                "vendor",
            ][..],
            true,
        ),
    ];
    options
        .into_iter()
        .find_map(|(name, label, allowed, list)| {
            let value = param(name)?;
            let invalid = if list {
                listed(value, allowed).is_some()
            } else {
                !allowed.contains(&value)
            };
            invalid.then(|| format!("Invalid option for `{label}` parameter"))
        })
}

fn suggest_json(visit: &Visit<'_>) -> Reply {
    if let Some(description) = suggest_parameter_error(visit.incoming) {
        return Reply::json(
            422,
            &json!({
                "status": 422,
                "message": "Invalid parameter error",
                "description": description,
            }),
        );
    }
    let site = visit.site();
    let search = predictive_search_value(&site);
    let resources = search.get("resources");
    let list = |key: &str| -> Vec<Json> {
        resources
            .get(key)
            .items()
            .unwrap_or_default()
            .iter()
            .map(suggestion)
            .collect()
    };
    Reply::json(
        200,
        &json!({
            "resources": {
                "results": {
                    "queries": [],
                    "collections": list("collections"),
                    "pages": list("pages"),
                    "articles": list("articles"),
                    "products": list("products"),
                }
            }
        }),
    )
}

/// What Shopify answers instead of recommendations: `422` for a `product_id` that is missing or
/// not a number and for an `intent` it does not take, `404` for an id that is not a product of
/// the online store (none, a draft, one not published there).
/// https://shopify.dev/docs/api/ajax/reference/product-recommendations
fn recommendations_error(visit: &Visit<'_>) -> Option<Reply> {
    let error = |status: u16, message: &str, description: String| {
        Some(Reply::json(
            status,
            &json!({"status": status, "message": message, "description": description}),
        ))
    };
    let invalid = |description: String| error(422, "Invalid parameter error", description);
    let Some(id) = visit
        .incoming
        .query_param("product_id")
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return invalid("A product_id value is missing".to_string());
    };
    let Ok(number) = id.parse::<i64>() else {
        return invalid(format!("product_id must be a number but received: {id}"));
    };
    if let Some(intent) = visit.incoming.query_param("intent")
        && !intent.is_empty()
        && !["related", "complementary"].contains(&intent)
    {
        return invalid("The intent parameter must be one of related, complementary".to_string());
    }
    let listed = u64::try_from(number)
        .ok()
        .and_then(|id| visit.store.product_index_by_id(id));
    match listed {
        Some(_) => None,
        None => error(
            404,
            "Product not found",
            format!("No product with id {number} is published in the online store"),
        ),
    }
}

fn recommendations_json(visit: &Visit<'_>) -> Reply {
    if let Some(reply) = recommendations_error(visit) {
        return reply;
    }
    let site = visit.site();
    let recommendations = recommendations_value(&site);
    let products: Vec<Json> = recommendations
        .get("products")
        .items()
        .unwrap_or_default()
        .iter()
        .map(|product| product.to_json())
        .collect();
    Reply::json(
        200,
        &json!({ "intent": recommendations.get("intent").to_json(), "products": products }),
    )
}
