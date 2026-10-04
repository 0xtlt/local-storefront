//! What Shopify adds to every storefront page around the theme's own markup: the
//! `content_for_header` scripts (the `Shopify` JavaScript global, the feature loader), the
//! analytics objects before `</head>`, and the standard actions runtime before `</body>`.
//!
//! The markup follows what a Shopify storefront serves, with every URL pointing at the local
//! server. Scripts that only talk to Shopify's own services (analytics collection, Shop Pay,
//! bot protection) have no local counterpart and are left out.

use std::sync::OnceLock;

use chrono::Offset;
use serde_json::{Value as Json, json};

use super::page::{Page, Resource};
use crate::filters::misc_json;
use crate::site::Site;
use crate::theme::Theme;
use crate::urls;
use crate::util::short_hash;

const LOAD_FEATURES: &str = include_str!("../../assets/platform/load-features.js");
const STANDARD_ACTIONS: &str = include_str!("../../assets/platform/standard-actions.js");

/// `https://cdn.shopify.com/storefront/standard-actions.js` on Shopify: a fixed URL.
const STANDARD_ACTIONS_PATH: &str = "storefront/standard-actions.js";

/// Where the feature loader is served from, under `/cdn/`: Shopify's path, with a hash of the
/// content in the name as the real file has (`load_feature-1bd60354.js`). The URL changes
/// when the script does, which is what lets a browser keep it for a year.
pub fn load_features_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        format!(
            "shopifycloud/storefront/assets/storefront/load_feature-{}.js",
            &short_hash(LOAD_FEATURES)[..8]
        )
    })
}

/// A script of the platform, served from the local CDN.
pub struct PlatformAsset {
    pub content: &'static str,
    /// Whether the URL carries a hash of the content, and so never serves anything else.
    pub versioned: bool,
}

/// The script of a platform asset, by its path under `/cdn/`.
pub fn asset(path: &str) -> Option<PlatformAsset> {
    if path == load_features_path() {
        Some(PlatformAsset {
            content: LOAD_FEATURES,
            versioned: true,
        })
    } else if path == STANDARD_ACTIONS_PATH {
        Some(PlatformAsset {
            content: STANDARD_ACTIONS,
            versioned: false,
        })
    } else {
        None
    }
}

/// `theme_name` and `theme_version` of `config/settings_schema.json`.
pub fn theme_info(theme: &Theme) -> (String, String) {
    let info = theme
        .json("config/settings_schema.json")
        .ok()
        .flatten()
        .and_then(|schema| {
            schema
                .as_array()?
                .iter()
                .find(|group| group.get("name").and_then(Json::as_str) == Some("theme_info"))
                .cloned()
        });
    let field = |name: &str| {
        info.as_ref()
            .and_then(|info| info.get(name))
            .and_then(Json::as_str)
            .map(str::to_string)
    };
    (
        field("theme_name").unwrap_or_else(|| "Local theme".to_string()),
        field("theme_version").unwrap_or_else(|| "1.0.0".to_string()),
    )
}

/// The page type analytics and `__st` report: Shopify's own vocabulary, which differs from
/// the template names in two places.
fn page_kind(page: &Page) -> &str {
    match page.template.name.as_str() {
        "index" => "home",
        "search" => "searchresults",
        other => other,
    }
}

/// The resource a page is about, as `(type, id)`.
fn resource_of(site: &Site, page: &Page) -> Option<(&'static str, u64)> {
    let store = &site.store;
    Some(match &page.resource {
        Resource::Product { product, .. } => ("product", store.products[*product].id),
        Resource::Collection { collection, .. } => {
            ("collection", store.collections[*collection].id)
        }
        Resource::Page(index) => ("page", store.pages[*index].id),
        Resource::Blog { blog, .. } => ("blog", store.blogs[*blog].id),
        Resource::Article { blog, article } => {
            ("article", store.blogs[*blog].articles[*article].id)
        }
        _ => return None,
    })
}

