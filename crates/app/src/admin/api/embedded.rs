//! The admin an embedded app is previewed in: a page of lsf's origin that draws the admin around
//! the app (the shop's name, the app's navigation, its title bar, its save bar, toasts, the
//! resource picker) and opens the app in a frame at a new launch URL each time it loads. The
//! app's pages load lsf's App Bridge (`/__lsf/admin/app-bridge.js`) instead of Shopify's, which
//! talks to this page with `postMessage`.
//!
//! | Request | What answers |
//! |---|---|
//! | `GET /admin/apps/<app>/<path>?shop=&user=&locale=` | The admin with the app open at `<path>`: `<app>` is the app's handle or client id. The other parameters go to the app. |
//! | `GET /admin`, `GET /admin/apps` | A redirect to the app's page. |
//! | `GET /admin/charges/<app>/pricing_plans[/<plan>]` | The plan selection page of managed pricing, and the page that approves a plan (`api::pricing`). |
//! | `GET /admin/<other>` | A page of the admin lsf does not have (`/admin/products` of a `shopify:admin/products` link): it says so, `404`. |
//! | `GET /__lsf/admin/app-bridge.js` | lsf's App Bridge. |
//! | `GET /__lsf/admin/assets/<file>` | The files of the page. |
//! | `GET /__lsf/admin/resources?shop=&type=&query=&first=&after=` | What the resource picker lists: products (with their variants) or collections of the shop, in the shape `shopify.resourcePicker` answers. |
//!
//! See "The admin of an embedded app" in `crates/app/docs/admin/design.md`.

use serde_json::{Map, Value, json};

use super::{AdminApi, HttpRequest, HttpResponse, escape_html};
use crate::admin::assets;
use crate::admin::auth::oauth::{self, DEFAULT_USER};
use crate::admin::graphql;
use crate::admin::resolve::{Registry, ShopResolver};

/// Where the page's files are served.
pub const ASSETS: &str = "/__lsf/admin/assets";

/// Where the app's pages load lsf's App Bridge from.
pub const APP_BRIDGE: &str = "/__lsf/admin/app-bridge.js";

/// The parameters of the page's URL that are the admin's, not the app's.
const ADMIN_PARAMS: &[&str] = &["shop", "user", "locale"];

/// The parameters the admin adds to the app's URL: dropped from a URL the app was at, the
/// launch adds new ones.
const LAUNCH_PARAMS: &[&str] = &[
    "embedded",
    "hmac",
    "host",
    "id_token",
    "locale",
    "session",
    "shop",
    "timestamp",
];

/// The answer to a request of the admin, `None` for another path.
pub(super) fn handle(api: &AdminApi, request: &HttpRequest) -> Option<HttpResponse> {
    let path = request.path.as_str();
    if path == APP_BRIDGE {
        return Some(asset(request, "app-bridge.js"));
    }
    if let Some(name) = path.strip_prefix(&format!("{ASSETS}/")) {
        return Some(asset(request, name));
    }
    if path.trim_end_matches('/') == "/__lsf/admin/resources" {
        return Some(resources(api, request));
    }
    let rest = path.strip_prefix("/admin")?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    // The Admin API and OAuth answer their own paths before this.
    if rest.starts_with("/api/") || rest.starts_with("/oauth/") {
        return None;
    }
    let method = request.method.to_ascii_uppercase();
    if method != "GET" && method != "HEAD" {
        return Some(HttpResponse::not_allowed(&method));
    }
    Some(page(api, request, rest).header("cache-control", "no-store"))
}

/// A file of the page, or lsf's App Bridge.
fn asset(request: &HttpRequest, name: &str) -> HttpResponse {
    let method = request.method.to_ascii_uppercase();
    if method == "OPTIONS" {
        return HttpResponse::preflight(request.header("access-control-request-headers"));
    }
    if method != "GET" && method != "HEAD" {
        return HttpResponse::not_allowed(&method);
    }
    let Some(content) = assets::get(name).filter(|_| name != "admin.html") else {
        return HttpResponse::error(404, format!("lsf has no file {name} for the admin"));
    };
    HttpResponse {
        status: 200,
        headers: vec![
            (
                "content-type".to_string(),
                assets::content_type(name).to_string(),
            ),
            // Loaded by the app's pages, from their origin; a new lsf serves a new one.
            ("access-control-allow-origin".to_string(), "*".to_string()),
            ("cache-control".to_string(), "no-cache".to_string()),
        ],
        body: content.to_vec(),
    }
}

