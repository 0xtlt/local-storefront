//! Tests of the server's handlers, run without a network: requests are dispatched directly.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use lsf_core::theme::Revalidate;
use serde_json::{Value as Json, json};

use super::reply::Reply;
use super::throttle::Throttle;
use super::{Incoming, SESSION_HEADER, ServeOptions, ServerState};
use crate::app::App;

fn server() -> ServerState {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
    let app = App::open(&theme, None, Revalidate::Never, &[]).expect("fixture theme");
    let (state, diagnostics) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    state
}

/// A request from the session named `session`.
fn request(method: &str, path_and_query: &str, body: Json, session: &str) -> Incoming {
    let (path, query) = path_and_query
        .split_once('?')
        .unwrap_or((path_and_query, ""));
    let mut headers = HashMap::new();
    headers.insert(SESSION_HEADER.to_string(), session.to_string());
    headers.insert("host".to_string(), "shop.test".to_string());
    if method != "GET" {
        headers.insert("accept".to_string(), "application/json".to_string());
    }
    Incoming {
        method: method.to_string(),
        path: path.to_string(),
        query: super::params::query_pairs(query),
        headers,
        params: body,
        host: "shop.test".to_string(),
    }
}

fn get(state: &ServerState, path: &str, session: &str) -> Reply {
    state.dispatch(&request("GET", path, json!({}), session))
}

/// A `GET` from a client that accepts `accept`.
fn get_accepting(state: &ServerState, path: &str, accept: &str) -> Reply {
    let mut incoming = request("GET", path, json!({}), "accept");
    incoming
        .headers
        .insert("accept".to_string(), accept.to_string());
    state.dispatch(&incoming)
}

fn post(state: &ServerState, path: &str, body: Json, session: &str) -> Reply {
    state.dispatch(&request("POST", path, body, session))
}

fn text(reply: &Reply) -> String {
    String::from_utf8_lossy(&reply.body).into_owned()
}

fn body_json(reply: &Reply) -> Json {
    serde_json::from_slice(&reply.body).unwrap_or_else(|_| panic!("not JSON: {}", text(reply)))
}

fn header<'a>(reply: &'a Reply, name: &str) -> Option<&'a str> {
    reply
        .headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// The `Set-Cookie` headers of a reply.
fn cookies(reply: &Reply) -> Vec<String> {
    reply
        .headers
        .iter()
        .filter(|(key, _)| key == "set-cookie")
        .map(|(_, value)| value.clone())
        .collect()
}

fn variant_id(state: &ServerState, sku: &str) -> u64 {
    let loaded = state.loaded();
    loaded
        .store
        .products
        .iter()
        .flat_map(|product| &product.variants)
        .find(|variant| variant.sku == sku)
        .unwrap_or_else(|| panic!("no variant {sku}"))
        .id
}

#[test]
fn a_page_that_is_not_found_says_why() {
    // Shopify answers a 404 page without a word about the cause; a test that reads a page can
    // tell a missing page from a working one with `x-lsf-not-found`.
    let state = server();
    let reason = |path: &str| {
        let reply = get(&state, path, "not-found");
        assert_eq!(reply.status, 404, "{path}");
        header(&reply, "x-lsf-not-found").map(str::to_string)
    };
    assert_eq!(reason("/nope").as_deref(), Some("no page at \"/nope\""));
    assert_eq!(
        reason("/products/nope").as_deref(),
        Some("no product with the handle \"nope\"")
    );
    assert_eq!(
        reason("/pages/event/nope").as_deref(),
        Some("no metaobject \"event/nope\" in the store data")
    );
    // A URL can hold what a header cannot.
    assert_eq!(
        reason("/caf\u{e9}").as_deref(),
        Some("no page at \"/caf?\"")
    );
    // A page that exists does not carry it.
    let home = get(&state, "/", "not-found");
    assert_eq!(home.status, 200);
    assert_eq!(header(&home, "x-lsf-not-found"), None);
}

#[test]
fn a_new_visitor_gets_a_session_cookie() {
    let state = server();
    let mut incoming = request("GET", "/", json!({}), "ignored");
    incoming.headers.remove(SESSION_HEADER);
    let reply = state.dispatch(&incoming);
    assert_eq!(reply.status, 200);
    assert!(header(&reply, "set-cookie").is_some_and(|cookie| cookie.starts_with("_lsf_session=")));
    assert_eq!(header(&reply, "x-lsf-template"), Some("index"));
    assert_eq!(header(&reply, "x-lsf-liquid-errors"), None);
}

#[test]
fn the_cart_api_adds_changes_and_clears() {
    let state = server();
    let tote = variant_id(&state, "TOTE-NAT");
    let mug = variant_id(&state, "MUG-OAT");

    let added = post(
        &state,
        "/cart/add.js",
        json!({"id": tote, "quantity": 2, "properties": {"Note": "gift", "Empty": ""}}),
        "cart",
    );
    assert_eq!(added.status, 200, "{}", text(&added));
    let line = body_json(&added);
    assert_eq!(line["title"], "Canvas Tote Bag");
    assert_eq!(line["quantity"], 2);
    assert_eq!(line["line_price"], 4800);
    assert_eq!(line["properties"], json!({"Note": "gift"}));

    // Several items at once come back as a list.
    let many = post(
        &state,
        "/cart/add.js",
        json!({"items": [{"id": mug, "quantity": 1}]}),
        "cart",
    );
    assert_eq!(body_json(&many)["items"][0]["sku"], "MUG-OAT");

    let cart = body_json(&get(&state, "/cart.js", "cart"));
    assert_eq!(cart["item_count"], 3);
    assert_eq!(cart["total_price"], 4800 + 2200);
    // The most recently added line comes first.
    assert_eq!(cart["items"][0]["sku"], "MUG-OAT");

    let changed = body_json(&post(
        &state,
        "/cart/change.js",
        json!({"line": 1, "quantity": 3}),
        "cart",
    ));
    assert_eq!(changed["items"][0]["quantity"], 3);
    let key = changed["items"][1]["key"].as_str().unwrap().to_string();
    let removed = body_json(&post(
        &state,
        "/cart/change.js",
        json!({"id": key, "quantity": 0}),
        "cart",
    ));
    assert_eq!(removed["items"].as_array().unwrap().len(), 1);

    let updated = body_json(&post(
        &state,
        "/cart/update.js",
        json!({"updates": {mug.to_string(): 1}, "note": "Ring the bell"}),
        "cart",
    ));
    assert_eq!(updated["item_count"], 1);
    assert_eq!(updated["note"], "Ring the bell");

    let cleared = body_json(&post(&state, "/cart/clear.js", json!({}), "cart"));
    assert_eq!(cleared["item_count"], 0);

    // Another session never saw any of it.
    assert_eq!(
        body_json(&get(&state, "/cart.js", "someone-else"))["item_count"],
        0
    );
}

#[test]
fn the_cart_api_reports_errors_like_shopify() {
    let state = server();
    let sold_out = post(
        &state,
        "/cart/add.js",
        json!({"id": variant_id(&state, "CNDL-FIG")}),
        "errors",
    );
    assert_eq!(sold_out.status, 422);
    assert_eq!(
        body_json(&sold_out)["description"],
        "The product 'Scented Candle' is already sold out."
    );

    let missing = post(&state, "/cart/add.js", json!({"id": 1}), "errors");
    assert_eq!(missing.status, 404);

    // Only one overshirt in size L is in stock.
    let large = variant_id(&state, "OVS-L");
    assert_eq!(
        post(&state, "/cart/add.js", json!({"id": large}), "errors").status,
        200
    );
    let too_many = post(&state, "/cart/add.js", json!({"id": large}), "errors");
    assert_eq!(too_many.status, 422);
    assert_eq!(
        body_json(&too_many)["description"],
        "All 1 Linen Overshirt - L are in your cart."
    );
}

#[test]
fn cart_responses_bundle_the_requested_sections() {
    let state = server();
    let reply = post(
        &state,
        "/cart/add.js",
        json!({"id": variant_id(&state, "TOTE-NAT"), "sections": "footer", "sections_url": "/"}),
        "sections",
    );
    let footer = body_json(&reply)["sections"]["footer"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(footer.contains("|1 item</footer>"), "{footer}");
}

#[test]
fn the_control_api_sets_up_a_session() {
    let state = server();
    let reply = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({
            "customer": "jane.doe@example.com",
            "cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}], "note": "From a test"},
            "data": {
                "shop": {"name": "Overlaid Shop"},
                "products": [{"title": "Only Here", "price": "5.00"}]
            }
        }),
        "controlled",
    ));
    assert_eq!(reply.status, 200, "{}", text(&reply));

    let cart = body_json(&get(&state, "/cart.js", "controlled"));
    assert_eq!(
        cart["items"][0]["title"],
        "Organic Cotton T-Shirt - Black / M"
    );
    assert_eq!(cart["note"], "From a test");

    // The extra data only exists for that session.
    assert_eq!(get(&state, "/products/only-here", "controlled").status, 200);
    assert_eq!(get(&state, "/products/only-here", "other").status, 404);
    assert!(text(&get(&state, "/", "controlled")).contains("<title>Overlaid Shop</title>"));

    let session = body_json(&get(&state, "/__lsf/session", "controlled"));
    assert_eq!(session["customer"], "jane.doe@example.com");
    assert_eq!(session["custom_data"], true);

    state.dispatch(&request(
        "DELETE",
        "/__lsf/session",
        json!({}),
        "controlled",
    ));
    assert_eq!(
        body_json(&get(&state, "/cart.js", "controlled"))["item_count"],
        0
    );
}

#[test]
fn the_control_api_explains_invalid_input() {
    let state = server();
    let reply = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"custmer": "x", "cart": {"items": [{"variant": "NOPE"}]}}),
        "invalid",
    ));
    assert_eq!(reply.status, 422);
    let report = body_json(&reply);
    assert_eq!(report["ok"], false);
    assert_eq!(report["diagnostics"][0]["code"], "unknown_field");
    assert_eq!(
        report["diagnostics"][0]["hint"],
        "did you mean \"customer\"?"
    );

    let unknown = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": [{"variant": "TEE-BLK-XXL"}]}}),
        "invalid",
    ));
    let diagnostic = &body_json(&unknown)["diagnostics"][0];
    assert_eq!(diagnostic["code"], "unknown_variant");
    assert_eq!(diagnostic["path"], "/cart/items/0/variant");
    assert_eq!(
        diagnostic["message"],
        "\"TEE-BLK-XXL\" is neither the SKU of a variant nor the handle of a product"
    );
    assert_eq!(diagnostic["hint"], "did you mean \"TEE-BLK-XL\"?");

    let bad_data = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"data": {"products": [{"title": "x", "price": 1.5}]}}),
        "invalid",
    ));
    assert_eq!(bad_data.status, 422);
    assert_eq!(
        body_json(&bad_data)["diagnostics"][0]["path"],
        "/products/0/price"
    );
}

#[test]
fn json_endpoints() {
    let state = server();
    let product = body_json(&get(&state, "/products/ceramic-mug.js", "json"));
    assert_eq!(product["handle"], "ceramic-mug");
    assert_eq!(product["variants"].as_array().unwrap().len(), 3);
    assert_eq!(get(&state, "/products/nope.js", "json").status, 404);

    let suggestions = body_json(&get(
        &state,
        "/search/suggest.json?q=mug&resources[type]=product",
        "json",
    ));
    assert_eq!(
        suggestions["resources"]["results"]["products"][0]["title"],
        "Ceramic Mug"
    );

    let mug_id = product["id"].as_u64().unwrap();
    let recommended = body_json(&get(
        &state,
        &format!("/recommendations/products.json?product_id={mug_id}&limit=2"),
        "json",
    ));
    assert_eq!(recommended["products"].as_array().unwrap().len(), 2);

    let listed = body_json(&get(&state, "/collections/apparel/products.json", "json"));
    assert_eq!(listed["products"].as_array().unwrap().len(), 2);
}

#[test]
fn a_submitted_form_shows_its_outcome_once() {
    let state = server();
    let mut submit = request(
        "POST",
        "/contact",
        json!({"form_type": "contact", "contact": {"email": "not-an-email"}}),
        "form",
    );
    submit.headers.insert(
        "referer".to_string(),
        "http://shop.test/pages/contact".to_string(),
    );
    submit.headers.remove("accept");
    let redirect = state.dispatch(&submit);
    assert_eq!(redirect.status, 302);
    assert_eq!(
        header(&redirect, "location"),
        Some("/pages/contact#contact_form")
    );

    let page = text(&get(&state, "/pages/contact", "form"));
    assert!(page.contains("<li>Email is invalid</li>"), "{page}");
    assert!(page.contains("value=\"not-an-email\""));
    // Shown once.
    assert!(!text(&get(&state, "/pages/contact", "form")).contains("is invalid"));

    let mut submit = request(
        "POST",
        "/contact",
        json!({"contact": {"email": "a@b.co"}}),
        "form",
    );
    submit.headers.insert(
        "referer".to_string(),
        "http://shop.test/pages/contact".to_string(),
    );
    let redirect = state.dispatch(&submit);
    assert_eq!(
        header(&redirect, "location"),
        Some("/pages/contact?contact_posted=true#contact_form")
    );
    assert!(text(&get(&state, "/pages/contact", "form")).contains("<p class=\"ok\">Thanks</p>"));
}

#[test]
fn customers_log_in_and_out() {
    let state = server();
    let wrong = post(
        &state,
        "/account/login",
        json!({"customer": {"email": "jane.doe@example.com", "password": "nope"}}),
        "login",
    );
    // The fixture theme has no login page: accounts are hosted, and `/account` is the way in.
    assert_eq!(header(&wrong, "location"), Some("/account"));
    assert_eq!(
        body_json(&get(&state, "/__lsf/session", "login"))["customer"],
        Json::Null
    );

    let right = post(
        &state,
        "/account/login",
        json!({"customer": {"email": "jane.doe@example.com", "password": "password"}}),
        "login",
    );
    assert_eq!(header(&right, "location"), Some("/account"));
    assert_eq!(
        body_json(&get(&state, "/__lsf/session", "login"))["customer"],
        "jane.doe@example.com"
    );

    get(&state, "/account/logout", "login");
    assert_eq!(
        body_json(&get(&state, "/__lsf/session", "login"))["customer"],
        Json::Null
    );
}

#[test]
fn locale_prefixes_select_the_language() {
    let state = server();
    let french = get(&state, "/fr/pages/about", "locale");
    assert_eq!(french.status, 200);
    assert_eq!(header(&french, "content-language"), Some("fr"));
    assert!(text(&french).contains("Bonjour Shop owner"));

    let switched = post(
        &state,
        "/localization",
        json!({"locale_code": "fr", "country_code": "CA", "return_to": "/pages/about"}),
        "locale",
    );
    assert_eq!(header(&switched, "location"), Some("/fr/pages/about"));
    assert_eq!(
        body_json(&get(&state, "/__lsf/session", "locale"))["country"],
        "CA"
    );
}

/// The path of the file `{{ name | shopify_asset_url }}` points at, as a theme renders it.
fn shopify_asset_path(state: &ServerState, name: &str) -> String {
    let loaded = state.loaded();
    let request = state.storefront_request(
        &loaded.store,
        &request("GET", "/", json!({}), "cdn"),
        "/",
        Vec::new(),
    );
    let url = state.app.renderer.render_asset(
        loaded.store.clone(),
        request,
        &format!("{{{{ '{name}' | shopify_asset_url }}}}"),
        "assets/url.txt",
    );
    url.trim_start_matches("//shop.test").to_string()
}

#[test]
fn the_cdn_serves_the_global_assets_of_shopify_asset_url() {
    // Shopify lists the files that `shopify_asset_url` reaches, and the gift card of Dawn loads
    // `vendor/qrcode.js` from it:
    // https://shopify.dev/docs/api/liquid/filters/shopify_asset_url
    let state = server();
    let qrcode = get(
        &state,
        &shopify_asset_path(&state, "vendor/qrcode.js"),
        "cdn",
    );
    assert_eq!(qrcode.status, 200);
    assert_eq!(
        header(&qrcode, "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert!(text(&qrcode).contains("QRCode"));
    // The MIT license of the library travels with the file.
    assert!(text(&qrcode).contains("Copyright (c) 2012 davidshimjs"));
    assert!(text(&qrcode).contains("Permission is hereby granted"));

    let documented = get(
        &state,
        &shopify_asset_path(&state, "option_selection.js"),
        "cdn",
    );
    assert_eq!(documented.status, 200);
    let stylesheet = get(&state, &shopify_asset_path(&state, "customer.css"), "cdn");
    assert_eq!(
        header(&stylesheet, "content-type"),
        Some("text/css; charset=utf-8")
    );

    let unknown = get(
        &state,
        &shopify_asset_path(&state, "vendor/unknown.js"),
        "cdn",
    );
    assert_eq!(unknown.status, 404);

    // The gift card's badge is drawn as a stand-in; an unknown image is no placeholder.
    let badge = get(
        &state,
        &shopify_asset_path(&state, "gift-card/add-to-apple-wallet.svg"),
        "cdn",
    );
    assert_eq!(badge.status, 200);
    assert_eq!(header(&badge, "content-type"), Some("image/svg+xml"));
    let unknown_image = get(&state, &shopify_asset_path(&state, "unknown.svg"), "cdn");
    assert_eq!(unknown_image.status, 404);
}

#[test]
fn the_cdn_serves_assets_bundles_and_placeholder_images() {
    let state = server();
    let css = get(&state, "/cdn/shop/t/1/assets/base.css", "cdn");
    assert_eq!(
        header(&css, "content-type"),
        Some("text/css; charset=utf-8")
    );
    assert_eq!(text(&css), "body { margin: 0 }\n");

    let bundle = get(&state, "/cdn/shop/t/1/compiled_assets/styles.css", "cdn");
    assert!(text(&bundle).contains(".hero { display: block }"));

    // The demo store's images do not exist on disk: a placeholder of the right size is drawn.
    let image = get(
        &state,
        "/cdn/shop/files/products/tee-white.jpg?width=200",
        "cdn",
    );
    assert_eq!(image.status, 200);
    assert_eq!(header(&image, "content-type"), Some("image/jpeg"));
    assert_eq!(header(&image, "x-lsf-placeholder"), Some("1"));
    // A JPEG stores its size in the SOF0 segment: 250 × 200 for a 1600 × 2000 source.
    let position = image
        .body
        .windows(2)
        .position(|bytes| bytes == [0xFF, 0xC0])
        .expect("SOF0 marker");
    let height = u16::from_be_bytes([image.body[position + 5], image.body[position + 6]]);
    let width = u16::from_be_bytes([image.body[position + 7], image.body[position + 8]]);
    assert_eq!((width, height), (200, 250));

    let legacy = get(
        &state,
        "/cdn/shop/files/products/tee-white_100x100.jpg",
        "cdn",
    );
    assert_eq!(legacy.status, 200);

    let font = get(&state, "/cdn/fonts/work_sans/work_sans_n4.woff2", "cdn");
    assert_eq!(header(&font, "content-type"), Some("font/ttf"));

    // Shopify's shared assets are stubbed, with a drawing where an image is expected.
    let shared = get(
        &state,
        "/cdn/shopifycloud/storefront/assets/gift-card/card.svg",
        "cdn",
    );
    assert_eq!(header(&shared, "content-type"), Some("image/svg+xml"));
    assert!(text(&shared).starts_with("<svg "));
    let script = get(
        &state,
        "/cdn/shopifycloud/portable-wallets/latest/x.js",
        "cdn",
    );
    assert_eq!(script.status, 200);
    assert_eq!(
        header(&script, "content-type"),
        Some("text/javascript; charset=utf-8")
    );

    assert_eq!(
        get(&state, "/cdn/shop/files/missing.pdf", "cdn").status,
        404
    );
    assert_eq!(
        get(
            &state,
            "/cdn/shop/t/1/assets/../config/settings_data.json",
            "cdn"
        )
        .status,
        404
    );
}

#[test]
fn the_status_endpoint_describes_the_server() {
    let state = server();
    let status = body_json(&get(&state, "/__lsf/status", "status"));
    assert_eq!(status["counts"]["products"], 8);
    assert_eq!(status["data_diagnostics"]["ok"], true);
    assert!(
        status["routes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|route| route == "/products/ceramic-mug")
    );
    assert!(get(&state, "/__lsf/schema/product", "status").status == 200);
}

#[test]
fn the_cart_cookie_appears_with_the_cart() {
    let state = server();
    // Browsing does not create a cart.
    let home = get(&state, "/", "cookie");
    assert!(
        !cookies(&home)
            .iter()
            .any(|cookie| cookie.starts_with("cart="))
    );

    let added = post(
        &state,
        "/cart/add.js",
        json!({"id": variant_id(&state, "TOTE-NAT")}),
        "cookie",
    );
    let cart = cookies(&added)
        .into_iter()
        .find(|cookie| cookie.starts_with("cart="))
        .expect("the cart cookie");
    assert!(cart.ends_with("; Path=/; SameSite=Lax"), "{cart}");
    // The cookie holds the token `/cart.js` reports, and scripts must be able to read it.
    let token = body_json(&get(&state, "/cart.js", "cookie"))["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(cart, format!("cart={token}; Path=/; SameSite=Lax"));
    assert!(!cart.contains("HttpOnly"));

    // A browser that already has it is not sent it again.
    let mut again = request("GET", "/cart.js", json!({}), "cookie");
    again
        .headers
        .insert("cookie".to_string(), format!("cart={token}"));
    assert!(cookies(&state.dispatch(&again)).is_empty());

    // Two visitors have two carts.
    post(
        &state,
        "/cart/add.js",
        json!({"id": variant_id(&state, "TOTE-NAT")}),
        "someone-else",
    );
    assert_ne!(
        body_json(&get(&state, "/cart.js", "someone-else"))["token"],
        token
    );
}

#[test]
fn cart_attributes_can_be_removed_and_the_cart_bundles_sections() {
    let state = server();
    post(
        &state,
        "/cart/update.js",
        json!({"attributes": {"gift": "yes", "wrap": "red"}}),
        "attributes",
    );
    let cart = body_json(&post(
        &state,
        "/cart/update.js",
        json!({"attributes": {"gift": null, "wrap": "blue"}}),
        "attributes",
    ));
    assert_eq!(cart["attributes"], json!({"wrap": "blue"}));

    let with_sections = body_json(&get(&state, "/cart.js?sections=footer", "attributes"));
    assert!(
        with_sections["sections"]["footer"]
            .as_str()
            .is_some_and(|html| html.contains("</footer>")),
        "{with_sections}"
    );
}

#[test]
fn pages_carry_the_scripts_shopify_injects() {
    let state = server();
    let page = text(&get(&state, "/products/ceramic-mug", "platform"));
    let head = &page[..page.find("</head>").expect("a head")];
    for expected in [
        "window.performance.mark('shopify.content_for_header.start')",
        "Shopify.shop = \"local-supply-co.myshopify.com\";",
        "Shopify.routes.root = \"\\/\";",
        "Shopify.cdnHost = \"shop.test\\/cdn\";",
        "shopify.loadFeatures = queue();",
        "<script id=\"__st\">",
        "\"p\":\"product\",\"rtyp\":\"product\"",
        "window.ShopifyAnalytics.meta.currency = 'USD';",
        "\"pageType\":\"product\",\"resourceType\":\"product\"",
        "replayQueue: []",
    ] {
        assert!(head.contains(expected), "missing {expected} in\n{head}");
    }
    assert!(page.contains(
        "<script src=\"//shop.test/cdn/storefront/standard-actions.js\" type=\"module\" data-source-attribution=\"shopify.standard_actions\"></script></body>"
    ));

    // The scripts the page points at exist.
    let actions = get(&state, "/cdn/storefront/standard-actions.js", "platform");
    assert_eq!(
        header(&actions, "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert!(text(&actions).contains("Object.defineProperty(window.Shopify, 'actions'"));
    // The feature loader is where the page says, at a URL that carries a hash of its content.
    let loader_path = format!("/cdn/{}", lsf_core::render::platform::load_features_path());
    assert!(
        loader_path.starts_with("/cdn/shopifycloud/storefront/assets/storefront/load_feature-")
            && loader_path.ends_with(".js"),
        "{loader_path}"
    );
    assert!(
        head.contains(&format!("src=\"//shop.test{loader_path}\"")),
        "{head}"
    );
    let loader = get(&state, &loader_path, "platform");
    assert!(text(&loader).contains("Shopify.loadFeatures = loadFeatures;"));

    // A section on its own is not a page: nothing is added to it.
    let section = text(&get(&state, "/?section_id=footer", "platform"));
    assert!(!section.contains("standard-actions.js"));
}

/// A server whose options were given on the command line.
fn server_with_throttle(rules: &str) -> ServerState {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
    let app = App::open(&theme, None, Revalidate::Never, &[]).expect("fixture theme");
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Throttle::parse([rules]).expect("valid rules"),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    state
}

#[test]
fn requests_are_delayed_by_kind() {
    let ms = std::time::Duration::from_millis;
    let state = server_with_throttle("100ms,cart=300ms,cart-add=1s,asset=0");
    let delay =
        |method: &str, path: &str| state.delay_for(&request(method, path, json!({}), "throttled"));
    assert_eq!(delay("GET", "/"), ms(100));
    // The kind is told from the path without its locale prefix.
    assert_eq!(delay("GET", "/fr/products/ceramic-mug"), ms(100));
    assert_eq!(delay("GET", "/?section_id=footer"), ms(100));
    assert_eq!(delay("GET", "/cart.js"), ms(300));
    assert_eq!(delay("POST", "/fr/cart/change.js"), ms(300));
    assert_eq!(delay("POST", "/cart/add.js"), ms(1000));
    assert_eq!(delay("GET", "/cdn/shop/t/1/assets/base.css"), ms(0));
    assert_eq!(
        delay("GET", "/cdn/shop/files/products/tee-white.jpg"),
        ms(100)
    );
    // Tests set themselves up through the control API: it never waits.
    assert_eq!(delay("PUT", "/__lsf/session"), ms(0));

    // No rule, no delay.
    assert_eq!(
        server().delay_for(&request("POST", "/cart/add.js", json!({}), "x")),
        ms(0)
    );
    assert_eq!(
        body_json(&get(&state, "/__lsf/status", "throttled"))["throttle"],
        json!({"all": 100, "cart": 300, "cart-add": 1000, "asset": 0})
    );
}

#[test]
fn a_session_can_have_a_throttle_of_its_own() {
    let ms = std::time::Duration::from_millis;
    let state = server_with_throttle("cart=300ms");
    let put = |session: &str, body: Json| {
        state.dispatch(&request("PUT", "/__lsf/session", body, session))
    };
    let add = |session: &str| state.delay_for(&request("POST", "/cart/add.js", json!({}), session));

    let reply = put("slow", json!({"throttle": {"cart-add": "2s"}}));
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(
        body_json(&reply)["session"]["throttle"],
        json!({"cart-add": 2000})
    );
    assert_eq!(add("slow"), ms(2000));
    // It replaces the server's rules for that session: the cart is no longer delayed.
    assert_eq!(
        state.delay_for(&request("GET", "/cart.js", json!({}), "slow")),
        ms(0)
    );
    // Other sessions keep the server's.
    assert_eq!(add("other"), ms(300));

    // Zero lifts the server's throttle for one session.
    put("fast", json!({"throttle": 0}));
    assert_eq!(add("fast"), ms(0));

    // Setting the session again without a throttle goes back to the server's.
    put("slow", json!({}));
    assert_eq!(add("slow"), ms(300));

    let invalid = put("slow", json!({"throttle": {"cart-ad": 100}}));
    assert_eq!(invalid.status, 422);
    let diagnostic = &body_json(&invalid)["diagnostics"][0];
    assert_eq!(diagnostic["code"], "invalid_throttle");
    assert_eq!(diagnostic["path"], "/throttle");
    assert!(
        diagnostic["message"]
            .as_str()
            .unwrap()
            .contains("Did you mean \"cart-add\"?")
    );
}

/// The session as the control API reports it.
fn session_of(state: &ServerState, session: &str) -> Json {
    body_json(&get(state, "/__lsf/session", session))
}

#[test]
fn hosted_accounts_have_a_page_to_choose_the_customer() {
    let state = server();

    // Logged out, the page lists who the visitor can become.
    let page = get(&state, "/account", "hosted");
    assert_eq!(page.status, 200);
    let html = text(&page);
    assert!(
        html.contains("data-lsf-account data-lsf-customer=\"\""),
        "{html}"
    );
    assert!(html.contains("data-lsf-login=\"none\""));
    assert!(html.contains(
        "href=\"/__lsf/login?customer=jane.doe%40example.com&amp;return_to=%2Faccount\""
    ));
    assert!(html.contains("B2B · Northwind Hotels"));

    // Every account URL a theme links to leads there, and keeps where to come back to.
    for path in [
        "/account/login",
        "/account/register",
        "/account/addresses",
        "/account/orders/1",
        "/account/profile",
        "/customer_authentication/login",
        "/customer_identity/login",
    ] {
        let reply = get(&state, path, "hosted");
        assert_eq!(reply.status, 302, "{path}");
        assert_eq!(header(&reply, "location"), Some("/account"), "{path}");
    }
    let reply = get(&state, "/account/login?return_to=/cart", "hosted");
    assert_eq!(
        header(&reply, "location"),
        Some("/account?return_to=%2Fcart")
    );
    assert!(
        text(&get(&state, "/account?return_to=/cart", "hosted"))
            .contains("customer=none&amp;return_to=%2Fcart")
    );

    // The links log in without a password: as the first customer, as anyone, as nobody.
    let reply = get(&state, "/__lsf/login?customer=default", "hosted");
    assert_eq!(header(&reply, "location"), Some("/account"));
    assert_eq!(
        session_of(&state, "hosted")["customer"],
        "jane.doe@example.com"
    );
    let html = text(&get(&state, "/account", "hosted"));
    assert!(html.contains("data-lsf-customer=\"jane.doe@example.com\""));
    assert!(html.contains("<h2>Jane Doe</h2>"));
    assert!(html.contains("<h3>Orders</h3>"));

    let reply = get(
        &state,
        "/__lsf/login?customer=new.customer@example.com&return_to=/cart",
        "hosted",
    );
    assert_eq!(header(&reply, "location"), Some("/cart"));
    assert_eq!(
        session_of(&state, "hosted")["customer"],
        "new.customer@example.com"
    );
    // A link cannot send the visitor to another site.
    let reply = get(
        &state,
        "/__lsf/login?customer=none&return_to=//evil.example",
        "hosted",
    );
    assert_eq!(header(&reply, "location"), Some("/account"));
    assert_eq!(session_of(&state, "hosted")["customer"], Json::Null);

    let unknown = get(&state, "/__lsf/login?customer=nobody@example.com", "hosted");
    assert_eq!(unknown.status, 422);
}

#[test]
fn legacy_accounts_use_the_theme() {
    let state = server();
    let set = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"data": {"shop": {"customer_accounts": "legacy"}}}),
        "legacy",
    ));
    assert_eq!(set.status, 200, "{}", text(&set));
    let reply = get(&state, "/account", "legacy");
    assert_eq!(header(&reply, "location"), Some("/account/login"));
    let reply = get(&state, "/customer_authentication/login", "legacy");
    assert_eq!(header(&reply, "location"), Some("/account/login"));
    assert_eq!(
        body_json(&get(&state, "/__lsf/status", "legacy"))["customer_accounts"],
        "new"
    );
}

#[test]
fn b2b_customers_buy_for_a_company_location() {
    let state = server();
    let section = |session: &str| text(&get(&state, "/?section_id=b2b", session));

    // A customer without a company is not a B2B customer.
    get(&state, "/__lsf/login?customer=default", "b2b");
    assert!(section("b2b").contains("Jane is not a B2B customer"));

    let set = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"customer": "alex.morgan@example.com"}),
        "b2b",
    ));
    assert_eq!(set.status, 200, "{}", text(&set));
    let session = session_of(&state, "b2b");
    assert_eq!(session["company"], "Northwind Hotels");
    assert_eq!(session["company_location"], "Northwind Portland");

    let html = section("b2b");
    assert!(
        html.contains("<p class=\"company\">Northwind Hotels (NW-001)</p>"),
        "{html}"
    );
    assert!(html.contains("<p class=\"terms\">Net 30</p>"));
    assert!(html.contains("<p class=\"location\">Northwind Portland, Portland, United States</p>"));
    assert!(html.contains("Loading dock B"));
    assert!(html.contains("<p class=\"count\">2 of 2</p>"));
    assert!(html.contains(
        "<li class=\"current\"><a href=\"http://shop.test/company_location/update?location_id="
    ));

    // The location is chosen with the URL Shopify gives each location.
    let seattle = state.loaded().store.companies[0].locations[1].id;
    let reply = get(
        &state,
        &format!("/company_location/update?location_id={seattle}&return_to=/cart"),
        "b2b",
    );
    assert_eq!(header(&reply, "location"), Some("/cart"));
    assert_eq!(
        session_of(&state, "b2b")["company_location"],
        "Northwind Seattle"
    );
    assert!(
        section("b2b")
            .contains("<p class=\"location\">Northwind Seattle, Seattle, United States</p>")
    );
    // A location the customer has no access to changes nothing.
    get(&state, "/company_location/update?location_id=1", "b2b");
    assert_eq!(
        session_of(&state, "b2b")["company_location"],
        "Northwind Seattle"
    );

    // A test can start at a location.
    let set = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"customer": "alex.morgan@example.com", "company_location": "Northwind Seattle"}),
        "b2b-2",
    ));
    assert_eq!(set.status, 200, "{}", text(&set));
    assert_eq!(
        session_of(&state, "b2b-2")["company_location"],
        "Northwind Seattle"
    );
    let wrong = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"customer": "alex.morgan@example.com", "company_location": "Northwind Boston"}),
        "b2b-2",
    ));
    assert_eq!(wrong.status, 422);
    assert!(
        text(&wrong).contains("Northwind Portland, Northwind Seattle"),
        "{}",
        text(&wrong)
    );

    // The account page shows the company and lets the customer change location.
    let html = text(&get(&state, "/account", "b2b"));
    assert!(
        html.contains("<p data-lsf-company>Northwind Hotels</p>"),
        "{html}"
    );
    assert!(html.contains("data-lsf-location=\"Northwind Seattle\" data-lsf-current-location"));
}