/// A request id of the shape Shopify's have. It derives from the URL, so that the same page
/// renders the same HTML every time.
fn request_id(site: &Site) -> String {
    let hash = format!(
        "{}{}",
        short_hash(&site.request.path_with_query()),
        short_hash(&site.request.host)
    );
    format!(
        "{}-{}-{}-{}-{}",
        &hash[..8],
        &hash[8..12],
        &hash[12..16],
        &hash[16..20],
        &hash[20..32]
    )
}

fn script(attributes: &str, body: &str) -> String {
    if attributes.is_empty() {
        format!("<script>{body}</script>")
    } else {
        format!("<script {attributes}>{body}</script>")
    }
}

/// Which bundles the theme has: whether any `{% stylesheet %}` or `{% javascript %}` tag has
/// content, and the sections that have a `{% javascript %}` tag.
struct Bundles {
    stylesheet: bool,
    javascript: bool,
    scripted_sections: Vec<String>,
}

fn bundles(theme: &Theme) -> Bundles {
    // Scanning every file is too slow to do on each page: the answer is cached as text.
    let summary = theme.cached_text("platform_bundles", || {
        let mut stylesheet = false;
        let mut javascript = false;
        let mut sections = Vec::new();
        for directory in ["sections", "blocks", "snippets"] {
            for file in theme.files().list(directory).iter() {
                let Some(name) = file.strip_suffix(".liquid") else {
                    continue;
                };
                let Some(source) = theme.files().read(&format!("{directory}/{file}")) else {
                    continue;
                };
                let has = |tag: &str| {
                    crate::theme::schema::extract_blocks(&source, tag)
                        .iter()
                        .any(|body| !body.trim().is_empty())
                };
                stylesheet |= has("stylesheet");
                if has("javascript") {
                    javascript = true;
                    if directory == "sections" {
                        sections.push(name.to_string());
                    }
                }
            }
        }
        format!(
            "{}|{}|{}",
            u8::from(stylesheet),
            u8::from(javascript),
            sections.join(",")
        )
    });
    let mut parts = summary.splitn(3, '|');
    Bundles {
        stylesheet: parts.next() == Some("1"),
        javascript: parts.next() == Some("1"),
        scripted_sections: parts
            .next()
            .unwrap_or_default()
            .split(',')
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect(),
    }
}

/// The URL of the stylesheet built from the `{% stylesheet %}` tags, when the theme has some.
pub fn compiled_stylesheet_url(site: &Site) -> Option<String> {
    bundles(&site.theme).stylesheet.then(|| {
        format!(
            "{}/cdn/shop/t/{}/compiled_assets/styles.css?v={}",
            urls::cdn_origin(site),
            urls::THEME_ID,
            super::compiled_version(&site.theme)
        )
    })
}