/// Who the page is for: the shop, the user and the locale of the request, or why there is none.
pub(super) struct Viewer {
    pub(super) shop: String,
    user: u64,
    locale: String,
}

fn viewer(api: &AdminApi, request: &HttpRequest) -> Result<Viewer, HttpResponse> {
    let shop = oauth::explicit_shop(request).unwrap_or_else(|| api.default_shop());
    if !api.has_shop(&shop) {
        return Err(notice_page(
            api,
            None,
            404,
            "There is no shop here",
            &[format!(
                "lsf has no shop {shop}. Create it with POST /__lsf/admin/shops, or open the admin of {}.",
                api.default_shop()
            )],
        ));
    }
    let user = match request.query_param("user").map(str::parse::<u64>) {
        None => DEFAULT_USER,
        Some(Ok(user)) if api.admin_user(&shop, user).is_some() => user,
        Some(_) => {
            return Err(notice_page(
                api,
                None,
                404,
                "There is no such user",
                &[format!(
                    "{shop} has no user {}: user is the id of a staff member, 1 being the account owner.",
                    request.query_param("user").unwrap_or_default()
                )],
            ));
        }
    };
    let locale = request
        .query_param("locale")
        .filter(|locale| !locale.is_empty())
        .map_or_else(|| api.admin_locale(&shop), str::to_string);
    Ok(Viewer { shop, user, locale })
}

/// The handle the admin's URLs name the app by: its handle, else its client id.
pub(super) fn app_key(api: &AdminApi) -> String {
    api.app()
        .and_then(|app| app.admin_handle())
        .unwrap_or_else(|| api.client_id().to_string())
}

/// The parameters of the admin's URL that it keeps from page to page: the shop, the user and the
/// locale the request named.
pub(super) fn admin_query(request: &HttpRequest) -> Vec<(String, String)> {
    request
        .query
        .iter()
        .filter(|(name, _)| ADMIN_PARAMS.contains(&name.as_str()))
        .cloned()
        .collect()
}

pub(super) fn with_query(path: &str, params: &[(String, String)]) -> String {
    if params.is_empty() {
        return path.to_string();
    }
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (name, value) in params {
        serializer.append_pair(name, value);
    }
    format!("{path}?{}", serializer.finish())
}

/// `GET /admin…`: the app's page, a redirect to it, or a page lsf does not have.
fn page(api: &AdminApi, request: &HttpRequest, rest: &str) -> HttpResponse {
    let viewer = match viewer(api, request) {
        Ok(viewer) => viewer,
        Err(page) => return page,
    };
    let segments: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    match segments.as_slice() {
        [] | ["apps"] => HttpResponse::redirect(&with_query(
            &format!("/admin/apps/{}", app_key(api)),
            &admin_query(request),
        )),
        ["apps", app, path @ ..] => app_page(api, request, &viewer, app, path),
        ["charges", app, "pricing_plans", rest @ ..] => {
            super::pricing::page(api, request, &viewer, app, rest)
        }
        _ => notice_page(
            api,
            Some(&viewer),
            404,
            "lsf has no page here",
            &[
                format!(
                    "{} is a page of the admin itself, which lsf does not simulate: lsf draws the admin around an embedded app (its frame, its title bar, its navigation) and serves its App Bridge.",
                    request.path.trim_end_matches('/')
                ),
                "What the shop has is in the Admin API (POST /admin/api/<version>/graphql.json) and in the storefront.".to_string(),
            ],
        ),
    }
}

