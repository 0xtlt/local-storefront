//! Tests of the server's handlers, run without a network: requests are dispatched directly.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Value as Json, json};
use slt_core::theme::Revalidate;

use super::reply::Reply;
use super::{Incoming, SESSION_HEADER, ServeOptions, ServerState};
use crate::app::App;

fn server() -> ServerState {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
    let app = App::open(&theme, None, Revalidate::Never).expect("fixture theme");
    let (state, diagnostics) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
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
fn a_new_visitor_gets_a_session_cookie() {
    let state = server();
    let mut incoming = request("GET", "/", json!({}), "ignored");
    incoming.headers.remove(SESSION_HEADER);
    let reply = state.dispatch(&incoming);
    assert_eq!(reply.status, 200);
    assert!(header(&reply, "set-cookie").is_some_and(|cookie| cookie.starts_with("_slt_session=")));
    assert_eq!(header(&reply, "x-slt-template"), Some("index"));
    assert_eq!(header(&reply, "x-slt-liquid-errors"), None);
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
        "/__slt/session",
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

    let session = body_json(&get(&state, "/__slt/session", "controlled"));
    assert_eq!(session["customer"], "jane.doe@example.com");
    assert_eq!(session["custom_data"], true);

    state.dispatch(&request(
        "DELETE",
        "/__slt/session",
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
        "/__slt/session",
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
        "/__slt/session",
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
        "/__slt/session",
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
    assert_eq!(header(&wrong, "location"), Some("/account/login"));
    assert_eq!(
        body_json(&get(&state, "/__slt/session", "login"))["customer"],
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
        body_json(&get(&state, "/__slt/session", "login"))["customer"],
        "jane.doe@example.com"
    );

    get(&state, "/account/logout", "login");
    assert_eq!(
        body_json(&get(&state, "/__slt/session", "login"))["customer"],
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
        body_json(&get(&state, "/__slt/session", "locale"))["country"],
        "CA"
    );
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
    assert_eq!(header(&image, "x-slt-placeholder"), Some("1"));
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
        "/cdn/shopifycloud/storefront/assets/themes_support/gift-card/card.svg",
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
    let status = body_json(&get(&state, "/__slt/status", "status"));
    assert_eq!(status["counts"]["products"], 8);
    assert_eq!(status["data_diagnostics"]["ok"], true);
    assert!(
        status["routes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|route| route == "/products/ceramic-mug")
    );
    assert!(get(&state, "/__slt/schema/product", "status").status == 200);
}