#[test]
fn the_server_can_start_logged_in() {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
    let app = App::open(&theme, None, Revalidate::Never, &[]).expect("fixture theme");
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: false,
            customer: Some("alex.morgan@example.com".to_string()),
            checkout: None,
        },
    );
    assert_eq!(
        session_of(&state, "start")["customer"],
        "alex.morgan@example.com"
    );
    // A test still decides for itself.
    state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"customer": "none"}),
        "start",
    ));
    assert_eq!(session_of(&state, "start")["customer"], Json::Null);
}

#[test]
fn the_password_page_works_and_locks_nothing() {
    let state = server();

    // Nothing leads to the page: the storefront is always open.
    assert_eq!(get(&state, "/", "open").status, 200);
    let page = get(&state, "/password", "open");
    assert_eq!(page.status, 200);
    assert_eq!(header(&page, "x-lsf-template"), Some("password"));
    assert!(text(&page).contains("<body class=\"password\">"));
    assert!(text(&page).contains("action=\"/password\""));

    // A wrong password stays on the page, with an error shown once.
    let wrong = post(&state, "/password", json!({"password": "nope"}), "open");
    assert_eq!(wrong.status, 302);
    assert_eq!(header(&wrong, "location"), Some("/password"));
    assert!(
        text(&get(&state, "/password", "open")).contains("<p class=\"error\">Password incorrect")
    );
    assert!(!text(&get(&state, "/password", "open")).contains("class=\"error\""));
    let wrong = post(&state, "/fr/password", json!({"password": "nope"}), "open");
    assert_eq!(header(&wrong, "location"), Some("/fr/password"));

    // "password" is the password until the data says otherwise.
    let right = post(&state, "/password", json!({"password": "password"}), "open");
    assert_eq!(header(&right, "location"), Some("/"));
    assert_eq!(get(&state, "/password", "open").status, 200);

    let set = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"data": {"shop": {"password": "sesame", "password_message": "Opening soon"}}}),
        "custom",
    ));
    assert_eq!(set.status, 200, "{}", text(&set));
    assert_eq!(get(&state, "/", "custom").status, 200);
    assert!(
        text(&get(&state, "/password", "custom")).contains("<p class=\"message\">Opening soon</p>")
    );
    let wrong = post(
        &state,
        "/password",
        json!({"password": "password"}),
        "custom",
    );
    assert_eq!(header(&wrong, "location"), Some("/password"));
    let right = post(&state, "/password", json!({"password": "sesame"}), "custom");
    assert_eq!(header(&right, "location"), Some("/"));
}

#[test]
fn robots_and_sitemaps_are_served() {
    let state = server();
    let robots = get(&state, "/robots.txt", "seo");
    assert_eq!(robots.status, 200);
    assert_eq!(
        header(&robots, "content-type"),
        Some("text/plain; charset=utf-8")
    );
    let body = text(&robots);
    assert!(
        body.starts_with("# we use Shopify as our ecommerce platform"),
        "{body}"
    );
    assert!(
        body.contains("User-agent: *\nDisallow: /a/downloads/-/*\n"),
        "{body}"
    );
    assert!(body.contains("Disallow: /cart\n"));
    assert!(body.contains("Sitemap: http://shop.test/sitemap.xml\n"));
    assert!(body.contains("User-agent: AhrefsBot\nCrawl-delay: 10\n"));
    // The fixture theme writes its own rules in `templates/robots.txt.liquid`.
    assert!(
        body.contains("Disallow: /*?q=*\nDisallow: /*?view=\n"),
        "{body}"
    );

    // A theme without that template gets what Shopify serves by default.
    let root = changing_theme("robots");
    let app = App::open(&root, None, Revalidate::Never, &[]).unwrap();
    let (plain, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    let body = text(&get(&plain, "/robots.txt", "seo"));
    let _ = std::fs::remove_dir_all(root);
    assert!(
        body.starts_with(
            "# we use Shopify as our ecommerce platform\n\nUser-agent: *\nDisallow: /a/downloads/-/*\n"
        ),
        "{body}"
    );
    assert!(
        body.contains(
            "\n\n# Google adsbot ignores robots.txt unless specifically named!\nUser-agent: adsbot-google\nDisallow: /checkouts/\n"
        ),
        "{body}"
    );
    assert!(
        body.contains("Sitemap: http://shop.test/sitemap.xml\n"),
        "{body}"
    );

    let index = get(&state, "/sitemap.xml", "seo");
    assert_eq!(
        header(&index, "content-type"),
        Some("application/xml; charset=utf-8")
    );
    let body = text(&index);
    for kind in ["products", "pages", "collections", "blogs"] {
        assert!(
            body.contains(&format!("<loc>http://shop.test/sitemap_{kind}_1.xml")),
            "{kind}: {body}"
        );
    }
    let products = text(&get(&state, "/sitemap_products_1.xml", "seo"));
    assert!(
        products.contains("<loc>http://shop.test/products/ceramic-mug</loc>"),
        "{products}"
    );
    assert!(products.contains("<image:image>"));
    assert!(
        text(&get(&state, "/sitemap_pages_1.xml", "seo")).contains("<loc>http://shop.test/pages/")
    );
    assert_eq!(get(&state, "/sitemap_orders_1.xml", "seo").status, 404);
}

#[test]
fn the_cart_api_sells_with_selling_plans() {
    let state = server();
    let socks = variant_id(&state, "SOCK-S");
    let mug = variant_id(&state, "MUG-OAT");
    let plans: Vec<u64> = state.loaded().store.selling_plan_groups[0]
        .plans
        .iter()
        .map(|plan| plan.id)
        .collect();

    // The product JSON lists the plans and what each variant costs with them.
    let product = body_json(&get(&state, "/products/merino-crew-socks.js", "plans"));
    assert_eq!(
        product["selling_plan_groups"][0]["selling_plans"][0]["id"],
        plans[0]
    );
    assert_eq!(
        product["variants"][0]["selling_plan_allocations"][0]["price"],
        1440
    );

    let added = post(
        &state,
        "/cart/add.js",
        json!({"id": socks, "quantity": 2, "selling_plan": plans[0]}),
        "plans",
    );
    assert_eq!(added.status, 200, "{}", text(&added));
    let line = body_json(&added);
    assert_eq!(line["price"], 1440);
    assert_eq!(line["original_price"], 1440);
    assert_eq!(line["line_price"], 2880);
    assert_eq!(line["total_discount"], 0);
    let allocation = &line["selling_plan_allocation"];
    assert_eq!(allocation["price"], 1440);
    assert_eq!(allocation["compare_at_price"], 1600);
    assert_eq!(
        allocation["selling_plan"]["name"],
        "Deliver every month, 10% off"
    );
    assert_eq!(
        allocation["price_adjustments"],
        json!([{"position": 1, "price": 1440}])
    );

    // The same variant bought once is another line. A form sends its fields as text.
    let once = post(
        &state,
        "/cart/add.js",
        json!({"id": socks.to_string(), "selling_plan": ""}),
        "plans",
    );
    assert_eq!(once.status, 200, "{}", text(&once));
    let cart = body_json(&get(&state, "/cart.js", "plans"));
    assert_eq!(cart["items"].as_array().unwrap().len(), 2);
    assert!(cart["items"][0].get("selling_plan_allocation").is_none());
    assert_eq!(cart["total_price"], 1600 + 2880);
    assert_eq!(cart["original_total_price"], 1600 + 2880);

    // A product that is not sold with the plan refuses it.
    let refused = post(
        &state,
        "/cart/add.js",
        json!({"id": mug, "selling_plan": plans[0]}),
        "plans",
    );
    assert_eq!(refused.status, 422);
    assert_eq!(
        body_json(&refused)["description"],
        "The selling plan is not available for Ceramic Mug - Oat."
    );
    // So does every product, for something that is not a plan.
    let refused = post(
        &state,
        "/cart/add.js",
        json!({"id": socks, "selling_plan": "monthly"}),
        "plans",
    );
    assert_eq!(refused.status, 422);
    assert_eq!(
        body_json(&get(&state, "/cart.js", "plans"))["item_count"],
        3
    );

    // `change.js` moves a line to another plan, or to none.
    let key = cart["items"][1]["key"].as_str().unwrap().to_string();
    let changed = body_json(&post(
        &state,
        "/cart/change.js",
        json!({"id": key, "selling_plan": plans[1]}),
        "plans",
    ));
    assert_eq!(changed["items"][1]["line_price"], 2 * 1520);
    assert_eq!(
        session_of(&state, "plans")["cart"]["items"][1]["selling_plan"],
        plans[1]
    );
    let changed = body_json(&post(
        &state,
        "/cart/change.js",
        json!({"line": 2, "selling_plan": null}),
        "plans",
    ));
    assert_eq!(changed["items"][1]["line_price"], 2 * 1600);
    assert!(changed["items"][1].get("selling_plan_allocation").is_none());
    assert!(
        session_of(&state, "plans")["cart"]["items"][1]
            .get("selling_plan")
            .is_none()
    );

    // A test starts with a subscription in the cart by naming the plan.
    let set = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": [
            {"variant": "SOCK-L", "selling_plan": "Deliver every 3 months, 5% off"}
        ]}}),
        "plans-2",
    ));
    assert_eq!(set.status, 200, "{}", text(&set));
    let cart = body_json(&get(&state, "/cart.js", "plans-2"));
    assert_eq!(cart["items"][0]["selling_plan_allocation"]["price"], 1520);
    let wrong = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": [{"variant": "MUG-OAT", "selling_plan": "Monthly"}]}}),
        "plans-2",
    ));
    assert_eq!(wrong.status, 422);
    assert!(
        text(&wrong).contains("is not sold with a selling plan named \\\"Monthly\\\""),
        "{}",
        text(&wrong)
    );

    // A product sold by subscription only.
    let set = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"data": {"products": [{
            "title": "Coffee Club",
            "price": 1800,
            "variants": [{"sku": "COFFEE"}],
            "selling_plan_groups": ["Subscribe and save"],
            "requires_selling_plan": true
        }]}}),
        "plans-3",
    ));
    assert_eq!(set.status, 200, "{}", text(&set));
    let coffee = body_json(&get(&state, "/products/coffee-club.js", "plans-3"));
    assert_eq!(coffee["requires_selling_plan"], true);
    let coffee = coffee["variants"][0]["id"].as_u64().unwrap();
    let refused = post(&state, "/cart/add.js", json!({"id": coffee}), "plans-3");
    assert_eq!(refused.status, 422);
    assert_eq!(
        body_json(&refused)["description"],
        "Variant can only be purchased with a selling plan."
    );
    let added = post(
        &state,
        "/cart/add.js",
        json!({"id": coffee, "selling_plan": plans[0]}),
        "plans-3",
    );
    assert_eq!(body_json(&added)["price"], 1620);
    // Such a line cannot go back to a one-time purchase.
    let refused = post(
        &state,
        "/cart/change.js",
        json!({"line": 1, "selling_plan": ""}),
        "plans-3",
    );
    assert_eq!(refused.status, 422);

    // The checkout summary names the plan.
    assert!(
        text(&get(&state, "/checkout", "plans-3"))
            .contains("Coffee Club<br><small>Deliver every month, 10% off</small>")
    );
}

#[test]
fn sections_are_rendered_for_a_variant() {
    let state = server();
    let slate = variant_id(&state, "MUG-SLT");

    // What a theme fetches to show where a variant can be picked up.
    let reply = get(
        &state,
        &format!("/variants/{slate}/?section_id=pickup-availability"),
        "pickup",
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let html = text(&reply);
    assert!(
        html.contains(&format!("<div class=\"pickup\" data-variant=\"{slate}\">")),
        "{html}"
    );
    assert!(html.contains("<h2>Ceramic Mug - Slate</h2>"), "{html}");
    assert!(
        html.contains("<p>Ottawa flagship: Usually ready in 2 hours</p>"),
        "{html}"
    );
    assert!(
        html.contains("<p>Montreal studio: unavailable</p>"),
        "{html}"
    );

    let bundle = body_json(&get(
        &state,
        &format!("/variants/{slate}?sections=pickup-availability"),
        "pickup",
    ));
    assert!(
        bundle["pickup-availability"]
            .as_str()
            .is_some_and(|html| html.contains("Ceramic Mug - Slate"))
    );

    // Elsewhere there is no `product_variant`.
    let elsewhere = text(&get(
        &state,
        "/products/ceramic-mug?section_id=pickup-availability",
        "pickup",
    ));
    assert!(elsewhere.contains("data-variant=\"\""), "{elsewhere}");

    // Without a section, the URL leads to the product with the variant selected.
    let reply = get(&state, &format!("/variants/{slate}"), "pickup");
    assert_eq!(
        header(&reply, "location"),
        Some(format!("/products/ceramic-mug?variant={slate}").as_str())
    );
    assert_eq!(get(&state, "/variants/1", "pickup").status, 404);
}

#[test]
fn a_session_brings_its_own_plans_and_locations() {
    let state = server();
    let oat = variant_id(&state, "MUG-OAT");
    let set = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({
            "data": {
                "selling_plan_groups": [{
                    "name": "Coffee club",
                    "selling_plans": [{
                        "name": "Every month",
                        "price_adjustments": [{"value_type": "percentage", "value": 15}]
                    }],
                    "products": ["ceramic-mug"]
                }],
                "locations": [{"name": "Pop-up store", "pick_up_time": "Ready in 1 hour"}],
                "swatches": {"Oat": "#e8dcc4"}
            },
            "cart": {"items": [{"variant": "MUG-OAT", "selling_plan": "Every month"}]}
        }),
        "own",
    ));
    assert_eq!(set.status, 200, "{}", text(&set));

    let cart = body_json(&get(&state, "/cart.js", "own"));
    assert_eq!(cart["items"][0]["price"], 1870);
    assert_eq!(
        cart["items"][0]["selling_plan_allocation"]["selling_plan"]["name"],
        "Every month"
    );
    let pickup = format!("/variants/{oat}?section_id=pickup-availability");
    assert!(text(&get(&state, &pickup, "own")).contains("<p>Pop-up store: Ready in 1 hour</p>"));

    // The other sessions do not see any of it.
    assert!(!text(&get(&state, &pickup, "other")).contains("Pop-up store"));
    let mug = body_json(&get(&state, "/products/ceramic-mug.js", "other"));
    assert_eq!(mug["selling_plan_groups"], json!([]));
}

/// What a browser asks an image with.
const IMAGE_ACCEPT: &str = "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8";

#[test]
fn images_come_in_the_lightest_format_the_client_reads() {
    let state = server();
    let image = |path: &str, accept: &str| {
        let reply = get_accepting(&state, path, accept);
        assert_eq!(reply.status, 200, "{path}");
        assert_eq!(header(&reply, "vary"), Some("Accept"), "{path}");
        assert_eq!(
            header(&reply, "cache-control"),
            Some("public, max-age=31557600"),
            "{path}"
        );
        reply
    };
    let kind = |reply: &Reply| header(reply, "content-type").map(str::to_string);

    // A placeholder is a JPEG for the clients that read nothing lighter, as its name says.
    let placeholder = "/cdn/shop/files/products/tee-white.jpg?width=200";
    let jpeg = image(placeholder, "*/*");
    assert_eq!(kind(&jpeg).as_deref(), Some("image/jpeg"));
    let webp = image(placeholder, "image/webp,*/*");
    assert_eq!(kind(&webp).as_deref(), Some("image/webp"));
    assert_eq!(&webp.body[..4], b"RIFF");
    assert!(webp.body.len() < jpeg.body.len());
    // Each client gets its own variant, the second time too.
    for _ in 0..2 {
        let again = image(placeholder, "*/*");
        assert_eq!(again.body, jpeg.body);
        assert_eq!(header(&again, "x-lsf-placeholder"), Some("1"));
        let again = image(placeholder, IMAGE_ACCEPT);
        assert_ne!(kind(&again).as_deref(), Some("image/jpeg"));
        assert!(again.body.len() <= webp.body.len());
        assert_eq!(header(&again, "x-lsf-placeholder"), Some("1"));
    }
    // `format: 'pjpg'` asks for a JPEG, whatever the client reads.
    let progressive = image(&format!("{placeholder}&format=pjpg"), IMAGE_ACCEPT);
    assert_eq!(kind(&progressive).as_deref(), Some("image/jpeg"));
}