/// `{{ content_for_header }}`.
pub fn content_for_header(site: &Site, page: &Page) -> String {
    let request = &site.request;
    let store = &site.store;
    let shop = &store.shop;
    let cdn = urls::cdn_origin(site);
    let (theme_name, theme_version) = theme_info(&site.theme);
    let mark = |name: &str| {
        format!(
            "window.performance && window.performance.mark && window.performance.mark('shopify.content_for_header.{name}');"
        )
    };
    let mut out: Vec<String> = Vec::new();

    out.push(script("", &mark("start")));
    out.push(format!(
        "<meta id=\"shopify-digital-wallet\" name=\"shopify-digital-wallet\" content=\"/{}/digital_wallets/dialog\">",
        shop.id
    ));

    // One alternate per published language, when there is more than one.
    if store.languages.len() > 1 {
        let href = |root: &str| {
            // The primary language is served at `/`, the others under `/<code>`.
            let path = match (root.trim_end_matches('/'), page.canonical_path.as_str()) {
                ("", path) => path.to_string(),
                (root, "/") => root.to_string(),
                (root, path) => format!("{root}{path}"),
            };
            format!("{}{path}", request.origin())
        };
        let primary = store.primary_language();
        out.push(format!(
            "<link rel=\"alternate\" hreflang=\"x-default\" href=\"{}\">",
            href(&primary.root_url)
        ));
        for language in &store.languages {
            out.push(format!(
                "<link rel=\"alternate\" hreflang=\"{}\" href=\"{}\">",
                language.iso_code,
                href(&language.root_url)
            ));
        }
    }

    out.push(script(
        "id=\"shopify-features\" type=\"application/json\"",
        &misc_json(&json!({
            // The Storefront API is not served locally: the token only has the right shape.
            "accessToken": &format!("{}{}", short_hash(&shop.permanent_domain), short_hash(&shop.name))[..32],
            "betas": [],
            "domain": request.host,
            "predictiveSearch": true,
            "shopId": shop.id,
            "locale": request.locale,
        })),
    ));

    let theme = json!({
        "name": theme_name,
        "id": urls::THEME_ID,
        "schema_name": theme_name,
        "schema_version": theme_version,
        "theme_store_id": null,
        "role": "main",
    });
    out.push(script(
        "",
        &format!(
            "var Shopify = Shopify || {{}};\n\
             Shopify.shop = {};\n\
             Shopify.locale = {};\n\
             Shopify.currency = {};\n\
             Shopify.country = {};\n\
             Shopify.theme = {};\n\
             Shopify.theme.handle = \"null\";\n\
             Shopify.theme.style = {{\"id\":null,\"handle\":null}};\n\
             Shopify.cdnHost = {};\n\
             Shopify.routes = Shopify.routes || {{}};\n\
             Shopify.routes.root = {};",
            misc_json(&json!(shop.permanent_domain)),
            misc_json(&json!(request.locale)),
            misc_json(&json!({"active": site.currency(), "rate": "1.0"})),
            misc_json(&json!(site.country().iso_code)),
            misc_json(&theme),
            misc_json(&json!(format!("{}/cdn", request.host))),
            misc_json(&json!(format!("{}/", request.root))),
        ),
    ));

    // `Shopify.modules` tells whether the browser runs module scripts.
    out.push(script(
        "type=\"module\"",
        "window.Shopify = window.Shopify || {}; window.Shopify.modules = true;",
    ));
    // Until the loader arrives, `Shopify.loadFeatures` queues what it is asked for.
    out.push(script(
        "",
        "(function () { var queue = function () { var calls = []; var push = function () { calls.push(Array.prototype.slice.call(arguments)); }; push.q = calls; return push; }; var shopify = window.Shopify = window.Shopify || {}; shopify.loadFeatures = queue(); shopify.autoloadFeatures = queue(); })();",
    ));

    let mut tracking = json!({
        "a": shop.id,
        "offset": site.now.with_timezone(&shop.timezone).offset().fix().local_minus_utc(),
        "reqid": request_id(site),
        "pageurl": format!("{}{}", request.host, request.path_with_query()),
        "u": &short_hash(&request.path_with_query())[..12],
        "p": page_kind(page),
    });
    if let Some((kind, id)) = resource_of(site, page) {
        tracking["rtyp"] = json!(kind);
        tracking["rid"] = json!(id);
    }
    if let Some(customer) = site.session.customer_id {
        tracking["cid"] = json!(customer);
    }
    out.push(script(
        "id=\"__st\"",
        &format!("var __st={};", misc_json(&tracking)),
    ));

    out.push(format!(
        "<script data-source-attribution=\"shopify.loadfeatures\" defer=\"defer\" src=\"{cdn}/cdn/{}\" crossorigin=\"anonymous\"></script>",
        load_features_path()
    ));
    // Dynamic checkout buttons are drawn by Shopify's own script: `init` has nothing to load.
    out.push(script(
        "data-source-attribution=\"shopify.dynamic_checkout.dynamic.init\"",
        "var Shopify=Shopify||{};Shopify.PaymentButton=Shopify.PaymentButton||{isStorefrontPortableWallets:true,init:function(){}};",
    ));

    let bundles = bundles(&site.theme);
    let compiled = format!("{cdn}/cdn/shop/t/{}/compiled_assets", urls::THEME_ID);
    let version = super::compiled_version(&site.theme);
    if let Some(stylesheet) = compiled_stylesheet_url(site) {
        out.push(format!(
            "<link rel=\"stylesheet\" media=\"screen\" href=\"{stylesheet}\">"
        ));
    }
    if bundles.javascript {
        out.push(format!(
            "<script id=\"sections-script\" data-sections=\"{}\" defer=\"defer\" src=\"{compiled}/scripts.js?v={version}\"></script>",
            bundles.scripted_sections.join(",")
        ));
    }

    out.push(script("id=\"shopify-cfh-end\"", &mark("end")));
    out.join("\n")
}

