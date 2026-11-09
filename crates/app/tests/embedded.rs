//! The admin an embedded app is previewed in, through `AdminApi::handle`: its page at
//! `/admin/apps/<app>`, the pages of the admin lsf does not have, lsf's App Bridge and the files
//! of the page, and what the resource picker lists.

mod support;

use std::time::Duration;

use lsf_app::admin::api::{
    AdminApi, AdminOptions, HttpRequest, HttpResponse, Preview, ThrottleSetting,
};
use lsf_app::admin::auth::session_token;
use lsf_app::admin::webhooks::DeliveryOptions;
use serde_json::Value;
use support::json_of;

const SECRET: &str = "test-secret";

fn api_with(app: Option<lsf_app::config::App>, app_url: Option<&str>) -> AdminApi {
    AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app,
            client_secret: SECRET.to_string(),
            app_url: app_url.map(str::to_string),
            throttle: ThrottleSetting::Off,
            delivery: DeliveryOptions {
                timeout: Duration::from_secs(1),
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(7),
        },
    )
}

/// The demo store with the app of `tests/fixtures/admin-app`, its backend on localhost:3000.
fn api() -> AdminApi {
    api_with(Some(support::fixture_app()), Some("http://localhost:3000"))
}

fn get(api: &AdminApi, path: &str) -> HttpResponse {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    api.handle(&HttpRequest {
        method: "GET".to_string(),
        path: path.to_string(),
        query: form_urlencoded::parse(query.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect(),
        headers: [("host".to_string(), "127.0.0.1:9292".to_string())].into(),
        body: Vec::new(),
    })
    .unwrap_or_else(|| panic!("{path} is not routed"))
}

fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn text(response: &HttpResponse) -> String {
    String::from_utf8_lossy(&response.body).into_owned()
}

/// The JSON `admin.js` reads, from the page.
fn page_config(html: &str) -> Value {
    let start = html
        .find("<script type=\"application/json\" id=\"lsf-admin-config\">")
        .expect("the page has its config")
        + "<script type=\"application/json\" id=\"lsf-admin-config\">".len();
    let end = start + html[start..].find("</script>").unwrap();
    serde_json::from_str(&html[start..end]).expect("the config is JSON")
}

/// The `src` of the app's frame.
fn frame_src(html: &str) -> String {
    let start = html
        .find("class=\"lsf-frame\"")
        .expect("the page has the app's frame");
    let src = start + html[start..].find("src=\"").unwrap() + 5;
    let end = src + html[src..].find('"').unwrap();
    html[src..end].replace("&amp;", "&")
}

fn query_of(url: &str) -> Vec<(String, String)> {
    let query = url.split_once('?').map_or("", |(_, query)| query);
    form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn opens_the_app_in_a_frame_at_a_new_launch_url_each_time() {
    let api = api();
    let first = get(&api, "/admin/apps/admin-fixtures");
    assert_eq!(first.status, 200);
    assert_eq!(
        header(&first, "content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(header(&first, "cache-control"), Some("no-store"));
    let html = text(&first);
    assert!(html.contains("<title>Admin fixtures · "), "{html}");
    assert!(
        html.contains("<script type=\"module\" src=\"/__lsf/admin/assets/admin.js\"></script>")
    );
    assert!(html.contains("href=\"/__lsf/admin/assets/admin.css\""));
    assert!(html.contains("Local admin"));
    assert!(!html.contains("{{"), "every placeholder is filled");

    let src = frame_src(&html);
    assert!(src.starts_with("http://localhost:3000?"), "{src}");
    let params = query_of(&src);
    for name in [
        "embedded",
        "hmac",
        "host",
        "id_token",
        "locale",
        "session",
        "shop",
        "timestamp",
    ] {
        assert!(param(&params, name).is_some(), "{name} in {src}");
    }
    let shop = api.default_shop();
    assert_eq!(param(&params, "shop"), Some(shop.as_str()));
    assert_eq!(param(&params, "embedded"), Some("1"));
    let token = param(&params, "id_token").unwrap();
    let claims = session_token::verify(
        token,
        SECRET,
        "11111111111111111111111111111111",
        chrono::Utc::now().timestamp(),
    )
    .expect("the id token verifies with the client secret");
    assert_eq!(claims.sub, "1");
    assert_eq!(claims.shop(), shop);

    let config = page_config(&html);
    assert_eq!(config["shop"], shop.as_str());
    assert_eq!(
        config["app"]["clientId"],
        "11111111111111111111111111111111"
    );
    assert_eq!(config["app"]["handle"], "admin-fixtures");
    assert_eq!(config["app"]["base"], "/admin/apps/admin-fixtures");
    assert_eq!(config["frame"]["origin"], "http://localhost:3000");
    assert_eq!(config["frame"]["path"], "/");
    assert_eq!(config["user"]["id"], 1);
    assert_eq!(config["user"]["accountOwner"], true);
    assert_eq!(
        config["endpoints"]["sessionToken"],
        "/__lsf/admin/session-token"
    );
    assert_eq!(
        config["app"]["scopes"]["required"],
        serde_json::json!([
            "read_products",
            "write_products",
            "read_orders",
            "read_customers",
            "write_discounts"
        ])
    );
    assert_eq!(
        config["app"]["scopes"]["granted"],
        config["app"]["scopes"]["required"]
    );
    assert!(
        config["shopName"]
            .as_str()
            .is_some_and(|name| !name.is_empty())
    );

    // A load is a new launch: a new session token, a new signature.
    let again = frame_src(&text(&get(&api, "/admin/apps/admin-fixtures")));
    let again = query_of(&again);
    assert_ne!(param(&again, "id_token"), Some(token));
}

#[test]
fn opens_the_app_at_the_path_and_query_of_the_admin_url() {
    let api = api();
    let html = text(&get(
        &api,
        "/admin/apps/admin-fixtures/app/settings?tab=billing&locale=fr&user=2&host=stale&id_token=old",
    ));
    let src = frame_src(&html);
    assert!(
        src.starts_with("http://localhost:3000/app/settings?"),
        "{src}"
    );
    let params = query_of(&src);
    assert_eq!(
        param(&params, "tab"),
        Some("billing"),
        "the app's own parameters go to the app"
    );
    assert_eq!(param(&params, "locale"), Some("fr"));
    assert_ne!(
        param(&params, "id_token"),
        Some("old"),
        "a stale launch parameter is replaced"
    );
    assert_ne!(param(&params, "host"), Some("stale"));
    assert_eq!(params.iter().filter(|(name, _)| name == "host").count(), 1);
    let claims = session_token::verify(
        param(&params, "id_token").unwrap(),
        SECRET,
        "11111111111111111111111111111111",
        chrono::Utc::now().timestamp(),
    )
    .unwrap();
    assert_eq!(claims.sub, "2", "the user of the admin URL");
    let config = page_config(&html);
    assert_eq!(config["frame"]["path"], "/app/settings?tab=billing");
    assert_eq!(config["locale"], "fr");
    assert_eq!(
        config["query"],
        serde_json::json!({"locale": "fr", "user": "2"})
    );

    // The client id names the app too, as on Shopify.
    let by_id = get(&api, "/admin/apps/11111111111111111111111111111111/app");
    assert_eq!(by_id.status, 200);
    assert!(frame_src(&text(&by_id)).starts_with("http://localhost:3000/app?"));
}

#[test]
fn redirects_the_admin_and_its_apps_to_the_app() {
    let api = api();
    for path in ["/admin", "/admin/", "/admin/apps"] {
        let response = get(&api, path);
        assert_eq!(response.status, 302, "{path}");
        assert_eq!(
            header(&response, "location"),
            Some("/admin/apps/admin-fixtures"),
            "{path}"
        );
    }
    let response = get(&api, "/admin?shop=local-supply-co&locale=fr&other=1");
    assert_eq!(
        header(&response, "location"),
        Some("/admin/apps/admin-fixtures?shop=local-supply-co&locale=fr")
    );
}

#[test]
fn says_plainly_what_it_has_no_page_for() {
    let api = api();
    let products = get(&api, "/admin/products/123");
    assert_eq!(products.status, 404);
    let html = text(&products);
    assert!(html.contains("lsf has no page here"), "{html}");
    assert!(html.contains(
        "/admin/products/123 is a page of the admin itself, which lsf does not simulate"
    ));
    assert!(html.contains("Back to Admin fixtures"));
    assert!(!html.contains("lsf-frame"));

    let other_app = get(&api, "/admin/apps/someone-else");
    assert_eq!(other_app.status, 404);
    assert!(
        text(&other_app)
            .contains("lsf runs one app, Admin fixtures: its page is /admin/apps/admin-fixtures")
    );

    let no_shop = get(&api, "/admin/apps/admin-fixtures?shop=nowhere");
    assert_eq!(no_shop.status, 404);
    assert!(text(&no_shop).contains("lsf has no shop nowhere.myshopify.com"));

    let no_user = get(&api, "/admin/apps/admin-fixtures?user=abc");
    assert_eq!(no_user.status, 404);
    assert!(text(&no_user).contains("has no user abc"));

    // A POST is not a page, and the Admin API and OAuth keep their paths.
    let post = api
        .handle(&HttpRequest {
            method: "POST".into(),
            path: "/admin/apps/admin-fixtures".into(),
            ..HttpRequest::default()
        })
        .unwrap();
    assert_eq!(post.status, 405);
    assert_eq!(get(&api, "/admin/api/2026-10/products.json").status, 404);
    assert!(
        header(
            &get(&api, "/admin/api/2026-10/products.json"),
            "content-type"
        )
        .unwrap()
        .contains("json")
    );
}

#[test]
fn says_what_is_missing_to_open_the_app() {
    let no_app = api_with(None, None);
    let response = get(&no_app, "/admin/apps/anything");
    assert_eq!(response.status, 404);
    assert!(text(&response).contains("lsf runs no app"));

    let no_url = api_with(Some(support::fixture_app()), None);
    // application_url of the fixture is https://app.example.com: the admin opens it there.
    let html = text(&get(&no_url, "/admin/apps/admin-fixtures"));
    assert!(frame_src(&html).starts_with("https://app.example.com?"));

    let mut app = support::fixture_app();
    app.config.application_url = None;
    let response = get(&api_with(Some(app), None), "/admin/apps/admin-fixtures");
    assert_eq!(response.status, 503);
    assert!(text(&response).contains("--app-url http://localhost:3000"));

    let mut app = support::fixture_app();
    app.config.embedded = Some(false);
    let response = get(
        &api_with(Some(app), Some("http://localhost:3000")),
        "/admin/apps/admin-fixtures/app",
    );
    assert_eq!(response.status, 200);
    let html = text(&response);
    assert!(html.contains("This app is not embedded"));
    assert!(html.contains("/__lsf/admin/launch?shop="));
    assert!(!html.contains("lsf-frame"));
}

#[test]
fn serves_app_bridge_and_the_files_of_the_page() {
    let api = api();
    let script = get(&api, "/__lsf/admin/app-bridge.js");
    assert_eq!(script.status, 200);
    assert_eq!(
        header(&script, "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(header(&script, "access-control-allow-origin"), Some("*"));
    assert_eq!(header(&script, "cache-control"), Some("no-cache"));
    assert_eq!(
        script.body,
        lsf_app::admin::assets::get("app-bridge.js").unwrap()
    );
    for (name, kind) in [
        ("admin.js", "text/javascript"),
        ("admin.css", "text/css"),
        ("app-bridge.js", "text/javascript"),
    ] {
        let response = get(&api, &format!("/__lsf/admin/assets/{name}"));
        assert_eq!(response.status, 200, "{name}");
        assert!(
            header(&response, "content-type").unwrap().starts_with(kind),
            "{name}"
        );
    }
    assert_eq!(
        get(&api, "/__lsf/admin/assets/admin.html").status,
        404,
        "the template is not a file of the page"
    );
    assert_eq!(get(&api, "/__lsf/admin/assets/nothing.js").status, 404);
}

fn resources(api: &AdminApi, query: &str) -> Value {
    let response = get(api, &format!("/__lsf/admin/resources?{query}"));
    assert_eq!(response.status, 200, "{}", text(&response));
    json_of(&response)
}

#[test]
fn lists_products_with_their_variants_in_the_shape_of_the_resource_picker() {
    let api = api();
    let answer = resources(&api, "type=product&first=3");
    let products = answer["resources"].as_array().unwrap();
    assert_eq!(products.len(), 3);
    assert_eq!(answer["pageInfo"]["hasNextPage"], true);
    let titles: Vec<&str> = products
        .iter()
        .map(|p| p["title"].as_str().unwrap())
        .collect();
    let mut sorted = titles.clone();
    sorted.sort_by_key(|title| title.to_lowercase());
    assert_eq!(titles, sorted, "by title");
    let product = &products[0];
    for field in [
        "id",
        "title",
        "handle",
        "descriptionHtml",
        "productType",
        "vendor",
        "tags",
        "status",
        "createdAt",
        "updatedAt",
        "totalInventory",
        "tracksInventory",
        "hasOnlyDefaultVariant",
        "availablePublicationCount",
        "totalVariants",
        "options",
        "images",
        "variants",
    ] {
        assert!(product.get(field).is_some(), "{field} in {product:#}");
    }
    assert!(
        product["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/Product/")
    );
    let variants = product["variants"].as_array().unwrap();
    assert_eq!(product["totalVariants"], variants.len());
    let variant = &variants[0];
    for field in [
        "id",
        "title",
        "displayName",
        "price",
        "position",
        "availableForSale",
        "inventoryPolicy",
        "inventoryManagement",
        "inventoryItem",
        "requiresShipping",
        "selectedOptions",
        "product",
        "weightUnit",
    ] {
        assert!(variant.get(field).is_some(), "{field} in {variant:#}");
    }
    assert!(
        variant["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/ProductVariant/")
    );
    assert!(
        variant["price"].is_string(),
        "Money is a string: {}",
        variant["price"]
    );
    assert_eq!(variant["product"]["id"], product["id"]);
    assert!(
        variant["inventoryItem"]["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/InventoryItem/")
    );

    // The next page, and a search in Shopify's syntax.
    let next = resources(
        &api,
        &format!(
            "type=variant&first=3&after={}",
            answer["pageInfo"]["endCursor"].as_str().unwrap()
        ),
    );
    assert_ne!(next["resources"][0]["id"], product["id"]);
    let title = product["title"].as_str().unwrap();
    let found = resources(
        &api,
        &format!("query={}", urlencode(&format!("title:'{title}'"))),
    );
    assert!(
        found["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == product["id"])
    );
    let none = resources(&api, "query=title:no-such-product-anywhere");
    assert_eq!(none["resources"], serde_json::json!([]));
}

#[test]
fn lists_collections_in_the_shape_of_the_resource_picker() {
    let api = api();
    let answer = resources(&api, "type=collection");
    let collections = answer["resources"].as_array().unwrap();
    assert!(!collections.is_empty());
    let collection = &collections[0];
    for field in [
        "id",
        "title",
        "handle",
        "description",
        "descriptionHtml",
        "image",
        "productsCount",
        "productsAutomaticallySortedCount",
        "productsManuallySortedCount",
        "publicationCount",
        "availablePublicationCount",
        "ruleSet",
        "seo",
        "sortOrder",
        "storefrontId",
        "updatedAt",
    ] {
        assert!(collection.get(field).is_some(), "{field} in {collection:#}");
    }
    assert!(
        collection["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/Collection/")
    );
    assert!(collection["productsCount"].is_u64());
}

#[test]
fn refuses_what_the_resource_picker_cannot_list() {
    let api = api();
    for (query, status, says) in [
        ("type=order", 400, "type is product, variant or collection"),
        ("first=0", 400, "first is a number from 1 to 250"),
        ("first=251", 400, "first is a number from 1 to 250"),
        ("shop=nowhere", 404, "lsf has no shop nowhere.myshopify.com"),
    ] {
        let response = get(&api, &format!("/__lsf/admin/resources?{query}"));
        assert_eq!(response.status, status, "{query}");
        assert!(
            json_of(&response)["error"].as_str().unwrap().contains(says),
            "{query}"
        );
    }
}

fn urlencode(text: &str) -> String {
    form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

#[test]
fn links_to_the_previews_the_server_gives_it_for_the_default_shop() {
    // None by default.
    let html = text(&get(&api(), "/admin/apps/admin-fixtures"));
    assert!(!html.contains("lsf-nav-preview"), "{html}");
    assert!(!html.contains(">Preview</div>"), "{html}");

    let api = api().with_previews(vec![
        Preview {
            label: "Online Store".into(),
            path: "/".into(),
        },
        Preview {
            label: "Checkout".into(),
            path: "/__lsf/checkouts".into(),
        },
    ]);
    assert_eq!(api.previews().len(), 2);
    let html = text(&get(&api, "/admin/apps/admin-fixtures"));
    let preview = html
        .split_once(">Preview</div>")
        .unwrap_or_else(|| panic!("{html}"))
        .1;
    let store = preview
        .find("href=\"/\" target=\"_blank\" rel=\"noopener\"")
        .expect(preview);
    let checkout = preview
        .find("href=\"/__lsf/checkouts\" target=\"_blank\" rel=\"noopener\"")
        .expect(preview);
    assert!(store < checkout, "{preview}");
    assert!(preview.contains(">Online Store<") && preview.contains(">Checkout<"));
    // The page of a section lsf lacks has them too.
    assert!(text(&get(&api, "/admin/products")).contains(">Preview</div>"));
}