#[test]
fn images_of_the_theme_and_of_the_store_go_through_the_image_cdn() {
    let root = changing_theme("images");
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::create_dir_all(root.join("shopify-local/files")).unwrap();
    // A picture with grain, which AVIF keeps for less than WebP does.
    let picture = image::RgbImage::from_fn(480, 360, |x, y| {
        let speck = (x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)).wrapping_mul(2246822519);
        let grain = (speck >> 24) as f64 / 255.0 * 20.0;
        image::Rgb([
            (f64::from(x) / 480.0 * 180.0 + grain + 40.0) as u8,
            (f64::from(y) / 360.0 * 150.0 + grain + 40.0) as u8,
            (f64::from(x + y) / 840.0 * 140.0 + grain + 40.0) as u8,
        ])
    });
    picture.save(root.join("assets/hero.png")).unwrap();
    picture
        .save(root.join("shopify-local/files/hero.png"))
        .unwrap();
    let file = std::fs::read(root.join("assets/hero.png")).unwrap();

    let app = App::open(&root, None, Revalidate::Every(Duration::ZERO), &[]).unwrap();
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    let image = |path: &str, accept: &str| {
        let reply = get_accepting(&state, path, accept);
        assert_eq!(reply.status, 200, "{path}");
        assert_eq!(header(&reply, "vary"), Some("Accept"), "{path}");
        assert_eq!(
            header(&reply, "cache-control"),
            Some("public, max-age=31557600"),
            "{path}"
        );
        let kind = header(&reply, "content-type")
            .unwrap_or_default()
            .to_string();
        (kind, reply.body)
    };
    // The width a PNG stores in its header.
    let width = |png: &[u8]| u32::from_be_bytes([png[16], png[17], png[18], png[19]]);

    // The file as it is for the clients that read nothing lighter, and resized on demand.
    let hero = "/cdn/shop/t/1/assets/hero.png";
    assert_eq!(image(hero, "*/*"), ("image/png".to_string(), file.clone()));
    let (kind, small) = image(&format!("{hero}?v=1&width=240"), "*/*");
    assert_eq!(kind, "image/png");
    assert_eq!(width(&small), 240);
    // `asset_img_url` names the size in the file name.
    let (kind, legacy) = image("/cdn/shop/t/1/assets/hero_120x.png", "*/*");
    assert_eq!(kind, "image/png");
    assert_eq!(width(&legacy), 120);

    // A lighter format for the clients that read one.
    let (kind, webp) = image(&format!("{hero}?v=1&width=240"), "image/webp");
    assert_eq!(kind, "image/webp");
    let (kind, avif) = image(&format!("{hero}?v=1&width=240"), IMAGE_ACCEPT);
    assert_eq!(kind, "image/avif");
    assert!(avif.len() < webp.len() && webp.len() < small.len());
    let (kind, whole) = image(hero, IMAGE_ACCEPT);
    assert_eq!(kind, "image/avif");
    assert!(whole.len() < file.len());

    // An image of the store is served the same way.
    let stored = "/cdn/shop/files/hero.png";
    assert_eq!(
        image(stored, "*/*"),
        ("image/png".to_string(), file.clone())
    );
    assert_eq!(
        image(stored, IMAGE_ACCEPT),
        ("image/avif".to_string(), whole.clone())
    );

    // A variant is kept, until the file changes.
    let plain = image::RgbImage::from_pixel(480, 360, image::Rgb([200, 30, 30]));
    for (path, url) in [
        ("assets/hero.png", hero),
        ("shopify-local/files/hero.png", stored),
    ] {
        assert_eq!(image(url, IMAGE_ACCEPT).1, whole);
        plain.save(root.join(path)).unwrap();
        assert!(image(url, IMAGE_ACCEPT).1.len() < whole.len(), "{url}");
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_video_is_served_in_the_parts_the_browser_asks_for() {
    let root = changing_theme("video");
    std::fs::create_dir_all(root.join("shopify-local/files")).unwrap();
    let video: Vec<u8> = (0..=255).collect();
    std::fs::write(root.join("shopify-local/files/clip.mp4"), &video).unwrap();
    let app = App::open(&root, None, Revalidate::Every(Duration::ZERO), &[]).unwrap();
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    let path = "/cdn/shop/files/clip.mp4";
    let ranged = |range: &str| {
        let mut incoming = request("GET", path, json!({}), "video");
        incoming
            .headers
            .insert("range".to_string(), range.to_string());
        state.dispatch(&incoming)
    };

    let whole = get(&state, path, "video");
    assert_eq!(whole.status, 200);
    assert_eq!(whole.body, video);
    assert_eq!(header(&whole, "accept-ranges"), Some("bytes"));
    assert_eq!(header(&whole, "content-type"), Some("video/mp4"));

    let part = ranged("bytes=0-1");
    assert_eq!(part.status, 206);
    assert_eq!(part.body, vec![0, 1]);
    assert_eq!(header(&part, "content-range"), Some("bytes 0-1/256"));
    assert_eq!(header(&part, "content-type"), Some("video/mp4"));

    let rest = ranged("bytes=250-");
    assert_eq!(rest.status, 206);
    assert_eq!(rest.body, video[250..]);
    assert_eq!(header(&rest, "content-range"), Some("bytes 250-255/256"));

    let beyond = ranged("bytes=300-");
    assert_eq!(beyond.status, 416);
    assert_eq!(header(&beyond, "content-range"), Some("bytes */256"));

    // An `.m4v` is an MP4 by another name.
    std::fs::write(root.join("shopify-local/files/clip.m4v"), &video).unwrap();
    let m4v = get(&state, "/cdn/shop/files/clip.m4v", "video");
    assert_eq!(header(&m4v, "content-type"), Some("video/mp4"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_theme_with_partials_gets_their_runtime_with_the_preview() {
    let root = changing_theme("partials");
    std::fs::write(
        root.join("layout/theme.liquid"),
        "<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
    )
    .unwrap();
    std::fs::write(
        root.join("templates/index.liquid"),
        "{% partial 'count' %}<p>1</p>{% endpartial %}",
    )
    .unwrap();
    let serve = |previews: &[lsf_core::Preview]| {
        let app = App::open(&root, None, Revalidate::Never, previews).unwrap();
        ServerState::new(
            app,
            ServeOptions {
                live_reload: false,
                watch: false,
                quiet: true,
                compress: false,
                minify: false,
                throttle: Default::default(),
                timings: false,
                customer: None,
                checkout: None,
            },
        )
        .0
    };

    // As on a live store: the tag is unknown, and nothing is added.
    let live = text(&get(&serve(&[]), "/", "partials"));
    assert!(live.contains("Unknown tag 'partial'"), "{live}");
    assert!(!live.contains("importmap"), "{live}");

    let state = serve(&[lsf_core::Preview::LiquidJuly26]);
    let page = text(&get(&state, "/", "partials"));
    let module = page
        .split("\"@shopify/partial-rendering\":\"//")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("an import map for the runtime");
    let path = &module[module.find('/').unwrap()..];
    let script = get(&state, path, "partials");
    assert_eq!(script.status, 200, "{path}");
    assert_eq!(
        header(&script, "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert!(text(&script).contains("export const partials"));
    assert!(page.contains("<!--shopify-partial name=\"count\"--><p>1</p><!--/shopify-partial-->"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn preloads_are_one_link_header_written_as_shopify_writes_it() {
    let root = changing_theme("preloads");
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(root.join("assets/base.css"), "body { margin: 0 }").unwrap();
    // A head with what blocks rendering, and what does not.
    std::fs::write(
        root.join("layout/theme.liquid"),
        "<html><head>\n\
         <link rel=\"stylesheet\" href=\"https://fonts.example/css2?family=Inter&amp;display=swap\">\n\
         {{ 'theme.css' | asset_url | stylesheet_tag }}\n\
         <link rel=\"stylesheet\" href=\"{{ 'late.css' | asset_url }}\" media=\"print\">\n\
         <script src=\"{{ 'blocking.js' | asset_url }}\"></script>\n\
         <script src=\"{{ 'late.js' | asset_url }}\" defer></script>\n\
         </head><body>{{ content_for_layout }}</body></html>",
    )
    .unwrap();
    std::fs::write(
        root.join("templates/index.liquid"),
        "{{ 'body.woff2' | asset_url | preload_tag: as: 'font', type: 'font/woff2', \
            fetchpriority: 'low' }}\n\
         {{ 'base.css' | asset_url | stylesheet_tag: preload: true }}\n\
         {% assign image = collections.all.products.first.featured_image %}\n\
         {{ image | image_url: width: 600 | image_tag: preload: true, widths: '200, 400', \
            sizes: '(min-width: 750px) 50vw, 100vw' }}\n\
         {{ image | image_url: width: 100 | image_tag: preload: true, srcset: nil }}\n\
         {{ 'base.css' | asset_url | stylesheet_tag: preload: true }}\n\
         {{ 'cart.js' | asset_url | preload_tag: as: 'script' }}\n\
         {{ 'title.woff2' | asset_url | preload_tag: as: 'font', crossorigin: 'anonymous' }}",
    )
    .unwrap();
    let app = App::open(&root, None, Revalidate::Never, &[]).unwrap();
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    let page = get(&state, "/", "preloads");
    assert_eq!(page.status, 200);
    let html = text(&page);
    // An image does not say that it is preloaded.
    assert!(
        html.contains("<img src=") && !html.contains("preload=\"true\""),
        "{html}"
    );
    // The attributes of the tag that preloads a file of the theme.
    let tag = |name: &str| {
        let start = html.find(&format!("/assets/{name}?v=")).expect(name);
        let (_, attributes) = html[start..].split_once("\" ").expect(name);
        attributes[..attributes.find('>').expect(name)].to_string()
    };
    // A font is asked for without credentials, whether the theme says so or not: a browser
    // would not use the preloaded file otherwise.
    assert_eq!(
        tag("body.woff2"),
        "as=\"font\" type=\"font/woff2\" fetchpriority=\"low\" crossorigin=\"anonymous\" \
         rel=\"preload\""
    );
    assert_eq!(
        tag("title.woff2"),
        "as=\"font\" crossorigin=\"anonymous\" rel=\"preload\""
    );
    assert_eq!(tag("cart.js"), "as=\"script\" rel=\"preload\"");

    let links: Vec<&str> = page
        .headers
        .iter()
        .filter(|(name, _)| name == "link")
        .map(|(_, value)| value.as_str())
        .collect();
    let [link] = links.as_slice() else {
        panic!("one Link header is expected: {links:?}");
    };
    // The entries are separated by commas, like the sizes of an image: these are quoted.
    let entries: Vec<&str> = link.split(", <").collect();
    // What blocks rendering in the head comes first, without being asked, after the other
    // origin it comes from. Then what the theme asks for, each file once, stylesheets first.
    let [
        origin,
        fonts,
        theme,
        blocking,
        stylesheet,
        body_font,
        image,
        small,
        script,
        title_font,
    ] = entries.as_slice()
    else {
        panic!("an origin and nine files are expected: {link}");
    };
    assert_eq!(*origin, "<https://fonts.example>; rel=\"preconnect\"");
    assert_eq!(
        *fonts,
        "https://fonts.example/css2?family=Inter&display=swap>; as=\"style\"; rel=\"preload\""
    );
    // What an entry says of a file of the theme.
    let hint = |entry: &str, name: &str| {
        let (url, hint) = entry.split_once(">; ").expect(name);
        assert!(
            url.trim_start_matches('<')
                .starts_with(&format!("//shop.test/cdn/shop/t/1/assets/{name}?v=")),
            "{entry}"
        );
        hint.to_string()
    };
    assert_eq!(hint(theme, "theme.css"), "as=\"style\"; rel=\"preload\"");
    assert_eq!(
        hint(blocking, "blocking.js"),
        "as=\"script\"; rel=\"preload\""
    );
    assert_eq!(
        hint(stylesheet, "base.css"),
        "as=\"style\"; rel=\"preload\""
    );
    // `preload_tag` repeats the attributes of its tag.
    assert_eq!(
        hint(body_font, "body.woff2"),
        "as=\"font\"; type=\"font/woff2\"; fetchpriority=\"low\"; crossorigin; rel=\"preload\""
    );
    assert_eq!(
        hint(title_font, "title.woff2"),
        "as=\"font\"; crossorigin; rel=\"preload\""
    );
    assert_eq!(hint(script, "cart.js"), "as=\"script\"; rel=\"preload\"");
    let (url, sized) = image.split_once(">; ").unwrap();
    assert!(url.starts_with("//shop.test/cdn/shop/files/") && url.ends_with("&width=600"));
    let (start, sizes) = sized.split_once("; imagesizes=").unwrap();
    assert_eq!(sizes, "\"(min-width: 750px) 50vw, 100vw\"");
    let srcset = start
        .strip_prefix("as=\"image\"; rel=\"preload\"; imagesrcset=\"")
        .and_then(|srcset| srcset.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{start}"));
    let candidates: Vec<&str> = srcset.split(", ").collect();
    assert_eq!(candidates.len(), 2, "{srcset}");
    assert!(candidates[0].ends_with("&width=200 200w"), "{srcset}");
    assert!(candidates[1].ends_with("&width=400 400w"), "{srcset}");
    // An image without sizes to choose from only names its URL.
    assert!(
        small.ends_with("&width=100>; as=\"image\"; rel=\"preload\""),
        "{small}"
    );
    let _ = std::fs::remove_dir_all(&root);

    // The stylesheet built from the `{% stylesheet %}` tags is preloaded too, after the one
    // of the theme.
    let home = get(&server(), "/", "preloads");
    let link = header(&home, "link").unwrap_or_default();
    let entries: Vec<&str> = link.split(", <").collect();
    let [base, compiled] = entries.as_slice() else {
        panic!("two stylesheets are expected: {link}");
    };
    assert_eq!(hint(base, "base.css"), "as=\"style\"; rel=\"preload\"");
    let (url, compiled) = compiled.split_once(">; ").unwrap_or_default();
    assert!(
        url.starts_with("//shop.test/cdn/shop/t/1/compiled_assets/styles.css?v="),
        "{link}"
    );
    assert!(text(&home).contains(url));
    assert_eq!(compiled, "as=\"style\"; rel=\"preload\"");
}

#[test]
fn stylesheets_and_scripts_are_minified_as_on_shopify() {
    let root = changing_theme("minified");
    for directory in ["assets", "sections"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    let stylesheet =
        ".card {\n  /* A card. */\n  color: #ff0000;\n  margin: 0px 0px 0px 0px;\n\n  \
        &:hover {\n    opacity: 0.5;\n  }\n}\n\n\
        .card__title {\n  font-weight: normal;\n  text-decoration: none;\n}\n"
            .repeat(2);
    let script = "function debounce(callback, wait) {\n  let timer;\n\n  \
        return (...args) => {\n    clearTimeout(timer);\n    \
          timer = setTimeout(() => callback.apply(this, args), wait);\n  };\n}\n\n\
        // Tells whether a value is missing.\n\
        function isMissing(value) {\n  return value === undefined;\n}\n";
    let write = |name: &str, content: &str| std::fs::write(root.join(name), content).unwrap();
    write("assets/theme.css", &stylesheet);
    write("assets/global.js", script);
    write("assets/vendor.min.js", script);
    write("assets/small.css", "body { margin: 0 }\n");
    write(
        "assets/colors.css.liquid",
        &format!("{stylesheet}\n.shop::after {{\n  content: '{{{{ shop.name }}}}';\n}}\n"),
    );
    write(
        "sections/hero.liquid",
        &format!(
            "<div class=\"hero\"></div>\n{{% stylesheet %}}\n{stylesheet}{{% endstylesheet %}}\n"
        ),
    );
    let serve = |minify: bool| {
        let app = App::open(&root, None, Revalidate::Never, &[]).unwrap();
        let (state, _) = ServerState::new(
            app,
            ServeOptions {
                live_reload: false,
                watch: false,
                quiet: true,
                compress: true,
                minify,
                throttle: Default::default(),
                timings: false,
                customer: None,
                checkout: None,
            },
        );
        state
    };
    let state = serve(true);
    let file = |name: &str| get(&state, &format!("/cdn/shop/t/1/{name}"), "minified");
    let year = Some("public, max-age=31557600");

    // A stylesheet: one line, rewritten for older browsers, then the link to its source map.
    let theme = file("assets/theme.css?v=1");
    assert_eq!(theme.status, 200);
    assert_eq!(
        header(&theme, "content-type"),
        Some("text/css; charset=utf-8")
    );
    assert_eq!(header(&theme, "cache-control"), year);
    let served = text(&theme);
    let (code, link) = served.split_once('\n').unwrap();
    assert!(
        code.starts_with(".card{color:red;margin:0}.card:hover{opacity:.5}"),
        "{code}"
    );
    assert_eq!(
        link,
        "/*# sourceMappingURL=/cdn/shop/t/1/assets/theme.css.map */\n"
    );
    assert!(served.len() < stylesheet.len());
    // The map leads back to the file as it is written.
    let map = file("assets/theme.css.map");
    assert_eq!(map.status, 200);
    assert_eq!(header(&map, "cache-control"), year);
    let map = body_json(&map);
    assert_eq!(map["sources"], json!(["/cdn/shop/t/1/assets/theme.css"]));
    assert_eq!(map["sourcesContent"], json!([stylesheet]));

    // A script: its names are kept.
    let global = text(&file("assets/global.js"));
    assert_eq!(
        global,
        "function debounce(callback,wait){let timer;return(...args)=>{clearTimeout(timer),\
         timer=setTimeout(()=>callback.apply(this,args),wait)}}\
         function isMissing(value){return value===void 0}\n\
         //# sourceMappingURL=/cdn/shop/t/1/assets/global.js.map\n"
    );
    assert_eq!(
        body_json(&file("assets/global.js.map"))["sourcesContent"],
        json!([script])
    );

    // A `.liquid` file is minified once it is rendered.
    let colors = text(&file("assets/colors.css"));
    assert!(colors.starts_with(".card{color:red;margin:0}"), "{colors}");
    assert!(
        colors.contains(".shop:after{content:\"Local Supply Co.\"}"),
        "{colors}"
    );
    assert!(colors.ends_with("/*# sourceMappingURL=/cdn/shop/t/1/assets/colors.css.map */\n"));
    assert_eq!(file("assets/colors.css.map").status, 200);

    // So is the stylesheet built from the `{% stylesheet %}` tags.
    let compiled = file("compiled_assets/styles.css?v=1");
    assert_eq!(
        header(&compiled, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    let compiled = text(&compiled);
    assert!(
        compiled.starts_with(".card{color:red;margin:0}"),
        "{compiled}"
    );
    assert!(
        compiled
            .ends_with("/*# sourceMappingURL=/cdn/shop/t/1/compiled_assets/styles.css.map */\n"),
        "{compiled}"
    );
    assert_eq!(file("compiled_assets/styles.css.map").status, 200);

    // A file that says it is minified, and one that would not get lighter, are served as
    // written, and have no map.
    assert_eq!(text(&file("assets/vendor.min.js")), script);
    assert_eq!(file("assets/vendor.min.js.map").status, 404);
    assert_eq!(text(&file("assets/small.css")), "body { margin: 0 }\n");
    assert_eq!(file("assets/small.css.map").status, 404);
    assert_eq!(file("assets/missing.css.map").status, 404);

    // With `--no-minify`, every file is.
    let state = serve(false);
    let file = |name: &str| get(&state, &format!("/cdn/shop/t/1/{name}"), "minified");
    assert_eq!(text(&file("assets/theme.css")), stylesheet);
    assert_eq!(text(&file("assets/global.js")), script);
    assert_eq!(file("assets/theme.css.map").status, 404);
    assert!(text(&file("compiled_assets/styles.css")).contains("  color: #ff0000;"));

    let _ = std::fs::remove_dir_all(&root);
}

/// What a served theme is made of at the least, in a directory of its own.
fn changing_theme(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("lsf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for directory in ["layout", "templates", "src"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(
        root.join("layout/theme.liquid"),
        "<html><body>{{ content_for_layout }}</body></html>",
    )
    .unwrap();
    std::fs::write(root.join("templates/index.liquid"), "<h1>before</h1>").unwrap();
    root
}

/// One request on a connection of its own. Returns the response, headers included.
async fn http_get(address: std::net::SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: shop.test\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

/// The text of the next WebSocket frame the server sends. Server frames are not masked, and
/// the tokens are short enough for the length to fit in the second byte.
async fn next_frame(stream: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;
    let frame = async {
        let mut header = [0u8; 2];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0], 0x81, "a final text frame");
        assert!(header[1] < 126, "a short, unmasked frame");
        let mut payload = vec![0u8; usize::from(header[1])];
        stream.read_exact(&mut payload).await.unwrap();
        String::from_utf8(payload).unwrap()
    };
    tokio::time::timeout(Duration::from_secs(10), frame)
        .await
        .expect("the server says nothing")
}

#[tokio::test(flavor = "multi_thread")]
async fn live_reload_tells_pages_about_changes_over_a_websocket() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let root = changing_theme("live-reload");
    // Files are checked once a minute: a page reloaded at once only shows a change if live
    // reload says that the files changed.
    let app = App::open(&root, None, Revalidate::Every(Duration::from_secs(60)), &[]).unwrap();
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: true,
            watch: true,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(super::run(std::sync::Arc::new(state), listener));

    // The page carries the script, which opens the socket.
    let page = http_get(address, "/").await;
    assert!(page.contains("<h1>before</h1>"), "{page}");
    assert!(page.contains("<script data-lsf-live-reload>"), "{page}");
    assert!(page.contains("new WebSocket("), "{page}");

    // The handshake of RFC 6455, with the key and the answer of its example.
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            b"GET /__lsf/livereload HTTP/1.1\r\nHost: shop.test\r\nConnection: Upgrade\r\n\
              Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let mut handshake = Vec::new();
    while !handshake.ends_with(b"\r\n\r\n") {
        handshake.push(socket.read_u8().await.unwrap());
    }
    let handshake = String::from_utf8(handshake).unwrap().to_lowercase();
    assert!(handshake.starts_with("http/1.1 101 "), "{handshake}");
    assert!(
        handshake.contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
        "{handshake}"
    );

    // First what the files are now. A page without a socket is told the same when it asks.
    let before = next_frame(&mut socket).await;
    let asked = http_get(address, "/__lsf/livereload").await;
    assert!(asked.starts_with("HTTP/1.1 200 "), "{asked}");
    assert_eq!(asked.rsplit("\r\n\r\n").next(), Some(before.as_str()));

    // A file that is not part of the theme changes nothing.
    std::fs::write(root.join("src/app.ts"), "export {};").unwrap();
    let quiet = tokio::time::timeout(Duration::from_millis(400), next_frame(&mut socket)).await;
    assert!(quiet.is_err(), "{quiet:?}");

    // A template does, and the page rendered next is the new one.
    std::fs::write(root.join("templates/index.liquid"), "<h1>after!</h1>").unwrap();
    let after = next_frame(&mut socket).await;
    assert_ne!(after, before);
    let page = http_get(address, "/").await;
    assert!(page.contains("<h1>after!</h1>"), "{page}");
    let asked = http_get(address, "/__lsf/livereload").await;
    assert_eq!(asked.rsplit("\r\n\r\n").next(), Some(after.as_str()));

    server.abort();
    let _ = std::fs::remove_dir_all(root);
}

/// One request with headers of its own. Returns the head of the response, in lowercase, and
/// the bytes of its body.
async fn http_request(
    address: std::net::SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
) -> (String, Vec<u8>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: shop.test\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("a complete response");
    (
        String::from_utf8_lossy(&response[..end]).to_lowercase(),
        response[end + 4..].to_vec(),
    )
}

/// The value of a header in the head of a response.
fn head_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .filter_map(|line| line.split_once(": "))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.trim())
}

async fn serve_fixture(compress: bool) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
    let app = App::open(&theme, None, Revalidate::Never, &[]).expect("fixture theme");
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress,
            minify: true,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = super::run(std::sync::Arc::new(state), listener).await;
    });
    (address, server)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_part_of_a_file_is_sent_as_it_is_even_to_a_browser_that_accepts_compression() {
    let root = changing_theme("ranged-compressed");
    std::fs::create_dir_all(root.join("shopify-local/files")).unwrap();
    let data = format!("[{}]", vec!["\"compressible\""; 400].join(","));
    std::fs::write(root.join("shopify-local/files/data.json"), &data).unwrap();
    let app = App::open(&root, None, Revalidate::Never, &[]).unwrap();
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = super::run(std::sync::Arc::new(state), listener).await;
    });
    let browser = ("accept-encoding", "gzip, deflate, br, zstd");
    let path = "/cdn/shop/files/data.json";

    let (head, body) = http_request(address, path, &[browser, ("range", "bytes=0-99")]).await;
    assert!(head.starts_with("http/1.1 206 "), "{head}");
    assert_eq!(head_value(&head, "content-encoding"), None);
    assert_eq!(body, data.as_bytes()[..100]);
    // The whole file is compressed, and then offers no parts.
    let (head, _) = http_request(address, path, &[browser]).await;
    assert_eq!(head_value(&head, "content-encoding"), Some("br"));
    assert_eq!(head_value(&head, "accept-ranges"), None);

    server.abort();
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn responses_are_compressed_for_the_clients_that_accept_it() {
    use super::compress::{Encoding, decompress};

    let (address, server) = serve_fixture(true).await;
    let session = ("x-lsf-session", "compression");
    let (head, plain) = http_request(address, "/", &[session]).await;
    assert!(head.starts_with("http/1.1 200 "), "{head}");
    assert_eq!(head_value(&head, "content-encoding"), None);
    assert_eq!(head_value(&head, "vary"), Some("accept-encoding, accept"));
    assert!(plain.len() > 1024, "{}", plain.len());

    // What a browser sends: Brotli, the same page, much smaller.
    let browser = ("accept-encoding", "gzip, deflate, br, zstd");
    let (head, body) = http_request(address, "/", &[session, browser]).await;
    assert_eq!(head_value(&head, "content-encoding"), Some("br"));
    assert_eq!(head_value(&head, "vary"), Some("accept-encoding, accept"));
    assert_eq!(
        head_value(&head, "content-length"),
        Some(body.len().to_string().as_str())
    );
    assert!(
        body.len() < plain.len() / 2,
        "{} of {}",
        body.len(),
        plain.len()
    );
    assert_eq!(decompress(Encoding::Brotli, &body), plain);

    // A client that only knows gzip.
    let (head, body) = http_request(address, "/", &[session, ("accept-encoding", "gzip")]).await;
    assert_eq!(head_value(&head, "content-encoding"), Some("gzip"));
    assert_eq!(decompress(Encoding::Gzip, &body), plain);

    // Theme files and JSON are compressed too; images and small answers are not.
    let (head, body) =
        http_request(address, "/products/organic-cotton-t-shirt.js", &[browser]).await;
    assert_eq!(head_value(&head, "content-encoding"), Some("br"));
    let product: Json = serde_json::from_slice(&decompress(Encoding::Brotli, &body)).unwrap();
    assert_eq!(product["handle"], "organic-cotton-t-shirt");
    let (head, _) = http_request(
        address,
        "/cdn/shop/files/products/tee-white.jpg?width=200",
        &[browser],
    )
    .await;
    assert!(head.starts_with("http/1.1 200 "), "{head}");
    assert_eq!(head_value(&head, "content-encoding"), None);
    assert_eq!(head_value(&head, "vary"), Some("accept"));
    let (head, body) = http_request(address, "/cart.js", &[session, browser]).await;
    assert_eq!(head_value(&head, "content-encoding"), None);
    assert_eq!(
        serde_json::from_slice::<Json>(&body).unwrap()["item_count"],
        0
    );
    server.abort();

    // With `--no-compression`, nothing is, whatever the client accepts.
    let (address, server) = serve_fixture(false).await;
    let (head, body) = http_request(address, "/", &[session, browser]).await;
    assert_eq!(head_value(&head, "content-encoding"), None);
    assert_eq!(head_value(&head, "vary"), Some("accept"));
    assert_eq!(body, plain);
    server.abort();
}

#[test]
fn responses_say_how_long_to_keep_them_as_shopify_does() {
    let state = server();
    let mug = variant_id(&state, "MUG-OAT");
    let headers = |reply: &Reply| {
        (
            header(reply, "cache-control").map(str::to_string),
            header(reply, "vary").map(str::to_string),
        )
    };
    let of = |path: &str| headers(&get(&state, path, "cache"));
    let policy = |path: &str| of(path).0;

    // What the storefront renders is checked again every time.
    for path in [
        "/",
        "/products/ceramic-mug",
        "/collections/all",
        "/cart",
        "/search?q=mug",
        "/?section_id=footer",
        "/?sections=footer",
        "/products/ceramic-mug.js",
        "/collections/all/products.json",
        "/search/suggest.json?q=mug",
        "/robots.txt",
        "/sitemap.xml",
    ] {
        assert_eq!(
            of(path),
            (
                Some("private, max-age=0, must-revalidate".to_string()),
                Some("Accept".to_string())
            ),
            "{path}"
        );
    }
    // What is not found and redirects are not kept.
    for path in ["/pages/nope", "/products/nope", "/products/nope.js"] {
        assert_eq!(policy(path).as_deref(), Some("private, no-store"), "{path}");
    }
    let redirect = get(&state, &format!("/variants/{mug}"), "cache");
    assert_eq!(redirect.status, 302);
    assert_eq!(
        headers(&redirect),
        (
            Some("private, no-store".to_string()),
            Some("Accept".to_string())
        )
    );
    // Reading the cart says nothing; changing it is never kept, whatever the outcome.
    assert_eq!(of("/cart.js"), (None, Some("Accept".to_string())));
    let added = post(&state, "/cart/add.js", json!({"id": mug}), "cache");
    assert_eq!(added.status, 200);
    assert_eq!(
        headers(&added),
        (
            Some("no-cache, no-store".to_string()),
            Some("Accept".to_string())
        )
    );
    let refused = post(&state, "/cart/add.js", json!({"id": 1}), "cache");
    assert_eq!(refused.status, 404);
    assert_eq!(
        header(&refused, "cache-control"),
        Some("no-cache, no-store")
    );
    for (path, body) in [
        ("/cart/change.js", json!({"line": 1, "quantity": 2})),
        ("/cart/update.js", json!({"note": "x"})),
        ("/cart/clear.js", json!({})),
        ("/cart", json!({"updates": [1]})),
    ] {
        assert_eq!(
            header(&post(&state, path, body, "cache"), "cache-control"),
            Some("no-cache, no-store"),
            "{path}"
        );
    }

    // Files are kept for a year. The values are Shopify's, kind by kind.
    let year = Some("public, max-age=31557600".to_string());
    let immutable = Some("public, max-age=31536000, immutable".to_string());
    assert_eq!(of("/cdn/shop/t/1/assets/base.css"), (year.clone(), None));
    assert_eq!(policy("/cdn/shop/t/1/assets/icon.svg"), year);
    // A `.liquid` asset is rendered with the settings, which its URL follows.
    let rendered = get(&state, "/cdn/shop/t/1/assets/colors.css", "cache");
    assert!(
        text(&rendered).starts_with(":root { --page-size: "),
        "{}",
        text(&rendered)
    );
    assert_eq!(headers(&rendered).0, year);
    assert_eq!(policy("/cdn/shop/t/1/compiled_assets/scripts.js"), year);
    assert_eq!(
        policy("/cdn/shop/t/1/compiled_assets/styles.css"),
        immutable
    );
    assert_eq!(policy("/cdn/fonts/work_sans/work_sans_n4.woff2"), immutable);
    // An image also says that its format depends on what the browser accepts.
    assert_eq!(
        of("/cdn/shop/files/products/tee-white.jpg?width=200"),
        (year.clone(), Some("Accept".to_string()))
    );
    // Shopify's own files: a year at a URL with a hash, ten minutes at one that never changes.
    assert_eq!(
        policy(&format!(
            "/cdn/{}",
            lsf_core::render::platform::load_features_path()
        ))
        .as_deref(),
        Some("public, max-age=31536000")
    );
    assert_eq!(
        policy("/cdn/shopifycloud/storefront/assets/payment_icons/visa.svg").as_deref(),
        Some("public, max-age=31536000")
    );
    assert_eq!(
        policy("/cdn/storefront/standard-actions.js").as_deref(),
        Some("public, max-age=600, must-revalidate")
    );
    // A theme asset that does not exist.
    let missing = get(&state, "/cdn/shop/t/1/assets/nope.css", "cache");
    assert_eq!(missing.status, 404);
    assert_eq!(
        header(&missing, "cache-control"),
        Some("public, max-age=60")
    );
    // The control API is not Shopify's: it says nothing.
    assert_eq!(policy("/__lsf/status"), None);
}

/// The entries of the `Server-Timing` header of a reply: their name, followed by their
/// description when they have one.
fn timing_entries(value: &str) -> Vec<String> {
    value
        .split(", ")
        .map(|entry| {
            let (name, rest) = entry.split_once(";dur=").expect(entry);
            let (duration, description) = rest.split_once(";desc=").unwrap_or((rest, ""));
            assert!(duration.parse::<f64>().is_ok(), "{entry}");
            format!("{name}{}", description.replace('"', " ").trim_end())
        })
        .collect()
}

#[test]
fn what_is_rendered_says_how_long_it_took() {
    let state = server();
    let entries = |state: &ServerState, path: &str| {
        let reply = get(state, path, "timing");
        timing_entries(header(&reply, "server-timing").expect(path))
    };
    // A page: the render, and the two parts of it.
    assert_eq!(entries(&state, "/"), ["render", "template", "layout"]);
    // Sections that are asked for alone have neither.
    assert_eq!(entries(&state, "/?section_id=footer"), ["render"]);
    assert_eq!(entries(&state, "/?sections=footer"), ["render"]);
    assert_eq!(entries(&state, "/robots.txt"), ["render"]);
    // What is not rendered says nothing of a render.
    let cart = get(&state, "/cart.js", "timing");
    assert_eq!(header(&cart, "server-timing"), None);

    // With `--timings`, every section in the order they were rendered: the template's, then
    // the layout's. Each one by its type and its id on the page, and followed by its theme
    // blocks: a dash for each level, the type, then the key when it is another word.
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
    let app = App::open(&theme, None, Revalidate::Never, &[]).expect("fixture theme");
    let (naming, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: true,
            customer: None,
            checkout: None,
        },
    );
    let of_template = |key: &str| lsf_core::render::template_section_id("index", key);
    let of_group = lsf_core::tags::group_id_prefix("header-group");
    assert_eq!(
        entries(&naming, "/"),
        [
            "render".to_string(),
            "template".to_string(),
            "layout".to_string(),
            format!("section hero {}", of_template("hero")),
            "block - _title title".to_string(),
            "block - text intro".to_string(),
            "block - group".to_string(),
            "block - - text nested".to_string(),
            format!("section announcement {}", of_template("second")),
            format!("section announcement {of_group}announcement"),
            "section footer".to_string(),
        ]
    );
    assert_eq!(
        entries(&naming, "/?section_id=footer"),
        ["render", "section footer"]
    );
}

#[test]
fn the_control_api_adds_up_what_was_rendered() {
    let state = server();
    let report = || body_json(&get(&state, "/__lsf/timings", "timing"));
    let nothing = json!({ "templates": [], "sections": [], "blocks": [] });
    assert_eq!(report(), nothing);

    get(&state, "/", "timing");
    get(&state, "/", "timing");
    get(&state, "/products/ceramic-mug", "timing");
    get(&state, "/?section_id=footer", "timing");
    // The sections a cart response brings along count as well.
    let added = post(
        &state,
        "/cart/add.js",
        json!({"id": variant_id(&state, "TOTE-NAT"), "sections": "footer", "sections_url": "/"}),
        "timing",
    );
    assert_eq!(added.status, 200, "{}", text(&added));

    let report = report();
    let entries = |list: &str| report[list].as_array().expect("a list").clone();
    let find = |list: &str, key: &str, value: &str| {
        entries(list)
            .into_iter()
            .find(|entry| entry[key] == value)
            .unwrap_or_else(|| panic!("no {value} in {report}"))
    };
    // Pages by template: sections asked for alone are not pages.
    assert_eq!(find("templates", "template", "index")["count"], 2);
    assert_eq!(find("templates", "template", "product")["count"], 1);
    assert_eq!(entries("templates").len(), 2);
    // Sections by id, with their type.
    let hero = find(
        "sections",
        "id",
        &lsf_core::render::template_section_id("index", "hero"),
    );
    assert_eq!((&hero["type"], &hero["count"]), (&json!("hero"), &json!(2)));
    assert_eq!(find("sections", "id", "footer")["count"], 5);
    // Theme blocks by id, with their type and their section.
    let title = find("blocks", "type", "_title");
    assert_eq!(
        (&title["section"], &title["count"]),
        (&hero["id"], &json!(2))
    );
    assert!(
        title["id"].as_str().unwrap().ends_with("__title"),
        "{title}"
    );
    assert_eq!(entries("blocks").len(), 4);

    // Milliseconds that hold together, and what took the most time first.
    for list in ["templates", "sections", "blocks"] {
        let figure = |entry: &Json, name: &str| entry[name].as_f64().expect(name);
        for entry in entries(list) {
            let of = |name: &str| figure(&entry, name);
            assert!(of("min") >= 0.0 && of("total") > 0.0, "{entry}");
            assert!(of("min") <= of("p50") && of("p50") <= of("p95"), "{entry}");
            assert!(
                of("p95") <= of("max") && of("max") <= of("total"),
                "{entry}"
            );
            assert!(
                of("min") <= of("mean") && of("mean") <= of("max"),
                "{entry}"
            );
        }
        let totals: Vec<f64> = entries(list)
            .iter()
            .map(|entry| figure(entry, "total"))
            .collect();
        assert!(totals.is_sorted_by(|a, b| a >= b), "{totals:?}");
    }

    // Forgotten on demand, to measure from a known point.
    let cleared = state.dispatch(&request("DELETE", "/__lsf/timings", json!({}), "timing"));
    assert_eq!(body_json(&cleared), json!({ "ok": true }));
    assert_eq!(body_json(&get(&state, "/__lsf/timings", "timing")), nothing);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_response_says_how_long_the_server_took() {
    let (address, server) = serve_fixture(true).await;
    let entries = |head: &str| timing_entries(head_value(head, "server-timing").expect(head));

    let (head, _) = http_request(address, "/", &[]).await;
    assert_eq!(
        entries(&head),
        ["processing", "render", "template", "layout"]
    );
    // Compressing is named when it was done.
    let (head, _) = http_request(address, "/", &[("accept-encoding", "br")]).await;
    assert_eq!(
        entries(&head),
        ["processing", "render", "template", "layout", "compress"]
    );
    // What is not rendered only says the whole.
    for path in ["/cdn/shop/t/1/assets/base.css", "/cart.js", "/__lsf/status"] {
        let (head, _) = http_request(address, path, &[]).await;
        assert_eq!(entries(&head), ["processing"], "{path}");
    }
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_throttle_is_named_and_is_not_time_the_server_took() {
    use axum::extract::State;

    let state = std::sync::Arc::new(server_with_throttle("page=200ms"));
    let request = axum::http::Request::builder()
        .uri("/")
        .header("host", "shop.test")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = super::handle(State(state), request).await;
    let header = |name: &str| response.headers()[name].to_str().unwrap().to_string();
    assert_eq!(header("x-lsf-throttle"), "200ms");
    let timing = header("server-timing");
    assert_eq!(
        timing_entries(&timing),
        ["processing", "render", "template", "layout", "throttle"]
    );
    assert!(timing.ends_with(", throttle;dur=200"), "{timing}");
    let processing: f64 = timing
        .strip_prefix("processing;dur=")
        .and_then(|rest| rest.split(',').next())
        .and_then(|duration| duration.parse().ok())
        .expect("a duration");
    assert!(processing < 200.0, "{timing}");
}

#[test]
fn the_control_api_profiles_a_page() {
    let state = server();
    let profile = |query: &str| get(&state, &format!("/__lsf/profile{query}"), "profiling");

    // Text for a person: what was measured, then the tree, then the slowest frames.
    let reply = profile("?path=/&runs=3");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(
        header(&reply, "content-type"),
        Some("text/plain; charset=utf-8")
    );
    let report = text(&reply);
    let mut lines = report.lines();
    let first = lines.next().unwrap();
    assert!(
        first.starts_with("/ \u{b7} template index \u{b7} "),
        "{report}"
    );
    assert!(
        lines
            .next()
            .unwrap()
            .starts_with("The render in the middle of 3, "),
        "{report}"
    );
    let names: Vec<&str> = report
        .lines()
        .skip_while(|line| !line.starts_with("   total"))
        .skip(1)
        .take_while(|line| !line.is_empty())
        .map(|line| line.split_at(26).1)
        .collect();
    let hero = lsf_core::render::template_section_id("index", "hero");
    // The slowest first under each row: the template or the layout, whichever took longer.
    assert_eq!(names[0], "render", "{report}");
    assert!(names.contains(&"  templates/index.json"), "{report}");
    assert!(
        names.contains(&format!("    section {hero}").as_str()),
        "{report}"
    );
    assert!(names.contains(&"      sections/hero"), "{report}");
    assert!(names.contains(&"  layout/theme"), "{report}");
    assert!(report.contains("\nSlowest on their own:\n"), "{report}");

    // Every row and every line, on demand.
    let detailed = text(&profile("?path=/&runs=1&all=1&lines=1"));
    assert!(detailed.contains("blocks/text"), "{detailed}");
    assert!(detailed.contains(" sections/hero:2\n"), "{detailed}");
    assert!(detailed.contains("Timing every line"), "{detailed}");
    assert!(!report.contains("Timing every line"), "{report}");

    // The file of a flame graph, which the page of speedscope may fetch.
    let reply = profile("?path=/products/ceramic-mug&format=speedscope&runs=1");
    assert_eq!(header(&reply, "access-control-allow-origin"), Some("*"));
    let exported = body_json(&reply);
    assert_eq!(
        exported["profiles"][0]["name"],
        "/products/ceramic-mug (time)"
    );
    assert_eq!(exported["activeProfileIndex"], 0);
    assert_eq!(exported["shared"]["frames"][0]["name"], "render");
    let frames = exported["shared"]["frames"].as_array().unwrap();
    assert!(
        frames
            .iter()
            .any(|frame| frame["file"] == "sections/main-product.liquid"),
        "{frames:?}"
    );
    let events = exported["profiles"][0]["events"].as_array().unwrap();
    assert_eq!(events.len() % 2, 0);
    assert!(events.len() >= 2 * frames.len());

    // One section alone, and what a profile needs to be told.
    let footer = text(&profile("?path=/&section_id=footer&runs=1&all=1"));
    assert!(footer.contains("  section footer\n"), "{footer}");
    assert!(!footer.contains("layout/theme"), "{footer}");
    let missing = profile("");
    assert_eq!(missing.status, 400);
    assert!(
        text(&missing).contains("/__lsf/profile?path="),
        "{}",
        text(&missing)
    );

    // The status page leads to the profiles of every page.
    let dashboard = text(&get(&state, "/__lsf", "profiling"));
    for link in [
        "\">time</a>",
        "&amp;points=1\">points</a>",
        "&amp;html=1\">flame graph</a>",
    ] {
        let href = format!("href=\"/__lsf/profile?path=%2Fproducts%2Fceramic%2Dmug{link}");
        assert!(dashboard.contains(&href), "{href}\n{dashboard}");
    }
}

#[test]
fn a_profile_is_also_read_in_points() {
    let state = server();
    let profile = |query: &str| get(&state, &format!("/__lsf/profile{query}"), "points");
    // The row of a kind in what the points are made of: points, count, each.
    let row = |report: &str, kind: &str| -> Vec<u64> {
        let made_of = report
            .split("What the points are made of:")
            .nth(1)
            .expect(report);
        let line = made_of
            .lines()
            .find(|line| line.split_whitespace().nth(3) == Some(kind))
            .unwrap_or_else(|| panic!("no {kind} in {report}"));
        line.split_whitespace()
            .take(3)
            .map(|figure| figure.parse().unwrap())
            .collect()
    };

    let report = text(&profile("?path=/collections/all&points=1&runs=1"));
    let first = report.lines().next().unwrap();
    assert!(
        first.starts_with("/collections/all \u{b7} template collection \u{b7} ")
            && first.ends_with(" points"),
        "{report}"
    );
    let total: u64 = first.split(' ').rev().nth(1).unwrap().parse().unwrap();
    assert!(report.contains("\nCostliest on their own:\n"), "{report}");
    assert_eq!(row(&report, "product"), [800, 8, 100]);
    assert_eq!(row(&report, "collection"), [100, 1, 100]);
    let liquid = row(&report, "liquid");
    assert_eq!(liquid[0] + 900, total, "{report}");
    // The tree is in points too: the render as a whole first.
    let tree = report.split("   total      own  calls\n").nth(1).unwrap();
    assert!(
        tree.starts_with(&format!("{total:>8} "))
            && tree.lines().next().unwrap().ends_with("  render"),
        "{report}"
    );
    // The same at every render, unlike the time.
    assert_eq!(
        text(&profile("?path=/collections/all&points=1&runs=3")),
        report
    );
    // Without `points`, nothing of them.
    let time = text(&profile("?path=/collections/all&runs=1"));
    assert!(!time.contains("points"), "{time}");

    // Other costs for the kinds that are named.
    let cheap = text(&profile(
        "?path=/collections/all&points=1&runs=1&cost=product=5,liquid=0",
    ));
    assert_eq!(row(&cheap, "product"), [40, 8, 5]);
    assert_eq!(row(&cheap, "liquid"), [0, liquid[1], 0]);
    assert!(
        cheap.starts_with("/collections/all \u{b7} template collection \u{b7} 140 points\n"),
        "{cheap}"
    );
    let wrong = profile("?path=/&points=1&cost=products=5");
    assert_eq!(wrong.status, 400);
    assert!(
        text(&wrong)
            .starts_with("cost: \"products\" is not a kind of cost. Kinds: liquid, product, "),
        "{}",
        text(&wrong)
    );

    // The file of the flame graph holds both, and shows the one that is asked for.
    let exported = body_json(&profile(
        "?path=/collections/all&points=1&format=speedscope&runs=1",
    ));
    assert_eq!(exported["activeProfileIndex"], 1);
    let points = &exported["profiles"][1];
    assert_eq!(points["name"], "/collections/all (points)");
    assert_eq!(points["unit"], "none");
    assert_eq!(points["endValue"], total);
}

#[test]
fn a_profile_is_drawn_as_a_flame_graph() {
    let state = server();
    // `html=1` answers with the page of the viewer, at the address that was asked for: its
    // parameters can be changed in place. The page tells the viewer where to fetch the
    // profile: the same request, in the format of the viewer.
    let reply = get(
        &state,
        "/__lsf/profile?path=/collections/all&points=1&cost=product=5&html=1",
        "flame",
    );
    assert_eq!(reply.status, 200);
    assert_eq!(header(&reply, "location"), None);
    assert_eq!(
        header(&reply, "content-type"),
        Some("text/html; charset=utf-8")
    );
    let html = text(&reply);
    let profile_url =
        "/__lsf/profile?path=%2Fcollections%2Fall&points=1&cost=product%3D5&format=speedscope";
    assert!(
        html.contains(&format!("encodeURIComponent(\"{profile_url}\")")),
        "{html}"
    );
    let exported = body_json(&get(&state, profile_url, "flame"));
    assert_eq!(exported["activeProfileIndex"], 1);
    assert_eq!(exported["profiles"].as_array().unwrap().len(), 2);

    // The viewer is served from the binary: every file the page names.
    let named: Vec<&str> = html
        .split('"')
        .filter(|part| part.starts_with("/__lsf/speedscope/"))
        .collect();
    assert_eq!(named.len(), 5, "{html}");
    for file in named {
        let reply = get(&state, file, "flame");
        assert_eq!(reply.status, 200, "{file}");
        assert!(!reply.body.is_empty(), "{file}");
        assert_eq!(
            header(&reply, "cache-control"),
            Some("public, max-age=31536000"),
            "{file}"
        );
    }
    let script = get(&state, "/__lsf/speedscope/speedscope.6f107512.js", "flame");
    assert_eq!(
        header(&script, "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    // What the script loads when it reads a profile, and its font.
    for file in [
        "import.bcbb2033.js",
        "SourceCodePro-Regular.ttf.f546cbe0.woff2",
    ] {
        let reply = get(&state, &format!("/__lsf/speedscope/{file}"), "flame");
        assert_eq!(reply.status, 200, "{file}");
    }
    // Its licenses go where it goes.
    for file in ["LICENSE", "source-code-pro.LICENSE.md"] {
        let reply = get(&state, &format!("/__lsf/speedscope/{file}"), "flame");
        assert_eq!(reply.status, 200, "{file}");
        assert_eq!(
            header(&reply, "content-type"),
            Some("text/plain; charset=utf-8")
        );
    }
    assert!(
        text(&get(&state, "/__lsf/speedscope/LICENSE", "flame")).contains("Jamie Wong"),
        "the license of speedscope"
    );
    assert_eq!(
        get(&state, "/__lsf/speedscope/nope.js", "flame").status,
        404
    );
}

// --- The checkout of a Shopify app ---

fn fixture_app() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../checkout/tests/fixtures/app")
}

/// A server whose checkout runs the fixture app of `lsf-checkout`.
fn checkout_server(extensions: &[&str], standalone: bool) -> ServerState {
    checkout_server_of(fixture_app(), extensions, standalone)
}

/// A server whose checkout runs the app in `app_dir`.
fn checkout_server_of(
    app_dir: std::path::PathBuf,
    extensions: &[&str],
    standalone: bool,
) -> ServerState {
    checkout_server_with_data(app_dir, extensions, standalone, None)
}

/// A server whose checkout runs the app in `app_dir`, with the store data in `data` (the
/// built-in demo store without).
fn checkout_server_with_data(
    app_dir: std::path::PathBuf,
    extensions: &[&str],
    standalone: bool,
    data: Option<&Path>,
) -> ServerState {
    let options = |app: App| {
        ServerState::new(
            app,
            ServeOptions {
                live_reload: false,
                watch: false,
                quiet: true,
                compress: true,
                minify: true,
                throttle: Default::default(),
                timings: false,
                customer: None,
                checkout: Some(crate::checkout::CheckoutOptions {
                    app_dir,
                    extensions: extensions.iter().map(|handle| handle.to_string()).collect(),
                    variant: crate::checkout::Variant::Standard,
                    standalone,
                }),
            },
        )
        .0
    };
    if standalone {
        options(App::open_without_theme(Path::new("."), data, &[]).expect("no theme"))
    } else {
        let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
        options(App::open(&theme, data, Revalidate::Never, &[]).expect("fixture theme"))
    }
}

/// A data directory of the demo store with `checkouts/`: the scenarios as (file, content).
struct ScenarioData(std::path::PathBuf);

impl ScenarioData {
    fn new(label: &str, scenarios: &[(&str, &str)]) -> ScenarioData {
        let directory = std::env::temp_dir().join(format!(
            "lsf-server-scenarios-{label}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&directory).ok();
        for (name, content) in lsf_core::store::demo::FILES {
            let path = directory.join(name);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
            std::fs::write(path, content).expect("demo file");
        }
        std::fs::create_dir_all(directory.join("checkouts")).expect("checkouts");
        for (name, content) in scenarios {
            std::fs::write(directory.join("checkouts").join(name), content).expect("scenario");
        }
        ScenarioData(directory)
    }

    fn server(&self, extensions: &[&str], standalone: bool) -> ServerState {
        checkout_server_with_data(fixture_app(), extensions, standalone, Some(&self.0))
    }
}

impl Drop for ScenarioData {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// The session a reply set the cookie of.
fn session_cookie(reply: &Reply) -> String {
    let cookie = cookies(reply)
        .into_iter()
        .find(|cookie| cookie.starts_with("_lsf_session="))
        .unwrap_or_else(|| panic!("no session cookie in {:?}", reply.headers));
    cookie["_lsf_session=".len()..]
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

/// Puts `session` in a cart of `items` (SKU, quantity) and the store data `data` on top of the
/// files, then returns the path of its checkout.
fn start_checkout(state: &ServerState, session: &str, items: Json, data: Json) -> String {
    let reply = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": items}, "data": data}),
        session,
    ));
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let reply = get(state, "/checkout", session);
    assert_eq!(reply.status, 302);
    header(&reply, "location").expect("a location").to_string()
}

fn operations(state: &ServerState, path: &str, session: &str, operations: Json) -> Reply {
    post(
        state,
        &format!("{path}/update"),
        json!({ "operations": operations }),
        session,
    )
}

/// The email and address that make a checkout complete.
fn buyer_details() -> Json {
    json!([
        {"type": "email", "value": "buyer@example.com"},
        {"type": "shippingAddress", "address": {
            "firstName": "Ada", "lastName": "Lovelace", "address1": "1 Analytical Way",
            "city": "London", "countryCode": "gb"
        }},
        {"type": "paymentMethod", "handle": "card"}
    ])
}

#[test]
fn the_checkout_page_is_a_shell_with_the_state_in_it() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-shell",
        json!([{"variant": "TEE-BLK-M", "quantity": 2}]),
        json!({}),
    );
    assert!(path.starts_with("/checkouts/cn/c1-"), "{path}");

    let page = get(&state, &path, "co-shell");
    assert_eq!(page.status, 200);
    let html = text(&page);
    assert!(
        html.contains("src=\"/__lsf/checkout/assets/checkout.js\""),
        "{html}"
    );
    assert!(!html.contains("<!--LSF_"), "{html}");
    let start = html.find("id=\"lsf-checkout-state\">").expect("the state") + 24;
    let end = start + html[start..].find("</script>").expect("its end");
    let state_json: Json = serde_json::from_str(&html[start..end]).expect("JSON");
    assert_eq!(state_json["status"], "open");
    assert_eq!(state_json["variant"], "standard");
    assert_eq!(state_json["lines"][0]["quantity"], 2);

    // The page asks for another look, and keeps it.
    let shop = get(&state, &format!("{path}?variant=shop"), "co-shell");
    assert!(text(&shop).contains("\"variant\":\"shop\""));
    let json = body_json(&get(&state, &format!("{path}.json"), "co-shell"));
    assert_eq!(json["variant"], "shop");

    // Nobody else's cart is behind this token.
    assert_eq!(get(&state, &path, "co-other").status, 404);
    assert_eq!(get(&state, "/checkouts/cn/nope", "co-shell").status, 404);
}

#[test]
fn the_state_lists_the_countries_of_the_store() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-countries",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({"localization": {"countries": [
            {"iso_code": "fr"},
            {"iso_code": "BE", "name": "Belgique"}
        ]}}),
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-countries"));
    assert_eq!(
        json["shop"]["countries"],
        json!([
            {"code": "FR", "name": "France"},
            {"code": "BE", "name": "Belgique"}
        ])
    );
    // The page carries them too.
    let html = text(&get(&state, &path, "co-countries"));
    assert!(html.contains("\"countries\":[{\"code\":\"FR\""), "{html}");
}

#[test]
fn the_checkout_prices_the_lines_as_the_cart_does() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-pricing",
        json!([
            {"variant": "TEA", "quantity": 5},
            {"variant": "TEA", "quantity": 4, "selling_plan": "Monthly"},
            {"variant": "TEE-BLK-M", "quantity": 2},
            {"variant": "TEA", "quantity": 2},
        ]),
        json!({
            "products": [{
                "title": "Volume Tea",
                "price": 3200,
                "compare_at_price": 4000,
                "variants": [{
                    "sku": "TEA",
                    "quantity_price_breaks": [{"minimum_quantity": 3, "price": 2500}]
                }],
                "selling_plan_groups": ["Tea club"]
            }],
            "selling_plan_groups": [{
                "name": "Tea club",
                "selling_plans": [{
                    "name": "Monthly",
                    "price_adjustments": [{"value_type": "fixed_amount", "value": 500}]
                }]
            }]
        }),
    );
    let cart = body_json(&get(&state, "/cart.js", "co-pricing"));
    let checkout = body_json(&get(&state, &format!("{path}.json"), "co-pricing"));

    // The volume price, the price of the plan (which the break does not lower), the price of
    // the variant, and the quantity below the break.
    let prices: Vec<i64> = cart["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["price"].as_i64().unwrap())
        .collect();
    assert_eq!(prices[0..2], [2500, 2700]);
    assert_eq!(prices[3], 3200);

    let lines = checkout["lines"].as_array().unwrap();
    assert_eq!(lines.len(), cart["items"].as_array().unwrap().len());
    for (line, item) in lines.iter().zip(cart["items"].as_array().unwrap()) {
        let cents = |amount: &Json| (amount["amount"].as_f64().unwrap() * 100.0).round() as i64;
        assert_eq!(line["sku"], item["sku"]);
        assert_eq!(line["quantity"], item["quantity"]);
        assert_eq!(cents(&line["unitPrice"]), item["price"], "{}", item["sku"]);
        assert_eq!(cents(&line["subtotal"]), item["line_price"]);
        assert_eq!(cents(&line["total"]), item["final_line_price"]);
        assert_eq!(
            line.get("sellingPlan").is_some_and(|plan| !plan.is_null()),
            item.get("selling_plan_allocation").is_some()
        );
    }
    assert_eq!(
        (checkout["cost"]["subtotal"]["amount"].as_f64().unwrap() * 100.0).round() as i64,
        cart["items_subtotal_price"].as_i64().unwrap()
    );
}

#[test]
fn the_checkout_charges_what_the_cart_says_is_due_at_checkout() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-deposit",
        json!([
            {"variant": "PRE", "quantity": 2, "selling_plan": "Pre-order"},
            {"variant": "TEE-BLK-M", "quantity": 1},
        ]),
        json!({
            "products": [{
                "title": "Pre-order Tea",
                "price": 4000,
                "variants": [{"sku": "PRE"}],
                "selling_plan_groups": ["Pre-order"]
            }],
            "selling_plan_groups": [{
                "name": "Pre-order",
                "selling_plans": [{
                    "name": "Pre-order",
                    "checkout_charge": {"value_type": "percentage", "value": 25}
                }]
            }],
            "checkout": {"shipping_rates": [{"handle": "standard", "title": "Standard", "price": "5.00"}], "tax_rate": 10}
        }),
    );
    let cents = |amount: &Json| (amount["amount"].as_f64().unwrap() * 100.0).round() as i64;
    let cart = body_json(&get(&state, "/cart.js", "co-deposit"));
    let json = body_json(&get(&state, &format!("{path}.json"), "co-deposit"));
    let cost = &json["cost"];

    // 2 × 40.00 with a 25% deposit: 20.00 now and 60.00 later; the tee is paid now.
    // What the cart says is due at checkout (`cart.checkout_charge_amount` of Liquid, which
    // /cart.js does not carry): 25% of 2 × 40.00 and the whole price of the tee.
    let charged: i64 = 2000 + cart["items"][1]["line_price"].as_i64().unwrap();
    assert_eq!(charged, 2000 + 3200);
    assert_eq!(
        cents(&cost["checkoutChargeAmount"]),
        charged,
        "the checkout charge of the cart"
    );
    assert_eq!(cents(&cost["subtotal"]), 8000 + 3200);
    assert_eq!(cents(&json["lines"][0]["dueLater"]), 6000);
    assert_eq!(json["lines"][1].get("dueLater"), None);
    // The whole order costs its whole price; the tax follows what it is on.
    assert_eq!(cents(&cost["tax"]), 1120);
    assert_eq!(cents(&cost["total"]), 11200 + 500 + 1120);
    assert_eq!(cents(&cost["dueLater"]), 6000 + 600);
    assert_eq!(cents(&cost["dueNow"]), 2000 + 3200 + 500 + 520);
    assert_eq!(json["lines"][0]["sellingPlan"]["name"], "Pre-order");

    // The order records the balance, and the thank-you page shows it.
    let reply = operations(&state, &path, "co-deposit", buyer_details());
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-deposit");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let thanks = page_state(&text(&get(
        &state,
        &format!("{path}/thank-you"),
        "co-deposit",
    )));
    assert_eq!(thanks["status"], "completed");
    assert_eq!(cents(&thanks["order"]["totalOutstanding"]), 6600);
    assert_eq!(cents(&thanks["order"]["totalReceived"]), 6220);
    assert_eq!(cents(&thanks["cost"]["total"]), 12820);
    let received = cents(&thanks["order"]["totalReceived"]);
    let outstanding = cents(&thanks["order"]["totalOutstanding"]);
    assert_eq!(received + outstanding, cents(&thanks["cost"]["total"]));
}

