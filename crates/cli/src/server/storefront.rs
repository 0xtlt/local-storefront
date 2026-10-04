//! Storefront routes: pages, the Section Rendering API and the JSON endpoints themes call.

use std::sync::Arc;

use lsf_core::drops::collection::CollectionDrop;
use lsf_core::drops::product::ProductDrop;
use lsf_core::drops::search::{predictive_search_value, recommendations_value};
use lsf_core::render::page::{Page, Resource};
use lsf_core::render::{Rendered, Target, seo};
use lsf_core::{Request, Session, Site, Store};
use serde_json::{Value as Json, json};

use super::reply::{Reply, cache};
use super::{
    CART_COOKIE, Incoming, SESSION_COOKIE, ServerState, account, cart, forms, live_reload,
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

    pub fn update_session(&self, change: impl FnOnce(&mut Session)) {
        self.state
            .with_session(&self.session_id, |entry| change(&mut entry.session));
    }

    pub fn site(&self) -> Arc<Site> {
        self.state
            .app
            .renderer
            .site(self.store.clone(), self.request.clone(), self.session())
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
        self.state
            .app
            .renderer
            .site(self.store.clone(), request, self.session())
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
        let page = lsf_core::render::routes::resolve(&site);
        let rendered =
            self.state
                .app
                .renderer
                .render_page(&site, page, &Target::Sections(ids.to_vec()));
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
    let reply = with_cache_policy(&visit, with_cart_cookie(&visit, route(&visit)));
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
    let cookie = format!("{CART_COOKIE}={}", session.cart_token);
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
        ["account", "login"] if is_post => forms::login(visit),
        ["account", "logout"] => forms::logout(visit),
        ["account"] if is_post => forms::register(visit),
        ["account", "recover" | "reset" | "activate", ..] if is_post => {
            forms::unsupported(visit, "recover_customer_password")
        }
        ["account", "addresses", ..] if is_post => forms::unsupported(visit, "customer_address"),
        ["company_location", "update"] => account::switch_location(visit),
        // Shopify's own login pages, which a theme links to with `routes.storefront_login_url`.
        ["customer_authentication" | "customer_identity", ..] => account::entry(visit),
        // Accounts hosted by Shopify have a single page here, whatever the theme links to.
        ["account"] if account::hosted(visit) => account::page(visit),
        ["account", ..] if account::hosted(visit) => account::entry(visit),
        ["account"] | ["account", "addresses"] | ["account", "orders", _]
            if visit.session().customer_id.is_none() =>
        {
            Reply::redirect(&visit.localized("/account/login"))
        }
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
}

fn reply_from(visit: &Visit<'_>, rendered: Rendered, target: &Target) -> Reply {
    log_problems(visit.state, &rendered);
    let mut body = rendered.body;
    if visit.state.options.live_reload && *target == Target::Page {
        match body.rfind("</body>") {
            Some(index) => body.insert_str(index, live_reload::SCRIPT),
            None => body.push_str(live_reload::SCRIPT),
        }
    }
    let mut reply = Reply::new(rendered.status, rendered.content_type, body.into_bytes())
        .header("x-lsf-template", rendered.template)
        .header("content-language", visit.request.locale.clone());
    if !rendered.errors.is_empty() {
        reply = reply.header("x-lsf-liquid-errors", rendered.errors.len().to_string());
    }
    for preload in rendered.preloads {
        reply = reply.header("link", preload);
    }
    reply
}

fn page(visit: &Visit<'_>) -> Reply {
    let target = target(visit.incoming);
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
    if target == Target::Page {
        return Reply::text(
            400,
            "this endpoint needs a section_id or sections parameter",
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
    let Some(index) = site.store.product_index(handle) else {
        return Reply::json(404, &json!({"status": 404, "error": "Not Found"}));
    };
    let product = ProductDrop::value(&site, index).to_json();
    Reply::json(
        200,
        &if wrapped {
            json!({ "product": product })
        } else {
            product
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
            None => return Reply::json(404, &json!({"status": 404, "error": "Not Found"})),
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
        }
    }
    entry
}

fn suggest_json(visit: &Visit<'_>) -> Reply {
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

fn recommendations_json(visit: &Visit<'_>) -> Reply {
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
