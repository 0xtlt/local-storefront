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
    let app = App::open(&theme, None, Revalidate::Never).expect("fixture theme");
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
    let app = App::open(&theme, None, Revalidate::Never).expect("fixture theme");
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
            compress: true,
            minify: true,
            throttle: Default::default(),
            timings: false,
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
    // The fixture theme writes its own rules in `templates/robots.txt.liquid`.
    assert!(
        body.contains("Disallow: /*?q=*\nDisallow: /*?view=\n"),
        "{body}"
    );

    // A theme without that template gets what Shopify serves by default.
    let root = changing_theme("robots");
    let app = App::open(&root, None, Revalidate::Never).unwrap();
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

    let app = App::open(&root, None, Revalidate::Every(Duration::ZERO)).unwrap();
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
    let app = App::open(&root, None, Revalidate::Never).unwrap();
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
        let app = App::open(&root, None, Revalidate::Never).unwrap();
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
    let app = App::open(&root, None, Revalidate::Every(Duration::from_secs(60))).unwrap();
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
    let app = App::open(&theme, None, Revalidate::Never).expect("fixture theme");
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
    let app = App::open(&theme, None, Revalidate::Never).expect("fixture theme");
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