#[test]
fn an_order_without_a_deferred_payment_has_no_balance() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-no-balance",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({}),
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-no-balance"));
    assert_eq!(json["cost"]["dueLater"]["amount"], 0.0);
    assert_eq!(json["cost"]["dueNow"], json["cost"]["total"]);
    operations(&state, &path, "co-no-balance", buyer_details());
    post(
        &state,
        &format!("{path}/complete"),
        json!({}),
        "co-no-balance",
    );
    let thanks = page_state(&text(&get(
        &state,
        &format!("{path}/thank-you"),
        "co-no-balance",
    )));
    assert_eq!(thanks["order"]["totalOutstanding"]["amount"], 0.0);
    assert_eq!(thanks["order"]["totalReceived"], thanks["cost"]["total"]);
}

#[test]
fn the_state_describes_the_checkout() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-state",
        json!([{"variant": "TEE-BLK-M", "quantity": 2}]),
        json!({"checkout": {
            "shipping_rates": [
                {"handle": "standard", "title": "Standard", "price": "4.90"},
                {"handle": "express", "title": "Express", "price": "12.90"}
            ],
            "tax_rate": 20
        }}),
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-state"));
    assert_eq!(json["token"], path.rsplit('/').next().unwrap());
    assert_eq!(json["shop"]["currencyCode"], "USD");
    assert_eq!(json["shop"]["storefrontUrl"], "http://shop.test");
    assert_eq!(json["buyer"]["email"], Json::Null);
    assert_eq!(json["lines"][0]["id"], "gid://shopify/CartLine/1");
    assert_eq!(json["lines"][0]["sku"], "TEE-BLK-M");
    let group = &json["deliveryGroups"][0];
    assert_eq!(group["handle"], "group-1");
    assert_eq!(group["selected"], "standard");
    assert_eq!(group["options"][1]["cost"]["amount"], 12.9);
    let methods: Vec<&str> = json["paymentMethods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|method| method["handle"].as_str().unwrap())
        .collect();
    assert_eq!(methods, ["card", "cod", "bank"]);
    let subtotal = json["cost"]["subtotal"]["amount"].as_f64().unwrap();
    assert_eq!(json["cost"]["shipping"]["amount"], 4.9);
    let total = json["cost"]["total"]["amount"].as_f64().unwrap();
    assert!((total - (subtotal * 1.2 + 4.9)).abs() < 0.02, "{total}");

    // The built extensions of the app, with where they render.
    let extensions = json["extensions"].as_array().unwrap();
    let handles: Vec<&str> = extensions
        .iter()
        .map(|extension| extension["handle"].as_str().unwrap())
        .collect();
    assert_eq!(handles, ["gift-message", "order-notice"]);
    assert_eq!(
        extensions[0]["script"],
        "/__lsf/extensions/gift-message/gift-message.js"
    );
    assert!(
        extensions[0]["targets"]
            .as_array()
            .unwrap()
            .contains(&json!("purchase.checkout.block.render"))
    );
    // The manifest declares the settings; nothing sets them.
    assert_eq!(extensions[0]["settings"], json!({}));
    assert_eq!(extensions[0]["capabilities"]["block_progress"], true);
    // The metafields they declare: `owner_type` is `ownerType`, absent when not written.
    assert_eq!(extensions[0]["apiVersion"], "2026-10");
    assert_eq!(
        extensions[0]["metafields"],
        json!([{"namespace": "$app:gift", "key": "wrapped"}])
    );
    assert_eq!(extensions[1]["apiVersion"], "2027-01");
    assert_eq!(
        extensions[1]["metafields"],
        json!([{"namespace": "$app:notice", "key": "dismissed", "ownerType": "CART"}])
    );
    // The fixture keeps one legacy target on purpose: it still runs, with a warning.
    assert_eq!(
        json["diagnostics"],
        json!([
            "quantity-limit: `purchase.validation.run` is a legacy target: use `cart.validations.generate.run`"
        ])
    );

    // The control API says the same, for a test that is not the browser.
    let control = body_json(&get(&state, "/__lsf/checkout?session=co-state", "other"));
    assert_eq!(control["token"], json["token"]);
}