/// What Shopify adds right before `</head>`: the analytics objects themes and apps read.
pub fn head_end(site: &Site, page: &Page) -> String {
    let store = &site.store;
    let request = request_id(site);
    let mut meta = serde_json::Map::new();
    let mut described = json!({ "pageType": page_kind(page) });
    if let Some((kind, id)) = resource_of(site, page) {
        described["resourceType"] = json!(kind);
        described["resourceId"] = json!(id);
    }
    described["requestId"] = json!(request);
    if let Resource::Product { product, .. } = &page.resource {
        let product = &store.products[*product];
        meta.insert(
            "product".to_string(),
            json!({
                "id": product.id,
                "gid": format!("gid://shopify/Product/{}", product.id),
                "vendor": product.vendor,
                "type": product.product_type,
                "handle": product.handle,
                "variants": product.variants.iter().map(|variant| json!({
                    "id": variant.id,
                    "price": variant.price,
                    "name": if product.has_only_default_variant() {
                        product.title.clone()
                    } else {
                        format!("{} - {}", product.title, variant.title)
                    },
                    "public_title": if product.has_only_default_variant() {
                        Json::Null
                    } else {
                        json!(variant.title)
                    },
                    "sku": variant.sku,
                })).collect::<Vec<_>>(),
                "remote": false,
            }),
        );
    }
    meta.insert("page".to_string(), described);

    let analytics = format!(
        "window.ShopifyAnalytics = window.ShopifyAnalytics || {{}};\n\
         window.ShopifyAnalytics.meta = window.ShopifyAnalytics.meta || {{}};\n\
         window.ShopifyAnalytics.meta.currency = '{}';\n\
         var meta = {};\n\
         for (var attr in meta) {{\n  window.ShopifyAnalytics.meta[attr] = meta[attr];\n}}",
        site.currency(),
        misc_json(&Json::Object(meta)),
    );
    // Nothing is sent anywhere: the calls a theme or an app makes are kept in
    // `Shopify.analytics.replayQueue`, where Shopify also holds them until its pixels load.
    let collectors = "(function () {\n  \
         var noop = function () {};\n  \
         var trekkie = window.ShopifyAnalytics.lib = window.trekkie = window.trekkie || [];\n  \
         ['identify', 'page', 'ready', 'track', 'trackForm', 'trackLink'].forEach(function (name) {\n    \
           trekkie[name] = trekkie[name] || (name === 'ready' ? function (callback) { if (typeof callback === 'function') callback(); } : noop);\n  \
         });\n  \
         var shopify = window.Shopify = window.Shopify || {};\n  \
         shopify.analytics = shopify.analytics || {\n    \
           replayQueue: [],\n    \
           publish: function (name, payload, options) { this.replayQueue.push([name, payload, options]); return true; }\n  \
         };\n\
         })();";
    format!(
        "{}\n{}\n",
        script("", &analytics),
        script("class=\"analytics\"", collectors)
    )
}

/// What Shopify adds right before `</body>`: the standard storefront actions.
pub fn body_end(site: &Site) -> String {
    format!(
        "<script src=\"{}/cdn/{STANDARD_ACTIONS_PATH}\" type=\"module\" data-source-attribution=\"shopify.standard_actions\"></script>",
        urls::cdn_origin(site)
    )
}

/// Adds the markup of the platform to a rendered page that has a head and a body.
pub fn decorate(site: &Site, page: &Page, mut html: String) -> String {
    if let Some(index) = html.find("</head>") {
        html.insert_str(index, &head_end(site, page));
    }
    if let Some(index) = html.rfind("</body>") {
        html.insert_str(index, &body_end(site));
    }
    html
}
