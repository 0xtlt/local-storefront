//! Tests of the server's handlers, run without a network: requests are dispatched directly.

use std::collections::HashMap;
use std::path::Path;

use lsf_core::theme::Revalidate;
use serde_json::{Value as Json, json};

use super::reply::Reply;
use super::throttle::Throttle;
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
            throttle: Default::default(),
            customer: None,
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
    let loader = get(
        &state,
        "/cdn/shopifycloud/storefront/assets/storefront/load_feature.js",
        "platform",
    );
    assert!(text(&loader).contains("Shopify.loadFeatures = loadFeatures;"));

    // A section on its own is not a page: nothing is added to it.
    let section = text(&get(&state, "/?section_id=footer", "platform"));
    assert!(!section.contains("standard-actions.js"));
}

/// A server whose options were given on the command line.
fn server_with_throttle(rules: &str) -> ServerState {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
    let app = App::open(&theme, None, Revalidate::Never).expect("fixture theme");
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            throttle: Throttle::parse([rules]).expect("valid rules"),
            customer: None,
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
    let app = App::open(&theme, None, Revalidate::Never).expect("fixture theme");
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            throttle: Default::default(),
            customer: Some("alex.morgan@example.com".to_string()),
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