#[test]
fn an_extension_on_an_unsupported_api_version_is_not_offered() {
    let root =
        std::env::temp_dir().join(format!("lsf-checkout-ui-versions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (handle, version) in [
        ("supported", "api_version = \"2026-10\""),
        ("candidate", "api_version = \"2027-01\""),
        ("old", "api_version = \"2025-07\""),
        ("unversioned", ""),
    ] {
        let dir = root.join("extensions").join(handle);
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join("dist").join(format!("{handle}.js")), "// built").unwrap();
        std::fs::write(
            dir.join("shopify.extension.toml"),
            format!(
                "type = \"ui_extension\"\n{version}\n[[targeting]]\ntarget = \"purchase.checkout.block.render\"\n"
            ),
        )
        .unwrap();
    }
    let state = checkout_server_of(root.clone(), &[], false);
    let path = start_checkout(
        &state,
        "co-ui-versions",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({}),
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-ui-versions"));
    let mut handles: Vec<&str> = json["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|extension| extension["handle"].as_str().unwrap())
        .collect();
    handles.sort();
    assert_eq!(handles, ["candidate", "supported"]);
    let diagnostics: Vec<&str> = json["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    for message in [
        "old: api_version \"2025-07\" is not supported (supported: 2026-10, 2027-01): the extension is not offered to the checkout page",
        "unversioned: no `api_version` (supported: 2026-10, 2027-01): the extension is not offered to the checkout page",
    ] {
        assert!(diagnostics.contains(&message), "{diagnostics:?}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_metafield_without_owner_type_is_reported_from_2027_01() {
    let root = std::env::temp_dir().join(format!("lsf-checkout-ui-owner-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (handle, version) in [("stable", "2026-10"), ("candidate", "2027-01")] {
        let dir = root.join("extensions").join(handle);
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join("dist").join(format!("{handle}.js")), "// built").unwrap();
        std::fs::write(
            dir.join("shopify.extension.toml"),
            format!(
                "api_version = \"{version}\"\ntype = \"ui_extension\"\n[[targeting]]\ntarget = \"purchase.checkout.block.render\"\n[[metafields]]\nnamespace = \"$app:gift\"\nkey = \"wrapped\"\n"
            ),
        )
        .unwrap();
    }
    let state = checkout_server_of(root.clone(), &[], false);
    let path = start_checkout(
        &state,
        "co-ui-owner",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({}),
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-ui-owner"));
    // Both are offered: the page says why it refuses the second.
    let extensions = json["extensions"].as_array().unwrap();
    assert_eq!(extensions.len(), 2);
    for extension in extensions {
        assert_eq!(
            extension["metafields"],
            json!([{"namespace": "$app:gift", "key": "wrapped"}])
        );
    }
    let diagnostics: Vec<&str> = json["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert!(
        diagnostics.contains(&"candidate: API version 2027-01 requires `owner_type` on every metafield declaration: `$app:gift.wrapped` has none"),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| !d.starts_with("stable:") || !d.contains("owner_type")),
        "{diagnostics:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn only_the_extensions_asked_for_are_used() {
    let state = checkout_server(&["order-notice"], false);
    let path = start_checkout(
        &state,
        "co-some",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({}),
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-some"));
    let handles: Vec<&str> = json["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|extension| extension["handle"].as_str().unwrap())
        .collect();
    assert_eq!(handles, ["order-notice"]);
    // A bundle that is not used is not served.
    assert_eq!(
        get(
            &state,
            "/__lsf/extensions/gift-message/gift-message.js",
            "co-some"
        )
        .status,
        404
    );
}

/// The state JSON the page of a checkout carries.
fn page_state(html: &str) -> Json {
    let start = html.find("id=\"lsf-checkout-state\">").expect("the state") + 24;
    let end = start + html[start..].find("</script>").expect("its end");
    serde_json::from_str(&html[start..end]).expect("JSON")
}

#[test]
fn the_language_of_the_buyer_stays_through_the_whole_checkout() {
    let state = checkout_server(&[], false);
    state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}),
        "co-fr",
    ));

    // The cart form of the theme leads to `/fr/checkout`: the checkout is in French.
    let reply = get(&state, "/fr/checkout", "co-fr");
    assert_eq!(reply.status, 302);
    let path = header(&reply, "location").expect("a location").to_string();
    let bare = path.strip_prefix("/fr").expect("under the French root");
    assert!(bare.starts_with("/checkouts/cn/c1-"), "{path}");
    // Asking for the other look keeps the language.
    let shop = get(&state, "/fr/checkout?variant=shop", "co-fr");
    assert_eq!(
        header(&shop, "location"),
        Some(format!("{path}?variant=shop").as_str())
    );

    // The page and its state.
    let page = get(&state, &path, "co-fr");
    assert_eq!(page.status, 200);
    let html = text(&page);
    assert!(html.contains("<html lang=\"fr\">"), "{html}");
    let json = page_state(&html);
    assert_eq!(json["localization"]["language"]["isoCode"], "fr");
    assert_eq!(json["localization"]["rootUrl"], "/fr");
    assert_eq!(
        json["shop"]["locale"], "en",
        "the primary language of the shop"
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-fr"));
    assert_eq!(json["localization"]["language"]["isoCode"], "fr");

    // An update, the way the page sends it (under the root of its language).
    let reply = operations(&state, &path, "co-fr", buyer_details());
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let json = body_json(&reply);
    assert_eq!(json["localization"]["language"]["isoCode"], "fr");
    assert_eq!(json["localization"]["country"]["isoCode"], "GB");
    assert_eq!(json["shop"]["locale"], "en");

    // The order, and the way to its thank-you page.
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-fr");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(body_json(&reply)["redirect"], format!("{path}/thank-you"));
    let thanks = get(&state, &format!("{path}/thank-you"), "co-fr");
    assert_eq!(thanks.status, 200);
    let html = text(&thanks);
    assert!(html.contains("<html lang=\"fr\">"), "{html}");
    let json = page_state(&html);
    assert_eq!(json["status"], "completed");
    assert_eq!(json["localization"]["language"]["isoCode"], "fr");

    // The language is the one of the URL: the same checkout without the root is in the primary
    // language, and a checkout of the primary language leads to its own thank-you page.
    let json = body_json(&get(&state, &format!("{}.json", bare), "co-fr"));
    assert_eq!(json["localization"]["language"]["isoCode"], "en");
    assert_eq!(json["localization"]["rootUrl"], "");
    // So is its thank-you page, though the order was placed in French.
    let thanks = get(&state, &format!("{bare}/thank-you"), "co-fr");
    assert_eq!(thanks.status, 200);
    let html = text(&thanks);
    assert!(html.contains("<html lang=\"en\">"), "{html}");
    let json = page_state(&html);
    assert_eq!(json["status"], "completed");
    assert_eq!(json["localization"]["language"]["isoCode"], "en");
    assert_eq!(json["localization"]["rootUrl"], "");
    let again = start_checkout(
        &state,
        "co-fr",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({}),
    );
    assert!(again.starts_with("/checkouts/cn/"), "{again}");
    assert_eq!(
        operations(&state, &again, "co-fr", buyer_details()).status,
        200
    );
    let reply = post(&state, &format!("{again}/complete"), json!({}), "co-fr");
    assert_eq!(body_json(&reply)["redirect"], format!("{again}/thank-you"));

    // A thank-you page without an order leads back to the checkout in its language.
    state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}}),
        "co-fr-2",
    ));
    let path = header(&get(&state, "/fr/checkout", "co-fr-2"), "location")
        .expect("a location")
        .to_string();
    let reply = get(&state, &format!("{path}/thank-you"), "co-fr-2");
    assert_eq!(
        (reply.status, header(&reply, "location")),
        (302, Some(path.as_str()))
    );

    // The control API has no prefix to read: it takes the locale in the query.
    let json = body_json(&get(&state, "/__lsf/checkout?locale=fr", "co-fr-2"));
    assert_eq!(json["localization"]["language"]["isoCode"], "fr");
    assert_eq!(json["localization"]["rootUrl"], "/fr");
    let json = body_json(&get(&state, "/__lsf/checkout?locale=xx", "co-fr-2"));
    assert_eq!(json["localization"]["language"]["isoCode"], "en");
}

/// A language that only a session has, added to its store with the control API, is a locale of
/// the checkout like the ones of the data files: `/de/checkout` is the German checkout of the
/// session (and nothing is, for a visitor whose store has no German).
fn an_overlay_locale_is_a_locale_of_the_checkout(standalone: bool) {
    let state = checkout_server(&[], standalone);
    let session = if standalone { "co-de-alone" } else { "co-de" };
    let reply = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({
            "cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]},
            "data": {"localization": {"languages": [{"iso_code": "en"}, {"iso_code": "de"}]}}
        }),
        session,
    ));
    assert_eq!(reply.status, 200, "{}", text(&reply));

    let reply = get(&state, "/de/checkout", session);
    assert_eq!(reply.status, 302, "{}", text(&reply));
    let path = header(&reply, "location").expect("a location").to_string();
    let bare = path.strip_prefix("/de").expect("under the German root");
    assert!(bare.starts_with("/checkouts/cn/"), "{path}");

    let page = get(&state, &path, session);
    assert_eq!(page.status, 200, "{}", text(&page));
    let html = text(&page);
    assert!(html.contains("<html lang=\"de\">"), "{html}");
    assert_eq!(page_state(&html)["localization"]["rootUrl"], "/de");
    let json = body_json(&get(&state, &format!("{path}.json"), session));
    assert_eq!(json["localization"]["language"]["isoCode"], "de");

    let reply = operations(&state, &path, session, buyer_details());
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(
        body_json(&reply)["localization"]["language"]["isoCode"],
        "de"
    );
    let reply = post(&state, &format!("{path}/complete"), json!({}), session);
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(body_json(&reply)["redirect"], format!("{path}/thank-you"));
    let thanks = get(&state, &format!("{path}/thank-you"), session);
    assert_eq!(thanks.status, 200);
    assert!(text(&thanks).contains("<html lang=\"de\">"));

    // Another visitor's store has no German.
    let other = get(&state, "/de/checkout", "co-without-de");
    assert_ne!(header(&other, "location"), Some(path.as_str()));
    assert_ne!(
        header(&other, "location").map(|location| location.starts_with("/de/")),
        Some(true)
    );
}

#[test]
fn a_locale_added_to_a_session_is_a_locale_of_its_checkout() {
    an_overlay_locale_is_a_locale_of_the_checkout(false);
}

#[test]
fn a_locale_added_to_a_session_is_a_locale_of_its_standalone_checkout() {
    an_overlay_locale_is_a_locale_of_the_checkout(true);
}

/// Copies a directory, without the `target` of a build.
fn copy_directory(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "target" {
            continue;
        }
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The language the payment Function of `run` was given, as `LanguageCode`.
fn function_language(json: &Json) -> Json {
    let run = json["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["handle"] == "payment-rules")
        .expect("the run of the payment Function");
    assert_eq!(run["error"], Json::Null, "{run}");
    run["input"]["localization"]["language"]["isoCode"].clone()
}

#[test]
fn the_functions_read_the_language_of_the_buyer() {
    // A copy of the fixture app whose payment Function also asks for the language.
    let root =
        std::env::temp_dir().join(format!("lsf-checkout-fn-language-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_directory(&fixture_app(), &root);
    let query = root.join("extensions/payment-rules/src/run.graphql");
    let source = std::fs::read_to_string(&query).unwrap();
    std::fs::write(
        &query,
        source.replacen(
            "query Input {",
            "query Input {\n  localization {\n    language {\n      isoCode\n    }\n  }",
            1,
        ),
    )
    .unwrap();

    let state = checkout_server_of(root.clone(), &["payment-rules"], false);
    state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}}),
        "co-fn-fr",
    ));
    let path = header(&get(&state, "/fr/checkout", "co-fn-fr"), "location")
        .expect("a location")
        .to_string();

    // The page.
    let page = page_state(&text(&get(&state, &path, "co-fn-fr")));
    assert_eq!(function_language(&page), "FR");
    // An update.
    let reply = operations(&state, &path, "co-fn-fr", buyer_details());
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(function_language(&body_json(&reply)), "FR");
    // The order and its thank-you page: the Functions ran in French when it was placed.
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-fn-fr");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let thanks = page_state(&text(&get(
        &state,
        &format!("{path}/thank-you"),
        "co-fn-fr",
    )));
    assert_eq!(thanks["status"], "completed");
    assert_eq!(function_language(&thanks), "FR");

    // Without the root, the Functions are run in the primary language.
    state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}}),
        "co-fn-en",
    ));
    let path = header(&get(&state, "/checkout", "co-fn-en"), "location")
        .expect("a location")
        .to_string();
    let json = body_json(&operations(&state, &path, "co-fn-en", buyer_details()));
    assert_eq!(function_language(&json), "EN");
    let _ = std::fs::remove_dir_all(root);
}

/// A copy of the fixture app whose cart transform merges the first two lines into a bundle in
/// French only: a Function written by hand (the text format of Wasm, which the runner reads as
/// well) that looks for `"FR"` in its input, the `localization.language` it asks for.
fn app_with_a_bundle_in_french(name: &str, parent_variant: u64) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("lsf-checkout-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_directory(&fixture_app(), &root);
    let extension = root.join("extensions/bundle-transform");
    std::fs::write(
        extension.join("src/run.graphql"),
        "query Input {\n  localization {\n    language {\n      isoCode\n    }\n  }\n  cart {\n    lines {\n      id\n      quantity\n    }\n  }\n}\n",
    )
    .unwrap();
    let merge = json!({"operations": [{"linesMerge": {
        "cartLines": [
            {"cartLineId": "gid://shopify/CartLine/1", "quantity": 1},
            {"cartLineId": "gid://shopify/CartLine/2", "quantity": 1}
        ],
        "parentVariantId": format!("gid://shopify/ProductVariant/{parent_variant}"),
        "title": "Set"
    }}]})
    .to_string();
    let nothing = r#"{"operations":[]}"#;
    let text = |json: &str| json.replace('"', "\\\"");
    let module = format!(
        r#"(module
  (import "wasi_snapshot_preview1" "fd_read" (func $fd_read (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 100) "{merge_text}")
  (data (i32.const 700) "{nothing_text}")
  (func (export "run")
    (local $at i32) (local $end i32) (local $out i32) (local $length i32)
    (i32.store (i32.const 0) (i32.const 1024))
    (i32.store (i32.const 4) (i32.const 32768))
    (drop (call $fd_read (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 8)))
    (local.set $at (i32.const 1024))
    (local.set $end (i32.add (i32.const 1024) (i32.load (i32.const 8))))
    (local.set $out (i32.const 700))
    (local.set $length (i32.const {nothing_length}))
    (block $found
      (loop $scan
        (br_if $found (i32.gt_u (i32.add (local.get $at) (i32.const 4)) (local.get $end)))
        ;; The bytes of "FR" with its quotes, read as one number.
        (if (i32.eq (i32.load (local.get $at)) (i32.const 0x22524622))
          (then
            (local.set $out (i32.const 100))
            (local.set $length (i32.const {merge_length}))
            (br $found)))
        (local.set $at (i32.add (local.get $at) (i32.const 1)))
        (br $scan)))
    (i32.store (i32.const 16) (local.get $out))
    (i32.store (i32.const 20) (local.get $length))
    (drop (call $fd_write (i32.const 1) (i32.const 16) (i32.const 1) (i32.const 24)))))
"#,
        merge_text = text(&merge),
        nothing_text = text(nothing),
        merge_length = merge.len(),
        nothing_length = nothing.len(),
    );
    std::fs::write(extension.join("dist/function.wasm"), module).unwrap();
    root
}

/// The checkout of a cart of a T-shirt and a tote, which the cart transform of `root` merges
/// into a bundle in French only.
fn bundle_in_french_checkout(state: &ServerState, session: &str) -> String {
    let reply = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": [
            {"variant": "TEE-BLK-M", "quantity": 1},
            {"variant": "TOTE-NAT", "quantity": 1}
        ]}}),
        session,
    ));
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let path = header(&get(state, "/fr/checkout", session), "location")
        .expect("a location")
        .to_string();
    let json = body_json(&get(state, &format!("{path}.json"), session));
    let lines = json["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 1, "{json}");
    assert_eq!(lines[0]["transformedBy"], "merge");
    assert_eq!(lines[0]["id"], "gid://shopify/CartLine/3");
    path
}

#[test]
fn a_line_the_cart_transform_merges_in_french_is_changed_in_french() {
    let parent = variant_id(&checkout_server(&[], false), "MUG-OAT");
    let root = app_with_a_bundle_in_french("bundle-in-french", parent);
    let state = checkout_server_of(root.clone(), &["bundle-transform"], false);

    // The bundle is a line of the French checkout only.
    let path = bundle_in_french_checkout(&state, "co-fr-update");
    let english = header(&get(&state, "/checkout", "co-fr-update"), "location")
        .expect("a location")
        .to_string();
    let json = body_json(&get(&state, &format!("{english}.json"), "co-fr-update"));
    assert_eq!(json["lines"].as_array().unwrap().len(), 2, "{json}");

    // The change names the line of the French checkout, so the Functions run in French again.
    let reply = change_line(
        &state,
        &path,
        "co-fr-update",
        json!({"type": "updateCartLine", "id": "gid://shopify/CartLine/3", "quantity": 3}),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(
        cart_of(&state, "co-fr-update"),
        [("TEE-BLK-M".to_string(), 3), ("TOTE-NAT".to_string(), 3)]
    );

    let path = bundle_in_french_checkout(&state, "co-fr-remove");
    let reply = change_line(
        &state,
        &path,
        "co-fr-remove",
        json!({"type": "removeCartLine", "id": "gid://shopify/CartLine/3", "quantity": 1}),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(cart_of(&state, "co-fr-remove"), []);
    let _ = std::fs::remove_dir_all(root);
}

/// The part of the state JSON that the Buyer Identity and Localization APIs read, for a
/// logged-in customer of the demo data buying in French. The sandbox of the extensions reads the
/// same file (`crates/checkout/assets/tests/sandbox-api.test.mjs`): when the server writes
/// something else, both fail, so the two sides cannot drift apart. Run with `UPDATE_SNAPSHOTS=1`
/// to write the file again after a change meant to be.
#[test]
fn the_state_gives_extensions_the_buyer_and_the_localization_of_the_store() {
    let state = checkout_server(&[], false);
    let mut seen = serde_json::Map::new();
    for (name, email, location) in [
        ("customer", "jane.doe@example.com", None),
        (
            "company",
            "alex.morgan@example.com",
            Some("Northwind Seattle"),
        ),
    ] {
        // The country the visitor selected on the storefront: the address the customer ships
        // to (their default one, the location's) wins over it.
        let country = if name == "customer" { "CA" } else { "FR" };
        let session = format!("co-buyer-{name}");
        let mut body = json!({
            "customer": email,
            "country": country,
            "cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]},
        });
        if let Some(location) = location {
            body["company_location"] = json!(location);
        }
        let set = state.dispatch(&request("PUT", "/__lsf/session", body, &session));
        assert_eq!(set.status, 200, "{}", text(&set));
        let path = header(&get(&state, "/fr/checkout", &session), "location")
            .expect("a location")
            .to_string();
        let json = body_json(&get(&state, &format!("{path}.json"), &session));
        seen.insert(
            name.into(),
            json!({"buyer": json["buyer"], "localization": json["localization"]}),
        );
    }
    let seen = Json::Object(seen);

    let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../checkout/tests/fixtures/server-buyer-state.json");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&golden, serde_json::to_string_pretty(&seen).unwrap() + "\n").unwrap();
    }
    let expected: Json =
        serde_json::from_str(&std::fs::read_to_string(&golden).expect("the golden file"))
            .expect("JSON");
    assert_eq!(seen, expected);

    // What the sandbox must be able to read, whatever the file says.
    assert_eq!(
        seen["customer"]["buyer"]["customer"]["phone"],
        "+1 555 010 9988"
    );
    assert_eq!(
        seen["customer"]["localization"]["language"]["isoCode"],
        "fr"
    );
    assert_eq!(
        seen["company"]["buyer"]["purchasingCompany"]["company"]["name"],
        "Northwind Hotels"
    );
}

#[test]
fn operations_change_what_the_buyer_chose() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-ops",
        json!([{"variant": "TEE-BLK-M", "quantity": 2}]),
        json!({"checkout": {"shipping_rates": [
            {"handle": "standard", "price": "4.90"},
            {"handle": "express", "price": "12.90"}
        ]}}),
    );
    let reply = operations(
        &state,
        &path,
        "co-ops",
        json!([
            {"type": "email", "value": "buyer@example.com"},
            {"type": "phone", "value": "+33102030405"},
            {"type": "shippingAddress", "address": {"lastName": "Doe", "countryCode": "fr", "city": ""}},
            {"type": "deliveryOption", "group": "group-1", "handle": "express"},
            {"type": "paymentMethod", "handle": "cod"},
            {"type": "discountCode", "action": "add", "code": "SAVE10"},
            {"type": "note", "change": {"type": "updateNote", "note": "Ring twice"}},
            {"type": "attribute", "change": {"type": "updateAttribute", "key": "gift", "value": "yes"}},
            {"type": "metafield", "change": {
                "type": "updateCartMetafield", "namespace": "$app:gift", "key": "message",
                "valueType": "single_line_text_field", "value": "Happy birthday"
            }}
        ]),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let json = body_json(&reply);
    assert_eq!(json["buyer"]["email"], "buyer@example.com");
    assert_eq!(json["buyer"]["phone"], "+33102030405");
    assert_eq!(json["shippingAddress"]["countryCode"], "FR");
    assert_eq!(json["shippingAddress"]["city"], Json::Null);
    assert_eq!(json["deliveryGroups"][0]["selected"], "express");
    assert_eq!(json["selectedPayment"], "cod");
    assert_eq!(json["discountCodes"][0]["code"], "SAVE10");
    assert_eq!(json["discountCodes"][0]["applicable"], false);
    assert_eq!(json["note"], "Ring twice");
    assert_eq!(json["attributes"], json!([{"key": "gift", "value": "yes"}]));
    assert_eq!(json["metafields"][0]["namespace"], "$app:gift");
    assert_eq!(json["metafields"][0]["value"], "Happy birthday");

    // The note and the attribute are the ones of the cart the theme shows.
    let cart = body_json(&get(&state, "/cart.js", "co-ops"));
    assert_eq!(cart["note"], "Ring twice");
    assert_eq!(cart["attributes"]["gift"], "yes");

    // Codes, attributes and metafields can be taken back.
    let reply = operations(
        &state,
        &path,
        "co-ops",
        json!([
            {"type": "discountCode", "action": "remove", "code": "save10"},
            {"type": "note", "change": {"type": "removeNote"}},
            {"type": "attribute", "change": {"type": "removeAttribute", "key": "gift"}},
            {"type": "metafield", "change": {"type": "removeCartMetafield", "namespace": "$app:gift", "key": "message"}},
            {"type": "paymentMethod", "handle": null}
        ]),
    );
    let json = body_json(&reply);
    assert_eq!(json["discountCodes"], json!([]));
    assert_eq!(json["note"], Json::Null);
    assert_eq!(json["attributes"], json!([]));
    assert_eq!(json["metafields"], json!([]));
    assert_eq!(json["selectedPayment"], Json::Null);
}

#[test]
fn operations_change_the_cart_lines() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-lines",
        json!([{"variant": "TEE-BLK-M", "quantity": 2}]),
        json!({}),
    );
    let tote = variant_id(&state, "TOTE-NAT");
    let apply = |change: Json| {
        let reply = operations(
            &state,
            &path,
            "co-lines",
            json!([{"type": "cartLines", "change": change}]),
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        body_json(&reply)
    };
    let quantities = |json: &Json| -> Vec<(String, u64)> {
        json["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| {
                (
                    line["sku"].as_str().unwrap().to_string(),
                    line["quantity"].as_u64().unwrap(),
                )
            })
            .collect()
    };

    let json = apply(json!({
        "type": "addCartLine",
        "merchandiseId": format!("gid://shopify/ProductVariant/{tote}"),
        "quantity": 1,
        "attributes": [{"key": "engraving", "value": "AL"}]
    }));
    assert_eq!(
        quantities(&json),
        [("TEE-BLK-M".to_string(), 2), ("TOTE-NAT".to_string(), 1)]
    );
    assert_eq!(json["lines"][1]["attributes"][0]["key"], "engraving");

    let json =
        apply(json!({"type": "updateCartLine", "id": "gid://shopify/CartLine/1", "quantity": 5}));
    assert_eq!(json["lines"][0]["quantity"], 5);
    let json =
        apply(json!({"type": "removeCartLine", "id": "gid://shopify/CartLine/1", "quantity": 2}));
    assert_eq!(json["lines"][0]["quantity"], 3);
    let json =
        apply(json!({"type": "removeCartLine", "id": "gid://shopify/CartLine/2", "quantity": 1}));
    assert_eq!(quantities(&json), [("TEE-BLK-M".to_string(), 3)]);
    let json =
        apply(json!({"type": "updateCartLine", "id": "gid://shopify/CartLine/1", "quantity": 0}));
    assert_eq!(json["lines"], json!([]));

    // The cart is the one of the theme.
    let cart = body_json(&get(&state, "/cart.js", "co-lines"));
    assert_eq!(cart["item_count"], 0);
}

/// A checkout where the cart transform of the fixture app does what `operations` says.
fn transformed_checkout(
    state: &ServerState,
    session: &str,
    items: Json,
    operations: Json,
) -> String {
    start_checkout(
        state,
        session,
        items,
        json!({"checkout": {"functions": {"bundle-transform": {"metafields": {
            "$app:config.settings": {"type": "json", "value": {"operations": operations}}
        }}}}}),
    )
}