/// `GET /admin/apps/<app>/<path>`: the admin with the app open at `/<path>`.
fn app_page(
    api: &AdminApi,
    request: &HttpRequest,
    viewer: &Viewer,
    app: &str,
    path: &[&str],
) -> HttpResponse {
    let key = app_key(api);
    let Some(config) = api.app() else {
        return notice_page(
            api,
            Some(viewer),
            404,
            "lsf runs no app",
            &[
                "Start lsf with the app's directory (--app <directory>) and where its backend listens (--app-url http://localhost:3000): the admin opens it there.".to_string(),
            ],
        );
    };
    if app != key && app != api.client_id() {
        return notice_page(
            api,
            Some(viewer),
            404,
            "lsf has no such app",
            &[format!(
                "lsf runs one app, {}: its page is /admin/apps/{key}. /admin/apps/{app} names no app lsf has.",
                app_name(api)
            )],
        );
    }
    let mut app_path = format!("/{}", path.join("/"));
    if request.path.ends_with('/') && app_path != "/" {
        app_path.push('/');
    }
    let own: Vec<(String, String)> = request
        .query
        .iter()
        .filter(|(name, _)| {
            !ADMIN_PARAMS.contains(&name.as_str()) && !LAUNCH_PARAMS.contains(&name.as_str())
        })
        .cloned()
        .collect();
    let app_path = with_query(&app_path, &own);
    if config.config.embedded == Some(false) {
        let launch = with_query(
            "/__lsf/admin/launch",
            &[
                ("shop".to_string(), viewer.shop.clone()),
                ("path".to_string(), app_path.clone()),
            ],
        );
        return notice_page(
            api,
            Some(viewer),
            200,
            "This app is not embedded",
            &[format!(
                "shopify.app.toml says embedded = false: the admin opens {} in a tab of its own, not in a frame. Open it at {launch}.",
                app_name(api)
            )],
        );
    }
    let Some(frame) = api.launch_url(&viewer.shop, &app_path, viewer.user, &viewer.locale) else {
        return notice_page(
            api,
            Some(viewer),
            503,
            "The app has no URL",
            &[
                "Give lsf the URL its backend listens on (--app-url http://localhost:3000), or set application_url in shopify.app.toml: the admin opens the app there.".to_string(),
            ],
        );
    };
    let origin = origin_of(&frame);
    let main = format!(
        "<iframe class=\"lsf-frame\" name=\"app-iframe\" title=\"{}\" src=\"{}\" allow=\"clipboard-read; clipboard-write\"></iframe>",
        escape_html(&app_name(api)),
        escape_html(&frame)
    );
    let base = format!("/admin/apps/{key}");
    let mut page_config = admin_config(api, viewer, request);
    page_config.insert(
        "frame".into(),
        json!({
            "url": frame,
            "origin": origin,
            "path": app_path,
            // Where the app's paths start: the admin's URL has the app's path after it.
            "base": api.app_url(),
        }),
    );
    render(
        api,
        viewer,
        &app_name(api),
        &main,
        &Value::Object(page_config),
        &base,
        200,
    )
}

/// The name of the app: `name` of `shopify.app.toml`, else its handle or client id.
pub(super) fn app_name(api: &AdminApi) -> String {
    api.app()
        .and_then(|app| app.name.clone())
        .unwrap_or_else(|| app_key(api))
}

/// The origin of a URL: `http://localhost:3000`.
fn origin_of(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return String::new();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    format!("{scheme}://{}", &rest[..end])
}