/// The `(sku, quantity)` of the lines of the cart of the theme.
fn cart_of(state: &ServerState, session: &str) -> Vec<(String, u64)> {
    body_json(&get(state, "/cart.js", session))["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["sku"].as_str().unwrap().to_string(),
                item["quantity"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn change_line(state: &ServerState, path: &str, session: &str, change: Json) -> Reply {
    operations(
        state,
        path,
        session,
        json!([{"type": "cartLines", "change": change}]),
    )
}

#[test]
fn a_merged_line_is_removed_through_the_lines_it_is_made_of() {
    let state = checkout_server(&["bundle-transform"], false);
    let path = transformed_checkout(
        &state,
        "co-merge-remove",
        json!([
            {"variant": "TEE-BLK-M", "quantity": 1},
            {"variant": "TOTE-NAT", "quantity": 2},
            {"variant": "MUG-OAT", "quantity": 1}
        ]),
        json!([{"type": "merge", "lines": [1, 2], "parentVariant": variant_id(&state, "MUG-OAT"), "title": "Set"}]),
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-merge-remove"));
    let lines = json["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "{json}");
    assert_eq!(lines[0]["id"], "gid://shopify/CartLine/4");
    assert_eq!(lines[0]["transformedBy"], "merge");
    assert_eq!(lines[1]["id"], "gid://shopify/CartLine/3");

    // The line of the checkout is not a position in the cart.
    let reply = change_line(
        &state,
        &path,
        "co-merge-remove",
        json!({"type": "removeCartLine", "id": "gid://shopify/CartLine/4", "quantity": 1}),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(
        cart_of(&state, "co-merge-remove"),
        [("MUG-OAT".to_string(), 1)]
    );
    let json = body_json(&reply);
    assert_eq!(json["lines"].as_array().unwrap().len(), 1);
    assert_eq!(json["lines"][0]["sku"], "MUG-OAT");
}

#[test]
fn a_merged_line_is_scaled_through_the_lines_it_is_made_of() {
    let state = checkout_server(&["bundle-transform"], false);
    let path = transformed_checkout(
        &state,
        "co-merge-update",
        json!([
            {"variant": "TEE-BLK-M", "quantity": 1},
            {"variant": "TOTE-NAT", "quantity": 2},
            {"variant": "MUG-OAT", "quantity": 1}
        ]),
        json!([{"type": "merge", "lines": [1, 2], "parentVariant": variant_id(&state, "MUG-OAT")}]),
    );
    let reply = change_line(
        &state,
        &path,
        "co-merge-update",
        json!({"type": "updateCartLine", "id": "gid://shopify/CartLine/4", "quantity": 3}),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(
        cart_of(&state, "co-merge-update"),
        [
            ("TEE-BLK-M".to_string(), 3),
            ("TOTE-NAT".to_string(), 6),
            ("MUG-OAT".to_string(), 1)
        ]
    );

    // Its merchandise, and what belongs to one line only, cannot be changed.
    for change in [
        json!({"type": "updateCartLine", "id": "gid://shopify/CartLine/4", "merchandiseId": format!("gid://shopify/ProductVariant/{}", variant_id(&state, "MUG-OAT"))}),
        json!({"type": "updateCartLine", "id": "gid://shopify/CartLine/4", "attributes": [{"key": "a", "value": "b"}]}),
    ] {
        let reply = change_line(&state, &path, "co-merge-update", change);
        assert_eq!(reply.status, 400, "{}", text(&reply));
        assert!(text(&reply).contains("cart transform"), "{}", text(&reply));
    }
    assert_eq!(cart_of(&state, "co-merge-update").len(), 3);
}

#[test]
fn a_merged_line_of_several_bundles_is_scaled_by_bundles() {
    let state = checkout_server(&["bundle-transform"], false);
    let path = transformed_checkout(
        &state,
        "co-merge-bundles",
        json!([
            {"variant": "TEE-BLK-M", "quantity": 2},
            {"variant": "TOTE-NAT", "quantity": 4},
            {"variant": "MUG-OAT", "quantity": 1}
        ]),
        json!([{"type": "merge", "lines": [1, 2], "parentVariant": variant_id(&state, "MUG-OAT")}]),
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-merge-bundles"));
    let bundle = &json["lines"][0];
    assert_eq!(bundle["transformedBy"], "merge", "{json}");
    assert_eq!(
        bundle["quantity"], 2,
        "two bundles of one tee and two totes"
    );
    assert_eq!(bundle["components"][0]["quantity"], 2);
    assert_eq!(bundle["components"][1]["quantity"], 4);

    // A third bundle takes one more tee and two more totes.
    let reply = change_line(
        &state,
        &path,
        "co-merge-bundles",
        json!({"type": "updateCartLine", "id": bundle["id"], "quantity": 3}),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(
        cart_of(&state, "co-merge-bundles"),
        [
            ("TEE-BLK-M".to_string(), 3),
            ("TOTE-NAT".to_string(), 6),
            ("MUG-OAT".to_string(), 1)
        ]
    );
    let json = body_json(&reply);
    assert_eq!(json["lines"][0]["quantity"], 3, "{json}");
    assert_eq!(json["lines"][0]["components"][1]["quantity"], 6);
}

/// A cart of three T-shirts and a mug where one T-shirt and the mug were merged into a bundle
/// (as a cart transform does with part of a line): the cart line of the T-shirts is shared by
/// the bundle and by a line of two T-shirts. Returns the store, the cart, and the lines of the
/// checkout, the T-shirts first.
fn partly_merged() -> (
    std::sync::Arc<lsf_core::Store>,
    lsf_core::Session,
    lsf_checkout::ResultLine,
    lsf_checkout::ResultLine,
) {
    use lsf_checkout::{ResultLine, SourceLine};
    use lsf_core::store::CartLine;

    let state = checkout_server(&["bundle-transform"], false);
    let store = state.loaded().store.clone();
    let cart_line = |sku: &str, quantity: u32| CartLine {
        variant_id: variant_id(&state, sku),
        quantity,
        properties: Default::default(),
        selling_plan: None,
    };
    let session = lsf_core::Session {
        cart_lines: vec![cart_line("TEE-BLK-M", 3), cart_line("MUG-OAT", 1)],
        ..Default::default()
    };
    let tees = ResultLine {
        id: "gid://shopify/CartLine/1".into(),
        quantity: 2,
        sources: vec![SourceLine {
            index: 0,
            quantity: 2,
        }],
        ..ResultLine::default()
    };
    let bundle = ResultLine {
        id: "gid://shopify/CartLine/3".into(),
        quantity: 1,
        transformed_by: Some("merge".into()),
        sources: vec![
            SourceLine {
                index: 0,
                quantity: 1,
            },
            SourceLine {
                index: 1,
                quantity: 1,
            },
        ],
        ..ResultLine::default()
    };
    (store, session, tees, bundle)
}

/// The SKU, quantity and properties of a cart line.
type CartEntry = (String, u32, Vec<(String, String)>);

fn cart_skus(store: &lsf_core::Store, session: &lsf_core::Session) -> Vec<CartEntry> {
    session
        .cart_lines
        .iter()
        .map(|line| {
            let (_, variant) = store.variant(line.variant_id).expect("a variant");
            (
                variant.sku.clone(),
                line.quantity,
                line.properties
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        })
        .collect()
}

fn sku(name: &str, quantity: u32) -> CartEntry {
    (name.to_string(), quantity, vec![])
}

#[test]
fn a_change_of_merchandise_leaves_the_share_of_the_bundle_alone() {
    let state = checkout_server(&["bundle-transform"], false);
    let tote = format!(
        "gid://shopify/ProductVariant/{}",
        variant_id(&state, "TOTE-NAT")
    );
    let (store, mut session, tees, _) = partly_merged();

    // The two T-shirts become totes, the one in the bundle stays a T-shirt.
    let change = json!({"type": "updateCartLine", "id": tees.id, "merchandiseId": tote});
    super::checkout::update_line(&mut session, &tees, &change, &store).unwrap();
    assert_eq!(
        cart_skus(&store, &session),
        [sku("TEE-BLK-M", 1), sku("MUG-OAT", 1), sku("TOTE-NAT", 2)]
    );

    // With a new quantity at the same time, the new line has it.
    let (store, mut session, tees, _) = partly_merged();
    let change =
        json!({"type": "updateCartLine", "id": tees.id, "merchandiseId": tote, "quantity": 5});
    super::checkout::update_line(&mut session, &tees, &change, &store).unwrap();
    assert_eq!(
        cart_skus(&store, &session),
        [sku("TEE-BLK-M", 1), sku("MUG-OAT", 1), sku("TOTE-NAT", 5)]
    );
}

#[test]
fn a_change_of_attributes_leaves_the_share_of_the_bundle_alone() {
    let (store, mut session, tees, _) = partly_merged();
    let change = json!({"type": "updateCartLine", "id": tees.id,
        "attributes": [{"key": "engraving", "value": "Ada"}]});
    super::checkout::update_line(&mut session, &tees, &change, &store).unwrap();
    assert_eq!(
        cart_skus(&store, &session),
        [
            sku("TEE-BLK-M", 1),
            sku("MUG-OAT", 1),
            (
                "TEE-BLK-M".to_string(),
                2,
                vec![("engraving".to_string(), "Ada".to_string())]
            )
        ]
    );

    // An attribute that changes nothing does not split the line.
    let (store, mut session, tees, _) = partly_merged();
    let change = json!({"type": "updateCartLine", "id": tees.id, "attributes": []});
    super::checkout::update_line(&mut session, &tees, &change, &store).unwrap();
    assert_eq!(
        cart_skus(&store, &session),
        [sku("TEE-BLK-M", 3), sku("MUG-OAT", 1)]
    );
}

#[test]
fn a_change_that_is_refused_changes_nothing_of_a_shared_line() {
    let (store, mut session, tees, bundle) = partly_merged();
    let before = cart_skus(&store, &session);
    for (line, change, message) in [
        (
            &tees,
            json!({"type": "updateCartLine", "id": tees.id, "merchandiseId": "gid://shopify/ProductVariant/1"}),
            "there is no product variant",
        ),
        (
            &tees,
            json!({"type": "updateCartLine", "id": tees.id, "sellingPlanId": "gid://shopify/SellingPlan/1"}),
            "is not offered for this product",
        ),
        (
            &bundle,
            json!({"type": "updateCartLine", "id": bundle.id, "attributes": [{"key": "a", "value": "b"}]}),
            "only its quantity can be changed",
        ),
        (
            &bundle,
            json!({"type": "updateCartLine", "id": bundle.id, "merchandiseId": "gid://shopify/ProductVariant/1"}),
            "cart transform (merge)",
        ),
    ] {
        let problem = super::checkout::update_line(&mut session, line, &change, &store)
            .expect_err("the change is refused");
        assert!(problem.contains(message), "{problem}");
    }
    assert_eq!(cart_skus(&store, &session), before);
}

#[test]
fn a_change_of_quantity_stays_within_the_share_of_the_line() {
    // More of the two T-shirts: the one of the bundle is still there.
    let (store, mut session, tees, _) = partly_merged();
    let change = json!({"type": "updateCartLine", "id": tees.id, "quantity": 5});
    super::checkout::update_line(&mut session, &tees, &change, &store).unwrap();
    assert_eq!(
        cart_skus(&store, &session),
        [sku("TEE-BLK-M", 6), sku("MUG-OAT", 1)]
    );

    // Fewer of them.
    let (store, mut session, tees, _) = partly_merged();
    super::checkout::requantify(&mut session, &tees, 1).unwrap();
    assert_eq!(
        cart_skus(&store, &session),
        [sku("TEE-BLK-M", 2), sku("MUG-OAT", 1)]
    );
}

#[test]
fn a_removed_line_takes_its_share_of_a_shared_line_only() {
    // The two T-shirts go: the bundle keeps its T-shirt.
    let (store, mut session, tees, _) = partly_merged();
    let change = json!({"type": "updateCartLine", "id": tees.id, "quantity": 0});
    super::checkout::update_line(&mut session, &tees, &change, &store).unwrap();
    assert_eq!(
        cart_skus(&store, &session),
        [sku("TEE-BLK-M", 1), sku("MUG-OAT", 1)]
    );

    // The bundle goes: the two T-shirts stay.
    let (store, mut session, _, bundle) = partly_merged();
    super::checkout::requantify(&mut session, &bundle, 0).unwrap();
    assert_eq!(cart_skus(&store, &session), [sku("TEE-BLK-M", 2)]);
}

#[test]
fn an_expanded_line_is_removed_through_its_line_of_the_cart() {
    let state = checkout_server(&["bundle-transform"], false);
    let path = transformed_checkout(
        &state,
        "co-expand-remove",
        json!([{"variant": "TEE-BLK-M", "quantity": 2}, {"variant": "MUG-OAT", "quantity": 1}]),
        json!([{"type": "expand", "line": 1, "items": [
            {"variant": variant_id(&state, "MUG-OAT"), "quantity": 1},
            {"variant": variant_id(&state, "TOTE-NAT"), "quantity": 1}
        ]}]),
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-expand-remove"));
    assert_eq!(json["lines"][0]["transformedBy"], "expand");
    assert_eq!(json["lines"][0]["components"].as_array().unwrap().len(), 2);

    let reply = change_line(
        &state,
        &path,
        "co-expand-remove",
        json!({"type": "removeCartLine", "id": "gid://shopify/CartLine/1", "quantity": 1}),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(
        cart_of(&state, "co-expand-remove"),
        [("TEE-BLK-M".to_string(), 1), ("MUG-OAT".to_string(), 1)]
    );
    assert_eq!(body_json(&reply)["lines"][0]["quantity"], 1);

    // The id of a component is not a line to change.
    let component = body_json(&reply)["lines"][0]["components"][0]["id"].clone();
    let reply = change_line(
        &state,
        &path,
        "co-expand-remove",
        json!({"type": "removeCartLine", "id": component, "quantity": 1}),
    );
    assert_eq!(reply.status, 400, "{}", text(&reply));

    let reply = change_line(
        &state,
        &path,
        "co-expand-remove",
        json!({"type": "removeCartLine", "id": "gid://shopify/CartLine/1", "quantity": 1}),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(
        cart_of(&state, "co-expand-remove"),
        [("MUG-OAT".to_string(), 1)]
    );
}

#[test]
fn a_bad_operation_changes_nothing() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-bad",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({}),
    );
    for (bad, message) in [
        (json!({"type": "teleport"}), "unknown operation `teleport`"),
        (json!({"value": "x"}), "an operation needs a `type`"),
        (
            json!({"type": "deliveryOption", "group": "group-1", "handle": "drone"}),
            "there is no delivery option `drone`",
        ),
        (
            json!({"type": "paymentMethod", "handle": "bitcoin"}),
            "there is no payment method `bitcoin`",
        ),
        (
            json!({"type": "cartLines", "change": {"type": "updateCartLine", "id": "gid://shopify/CartLine/9", "quantity": 1}}),
            "there is no line `gid://shopify/CartLine/9`",
        ),
        (
            json!({"type": "cartLines", "change": {"type": "addCartLine", "merchandiseId": "gid://shopify/ProductVariant/1", "quantity": 1}}),
            "there is no product variant",
        ),
        (
            json!({"type": "discountCode", "action": "toggle", "code": "X"}),
            "`add` or `remove`",
        ),
    ] {
        // The email is valid and comes first: it must not stick.
        let reply = operations(
            &state,
            &path,
            "co-bad",
            json!([{"type": "email", "value": "kept@example.com"}, bad]),
        );
        assert_eq!(reply.status, 400, "{bad}");
        let error = body_json(&reply)["error"].as_str().unwrap().to_string();
        assert!(
            error.starts_with("operation 1: ") && error.contains(message),
            "{error}"
        );
    }
    let json = body_json(&get(&state, &format!("{path}.json"), "co-bad"));
    assert_eq!(json["buyer"]["email"], Json::Null);

    let reply = post(&state, &format!("{path}/update"), json!({}), "co-bad");
    assert_eq!(reply.status, 400);
}

#[test]
fn a_checkout_is_completed_when_nothing_is_missing() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-done",
        json!([{"variant": "TEE-BLK-M", "quantity": 2}]),
        json!({}),
    );

    // What the buyer has not filled in comes back as errors on the fields.
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-done");
    assert_eq!(reply.status, 422);
    let json = body_json(&reply);
    let targets: Vec<&str> = json["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error["target"].as_str().unwrap())
        .collect();
    assert!(
        targets.contains(&"$.cart.buyerIdentity.email"),
        "{targets:?}"
    );
    assert!(targets.contains(&"$.cart.selectedPayment"), "{targets:?}");
    assert!(
        json["lines"].is_array(),
        "the page reads the state of the reply"
    );
    assert_eq!(json["status"], "open");
    assert_eq!(json["orderGeneration"], 0);

    assert_eq!(
        operations(&state, &path, "co-done", buyer_details()).status,
        200
    );
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-done");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(body_json(&reply)["redirect"], format!("{path}/thank-you"));

    // The thank-you page shows the order, and the cart is empty.
    let thanks = get(&state, &format!("{path}/thank-you"), "co-done");
    assert_eq!(thanks.status, 200);
    let html = text(&thanks);
    assert!(html.contains("\"status\":\"completed\""), "{html}");
    assert!(html.contains("\"number\":\"#1001\""), "{html}");
    let json = body_json(&get(&state, &format!("{path}.json"), "co-done"));
    assert_eq!(json["status"], "completed");
    assert_eq!(json["lines"][0]["quantity"], 2);
    assert_eq!(
        body_json(&get(&state, "/cart.js", "co-done"))["item_count"],
        0
    );

    // A new cart is a new checkout, with an order number of its own.
    let again = start_checkout(
        &state,
        "co-done",
        json!([{"variant": "TOTE-NAT", "quantity": 1}]),
        json!({}),
    );
    let json = body_json(&get(&state, &format!("{again}.json"), "co-done"));
    assert_eq!(json["status"], "open");
    // The token is the one of the cart, so the checkout after an order is told by its generation:
    // what the extensions stored for the first one is not for this one.
    assert_eq!(again, path);
    assert_eq!(json["orderGeneration"], 1);
    assert_eq!(json["buyer"]["email"], Json::Null);
    assert_eq!(
        operations(&state, &again, "co-done", buyer_details()).status,
        200
    );
    post(&state, &format!("{again}/complete"), json!({}), "co-done");
    let json = body_json(&get(&state, &format!("{again}.json"), "co-done"));
    assert_eq!(json["order"]["number"], "#1002");

    // Without a completed order, the thank-you page leads back to the checkout.
    let reply = get(&state, &format!("{path}/thank-you"), "co-fresh");
    assert_eq!(reply.status, 404);
    let path = start_checkout(
        &state,
        "co-fresh",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({}),
    );
    let reply = get(&state, &format!("{path}/thank-you"), "co-fresh");
    assert_eq!(
        (reply.status, header(&reply, "location")),
        (302, Some(path.as_str()))
    );

    // An empty cart is not an order.
    let empty = start_checkout(&state, "co-empty", json!([]), json!({}));
    let reply = post(&state, &format!("{empty}/complete"), json!({}), "co-empty");
    assert_eq!(reply.status, 422);
}

/// A checkout of `session` that can be completed.
fn ready_checkout(state: &ServerState, session: &str) -> String {
    let path = start_checkout(
        state,
        session,
        json!([{"variant": "TEE-BLK-M", "quantity": 2}]),
        json!({}),
    );
    assert_eq!(
        operations(state, &path, session, buyer_details()).status,
        200
    );
    path
}

fn cart_count(state: &ServerState, session: &str) -> u64 {
    body_json(&get(state, "/cart.js", session))["item_count"]
        .as_u64()
        .expect("a count")
}

#[test]
fn a_cart_change_while_completing_is_kept_and_buys_nothing() {
    let state = checkout_server(&[], false);
    let path = ready_checkout(&state, "co-race");
    let token = path.rsplit('/').next().unwrap();

    // Another tab adds an item after the Functions ran on the cart, before the order takes it.
    let reply = super::checkout::place_order(
        &state,
        &request("POST", &format!("{path}/complete"), json!({}), "co-race"),
        "co-race",
        token,
        || {
            let reply = post(
                &state,
                "/cart/add.js",
                json!({"id": variant_id(&state, "TOTE-NAT"), "quantity": 1}),
                "co-race",
            );
            assert!(reply.status < 300, "{}", text(&reply));
        },
    );
    assert_eq!(reply.status, 409, "{}", text(&reply));
    let json = body_json(&reply);
    assert_eq!(json["status"], "open");
    assert_eq!(json["lines"].as_array().unwrap().len(), 2, "{json}");
    // Nothing was bought, nothing was wiped, no number was spent.
    assert_eq!(cart_count(&state, "co-race"), 3);
    assert_eq!(
        get(&state, &format!("{path}/thank-you"), "co-race").status,
        302
    );
    assert_eq!(state.orders.load(std::sync::atomic::Ordering::Relaxed), 0);

    // The buyer completes what they now see.
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-race");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(cart_count(&state, "co-race"), 0);
    assert_eq!(state.orders.load(std::sync::atomic::Ordering::Relaxed), 1);
}

/// Completes the checkout of `session` while `change` happens after the Functions ran, and
/// checks that nothing was bought.
///
/// A session starts with the store of the server, or with one of its own when `own_store`.
fn completion_buys_nothing_when(session: &str, own_store: bool, change: impl FnOnce(&ServerState)) {
    let state = checkout_server(&[], false);
    let path = if own_store {
        ready_checkout(&state, session)
    } else {
        let reply = post(
            &state,
            "/cart/add.js",
            json!({"id": variant_id(&state, "TEE-BLK-M"), "quantity": 2}),
            session,
        );
        assert!(reply.status < 300, "{}", text(&reply));
        let path = header(&get(&state, "/checkout", session), "location")
            .expect("a location")
            .to_string();
        assert_eq!(
            operations(&state, &path, session, buyer_details()).status,
            200
        );
        path
    };
    let token = path.rsplit('/').next().unwrap();
    let reply = super::checkout::place_order(
        &state,
        &request("POST", &format!("{path}/complete"), json!({}), session),
        session,
        token,
        || change(&state),
    );
    assert_eq!(reply.status, 409, "{session}: {}", text(&reply));
    assert_eq!(body_json(&reply)["status"], "open", "{session}");
    assert_eq!(cart_count(&state, session), 2, "{session}");
    assert_eq!(
        get(&state, &format!("{path}/thank-you"), session).status,
        302
    );
    assert_eq!(state.orders.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[test]
fn what_the_functions_read_of_the_session_cannot_change_while_completing() {
    completion_buys_nothing_when("co-login", false, |state| {
        let reply = get(state, "/__lsf/login?customer=default", "co-login");
        assert_eq!(reply.status, 302);
    });
    completion_buys_nothing_when("co-country", false, |state| {
        let reply = post(
            state,
            "/localization",
            json!({"country_code": "CA", "return_to": "/"}),
            "co-country",
        );
        assert!(reply.status < 400, "{}", text(&reply));
    });
    // The data is reloaded, or the store of the session replaced by the same cart on new data.
    completion_buys_nothing_when("co-reload", false, |state| {
        state.reload();
    });
    completion_buys_nothing_when("co-store", true, |state| {
        let reply = state.dispatch(&request(
            "PUT",
            "/__lsf/session",
            json!({"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}, "data": {}}),
            "co-store",
        ));
        assert_eq!(reply.status, 200, "{}", text(&reply));
    });
}

#[test]
fn an_update_after_the_completion_is_refused() {
    let state = checkout_server(&[], false);
    let path = ready_checkout(&state, "co-late");
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-late");
    assert_eq!(reply.status, 200, "{}", text(&reply));

    let reply = operations(
        &state,
        &path,
        "co-late",
        json!([{"type": "email", "value": "other@example.com"}]),
    );
    assert_eq!(reply.status, 409, "{}", text(&reply));
    let json = body_json(&reply);
    assert_eq!(json["status"], "completed");
    assert_eq!(json["order"]["number"], "#1001");

    // The order is still the order: the thank-you page, the state, another completion.
    assert_eq!(
        get(&state, &format!("{path}/thank-you"), "co-late").status,
        200
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-late"));
    assert_eq!(json["status"], "completed");
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-late");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(cart_count(&state, "co-late"), 0);
    assert_eq!(state.orders.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn an_update_that_read_the_cart_before_the_completion_writes_nothing() {
    let state = checkout_server(&[], false);
    let path = ready_checkout(&state, "co-stale");
    let token = path.rsplit('/').next().unwrap();
    let update = [json!({"type": "email", "value": "other@example.com"})];

    // The update read the cart, then the order took it, then the update wants to write.
    let reply = super::checkout::apply_update(
        &state,
        &request("POST", &format!("{path}/update"), json!({}), "co-stale"),
        "co-stale",
        &update,
        || {
            let reply = post(&state, &format!("{path}/complete"), json!({}), "co-stale");
            assert_eq!(reply.status, 200, "{}", text(&reply));
        },
    );
    assert_eq!(reply.status, 409, "{}", text(&reply));
    let json = body_json(&reply);
    assert_eq!(json["status"], "completed");
    assert_eq!(json["order"]["number"], "#1001");

    // The lines the order consumed did not come back, and the order stays one.
    assert_eq!(cart_count(&state, "co-stale"), 0);
    assert_eq!(
        get(&state, &format!("{path}/thank-you"), "co-stale").status,
        200
    );
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-stale");
    assert_eq!(
        body_json(&reply)["redirect"],
        format!("/checkouts/cn/{token}/thank-you")
    );
    assert_eq!(state.orders.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn an_update_does_not_overwrite_a_cart_change_made_meanwhile() {
    let state = checkout_server(&[], false);
    let path = ready_checkout(&state, "co-lost");
    let reply = super::checkout::apply_update(
        &state,
        &request("POST", &format!("{path}/update"), json!({}), "co-lost"),
        "co-lost",
        &[json!({"type": "email", "value": "other@example.com"})],
        || {
            let reply = post(
                &state,
                "/cart/add.js",
                json!({"id": variant_id(&state, "TOTE-NAT"), "quantity": 1}),
                "co-lost",
            );
            assert!(reply.status < 300, "{}", text(&reply));
        },
    );
    assert_eq!(reply.status, 409, "{}", text(&reply));
    let json = body_json(&reply);
    assert_eq!(json["status"], "open");
    assert_eq!(json["buyer"]["email"], "buyer@example.com");
    assert_eq!(cart_count(&state, "co-lost"), 3);
}

#[test]
fn completing_again_places_no_second_order() {
    let state = checkout_server(&[], false);
    let path = ready_checkout(&state, "co-twice");
    let token = path.rsplit('/').next().unwrap();

    // A second completion that started before the first one finished.
    let reply = super::checkout::place_order(
        &state,
        &request("POST", &format!("{path}/complete"), json!({}), "co-twice"),
        "co-twice",
        token,
        || {
            let reply = post(&state, &format!("{path}/complete"), json!({}), "co-twice");
            assert_eq!(reply.status, 200, "{}", text(&reply));
        },
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(body_json(&reply)["redirect"], format!("{path}/thank-you"));
    assert_eq!(state.orders.load(std::sync::atomic::Ordering::Relaxed), 1);

    // And one that comes after: the same thank-you page, not a new order.
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-twice");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(state.orders.load(std::sync::atomic::Ordering::Relaxed), 1);
    let json = body_json(&get(&state, &format!("{path}.json"), "co-twice"));
    assert_eq!(json["order"]["number"], "#1001");
}

#[test]
fn concurrent_completions_place_one_order() {
    let state = std::sync::Arc::new(checkout_server(&[], false));
    let path = ready_checkout(&state, "co-threads");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let replies: Vec<u16> = (0..4)
        .map(|_| {
            let (state, path, barrier) = (state.clone(), path.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                post(&state, &format!("{path}/complete"), json!({}), "co-threads").status
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|thread| thread.join().expect("a reply"))
        .collect();
    assert!(replies.iter().all(|status| *status == 200), "{replies:?}");
    assert_eq!(state.orders.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(cart_count(&state, "co-threads"), 0);
}

#[test]
fn the_functions_of_the_app_apply() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-fn",
        json!([{"variant": "TEE-BLK-M", "quantity": 3}]),
        json!({"checkout": {
            "discounts": [{
                "function": "volume-discount",
                "title": "Volume",
                "classes": ["PRODUCT"],
                "metafields": {"$app:config.settings": {
                    "type": "json", "value": {"minimumQuantity": 3, "percentage": 10}
                }}
            }],
            "functions": {"quantity-limit": {"metafields": {"$app:config.settings": {
                "type": "json", "value": {"maxQuantity": 2}
            }}}}
        }}),
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-fn"));

    // The discount took 10 % off the lines.
    let line = &json["lines"][0];
    let subtotal = line["subtotal"]["amount"].as_f64().unwrap();
    let total = line["total"]["amount"].as_f64().unwrap();
    assert!((total - subtotal * 0.9).abs() < 0.01, "{subtotal} {total}");
    assert_eq!(json["discounts"][0]["function"], "volume-discount");
    assert_eq!(
        json["cost"]["discounts"]["amount"],
        json!(((subtotal - total) * 100.0).round() / 100.0)
    );

    // Every run is there, with its input and its output, for the panel of the page.
    let run = json["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["handle"] == "volume-discount")
        .expect("the run");
    assert_eq!(run["target"], "cart.lines.discounts.generate.run");
    assert_eq!(run["input"]["cart"]["lines"][0]["quantity"], 3);
    assert!(run["output"]["operations"].is_array());
    assert_eq!(run["error"], Json::Null);

    // The validation Function blocks the order.
    assert_eq!(json["errors"][0]["function"], "quantity-limit");
    assert_eq!(
        operations(&state, &path, "co-fn", buyer_details()).status,
        200
    );
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-fn");
    assert_eq!(reply.status, 422);
    let blocked = body_json(&reply);
    assert_eq!(
        blocked["errors"][0]["message"],
        "Not possible to order more than 2 of each"
    );

    // Taking one off lets it through.
    let reply = operations(
        &state,
        &path,
        "co-fn",
        json!([{"type": "cartLines", "change": {"type": "removeCartLine", "id": "gid://shopify/CartLine/1", "quantity": 1}}]),
    );
    assert_eq!(body_json(&reply)["errors"], json!([]));
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-fn");
    assert_eq!(reply.status, 200, "{}", text(&reply));
}

#[test]
fn a_validation_function_sees_the_step_of_the_buyer_journey() {
    let state = checkout_server(&[], false);
    // 3 in the cart, 2 allowed, but only when the order is placed.
    let path = start_checkout(
        &state,
        "co-step",
        json!([{"variant": "TEE-BLK-M", "quantity": 3}]),
        json!({"checkout": {"functions": {"quantity-limit": {"metafields": {"$app:config.settings": {
            "type": "json", "value": {"maxQuantity": 2, "onlyAtCompletion": true}
        }}}}}}),
    );
    let step_of = |json: &Json| {
        let run = json["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["handle"] == "quantity-limit")
            .expect("the run");
        run["input"]["buyerJourney"]["step"].clone()
    };

    // The page, the state and an update are interactions: the Function does not check yet.
    let html = text(&get(&state, &path, "co-step"));
    let page = page_state(&html);
    assert_eq!(step_of(&page), "CHECKOUT_INTERACTION");
    assert_eq!(page["errors"], json!([]));
    let json = body_json(&get(&state, &format!("{path}.json"), "co-step"));
    assert_eq!(step_of(&json), "CHECKOUT_INTERACTION");
    assert_eq!(json["errors"], json!([]));
    let reply = operations(&state, &path, "co-step", buyer_details());
    assert_eq!(reply.status, 200);
    let json = body_json(&reply);
    assert_eq!(step_of(&json), "CHECKOUT_INTERACTION");
    assert_eq!(json["errors"], json!([]));
    // So is a change of a line.
    let reply = operations(
        &state,
        &path,
        "co-step",
        json!([{"type": "cartLines", "change": {
            "type": "updateCartLine", "id": "gid://shopify/CartLine/1", "quantity": 3
        }}]),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(step_of(&body_json(&reply)), "CHECKOUT_INTERACTION");

    // Placing the order is the completion: the Function checks, and the order is refused.
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-step");
    assert_eq!(reply.status, 422, "{}", text(&reply));
    let blocked = body_json(&reply);
    assert_eq!(step_of(&blocked), "CHECKOUT_COMPLETION");
    assert_eq!(
        blocked["errors"][0]["message"],
        "Not possible to order more than 2 of each"
    );
    // Nothing was bought, and the checkout is still an interaction.
    assert_eq!(state.orders.load(std::sync::atomic::Ordering::Relaxed), 0);
    let json = body_json(&get(&state, &format!("{path}.json"), "co-step"));
    assert_eq!(step_of(&json), "CHECKOUT_INTERACTION");
    assert_eq!(json["errors"], json!([]));

    // A Function that checks at every step shows its errors on the page, as it blocks at the end.
    let path = start_checkout(
        &state,
        "co-step-all",
        json!([{"variant": "TEE-BLK-M", "quantity": 3}]),
        json!({"checkout": {"functions": {"quantity-limit": {"metafields": {"$app:config.settings": {
            "type": "json", "value": {"maxQuantity": 2}
        }}}}}}),
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-step-all"));
    assert_eq!(json["errors"][0]["function"], "quantity-limit");
    operations(&state, &path, "co-step-all", buyer_details());
    let reply = post(
        &state,
        &format!("{path}/complete"),
        json!({}),
        "co-step-all",
    );
    assert_eq!(reply.status, 422);
}

#[test]
fn a_discount_code_triggers_its_discount() {
    let state = checkout_server(&["volume-discount"], false);
    let path = start_checkout(
        &state,
        "co-code",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({"checkout": {"discounts": [{
            "function": "volume-discount",
            "title": "Welcome",
            "code": "WELCOME",
            "metafields": {"$app:config.settings": {
                "type": "json", "value": {"minimumQuantity": 1, "percentage": 20}
            }}
        }]}}),
    );
    let before = body_json(&get(&state, &format!("{path}.json"), "co-code"));
    assert_eq!(before["discounts"], json!([]));
    let reply = operations(
        &state,
        &path,
        "co-code",
        json!([{"type": "discountCode", "action": "add", "code": "welcome"}]),
    );
    let json = body_json(&reply);
    assert_eq!(json["discountCodes"][0]["applicable"], true);
    assert_eq!(json["discounts"][0]["code"], "WELCOME");
}

#[test]
fn the_files_of_the_page_and_of_the_extensions_are_served() {
    let state = checkout_server(&[], false);
    let script = get(&state, "/__lsf/checkout/assets/checkout.js", "co-files");
    assert_eq!(script.status, 200);
    assert_eq!(
        header(&script, "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert!(text(&script).contains("startCheckout"));
    let css = get(&state, "/__lsf/checkout/assets/checkout.css", "co-files");
    assert_eq!(
        header(&css, "content-type"),
        Some("text/css; charset=utf-8")
    );
    let sandbox = get(&state, "/__lsf/checkout/assets/sandbox.html", "co-files");
    assert_eq!(
        header(&sandbox, "content-type"),
        Some("text/html; charset=utf-8")
    );
    // The tests of the scripts are not part of the binary.
    for missing in ["tests/fake-dom.mjs", "preview/serve.mjs", "nope.js"] {
        assert_eq!(
            get(
                &state,
                &format!("/__lsf/checkout/assets/{missing}"),
                "co-files"
            )
            .status,
            404,
            "{missing}"
        );
    }

    // The bundle `shopify app build` wrote.
    let bundle = get(
        &state,
        "/__lsf/extensions/gift-message/gift-message.js",
        "co-files",
    );
    assert_eq!(bundle.status, 200);
    assert_eq!(
        header(&bundle, "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    let on_disk =
        std::fs::read(fixture_app().join("extensions/gift-message/dist/gift-message.js")).unwrap();
    assert_eq!(bundle.body, on_disk);
    // Nothing of an extension runs with the origin of the checkout, even in a frame of its own.
    assert_eq!(
        header(&bundle, "content-security-policy"),
        Some("sandbox allow-scripts")
    );
    assert_eq!(header(&bundle, "x-content-type-options"), Some("nosniff"));
    for missing in [
        "/__lsf/extensions/gift-message/nope.js",
        "/__lsf/extensions/gift-message/../shopify.extension.toml",
        "/__lsf/extensions/volume-discount/function.wasm",
        "/__lsf/extensions/nope/nope.js",
        "/__lsf/extensions/gift-message",
    ] {
        assert_eq!(get(&state, missing, "co-files").status, 404, "{missing}");
    }
}

/// An app of one checkout UI extension, `ext`, whose `dist/` has a script and what `files` adds.
fn ui_extension_app(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("lsf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("extensions/ext");
    std::fs::create_dir_all(dir.join("dist")).unwrap();
    std::fs::write(dir.join("dist/ext.js"), "// built").unwrap();
    for (file, content) in files {
        std::fs::write(dir.join("dist").join(file), content).unwrap();
    }
    std::fs::write(
        dir.join("shopify.extension.toml"),
        "type = \"ui_extension\"\napi_version = \"2026-10\"\n[[targeting]]\ntarget = \"purchase.checkout.block.render\"\n",
    )
    .unwrap();
    root
}

#[cfg(unix)]
#[test]
fn an_extension_cannot_serve_files_outside_its_dist() {
    let root = ui_extension_app("checkout-ui-links", &[]);
    let dist = root.join("extensions/ext/dist");
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.js"), "// secret").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.js"), dist.join("file-link.js")).unwrap();
    std::os::unix::fs::symlink(&outside, dist.join("directory-link")).unwrap();
    std::os::unix::fs::symlink("../shopify.extension.toml", dist.join("toml-link.json")).unwrap();
    // A link that stays inside `dist/` is fine.
    std::fs::create_dir_all(dist.join("assets")).unwrap();
    std::fs::write(dist.join("assets/inner.js"), "// inner").unwrap();
    std::os::unix::fs::symlink("assets/inner.js", dist.join("inner-link.js")).unwrap();

    let state = checkout_server_of(root.clone(), &[], false);
    for escape in [
        "/__lsf/extensions/ext/file-link.js",
        "/__lsf/extensions/ext/directory-link/secret.js",
        "/__lsf/extensions/ext/toml-link.json",
    ] {
        assert_eq!(get(&state, escape, "co-links").status, 404, "{escape}");
    }
    for served in ["ext.js", "assets/inner.js", "inner-link.js"] {
        let reply = get(
            &state,
            &format!("/__lsf/extensions/ext/{served}"),
            "co-links",
        );
        assert_eq!(reply.status, 200, "{served}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_extension_cannot_serve_a_page_on_the_checkout_origin() {
    let root = ui_extension_app(
        "checkout-ui-pages",
        &[
            ("evil.html", "<script>parent.document</script>"),
            ("evil.xhtml", "<html/>"),
            ("evil.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            ("style.css", "a {}"),
            ("data.json", "{}"),
        ],
    );
    let state = checkout_server_of(root.clone(), &[], false);
    // A page the extension could send its frame to is not served at all.
    for page in ["evil.html", "evil.xhtml"] {
        let reply = get(&state, &format!("/__lsf/extensions/ext/{page}"), "co-pages");
        assert_eq!(reply.status, 404, "{page}");
    }
    // The rest comes with a policy that gives it an opaque origin, if it is ever navigated to.
    for (file, kind) in [
        ("ext.js", "text/javascript; charset=utf-8"),
        ("style.css", "text/css; charset=utf-8"),
        ("data.json", "application/json; charset=utf-8"),
        ("evil.svg", "image/svg+xml"),
    ] {
        let reply = get(&state, &format!("/__lsf/extensions/ext/{file}"), "co-pages");
        assert_eq!(reply.status, 200, "{file}");
        assert_eq!(header(&reply, "content-type"), Some(kind), "{file}");
        assert_eq!(
            header(&reply, "content-security-policy"),
            Some("sandbox allow-scripts"),
            "{file}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_logged_in_customer_completes_with_the_email_the_store_knows() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-login",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({}),
    );
    assert_eq!(
        get(&state, "/__lsf/login?customer=default", "co-login").status,
        302
    );
    // The page shows the email of the customer, so it does not ask for one.
    let json = body_json(&get(&state, &format!("{path}.json"), "co-login"));
    assert_eq!(json["buyer"]["email"], "jane.doe@example.com");
    let reply = operations(
        &state,
        &path,
        "co-login",
        json!([
            {"type": "shippingAddress", "address": {
                "lastName": "Doe", "address1": "1 Main St", "city": "London", "countryCode": "gb"
            }},
            {"type": "paymentMethod", "handle": "card"}
        ]),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-login");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(body_json(&reply)["redirect"], format!("{path}/thank-you"));
    let json = body_json(&get(&state, &format!("{path}.json"), "co-login"));
    assert_eq!(json["buyer"]["email"], "jane.doe@example.com");
}

/// The targets of the errors `complete` answers with.
fn complete_errors(state: &ServerState, path: &str, session: &str) -> Vec<(String, String)> {
    let reply = post(state, &format!("{path}/complete"), json!({}), session);
    assert_eq!(reply.status, 422, "{}", text(&reply));
    body_json(&reply)["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| {
            (
                error["message"].as_str().unwrap().to_string(),
                error["target"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn a_shippable_cart_needs_a_delivery_option_to_complete() {
    let state = checkout_server(&[], false);
    let path = start_checkout(
        &state,
        "co-no-delivery",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({"checkout": {"shipping_rates": []}}),
    );
    assert_eq!(
        operations(&state, &path, "co-no-delivery", buyer_details()).status,
        200
    );
    let errors = complete_errors(&state, &path, "co-no-delivery");
    assert!(
        errors
            .iter()
            .any(|(message, _)| message == "No delivery option is available"),
        "{errors:?}"
    );

    // With a rate, the same checkout goes through.
    let path = start_checkout(
        &state,
        "co-no-delivery",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({"checkout": {"shipping_rates": [{"handle": "standard", "price": "4.90"}]}}),
    );
    assert_eq!(
        operations(&state, &path, "co-no-delivery", buyer_details()).status,
        200
    );
    let reply = post(
        &state,
        &format!("{path}/complete"),
        json!({}),
        "co-no-delivery",
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
}

#[test]
fn a_checkout_needs_a_payment_method_to_complete() {
    let state = checkout_server(&["payment-rules"], false);
    // Nothing is configured.
    let path = start_checkout(
        &state,
        "co-no-pay",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({"checkout": {"payment_methods": []}}),
    );
    let details = json!([
        {"type": "email", "value": "buyer@example.com"},
        {"type": "shippingAddress", "address": {
            "lastName": "Lovelace", "address1": "1 Analytical Way",
            "city": "London", "countryCode": "gb"
        }}
    ]);
    assert_eq!(
        operations(&state, &path, "co-no-pay", details.clone()).status,
        200
    );
    let errors = complete_errors(&state, &path, "co-no-pay");
    assert!(
        errors
            .iter()
            .any(|(message, _)| message == "No payment method is available"),
        "{errors:?}"
    );

    // A Function hides the only one.
    let path = start_checkout(
        &state,
        "co-hidden-pay",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        json!({"checkout": {
            "payment_methods": [{"handle": "card", "name": "Card"}],
            "functions": {"payment-rules": {"metafields": {"$app:config.settings": {
                "type": "json", "value": {"hideName": "Card", "hideAbove": 0}
            }}}}
        }}),
    );
    assert_eq!(
        operations(&state, &path, "co-hidden-pay", details).status,
        200
    );
    let json = body_json(&get(&state, &format!("{path}.json"), "co-hidden-pay"));
    assert_eq!(json["paymentMethods"], json!([]));
    let errors = complete_errors(&state, &path, "co-hidden-pay");
    assert!(
        errors
            .iter()
            .any(|(message, _)| message == "No payment method is available"),
        "{errors:?}"
    );
}

#[test]
fn without_an_app_the_checkout_is_the_cart_listing() {
    let state = server();
    let page = get(&state, "/checkout", "co-none");
    assert_eq!(page.status, 200);
    assert!(text(&page).contains("data-lsf-checkout"));
    assert_eq!(get(&state, "/__lsf/checkout", "co-none").status, 404);
    assert_eq!(
        get(&state, "/__lsf/checkout/assets/checkout.js", "co-none").status,
        404
    );
}

#[test]
fn a_standalone_checkout_serves_nothing_else() {
    let state = checkout_server(&[], true);
    // `/` is the home page, and `/checkout` starts a checkout.
    let home = get(&state, "/", "co-alone");
    assert_eq!(home.status, 200);
    assert!(text(&home).contains("data-section=\"scenarios\""));
    let start = get(&state, "/checkout", "co-alone");
    let location = format!("/checkouts/cn/{}", super::cart_token_of("co-alone"));
    assert_eq!(start.status, 302);
    assert_eq!(header(&start, "location"), Some(location.as_str()));
    assert_eq!(get(&state, &location, "co-alone").status, 200);
    assert_eq!(get(&state, "/products/anything", "co-alone").status, 404);
    // A session that is named needs no cookie.
    let named = get(&state, "/checkout", "co-alone");
    assert!(cookies(&named).is_empty());
    // A visitor without one gets it.
    let mut incoming = request("GET", "/checkout", json!({}), "ignored");
    incoming.headers.remove(SESSION_HEADER);
    assert_eq!(cookies(&state.dispatch(&incoming)).len(), 1);
}

// --- Checkout scenarios ---

const TEES: &str = r#"{
    "title": "Three tees in French",
    "session": {
        "customer": "jane.doe@example.com",
        "country": "FR",
        "cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 3}], "note": "A gift"}
    },
    "entered": {
        "email": "gift@example.com",
        "shipping_address": {"last_name": "Dupont", "address1": "1 rue de Rivoli", "city": "Paris", "country_code": "fr"},
        "delivery": "express",
        "payment": "cod",
        "discount_codes": ["WELCOME"]
    },
    "language": "fr",
    "variant": "shop",
    "checkout": {
        "shipping_rates": [
            {"handle": "standard", "title": "Standard", "price": "4.90"},
            {"handle": "express", "title": "Express", "price": "12.90"}
        ],
        "tax_rate": 20
    }
}"#;

#[test]
fn a_scenario_opens_a_new_checkout_set_up_as_it_says() {
    let data = ScenarioData::new("opens", &[("tees.json", TEES)]);
    let state = data.server(&[], true);

    let reply = get(&state, "/checkout?scenario=tees", "viewer");
    assert_eq!(reply.status, 302);
    let id = session_cookie(&reply);
    assert_ne!(id, "viewer");
    // Under the locale root of the scenario's language.
    let location = header(&reply, "location").unwrap().to_string();
    assert_eq!(
        location,
        format!("/fr/checkouts/cn/{}", super::cart_token_of(&id))
    );

    let json = body_json(&get(&state, &format!("{location}.json"), &id));
    assert_eq!(json["lines"][0]["quantity"], 3);
    assert_eq!(
        json["scenario"],
        json!({"name": "tees", "title": "Three tees in French"})
    );
    assert_eq!(json["variant"], "shop");
    assert_eq!(json["localization"]["language"]["isoCode"], "fr");
    assert_eq!(json["localization"]["country"]["isoCode"], "FR");
    assert_eq!(json["buyer"]["customer"]["email"], "jane.doe@example.com");
    assert_eq!(json["buyer"]["email"], "gift@example.com");
    assert_eq!(json["note"], "A gift");
    assert_eq!(json["shippingAddress"]["countryCode"], "FR");
    assert_eq!(json["selectedPayment"], "cod");
    assert_eq!(json["deliveryGroups"][0]["selected"], "express");
    assert_eq!(json["cost"]["shipping"]["amount"], 12.9);
    let subtotal = json["cost"]["subtotal"]["amount"].as_f64().unwrap();
    assert!((json["cost"]["tax"]["amount"].as_f64().unwrap() - subtotal * 0.2).abs() < 0.02);
    // The discount code was entered, and no discount has it.
    assert_eq!(json["discountCodes"][0]["code"], "WELCOME");
    assert_eq!(json["discountCodes"][0]["applicable"], false);
    // The page asks for nothing the scenario did not fill.
    let page = get(&state, &location, &id);
    assert_eq!(page.status, 200);
    assert!(text(&page).contains("<html lang=\"fr\">"));
}

#[test]
fn a_scenario_opened_with_a_look_and_every_target_keeps_both_and_the_url_wins() {
    let data = ScenarioData::new("query", &[("tees.json", TEES)]);
    let state = data.server(&[], true);

    // The scenario asks for the Shop look; the address asks for the standard one.
    let reply = get(
        &state,
        "/checkout?scenario=tees&variant=standard&targets=all",
        "viewer",
    );
    assert_eq!(reply.status, 302);
    let id = session_cookie(&reply);
    let location = header(&reply, "location").unwrap().to_string();
    assert_eq!(
        location,
        format!(
            "/fr/checkouts/cn/{}?variant=standard&targets=all",
            super::cart_token_of(&id)
        )
    );

    let (path, query) = location.split_once('?').unwrap();
    let json = body_json(&get(&state, &format!("{path}.json?{query}"), &id));
    assert_eq!(json["variant"], "standard");
    // The session keeps it, as for any checkout opened with `?variant=`.
    let again = body_json(&get(&state, &format!("{path}.json"), &id));
    assert_eq!(again["variant"], "standard");

    // Without it, the look is the scenario's.
    let plain = get(&state, "/checkout?scenario=tees", "viewer");
    let plain_id = session_cookie(&plain);
    let plain_path = header(&plain, "location").unwrap().to_string();
    assert!(!plain_path.contains('?'));
    let json = body_json(&get(&state, &format!("{plain_path}.json"), &plain_id));
    assert_eq!(json["variant"], "shop");
}

#[test]
fn every_opening_of_a_scenario_is_a_new_session() {
    let data = ScenarioData::new("fresh", &[("tees.json", TEES)]);
    let state = data.server(&[], false);
    let first = session_cookie(&get(&state, "/checkout?scenario=tees", "viewer"));
    let second = session_cookie(&get(&state, "/checkout?scenario=tees", "viewer"));
    assert_ne!(first, second);

    // Changing one checkout leaves the other, and the session that opened them, alone.
    let path = format!("/fr/checkouts/cn/{}", super::cart_token_of(&first));
    let reply = operations(
        &state,
        &path,
        &first,
        json!([{"type": "email", "value": "other@example.com"}]),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let other = format!("/fr/checkouts/cn/{}.json", super::cart_token_of(&second));
    assert_eq!(
        body_json(&get(&state, &other, &second))["buyer"]["email"],
        "gift@example.com"
    );
    let viewer = body_json(&get(&state, "/__lsf/session", "viewer"));
    assert_eq!(viewer["cart"]["items"], json!([]));
    assert_eq!(viewer["customer"], Json::Null);
}

#[test]
fn a_scenario_that_cannot_open_says_why() {
    let data = ScenarioData::new(
        "refused",
        &[
            ("tees.json", TEES),
            ("typo.json", r#"{"titel": "x"}"#),
            ("ghost.json", r#"{"extensions": ["gift-message", "ghost"]}"#),
        ],
    );
    let state = data.server(&[], true);

    let missing = get(&state, "/checkout?scenario=nope", "viewer");
    assert_eq!(missing.status, 404);
    let page = text(&missing);
    assert!(page.contains("No scenario named nope"), "{page}");
    assert!(page.contains("ghost, tees, typo"), "{page}");
    assert!(cookies(&missing).is_empty());

    let typo = get(&state, "/checkout?scenario=typo", "viewer");
    assert_eq!(typo.status, 422);
    let page = text(&typo);
    assert!(page.contains("unknown_field"), "{page}");
    assert!(page.contains("did you mean &quot;title&quot;?"), "{page}");
    assert!(page.contains("checkouts/typo.json"), "{page}");
    assert!(cookies(&typo).is_empty());

    let ghost = get(&state, "/checkout?scenario=ghost", "viewer");
    assert_eq!(ghost.status, 422);
    assert!(
        text(&ghost).contains("no extension named `ghost`"),
        "{}",
        text(&ghost)
    );
}

#[test]
fn a_scenario_picks_the_extensions_that_run() {
    let scenario = r#"{
        "session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}},
        "extensions": ["quantity-limit", "gift-message"]
    }"#;
    let data = ScenarioData::new("extensions", &[("few.json", scenario)]);
    let state = data.server(&[], true);
    let id = session_cookie(&get(&state, "/checkout?scenario=few", "viewer"));
    let path = format!("/checkouts/cn/{}.json", super::cart_token_of(&id));
    let json = body_json(&get(&state, &path, &id));
    let handles = |key: &str, field: &str| -> Vec<String> {
        json[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry[field].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(handles("extensions", "handle"), ["gift-message"]);
    let mut ran = handles("runs", "handle");
    ran.dedup();
    assert_eq!(ran, ["quantity-limit"]);

    // `--extension` is the limit: a scenario cannot widen it.
    let state = data.server(&["gift-message"], true);
    let id = session_cookie(&get(&state, "/checkout?scenario=few", "viewer"));
    let path = format!("/checkouts/cn/{}.json", super::cart_token_of(&id));
    let json = body_json(&get(&state, &path, &id));
    assert_eq!(json["runs"], json!([]));
    assert_eq!(json["extensions"].as_array().unwrap().len(), 1);
}

#[test]
fn the_scenario_stays_with_the_session_after_an_order() {
    let data = ScenarioData::new("order", &[("tees.json", TEES)]);
    let state = data.server(&[], true);
    let id = session_cookie(&get(&state, "/checkout?scenario=tees", "viewer"));
    let path = format!("/fr/checkouts/cn/{}", super::cart_token_of(&id));
    let reply = post(&state, &format!("{path}/complete"), json!({}), &id);
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(
        body_json(&get(&state, &format!("{path}.json"), &id))["status"],
        "completed"
    );

    // A cart again: a new checkout, still set up as the scenario says.
    let put = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}}),
        &id,
    ));
    assert_eq!(put.status, 200, "{}", text(&put));
    let json = body_json(&get(&state, &format!("{path}.json"), &id));
    assert_eq!(json["status"], "open");
    assert_eq!(json["scenario"]["name"], "tees");
    assert_eq!(
        json["deliveryGroups"][0]["options"][1]["cost"]["amount"],
        12.9
    );
}

#[test]
fn a_scenario_is_reloaded_when_its_file_changes() {
    let data = ScenarioData::new("reload", &[("tees.json", TEES)]);
    let state = data.server(&[], true);
    let id = session_cookie(&get(&state, "/checkout?scenario=tees", "viewer"));
    let path = format!("/fr/checkouts/cn/{}", super::cart_token_of(&id));
    assert_eq!(
        body_json(&get(&state, &format!("{path}.json"), &id))["cost"]["shipping"]["amount"],
        12.9
    );
    std::fs::write(
        data.0.join("checkouts/tees.json"),
        TEES.replace("\"12.90\"", "\"9.90\""),
    )
    .unwrap();
    std::fs::write(data.0.join("checkouts/new.json"), "{}").unwrap();
    state.reload();
    // The open checkout follows the scenario; a new file is a new scenario.
    assert_eq!(
        body_json(&get(&state, &format!("{path}.json"), &id))["cost"]["shipping"]["amount"],
        9.9
    );
    assert_eq!(get(&state, "/checkout?scenario=new", "viewer").status, 302);
}

// --- Logging in at the checkout ---

/// A checkout of one tee for a guest, in `session`.
fn guest_checkout(state: &ServerState, session: &str) -> String {
    let reply = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}}),
        session,
    ));
    assert_eq!(reply.status, 200, "{}", text(&reply));
    header(&get(state, "/checkout", session), "location")
        .expect("a location")
        .to_string()
}

fn log_in_as(state: &ServerState, path: &str, session: &str, who: Json) -> Json {
    let reply = operations(
        state,
        path,
        session,
        json!([{"type": "customer", "customer": who}]),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    body_json(&reply)
}

#[test]
fn the_checkout_lists_the_customers_of_the_data_to_log_in_as() {
    let state = checkout_server(&[], false);
    let path = guest_checkout(&state, "co-login-list");
    let json = body_json(&get(&state, &format!("{path}.json"), "co-login-list"));
    let profiles = json["account"]["profiles"].as_array().expect("profiles");
    let emails: Vec<&str> = profiles
        .iter()
        .map(|profile| profile["email"].as_str().unwrap())
        .collect();
    assert_eq!(
        emails,
        [
            "jane.doe@example.com",
            "new.customer@example.com",
            "alex.morgan@example.com",
            "camille.martin@example.com"
        ]
    );
    let jane = &profiles[0];
    assert_eq!(jane["name"], "Jane Doe");
    assert_eq!(jane["tags"], json!(["vip"]));
    assert_eq!(jane["ordersCount"], 2);
    assert_eq!(jane["addressesCount"], 2);
    assert_eq!(jane["current"], false);
    assert_eq!(profiles[2]["company"], "Northwind Hotels");
    assert_eq!(profiles[2]["locationsCount"], 2);
    assert_eq!(profiles[3]["countryCode"], "FR");
    assert_eq!(json["account"]["addresses"], json!([]));
    assert_eq!(json["buyer"]["customer"], Json::Null);
}

#[test]
fn logging_in_gives_the_checkout_the_contact_and_the_default_address_of_the_customer() {
    let state = checkout_server(&[], false);
    let session = "co-login-jane";
    let path = guest_checkout(&state, session);
    let json = log_in_as(&state, &path, session, json!("jane.doe@example.com"));
    assert_eq!(
        json["token"],
        path.rsplit('/').next().unwrap(),
        "the same checkout"
    );
    assert_eq!(json["buyer"]["customer"]["email"], "jane.doe@example.com");
    assert_eq!(json["buyer"]["email"], "jane.doe@example.com");
    assert_eq!(json["buyer"]["phone"], "+1 555 010 9988");
    assert_eq!(json["shippingAddress"]["address1"], "12 Maple Street");
    assert_eq!(json["localization"]["country"]["isoCode"], "US");
    // With an address, the shipping rates are there.
    assert!(
        !json["deliveryGroups"][0]["options"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let addresses = json["account"]["addresses"].as_array().unwrap();
    assert_eq!(addresses.len(), 2);
    assert_eq!(addresses[1]["address"]["address1"], "400 Pine Avenue");
    assert_eq!(json["account"]["profiles"][0]["current"], true);
    // The theme sees the same customer: the session is the browser's.
    let session_json = body_json(&get(&state, "/__lsf/session", session));
    assert_eq!(session_json["customer"], "jane.doe@example.com");

    // Another saved address is a shipping address like any other.
    let second = addresses[1]["address"].clone();
    let reply = operations(
        &state,
        &path,
        session,
        json!([{"type": "shippingAddress", "address": second}]),
    );
    assert_eq!(body_json(&reply)["shippingAddress"]["city"], "Seattle");

    // Logging out is a guest checkout: nothing of Jane is left, the cart is.
    let json = log_in_as(&state, &path, session, Json::Null);
    assert_eq!(json["buyer"]["customer"], Json::Null);
    assert_eq!(json["buyer"]["email"], Json::Null);
    assert_eq!(json["buyer"]["phone"], Json::Null);
    assert_eq!(json["shippingAddress"], Json::Null);
    assert_eq!(json["lines"].as_array().unwrap().len(), 1);
    assert_eq!(json["account"]["addresses"], json!([]));
    let session_json = body_json(&get(&state, "/__lsf/session", session));
    assert_eq!(session_json["customer"], Json::Null);
}

#[test]
fn an_address_the_guest_typed_stays_for_a_customer_without_one() {
    let state = checkout_server(&[], false);
    let session = "co-login-typed";
    let path = guest_checkout(&state, session);
    let reply = operations(&state, &path, session, buyer_details());
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let json = log_in_as(&state, &path, session, json!("new.customer@example.com"));
    assert_eq!(
        json["buyer"]["email"], "new.customer@example.com",
        "the account's, not the typed one"
    );
    assert_eq!(json["shippingAddress"]["address1"], "1 Analytical Way");
    assert_eq!(json["selectedPayment"], "card", "the choices stay");
    // A customer with an address of their own ships there instead.
    let json = log_in_as(&state, &path, session, json!("camille.martin@example.com"));
    assert_eq!(json["shippingAddress"]["city"], "Lyon");
    assert_eq!(json["localization"]["country"]["isoCode"], "FR");
}

#[test]
fn a_b2b_customer_buys_for_one_of_their_locations_and_ships_there() {
    let state = checkout_server(&[], false);
    let session = "co-login-b2b";
    let path = guest_checkout(&state, session);
    let json = log_in_as(&state, &path, session, json!("alex.morgan@example.com"));
    let purchasing = &json["buyer"]["purchasingCompany"];
    assert_eq!(purchasing["company"]["name"], "Northwind Hotels");
    assert_eq!(purchasing["location"]["name"], "Northwind Portland");
    assert_eq!(json["shippingAddress"]["address1"], "815 Harbor Way");
    let locations = json["account"]["locations"].as_array().unwrap();
    assert_eq!(locations.len(), 2);
    assert_eq!(locations[0]["current"], true);
    assert_eq!(
        json["account"]["addresses"][0]["location"],
        "Northwind Portland"
    );

    // Another location, by its id: the order ships there.
    let seattle = locations[1]["id"].clone();
    let reply = operations(
        &state,
        &path,
        session,
        json!([{"type": "companyLocation", "location": seattle}]),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let json = body_json(&reply);
    assert_eq!(
        json["buyer"]["purchasingCompany"]["location"]["name"],
        "Northwind Seattle"
    );
    assert_eq!(json["shippingAddress"]["address1"], "2200 Alder Street");
    assert_eq!(json["account"]["locations"][1]["current"], true);
    let session_json = body_json(&get(&state, "/__lsf/session", session));
    assert_eq!(session_json["company_location"], "Northwind Seattle");

    // Logging in with a location, by its name.
    let reply = operations(
        &state,
        &path,
        session,
        json!([{"type": "customer", "customer": Json::Null},
               {"type": "customer", "customer": "alex.morgan@example.com", "location": "Northwind Seattle"}]),
    );
    assert_eq!(
        body_json(&reply)["buyer"]["purchasingCompany"]["location"]["name"],
        "Northwind Seattle"
    );
}

#[test]
fn a_login_the_data_does_not_allow_is_refused_and_changes_nothing() {
    let state = checkout_server(&[], false);
    let session = "co-login-refused";
    let path = guest_checkout(&state, session);
    for (operations_json, message) in [
        (
            json!([{"type": "email", "value": "x@example.com"},
                   {"type": "customer", "customer": "nobody@example.com"}]),
            "there is no customer with the email",
        ),
        (
            json!([{"type": "customer", "customer": 42}]),
            "an email, or null",
        ),
        (
            json!([{"type": "companyLocation", "location": "Northwind Seattle"}]),
            "needs a logged-in customer",
        ),
        (
            json!([{"type": "customer", "customer": "jane.doe@example.com", "location": "Northwind Seattle"}]),
            "does not buy for a company",
        ),
        (
            json!([{"type": "customer", "customer": "alex.morgan@example.com", "location": "Northwind Boston"}]),
            "cannot buy for a location",
        ),
    ] {
        let reply = operations(&state, &path, session, operations_json);
        assert_eq!(reply.status, 400, "{}", text(&reply));
        assert!(text(&reply).contains(message), "{}", text(&reply));
    }
    let json = body_json(&get(&state, &format!("{path}.json"), session));
    assert_eq!(json["buyer"]["customer"], Json::Null);
    assert_eq!(json["buyer"]["email"], Json::Null);
}

#[test]
fn the_checkout_follows_a_login_made_elsewhere() {
    let state = checkout_server(&[], false);
    let session = "co-login-elsewhere";
    let path = guest_checkout(&state, session);
    let reply = operations(&state, &path, session, buyer_details());
    assert_eq!(reply.status, 200, "{}", text(&reply));
    // The login link of the control API, as the storefront's account page uses it.
    let reply = get(
        &state,
        "/__lsf/login?customer=jane.doe@example.com&return_to=/cart",
        session,
    );
    assert_eq!(reply.status, 302);
    let json = body_json(&get(&state, &format!("{path}.json"), session));
    assert_eq!(json["buyer"]["email"], "jane.doe@example.com");
    assert_eq!(json["shippingAddress"]["address1"], "12 Maple Street");
    // And reading it again changes nothing more.
    let reply = operations(
        &state,
        &path,
        session,
        json!([{"type": "shippingAddress", "address": {"lastName": "Doe", "address1": "1 Elsewhere", "city": "Ottawa", "countryCode": "CA"}}]),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let json = body_json(&get(&state, &format!("{path}.json"), session));
    assert_eq!(json["shippingAddress"]["address1"], "1 Elsewhere");
}

#[test]
fn checkout_with_a_customer_in_the_query_opens_it_logged_in_as_them() {
    let state = checkout_server(&[], false);
    let session = "co-login-query";
    guest_checkout(&state, session);
    let reply = get(
        &state,
        "/checkout?customer=camille.martin@example.com",
        session,
    );
    assert_eq!(reply.status, 302);
    let path = header(&reply, "location").unwrap().to_string();
    let json = body_json(&get(&state, &format!("{path}.json"), session));
    assert_eq!(
        json["buyer"]["customer"]["email"],
        "camille.martin@example.com"
    );
    assert_eq!(json["shippingAddress"]["city"], "Lyon");
    // `none` is a guest again.
    let reply = get(&state, "/checkout?customer=none", session);
    let path = header(&reply, "location").unwrap().to_string();
    let json = body_json(&get(&state, &format!("{path}.json"), session));
    assert_eq!(json["buyer"]["customer"], Json::Null);
    // Someone the data does not have.
    let reply = get(&state, "/checkout?customer=nobody@example.com", session);
    assert_eq!(reply.status, 404);
    assert!(
        text(&reply).contains("jane.doe@example.com"),
        "{}",
        text(&reply)
    );
}

#[test]
fn a_scenario_opens_logged_in_as_the_customer_of_the_query() {
    let data = ScenarioData::new(
        "login",
        &[(
            "tees.json",
            r#"{"session": {"customer": "none", "cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}}"#,
        )],
    );
    let state = data.server(&[], false);
    let reply = get(
        &state,
        "/checkout?scenario=tees&customer=default",
        "co-login-scenario",
    );
    assert_eq!(reply.status, 302, "{}", text(&reply));
    let session = session_cookie(&reply);
    let path = header(&reply, "location").unwrap().to_string();
    let json = body_json(&get(&state, &format!("{path}.json"), &session));
    assert_eq!(json["scenario"]["name"], "tees");
    assert_eq!(json["buyer"]["customer"]["email"], "jane.doe@example.com");
    assert_eq!(json["lines"][0]["quantity"], 2);
    // Switching customer changes the session, not the scenario it started from.
    let json = log_in_as(&state, &path, &session, json!("new.customer@example.com"));
    assert_eq!(json["scenario"]["name"], "tees");
    assert_eq!(
        json["buyer"]["customer"]["email"],
        "new.customer@example.com"
    );
}

/// A guest scenario that enters a whole contact: email, phone and an address.
const ENTERED_CONTACT: &str = r#"{
    "session": {"customer": "none", "cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}},
    "entered": {
        "email": "typed@example.com",
        "phone": "+1 555 000 1111",
        "shipping_address": {"last_name": "Typed", "address1": "1 Analytical Way", "city": "Ottawa", "country_code": "CA", "zip": "K1A 0B1"}
    }
}"#;

/// What a login changes of the checkout, to compare one opened from a URL with one logged in
/// from the menu.
fn login_view(json: &Json) -> Json {
    json!({
        "buyer": json["buyer"],
        "shippingAddress": json["shippingAddress"],
        "country": json["localization"]["country"]["isoCode"],
        "addresses": json["account"]["addresses"],
        "scenario": json["scenario"],
        "lines": json["lines"].as_array().map(Vec::len),
    })
}

/// The checkout of `/checkout?scenario=<scenario>&customer=<who>`, and the one of
/// `/checkout?scenario=<scenario>` logged in as `who` from the menu.
fn url_and_menu_logins(state: &ServerState, scenario: &str, who: &str) -> (Json, Json) {
    let reply = get(
        state,
        &format!("/checkout?scenario={scenario}&customer={who}"),
        "co-login-url",
    );
    assert_eq!(reply.status, 302, "{}", text(&reply));
    let session = session_cookie(&reply);
    let path = header(&reply, "location").unwrap().to_string();
    let by_url = body_json(&get(state, &format!("{path}.json"), &session));

    let reply = get(
        state,
        &format!("/checkout?scenario={scenario}"),
        "co-login-menu",
    );
    let session = session_cookie(&reply);
    let path = header(&reply, "location").unwrap().to_string();
    body_json(&get(state, &format!("{path}.json"), &session));
    let by_menu = log_in_as(state, &path, &session, json!(who));
    (by_url, by_menu)
}

#[test]
fn a_customer_in_the_query_of_a_scenario_logs_in_as_the_menu_does() {
    let data = ScenarioData::new(
        "login-entered",
        &[
            ("entered.json", ENTERED_CONTACT),
            (
                "jane-entered.json",
                r#"{
                    "session": {"customer": "jane.doe@example.com", "cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}},
                    "entered": {"email": "orders@example.com"}
                }"#,
            ),
        ],
    );
    let state = data.server(&[], false);

    // Jane has addresses: the login brings her contact and her default address, whatever the
    // scenario entered for the guest before.
    let (by_url, by_menu) = url_and_menu_logins(&state, "entered", "jane.doe@example.com");
    assert_eq!(login_view(&by_url), login_view(&by_menu));
    assert_eq!(by_url["buyer"]["customer"]["email"], "jane.doe@example.com");
    assert_eq!(by_url["buyer"]["email"], "jane.doe@example.com");
    assert_eq!(by_url["buyer"]["phone"], "+1 555 010 9988");
    assert_eq!(by_url["shippingAddress"]["address1"], "12 Maple Street");
    assert_eq!(by_url["localization"]["country"]["isoCode"], "US");

    // A customer without an address keeps the one the scenario entered; the contact is theirs.
    let (by_url, by_menu) = url_and_menu_logins(&state, "entered", "new.customer@example.com");
    assert_eq!(login_view(&by_url), login_view(&by_menu));
    assert_eq!(by_url["buyer"]["email"], "new.customer@example.com");
    assert_eq!(by_url["buyer"]["phone"], Json::Null);
    assert_eq!(by_url["shippingAddress"]["address1"], "1 Analytical Way");
    assert_eq!(by_url["localization"]["country"]["isoCode"], "CA");

    // The customer the scenario is already logged in as: nothing changes, its entries stay.
    let (by_url, by_menu) = url_and_menu_logins(&state, "jane-entered", "jane.doe@example.com");
    assert_eq!(login_view(&by_url), login_view(&by_menu));
    assert_eq!(by_url["buyer"]["email"], "orders@example.com");
    assert_eq!(by_url["shippingAddress"]["address1"], "12 Maple Street");
}

#[test]
fn a_logged_in_checkout_keeps_the_contact_email_a_scenario_entered() {
    let data = ScenarioData::new(
        "login-contact",
        &[(
            "jane-entered.json",
            r#"{
                "session": {"customer": "jane.doe@example.com", "cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}},
                "entered": {"email": "orders@example.com"}
            }"#,
        )],
    );
    let state = data.server(&[], false);
    let reply = get(
        &state,
        "/checkout?scenario=jane-entered",
        "co-login-contact",
    );
    let session = session_cookie(&reply);
    let path = header(&reply, "location").unwrap().to_string();
    let json = body_json(&get(&state, &format!("{path}.json"), &session));
    // The contact is the scenario's; the account is Jane's: the page shows both.
    assert_eq!(json["buyer"]["email"], "orders@example.com");
    assert_eq!(json["buyer"]["customer"]["email"], "jane.doe@example.com");
    // Emptying the contact email gives the account's back.
    let reply = operations(
        &state,
        &path,
        &session,
        json!([{"type": "email", "value": Json::Null}]),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(body_json(&reply)["buyer"]["email"], "jane.doe@example.com");
    // And another one can be typed while logged in.
    let reply = operations(
        &state,
        &path,
        &session,
        json!([{"type": "email", "value": "billing@example.com"}]),
    );
    let json = body_json(&reply);
    assert_eq!(json["buyer"]["email"], "billing@example.com");
    assert_eq!(json["buyer"]["customer"]["email"], "jane.doe@example.com");
}

#[test]
fn the_functions_read_the_customer_the_checkout_switched_to() {
    // A copy of the fixture app whose payment Function also asks for the buyer.
    let root = std::env::temp_dir().join(format!("lsf-checkout-fn-buyer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_directory(&fixture_app(), &root);
    let query = root.join("extensions/payment-rules/src/run.graphql");
    let source = std::fs::read_to_string(&query).unwrap();
    std::fs::write(
        &query,
        source.replacen(
            "  cart {",
            r#"  cart {
    buyerIdentity {
      email
      isAuthenticated
      customer {
        id email displayName numberOfOrders
        amountSpent { amount currencyCode }
        vip: hasAnyTag(tags: ["vip"])
        tags: hasTags(tags: ["vip", "wholesale"]) { tag hasTag }
        tier: metafield(namespace: "custom", key: "loyalty_tier") { value }
      }
      purchasingCompany { company { name } location { name } }
    }"#,
            1,
        ),
    )
    .unwrap();
    let state = checkout_server_of(root.clone(), &["payment-rules"], false);
    let session = "co-fn-buyer";
    let path = guest_checkout(&state, session);
    let buyer = |json: &Json| {
        let run = json["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["handle"] == "payment-rules")
            .expect("the run of the payment Function")
            .clone();
        assert_eq!(run["error"], Json::Null, "{run}");
        run["input"]["cart"]["buyerIdentity"].clone()
    };

    let json = body_json(&get(&state, &format!("{path}.json"), session));
    assert_eq!(buyer(&json)["customer"], Json::Null);
    assert_eq!(buyer(&json)["isAuthenticated"], false);

    let identity = buyer(&log_in_as(
        &state,
        &path,
        session,
        json!("jane.doe@example.com"),
    ));
    assert_eq!(identity["isAuthenticated"], true);
    assert_eq!(identity["email"], "jane.doe@example.com");
    let customer = &identity["customer"];
    assert_eq!(customer["email"], "jane.doe@example.com");
    assert_eq!(customer["displayName"], "Jane Doe");
    assert_eq!(customer["numberOfOrders"], 2);
    assert_eq!(customer["vip"], true);
    assert_eq!(
        customer["tags"],
        json!([{"tag": "vip", "hasTag": true}, {"tag": "wholesale", "hasTag": false}])
    );
    assert_eq!(customer["tier"]["value"], "gold");
    assert!(
        customer["amountSpent"]["amount"]
            .as_str()
            .is_some_and(|amount| amount != "0.0")
    );
    assert_eq!(identity["purchasingCompany"], Json::Null);

    let identity = buyer(&log_in_as(
        &state,
        &path,
        session,
        json!("alex.morgan@example.com"),
    ));
    assert_eq!(identity["customer"]["vip"], false);
    assert_eq!(identity["customer"]["numberOfOrders"], 0);
    assert_eq!(
        identity["purchasingCompany"]["company"]["name"],
        "Northwind Hotels"
    );
    assert_eq!(
        identity["purchasingCompany"]["location"]["name"],
        "Northwind Portland"
    );

    let identity = buyer(&log_in_as(&state, &path, session, Json::Null));
    assert_eq!(identity["customer"], Json::Null);
    assert_eq!(identity["isAuthenticated"], false);
    let _ = std::fs::remove_dir_all(root);
}

// --- The home page ---

/// The same fixture app with an extra Function that has a `.fetch` target and an extra checkout
/// UI extension that is not built, declares a target the page has no place for, and an API
/// version that is not supported.
fn app_with_oddities(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("lsf-home-app-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_directory(&fixture_app(), &root);
    let write = |handle: &str, toml: &str| {
        let directory = root.join("extensions").join(handle);
        std::fs::create_dir_all(directory.join("src")).unwrap();
        std::fs::write(directory.join("shopify.extension.toml"), toml).unwrap();
        directory
    };
    let remote = write(
        "remote-rules",
        r#"api_version = "2026-10"

[[extensions]]
name = "Remote rules"
handle = "remote-rules"
type = "function"

  [[extensions.targeting]]
  target = "cart.validations.generate.run"
  input_query = "src/run.graphql"
  export = "run"

  [[extensions.targeting]]
  target = "cart.validations.generate.fetch"
  input_query = "src/fetch.graphql"
  export = "fetch"
"#,
    );
    std::fs::write(
        remote.join("src/run.graphql"),
        "query Input { cart { lines { quantity } } }",
    )
    .unwrap();
    std::fs::write(
        remote.join("src/fetch.graphql"),
        "query Input { cart { lines { quantity } } }",
    )
    .unwrap();
    write(
        "chat-widget",
        r#"api_version = "2025-07"

[[extensions]]
name = "Chat widget"
handle = "chat-widget"
type = "ui_extension"

  [[extensions.targeting]]
  target = "purchase.checkout.block.render"
  module = "./src/Block.jsx"

  [[extensions.targeting]]
  target = "purchase.checkout.chat.render"
  module = "./src/Chat.jsx"
"#,
    );
    write(
        "theme-widget",
        "api_version = \"2026-10\"\n\n[[extensions]]\nname = \"Widget\"\nhandle = \"theme-widget\"\ntype = \"theme\"\n",
    );
    root
}

#[test]
fn the_home_page_lists_every_extension_of_the_app() {
    let data = ScenarioData::new("home-extensions", &[]);
    let state = data.server(&[], true);
    let page = get(&state, "/", "home");
    assert_eq!(page.status, 200);
    assert!(
        header(&page, "content-type").is_some_and(|kind| kind.starts_with("text/html")),
        "{:?}",
        page.headers
    );
    let html = text(&page);
    // It wears the checkout's stylesheet and needs no script and no other host.
    assert!(html.contains("/__lsf/checkout/assets/checkout.css"));
    assert!(!html.contains("<script"));
    assert!(!html.contains("http://") && !html.contains("https://"));
    for handle in [
        "volume-discount",
        "delivery-rules",
        "payment-rules",
        "quantity-limit",
        "bundle-transform",
    ] {
        assert!(
            html.contains(&format!("data-function=\"{handle}\"")),
            "{handle}"
        );
    }
    for handle in ["gift-message", "order-notice"] {
        assert!(
            html.contains(&format!("data-ui-extension=\"{handle}\"")),
            "{handle}"
        );
    }
    // What each one is: its API version, targets and the places of the targets.
    assert!(html.contains("API 2027-01") && html.contains("API 2026-10"));
    assert!(html.contains("cart.lines.discounts.generate.run"));
    assert!(html.contains("discount API"));
    assert!(html.contains("purchase.checkout.reductions.render-after"));
    assert!(html.contains("Checkout summary: under the discount code field"));
    // And how to see them all.
    assert!(html.contains("href=\"/checkout?targets=all\""));
    assert!(html.contains("href=\"/checkout?variant=shop\""));

    let json = body_json(&get(&state, "/?format=json", "home"));
    let function = |handle: &str| {
        json["functions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|function| function["handle"] == handle)
            .unwrap_or_else(|| panic!("no {handle}"))
            .clone()
    };
    let discount = function("volume-discount");
    assert_eq!(discount["apiVersion"], "2026-10");
    assert_eq!(discount["built"], true);
    assert_eq!(discount["network"], false);
    assert_eq!(discount["apis"], json!(["discount"]));
    assert_eq!(discount["selected"], true);
    // The legacy target of the validation Function is said to be one.
    let validation = function("quantity-limit");
    assert_eq!(validation["targets"][0]["legacy"], true);
    assert_eq!(validation["problems"][0]["severity"], "warning");
}

#[test]
fn the_home_page_says_what_is_wrong_with_the_extensions() {
    let app = app_with_oddities("wrong");
    let state = checkout_server_with_data(app.clone(), &["volume-discount"], true, None);
    let json = body_json(&get(&state, "/?format=json", "home"));
    std::fs::remove_dir_all(&app).ok();

    let find = |list: &str, handle: &str| {
        json[list]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["handle"] == handle)
            .unwrap_or_else(|| panic!("no {handle} in {list}"))
            .clone()
    };
    // A Function with a `.fetch` target can reach the network, and is not built.
    let remote = find("functions", "remote-rules");
    assert_eq!(remote["network"], true);
    assert_eq!(remote["built"], false);
    assert_eq!(remote["selected"], false);
    assert!(
        remote["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|problem| problem["message"]
                .as_str()
                .unwrap()
                .contains("no Wasm module")),
        "{remote}"
    );
    assert_eq!(find("functions", "volume-discount")["selected"], true);

    // An extension on an API version the checkout does not run, and a target without a place.
    let chat = find("uiExtensions", "chat-widget");
    assert_eq!(chat["supported"], false);
    assert_eq!(chat["built"], false);
    assert_eq!(
        chat["targets"][0]["place"],
        "Checkout: a block between the shipping method and the payment"
    );
    assert_eq!(chat["targets"][1]["place"], Json::Null);
    assert!(
        chat["problems"].as_array().unwrap().iter().any(|problem| {
            problem["severity"] == "error"
                && problem["message"].as_str().unwrap().contains("2025-07")
        }),
        "{chat}"
    );
    assert_eq!(
        json["otherExtensions"],
        json!([{"handle": "theme-widget", "type": "theme"}])
    );

    let app = app_with_oddities("wrong-html");
    let state = checkout_server_with_data(app.clone(), &[], true, None);
    let html = text(&get(&state, "/", "home"));
    std::fs::remove_dir_all(&app).ok();
    assert!(html.contains("no place on the page"), "{html}");
    assert!(
        html.contains("has no place for purchase.checkout.chat.render"),
        "{html}"
    );
    assert!(html.contains("network access"));
    assert!(html.contains("not built"));
    assert!(html.contains("API 2025-07 is not supported"));
    assert!(html.contains("Other extensions"));
}

#[test]
fn the_home_page_lists_the_scenarios_and_the_ones_with_errors() {
    let broken = r#"{"session": {"cart": {"items": [{"variant": "NOPE"}]}}}"#;
    let data = ScenarioData::new(
        "home-scenarios",
        &[
            ("tees.json", TEES),
            ("broken.json", broken),
            ("unreadable.json", "{ nope"),
        ],
    );
    let state = data.server(&[], true);
    let json = body_json(&get(&state, "/?format=json", "home"));
    let scenarios = json["scenarios"].as_array().unwrap();
    let names: Vec<&str> = scenarios
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["broken", "tees", "unreadable"]);

    let tees = &scenarios[1];
    assert_eq!(tees["title"], "Three tees in French");
    assert_eq!(tees["open"], "/checkout?scenario=tees");
    assert_eq!(
        tees["openAllTargets"],
        "/checkout?scenario=tees&targets=all"
    );
    assert_eq!(tees["problems"], json!([]));
    let summary: HashMap<&str, &str> = tees["summary"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["label"].as_str().unwrap(),
                row["text"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary["Cart"],
        "3 × Organic Cotton T-Shirt (Black / M) · note “A gift”"
    );
    assert_eq!(summary["Buyer"], "customer jane.doe@example.com");
    assert_eq!(summary["Country, language"], "FR, French");
    assert!(
        summary["Entered"].contains("email gift@example.com"),
        "{summary:?}"
    );
    assert!(
        summary["Entered"].contains("address 1 rue de Rivoli, Paris, FR"),
        "{summary:?}"
    );
    assert_eq!(summary["Discounts"], "codes entered: WELCOME");
    assert_eq!(
        summary["Overrides"],
        "shipping Standard 4.90 USD, Express 12.90 USD · tax 20%"
    );
    assert_eq!(summary["Look"], "shop");

    // A scenario with errors is listed with them, and has nowhere to go.
    for name in [0, 2] {
        assert_eq!(scenarios[name]["open"], Json::Null);
        assert_eq!(scenarios[name]["summary"], json!([]));
    }
    assert!(
        scenarios[0]["problems"][0]
            .as_str()
            .unwrap()
            .contains("unknown_variant")
    );
    assert!(
        scenarios[2]["problems"][0]
            .as_str()
            .unwrap()
            .contains("invalid_json")
    );

    let html = text(&get(&state, "/", "home"));
    assert!(html.contains("data-scenario=\"tees\""));
    assert!(html.contains("href=\"/checkout?scenario=tees\""));
    assert!(html.contains("href=\"/checkout?scenario=tees&amp;targets=all\""));
    assert!(html.contains("Three tees in French"));
    assert!(html.contains("data-scenario=\"broken\""));
    assert!(html.contains("has errors"));
    assert!(
        html.contains("checkouts/broken.json at /session/cart/items/0/variant"),
        "{html}"
    );
    assert!(!html.contains("href=\"/checkout?scenario=broken\""));
    // The link of a scenario opens it.
    assert_eq!(get(&state, "/checkout?scenario=tees", "home").status, 302);
}

#[test]
fn the_home_page_says_how_to_get_scenarios() {
    let state = checkout_server(&[], true);
    let html = text(&get(&state, "/", "home"));
    assert!(html.contains("None yet"), "{html}");
    assert!(html.contains("lsf checkout init --app"), "{html}");
}

#[test]
fn the_customers_of_the_home_page_open_the_first_scenario_with_its_cart() {
    // The session of the data may have an empty cart: the link gives the checkout one.
    let data = ScenarioData::new(
        "customer-links",
        &[
            ("a-abroad.json", TEES),
            (
                "b-home.json",
                r#"{ "session": { "cart": { "items": [{ "variant": "TEE-BLK-M", "quantity": 3 }] } } }"#,
            ),
        ],
    );
    let state = data.server(&[], true);
    let json = body_json(&get(&state, "/?format=json", "home"));
    let jane = &json["customers"][0];
    // Not the French one, though it comes first: the one in the store's own language.
    assert_eq!(
        jane["open"],
        "/checkout?scenario=b-home&customer=jane.doe%40example.com"
    );
    let reply = get(&state, text_of_json(&jane["open"]), "customer-link");
    assert_eq!(reply.status, 302);
    let id = session_cookie(&reply);
    let location = header(&reply, "location").unwrap().to_string();
    let checkout = body_json(&get(&state, &format!("{location}.json"), &id));
    assert_eq!(checkout["lines"][0]["quantity"], 3);
    assert_eq!(
        checkout["buyer"]["customer"]["email"],
        "jane.doe@example.com"
    );
}

#[test]
fn the_home_page_lists_the_customers_to_log_in_as() {
    let state = checkout_server(&[], true);
    let json = body_json(&get(&state, "/?format=json", "home"));
    let customers = json["customers"].as_array().expect("customers");
    assert_eq!(customers.len(), 4);
    assert_eq!(customers[0]["email"], "jane.doe@example.com");
    assert_eq!(
        customers[0]["open"],
        "/checkout?customer=jane.doe%40example.com"
    );
    assert_eq!(customers[2]["company"], "Northwind Hotels");
    let html = text(&get(&state, "/", "home"));
    assert!(html.contains("Log in as a customer"), "{html}");
    assert!(
        html.contains("<li data-customer=\"alex.morgan@example.com\">"),
        "{html}"
    );
    assert!(html.contains("B2B · Northwind Hotels"), "{html}");
    // The link opens the checkout logged in as them.
    let reply = get(&state, text_of_json(&customers[3]["open"]), "home-login");
    let path = header(&reply, "location").expect("a location").to_string();
    let checkout = body_json(&get(&state, &format!("{path}.json"), "home-login"));
    assert_eq!(
        checkout["buyer"]["customer"]["email"],
        "camille.martin@example.com"
    );
}

fn text_of_json(value: &Json) -> &str {
    value.as_str().expect("a string")
}

#[test]
fn the_home_page_follows_the_language_of_the_url() {
    let data = ScenarioData::new("home-fr", &[("tees.json", TEES)]);
    let state = data.server(&[], true);
    let json = body_json(&get(&state, "/fr/?format=json", "home"));
    assert_eq!(json["scenarios"][0]["open"], "/fr/checkout?scenario=tees");
    assert_eq!(json["links"]["checkout"], "/fr/checkout");
    assert_eq!(json["links"]["allTargets"], "/fr/checkout?targets=all");
    // A checkout started there is in French, with the page's own parameters kept.
    let reply = get(&state, "/fr/checkout?targets=all", "home");
    assert_eq!(
        header(&reply, "location"),
        Some(
            format!(
                "/fr/checkouts/cn/{}?targets=all",
                super::cart_token_of("home")
            )
            .as_str()
        )
    );
}

#[test]
fn with_a_theme_the_home_page_is_under_the_control_api() {
    let data = ScenarioData::new("home-theme", &[("tees.json", TEES)]);
    let state = data.server(&[], false);
    // The theme owns `/`.
    let storefront = text(&get(&state, "/", "home"));
    assert!(!storefront.contains("data-section=\"scenarios\""));
    let html = text(&get(&state, "/__lsf/checkouts", "home"));
    assert!(html.contains("data-scenario=\"tees\""));
    let json = body_json(&get(&state, "/__lsf/checkouts?format=json", "home"));
    assert_eq!(json["scenarios"][0]["open"], "/checkout?scenario=tees");
    let french = body_json(&get(
        &state,
        "/__lsf/checkouts?format=json&locale=fr",
        "home",
    ));
    assert_eq!(french["scenarios"][0]["open"], "/fr/checkout?scenario=tees");
    // A notice leads back to it.
    let missing = text(&get(&state, "/checkout?scenario=nope", "home"));
    assert!(missing.contains("href=\"/__lsf/checkouts\""), "{missing}");
    // Without an app there is nothing to list.
    assert_eq!(get(&server(), "/__lsf/checkouts", "home").status, 404);
}

#[test]
fn every_scenario_that_init_writes_opens_and_runs_the_app() {
    let root = std::env::temp_dir().join(format!("lsf-server-init-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    crate::commands::checkout_init::run(
        &root,
        Some(&data),
        crate::commands::checkout_init::InitArgs::for_app(fixture_app()),
    )
    .unwrap();
    let state = checkout_server_with_data(fixture_app(), &[], true, Some(&data));
    let home = body_json(&get(&state, "/?format=json", "init"));
    let scenarios = home["scenarios"].as_array().unwrap();
    assert!(scenarios.len() >= 12, "{}", scenarios.len());
    for scenario in scenarios {
        let name = scenario["name"].as_str().unwrap();
        assert_eq!(scenario["problems"], json!([]), "{name}");
        let opened = get(&state, scenario["open"].as_str().unwrap(), "init");
        assert_eq!(opened.status, 302, "{name}: {}", text(&opened));
        let id = session_cookie(&opened);
        let location = header(&opened, "location").unwrap().to_string();
        let json = body_json(&get(&state, &format!("{location}.json"), &id));
        assert!(
            !json["lines"].as_array().unwrap().is_empty(),
            "{name} has a cart"
        );
        for run in json["runs"].as_array().unwrap() {
            assert_eq!(run["error"], Json::Null, "{name}: {}", run["handle"]);
        }
        assert_eq!(json["scenario"]["name"], *name);
        // The page itself, and with every target.
        assert_eq!(get(&state, &location, &id).status, 200, "{name}");
    }
    std::fs::remove_dir_all(&root).ok();
}