/// What `admin.js` knows of the shop, the user and the app, as JSON.
pub(super) fn admin_config(
    api: &AdminApi,
    viewer: &Viewer,
    request: &HttpRequest,
) -> Map<String, Value> {
    let shop_name = api
        .with_shop(&viewer.shop, |state| {
            state
                .shop()
                .and_then(|shop| shop.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .flatten()
        .unwrap_or_else(|| viewer.shop.clone());
    let user = api.admin_user(&viewer.shop, viewer.user);
    let app = api.app();
    let required = app.map(|app| app.config.scopes.clone()).unwrap_or_default();
    let optional = app
        .map(|app| app.config.optional_scopes.clone())
        .unwrap_or_default();
    let granted = api.installed_scopes(&viewer.shop).unwrap_or_default();
    let mut config = Map::new();
    config.insert("shop".into(), json!(viewer.shop));
    config.insert("shopName".into(), json!(shop_name));
    config.insert("locale".into(), json!(viewer.locale));
    config.insert("query".into(), json!(admin_params(request)));
    config.insert(
        "user".into(),
        match user {
            Some(user) => json!({
                "id": user.id,
                "name": format!("{} {}", user.first_name, user.last_name).trim().to_string(),
                "firstName": user.first_name,
                "lastName": user.last_name,
                "email": user.email,
                "accountOwner": user.account_owner,
                "accountAccess": if user.account_owner { "Account owner" } else { "Full access" },
                "locale": user.locale,
            }),
            None => Value::Null,
        },
    );
    config.insert(
        "app".into(),
        json!({
            "name": app_name(api),
            "handle": app_key(api),
            "clientId": api.client_id(),
            "base": format!("/admin/apps/{}", app_key(api)),
            "scopes": {"granted": granted, "required": required, "optional": optional},
        }),
    );
    config.insert(
        "warnings".into(),
        json!(
            app.map(|app| app.admin_warnings.clone())
                .unwrap_or_default()
        ),
    );
    config.insert(
        "endpoints".into(),
        json!({
            "sessionToken": "/__lsf/admin/session-token",
            "resources": "/__lsf/admin/resources",
            "appBridge": APP_BRIDGE,
        }),
    );
    config
}

/// The admin's own parameters of the request, as an object.
fn admin_params(request: &HttpRequest) -> Map<String, Value> {
    admin_query(request)
        .into_iter()
        .map(|(name, value)| (name, Value::String(value)))
        .collect()
}

/// A page of the admin that says something instead of showing the app.
pub(super) fn notice_page(
    api: &AdminApi,
    viewer: Option<&Viewer>,
    status: u16,
    title: &str,
    paragraphs: &[String],
) -> HttpResponse {
    let default;
    let viewer = match viewer {
        Some(viewer) => viewer,
        None => {
            default = Viewer {
                shop: api.default_shop(),
                user: DEFAULT_USER,
                locale: api.admin_locale(&api.default_shop()),
            };
            &default
        }
    };
    let mut main = format!(
        "<section class=\"lsf-notice\"><h1>{}</h1>",
        escape_html(title)
    );
    for paragraph in paragraphs {
        main.push_str(&format!("<p>{}</p>", escape_html(paragraph)));
    }
    let home = with_query(
        &format!("/admin/apps/{}", app_key(api)),
        &[("shop".to_string(), viewer.shop.clone())]
            .into_iter()
            .filter(|(_, shop)| *shop != api.default_shop())
            .collect::<Vec<_>>(),
    );
    if api.app().is_some() {
        main.push_str(&format!(
            "<p><a class=\"lsf-link\" href=\"{}\">Back to {}</a></p>",
            escape_html(&home),
            escape_html(&app_name(api))
        ));
    }
    main.push_str("</section>");
    let config = admin_config(api, viewer, &HttpRequest::default());
    let base = format!("/admin/apps/{}", app_key(api));
    render(
        api,
        viewer,
        title,
        &main,
        &Value::Object(config),
        &base,
        status,
    )
}

/// The page around `main`: the admin's top bar, its navigation, the title bar.
pub(super) fn render(
    api: &AdminApi,
    viewer: &Viewer,
    title: &str,
    main: &str,
    config: &Value,
    base: &str,
    status: u16,
) -> HttpResponse {
    let template = String::from_utf8_lossy(assets::get("admin.html").unwrap_or_default());
    let shop_name = config["shopName"]
        .as_str()
        .unwrap_or(&viewer.shop)
        .to_string();
    let user = config["user"]["name"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let keep = |path: &str| {
        with_query(
            path,
            &[("shop".to_string(), viewer.shop.clone())]
                .into_iter()
                .filter(|(_, shop)| *shop != api.default_shop())
                .collect::<Vec<_>>(),
        )
    };
    // The storefront and the checkout are the default shop's: another shop's admin has none.
    let previews: String = if viewer.shop == api.default_shop() && !api.previews().is_empty() {
        let links: String = api
            .previews()
            .iter()
            .map(|preview| {
                format!(
                    "<li><a class=\"lsf-nav-preview\" href=\"{}\" target=\"_blank\" rel=\"noopener\" title=\"Opens in a new tab\">{}<span class=\"lsf-nav-external\" aria-hidden=\"true\">↗</span></a></li>",
                    escape_html(&preview.path),
                    escape_html(&preview.label)
                )
            })
            .collect();
        format!(
            "<div class=\"lsf-nav-heading\">Preview</div><ul class=\"lsf-nav-previews\">{links}</ul>"
        )
    } else {
        String::new()
    };
    let app_name = app_name(api);
    let initial = app_name
        .chars()
        .next()
        .map(|first| first.to_uppercase().to_string())
        .unwrap_or_default();
    // In a `<script type="application/json">`: nothing in it may close the element.
    let config_json = serde_json::to_string(config)
        .unwrap_or_default()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    let page_title = if title == app_name {
        format!("{app_name} · {shop_name}")
    } else {
        format!("{title} · {shop_name}")
    };
    let html = template
        .replace("{{ASSETS}}", ASSETS)
        .replace("{{PAGE_TITLE}}", &escape_html(&page_title))
        .replace("{{CONFIG}}", &config_json)
        .replace("{{SHOP_DOMAIN}}", &escape_html(&viewer.shop))
        .replace("{{SHOP_NAME}}", &escape_html(&shop_name))
        .replace("{{USER}}", &escape_html(&user))
        .replace("{{PREVIEWS}}", &previews)
        .replace("{{APP_HOME}}", &escape_html(&keep(base)))
        .replace("{{APP_INITIAL}}", &escape_html(&initial))
        .replace("{{APP_NAME}}", &escape_html(&app_name))
        .replace("{{TITLE}}", &escape_html(title))
        .replace("{{MAIN}}", main);
    HttpResponse::html(status, html)
}

// ---- the resource picker ------------------------------------------------------------------

/// What the picker lists of a type.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Product,
    Variant,
    Collection,
}

const PRODUCTS: &str = r#"query LsfResourcePicker($first: Int!, $after: String, $query: String) {
  products(first: $first, after: $after, query: $query, sortKey: TITLE) {
    pageInfo { hasNextPage endCursor }
    nodes {
      id title handle descriptionHtml productType vendor tags status templateSuffix
      createdAt updatedAt publishedAt totalInventory tracksInventory hasOnlyDefaultVariant
      availablePublicationsCount { count }
      variantsCount { count }
      options { id name position values }
      media(first: 25) { nodes { ... on MediaImage { id alt image { url altText } } } }
      variants(first: 250) {
        nodes {
          id title displayName sku barcode price compareAtPrice position availableForSale
          taxable inventoryPolicy inventoryQuantity createdAt updatedAt
          selectedOptions { name value }
          image { id url altText }
          inventoryItem { id tracked requiresShipping measurement { weight { value unit } } }
        }
      }
    }
  }
}"#;

const COLLECTIONS: &str = r#"query LsfResourcePicker($first: Int!, $after: String, $query: String) {
  collections(first: $first, after: $after, query: $query, sortKey: TITLE) {
    pageInfo { hasNextPage endCursor }
    nodes {
      id title handle description descriptionHtml templateSuffix updatedAt sortOrder
      image { id url altText }
      productsCount { count }
      ruleSet { appliedDisjunctively rules { column condition relation } }
      seo { title description }
      availablePublicationsCount { count }
      resourcePublicationsCount { count }
    }
  }
}"#;

/// `GET /__lsf/admin/resources?shop=&type=product|variant|collection&query=&first=&after=`: a
/// page of what the resource picker lists, `{"resources", "pageInfo"}`. Products come with their
/// variants for both `product` and `variant`: the picker shows the variants under their product.
/// `query` is in Shopify's search syntax, as the Admin API's `query` argument takes it.
fn resources(api: &AdminApi, request: &HttpRequest) -> HttpResponse {
    let method = request.method.to_ascii_uppercase();
    if method != "GET" {
        return HttpResponse::not_allowed(&method);
    }
    let shop = oauth::explicit_shop(request).unwrap_or_else(|| api.default_shop());
    if !api.has_shop(&shop) {
        return HttpResponse::error(404, format!("lsf has no shop {shop}"));
    }
    let kind = match request.query_param("type").unwrap_or("product") {
        "product" => Kind::Product,
        "variant" => Kind::Variant,
        "collection" => Kind::Collection,
        other => {
            return HttpResponse::error(
                400,
                format!("type is product, variant or collection, not {other}"),
            );
        }
    };
    let first = match request.query_param("first").map(str::parse::<u32>) {
        None => 50,
        Some(Ok(first)) if (1..=250).contains(&first) => first,
        Some(_) => return HttpResponse::error(400, "first is a number from 1 to 250"),
    };
    let mut variables = json!({ "first": first });
    if let Some(after) = request
        .query_param("after")
        .filter(|after| !after.is_empty())
    {
        variables["after"] = json!(after);
    }
    if let Some(query) = request
        .query_param("query")
        .filter(|query| !query.trim().is_empty())
    {
        variables["query"] = json!(query);
    }
    let document = if kind == Kind::Collection {
        COLLECTIONS
    } else {
        PRODUCTS
    };
    let version = api.versions().primary();
    // The admin's own query: every scope, and no cost limit, which is the app's.
    let Some(variables) = variables.as_object() else {
        return HttpResponse::error(500, "the variables are not an object");
    };
    let answer = api.change_shop(&shop, |state| {
        let mut resolver = ShopResolver::new(state, Registry::standard()).with_version(version);
        graphql::prepare(version.schema(), document, None, variables)
            .map(|prepared| prepared.execute(&mut resolver))
            .map_err(|errors| {
                errors
                    .into_iter()
                    .map(|error| error.message)
                    .collect::<Vec<_>>()
            })
            .and_then(|executed| {
                if executed.errors.is_empty() {
                    Ok(executed.data)
                } else {
                    Err(executed
                        .errors
                        .into_iter()
                        .map(|error| error.message)
                        .collect())
                }
            })
    });
    let data = match answer {
        None => return HttpResponse::error(404, format!("lsf has no shop {shop}")),
        Some(Err(messages)) => return HttpResponse::error(400, messages.join("; ")),
        Some(Ok(data)) => data,
    };
    let connection = if kind == Kind::Collection {
        &data["collections"]
    } else {
        &data["products"]
    };
    let nodes = connection["nodes"].as_array().cloned().unwrap_or_default();
    let resources: Vec<Value> = nodes
        .iter()
        .map(|node| match kind {
            Kind::Collection => collection_resource(node),
            Kind::Product | Kind::Variant => product_resource(node),
        })
        .collect();
    HttpResponse::json(
        200,
        &json!({
            "resources": resources,
            "pageInfo": connection["pageInfo"],
        }),
    )
    .header("cache-control", "no-store")
}

/// The `count` of a `Count`, `0` without one.
fn count(value: &Value) -> Value {
    json!(value["count"].as_u64().unwrap_or(0))
}

/// An image as the resource picker answers it: `{id, altText, originalSrc}`.
fn image(id: &Value, url: &Value, alt: &Value) -> Value {
    if url.is_null() {
        return Value::Null;
    }
    json!({ "id": id, "altText": alt, "originalSrc": url })
}

/// A product as `shopify.resourcePicker` answers it (`Product` of `@shopify/app-bridge-types`),
/// from the Admin API's: the names it has kept from the older Admin API (`images`,
/// `totalVariants`, `availablePublicationCount`).
fn product_resource(node: &Value) -> Value {
    let images: Vec<Value> = node["media"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|media| !media["image"].is_null())
        .map(|media| {
            image(
                &media["id"],
                &media["image"]["url"],
                &media["image"]["altText"],
            )
        })
        .collect();
    let product = json!({
        "id": node["id"],
        "title": node["title"],
        "handle": node["handle"],
        "status": node["status"],
        "vendor": node["vendor"],
        "productType": node["productType"],
        "images": images,
        "hasOnlyDefaultVariant": node["hasOnlyDefaultVariant"],
    });
    let variants: Vec<Value> = node["variants"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|variant| variant_resource(variant, &product))
        .collect();
    json!({
        "id": node["id"],
        "title": node["title"],
        "handle": node["handle"],
        "descriptionHtml": node["descriptionHtml"],
        "productType": node["productType"],
        "vendor": node["vendor"],
        "tags": node["tags"],
        "status": node["status"],
        "templateSuffix": node["templateSuffix"],
        "createdAt": node["createdAt"],
        "updatedAt": node["updatedAt"],
        "publishedAt": node["publishedAt"],
        "totalInventory": node["totalInventory"],
        "tracksInventory": node["tracksInventory"],
        "hasOnlyDefaultVariant": node["hasOnlyDefaultVariant"],
        "availablePublicationCount": count(&node["availablePublicationsCount"]),
        "totalVariants": count(&node["variantsCount"]),
        "options": node["options"],
        "images": images,
        "variants": variants,
    })
}

/// A variant as `shopify.resourcePicker` answers it (`ProductVariant`), with what the Admin API
/// now keeps on its inventory item (`requiresShipping`, the weight, whether it is tracked).
fn variant_resource(variant: &Value, product: &Value) -> Value {
    let item = &variant["inventoryItem"];
    let weight = &item["measurement"]["weight"];
    json!({
        "id": variant["id"],
        "title": variant["title"],
        "displayName": variant["displayName"],
        "sku": variant["sku"],
        "barcode": variant["barcode"],
        "price": variant["price"],
        "compareAtPrice": variant["compareAtPrice"],
        "position": variant["position"],
        "availableForSale": variant["availableForSale"],
        "taxable": variant["taxable"],
        "inventoryPolicy": variant["inventoryPolicy"],
        "inventoryQuantity": variant["inventoryQuantity"],
        "inventoryManagement": if item["tracked"].as_bool() == Some(true) { "SHOPIFY" } else { "NOT_MANAGED" },
        "inventoryItem": { "id": item["id"] },
        "requiresShipping": item["requiresShipping"],
        "weight": weight["value"],
        "weightUnit": weight["unit"],
        "selectedOptions": variant["selectedOptions"],
        "image": image(&variant["image"]["id"], &variant["image"]["url"], &variant["image"]["altText"]),
        "createdAt": variant["createdAt"],
        "updatedAt": variant["updatedAt"],
        "product": product,
    })
}

/// A collection as `shopify.resourcePicker` answers it (`Collection`).
fn collection_resource(node: &Value) -> Value {
    let products = count(&node["productsCount"]);
    let manual = node["sortOrder"] == "MANUAL";
    json!({
        "id": node["id"],
        "title": node["title"],
        "handle": node["handle"],
        "description": node["description"],
        "descriptionHtml": node["descriptionHtml"],
        "templateSuffix": node["templateSuffix"],
        "updatedAt": node["updatedAt"],
        "sortOrder": node["sortOrder"],
        "image": image(&node["image"]["id"], &node["image"]["url"], &node["image"]["altText"]),
        "productsCount": products,
        "productsManuallySortedCount": if manual { products.clone() } else { json!(0) },
        "productsAutomaticallySortedCount": if manual { json!(0) } else { products },
        "ruleSet": node["ruleSet"],
        "seo": node["seo"],
        "availablePublicationCount": count(&node["availablePublicationsCount"]),
        "publicationCount": count(&node["resourcePublicationsCount"]),
        "storefrontId": base64_gid(node["id"].as_str().unwrap_or_default()),
    })
}

/// The Storefront API's id of a resource: its GID in base64, as the older Storefront API gave it.
fn base64_gid(gid: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(gid)
}
