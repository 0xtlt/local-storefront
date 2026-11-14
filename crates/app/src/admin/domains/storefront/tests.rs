//! Script tags, pixels and storefront access tokens, through queries over the demo store and
//! the app of `tests/fixtures/platform-app` (a web pixel extension, a Flow trigger).

use serde_json::{Value, json};

use crate::admin::graphql::{self, Request};
use crate::admin::resolve::{Registry, ShopResolver};
use crate::admin::state::ShopState;

/// The app of `tests/fixtures/platform-app`.
pub(crate) fn platform_app() -> crate::config::App {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/platform-app");
    crate::config::load(&path).expect("the platform fixture app loads")
}

/// The demo store, with the platform app installed.
pub(crate) fn shop() -> ShopState {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let mut shop = ShopState::seeded(&store, Some(&platform_app()));
    shop.take_events();
    shop
}

/// The answer to a request, `extensions` without `cost`.
pub(crate) fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let request = Request::new(query).with_variables(variables);
    let mut answer = graphql::run(crate::admin::schema::admin(), &mut resolver, &request, None);
    answer.extensions.extend(resolver.extensions());
    answer.extensions.remove("cost");
    answer.to_json()
}

pub(crate) fn topics(shop: &mut ShopState) -> Vec<String> {
    shop.take_events()
        .into_iter()
        .map(|event| event.topic)
        .collect()
}

/// A script tag the app created before October 1, 2026, when Shopify still let it: `src`, shown
/// on the online store. Its GID.
pub(crate) fn existing_script_tag(shop: &mut ShopState, src: &str) -> String {
    let now = shop.timestamp();
    shop.insert(crate::admin::state::value::object(json!({
        "__typename": "ScriptTag",
        "src": src,
        "displayScope": "ONLINE_STORE",
        "cache": false,
        "createdAt": now,
        "updatedAt": now,
    })))
}

/// Shopify refuses `scriptTagCreate` and `scriptTagUpdate` since October 1, 2026, on every API
/// version: a user error, and nothing changes.
#[test]
fn script_tags_can_no_longer_be_created_or_updated() {
    let mut shop = shop();
    let tag = existing_script_tag(&mut shop, "https://js.example.org/bar.js");
    let before = shop.get(&tag).cloned();
    let refused = json!({"scriptTag": null, "userErrors": [{
        "field": null,
        "message": "Script tags can no longer be created or updated: use an app embed block or a web pixel",
    }]});
    let created = run(
        &mut shop,
        "mutation { scriptTagCreate(input: {src: \"https://js.example.org/new.js\", displayScope: ONLINE_STORE}) { scriptTag { id } userErrors { field message } } }",
        json!({}),
    );
    assert_eq!(created["data"]["scriptTagCreate"], refused, "{created}");
    let updated = run(
        &mut shop,
        "mutation($id: ID!) { scriptTagUpdate(id: $id, input: {src: \"https://js.example.org/updated.js\"}) { scriptTag { id } userErrors { field message } } }",
        json!({"id": tag}),
    );
    assert_eq!(updated["data"]["scriptTagUpdate"], refused, "{updated}");
    assert_eq!(shop.count("ScriptTag"), 1);
    assert_eq!(shop.get(&tag).cloned(), before);
    assert!(topics(&mut shop).is_empty());
}

/// The script tags that exist are still listed, loaded by the storefront and deleted.
#[test]
fn existing_script_tags_are_listed_loaded_and_deleted() {
    let mut shop = shop();
    let bar = existing_script_tag(&mut shop, "https://js.example.org/bar.js");
    existing_script_tag(&mut shop, "https://js.example.org/other.js");
    let listed = run(
        &mut shop,
        "{ all: scriptTags(first: 5) { nodes { src } } one: scriptTags(first: 5, src: \"https://js.example.org/other.js\") { nodes { src } } }",
        json!({}),
    );
    assert_eq!(listed["data"]["all"]["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(
        listed["data"]["one"]["nodes"],
        json!([{"src": "https://js.example.org/other.js"}])
    );
    assert_eq!(
        super::storefront_scripts(&shop),
        [
            "https://js.example.org/bar.js",
            "https://js.example.org/other.js"
        ]
    );
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { scriptTagDelete(id: $id) { deletedScriptTagId userErrors { message } } }",
        json!({"id": bar}),
    );
    assert_eq!(
        deleted["data"]["scriptTagDelete"]["deletedScriptTagId"],
        json!(bar)
    );
    assert_eq!(
        super::storefront_scripts(&shop),
        ["https://js.example.org/other.js"]
    );
    let missing = run(
        &mut shop,
        "mutation { scriptTagDelete(id: \"gid://shopify/ScriptTag/999\") { deletedScriptTagId userErrors { field message } } }",
        json!({}),
    );
    assert_eq!(
        missing["data"]["scriptTagDelete"]["userErrors"][0]["field"],
        json!(["id"])
    );
}

const CREATE_PIXEL: &str = "mutation($settings: JSON!) {
    webPixelCreate(webPixel: {settings: $settings}) {
        webPixel { id settings }
        userErrors { field message code }
    }
}";

#[test]
fn a_web_pixel_is_checked_against_the_extension_settings() {
    let mut shop = shop();
    // The reference's error, and the extension's own validation (`min`).
    let empty = run(&mut shop, CREATE_PIXEL, json!({"settings": {}}));
    assert_eq!(
        empty["data"]["webPixelCreate"]["userErrors"],
        json!([{"field": ["webPixel"], "message": "Missing keys: accountID", "code": "INVALID_SETTINGS"}])
    );
    let short = run(
        &mut shop,
        CREATE_PIXEL,
        json!({"settings": {"accountID": ""}}),
    );
    assert_eq!(
        short["data"]["webPixelCreate"]["userErrors"][0]["code"], "INVALID_SETTINGS",
        "{short}"
    );
    let unknown = run(
        &mut shop,
        CREATE_PIXEL,
        json!({"settings": {"accountID": "1", "other": "x"}}),
    );
    assert_eq!(
        unknown["data"]["webPixelCreate"]["userErrors"][0]["message"],
        "Unknown keys: other"
    );
    assert_eq!(shop.count("WebPixel"), 0);
    assert!(topics(&mut shop).is_empty());

    // A JSON string, as the guide sends it; the settings come back as a string of JSON.
    let created = run(
        &mut shop,
        CREATE_PIXEL,
        json!({"settings": "{\"accountID\":\"123\"}"}),
    );
    let pixel = &created["data"]["webPixelCreate"]["webPixel"];
    assert_eq!(pixel["settings"], "{\"accountID\":\"123\"}", "{created}");
    let id = pixel["id"].as_str().unwrap().to_string();
    let again = run(
        &mut shop,
        CREATE_PIXEL,
        json!({"settings": {"accountID": "9"}}),
    );
    assert_eq!(
        again["data"]["webPixelCreate"]["userErrors"][0]["code"], "TAKEN",
        "one web pixel per app"
    );
    let read = run(&mut shop, "{ webPixel { id settings } }", json!({}));
    assert_eq!(read["data"]["webPixel"]["id"], json!(id));
    let updated = run(
        &mut shop,
        "mutation($id: ID!) { webPixelUpdate(id: $id, webPixel: {settings: {accountID: \"456\"}}) { webPixel { settings } userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        updated["data"]["webPixelUpdate"]["webPixel"]["settings"],
        "{\"accountID\":\"456\"}"
    );
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { webPixelDelete(id: $id) { deletedWebPixelId userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        deleted["data"]["webPixelDelete"]["deletedWebPixelId"],
        json!(id)
    );
    assert_eq!(
        topics(&mut shop),
        [
            "web_pixels/create",
            "web_pixels/update",
            "web_pixels/delete"
        ]
    );
}

#[test]
fn without_a_web_pixel_extension_there_is_no_web_pixel() {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let mut shop = ShopState::seeded(&store, None);
    let answer = run(
        &mut shop,
        CREATE_PIXEL,
        json!({"settings": {"accountID": "1"}}),
    );
    assert_eq!(
        answer["data"]["webPixelCreate"]["userErrors"][0]["code"],
        "NO_EXTENSION"
    );
    assert_eq!(shop.count("WebPixel"), 0);
}

#[test]
fn a_server_pixel_is_created_once_then_connected() {
    let mut shop = shop();
    let unconfigured = run(
        &mut shop,
        "mutation { eventBridgeServerPixelUpdate(arn: \"arn:aws:events:us-east-1::event-source/aws.partner/shopify.com/1/pixel\") { serverPixel { id } userErrors { code } } }",
        json!({}),
    );
    assert_eq!(
        unconfigured["data"]["eventBridgeServerPixelUpdate"]["userErrors"][0]["code"], "NOT_FOUND",
        "nothing to connect yet"
    );
    let created = run(
        &mut shop,
        "mutation { serverPixelCreate { serverPixel { id status webhookEndpointAddress } userErrors { code } } }",
        json!({}),
    );
    assert_eq!(
        created["data"]["serverPixelCreate"]["serverPixel"]["status"],
        "DISCONNECTED_UNCONFIGURED"
    );
    let twice = run(
        &mut shop,
        "mutation { serverPixelCreate { serverPixel { id } userErrors { code } } }",
        json!({}),
    );
    assert_eq!(
        twice["data"]["serverPixelCreate"]["userErrors"][0]["code"],
        "ALREADY_EXISTS"
    );
    let bad = run(
        &mut shop,
        "mutation { eventBridgeServerPixelUpdate(arn: \"arn:aws:s3:::bucket\") { serverPixel { id } userErrors { field code } } }",
        json!({}),
    );
    assert_eq!(
        bad["data"]["eventBridgeServerPixelUpdate"]["userErrors"][0]["field"],
        json!(["arn"])
    );
    let connected = run(
        &mut shop,
        "mutation { pubSubServerPixelUpdate(pubSubProject: \"my-project\", pubSubTopic: \"events\") { serverPixel { status webhookEndpointAddress } userErrors { code } } }",
        json!({}),
    );
    assert_eq!(
        connected["data"]["pubSubServerPixelUpdate"]["serverPixel"],
        json!({"status": "CONNECTED", "webhookEndpointAddress": "pubsub://my-project:events"})
    );
    let read = run(&mut shop, "{ serverPixel { status } }", json!({}));
    assert_eq!(read["data"]["serverPixel"]["status"], "CONNECTED");
    let deleted = run(
        &mut shop,
        "mutation { serverPixelDelete { deletedServerPixelId userErrors { code } } }",
        json!({}),
    );
    assert!(deleted["data"]["serverPixelDelete"]["deletedServerPixelId"].is_string());
    assert_eq!(shop.count("ServerPixel"), 0);
}

#[test]
fn storefront_access_tokens_have_the_apps_unauthenticated_scopes() {
    let mut shop = shop();
    let created = run(
        &mut shop,
        "mutation { storefrontAccessTokenCreate(input: {title: \"Headless\"}) { storefrontAccessToken { id title accessToken accessScopes { handle description } createdAt } shop { id } userErrors { field message } } }",
        json!({}),
    );
    let token = &created["data"]["storefrontAccessTokenCreate"]["storefrontAccessToken"];
    assert_eq!(token["title"], "Headless", "{created}");
    let secret = token["accessToken"].as_str().unwrap();
    assert_eq!(secret.len(), 32);
    assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(
        token["accessScopes"],
        json!([
            {"handle": "unauthenticated_read_product_listings", "description": "Read product listings (Storefront API)"},
            {"handle": "unauthenticated_read_checkouts", "description": "Read checkouts (Storefront API)"},
        ])
    );
    assert!(created["data"]["storefrontAccessTokenCreate"]["shop"]["id"].is_string());
    let id = token["id"].as_str().unwrap().to_string();
    let listed = run(
        &mut shop,
        "{ shop { storefrontAccessTokens(first: 5) { nodes { id title accessScopes { handle } } } } }",
        json!({}),
    );
    assert_eq!(
        listed["data"]["shop"]["storefrontAccessTokens"]["nodes"][0]["id"],
        json!(id)
    );
    assert!(listed.get("extensions").is_none(), "{listed}");

    let blank = run(
        &mut shop,
        "mutation { storefrontAccessTokenCreate(input: {title: \" \"}) { storefrontAccessToken { id } userErrors { field message } } }",
        json!({}),
    );
    assert_eq!(
        blank["data"]["storefrontAccessTokenCreate"]["userErrors"],
        json!([{"field": ["input", "title"], "message": "Title can't be blank"}])
    );
    assert_eq!(shop.count("StorefrontAccessToken"), 1);
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { storefrontAccessTokenDelete(input: {id: $id}) { deletedStorefrontAccessTokenId userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        deleted["data"]["storefrontAccessTokenDelete"]["deletedStorefrontAccessTokenId"],
        json!(id)
    );
    assert_eq!(shop.count("StorefrontAccessToken"), 0);
}

/// Uninstalling the app removes its script tags, which the storefront then stops loading, and its
/// web pixel, as Shopify does; a reinstalled app creates its web pixel again (no script tag: Shopify
/// refuses them since October 1, 2026). No webhook: the app's subscriptions go with it.
#[test]
fn uninstalling_the_app_removes_its_script_tags_and_its_web_pixel() {
    let mut shop = shop();
    for src in ["https://js.example.org/a.js", "https://js.example.org/b.js"] {
        existing_script_tag(&mut shop, src);
    }
    let pixel = run(
        &mut shop,
        CREATE_PIXEL,
        json!({"settings": {"accountID": "123"}}),
    );
    assert_eq!(
        pixel["data"]["webPixelCreate"]["userErrors"],
        json!([]),
        "{pixel}"
    );
    shop.take_events();

    let answer = run(
        &mut shop,
        "mutation { appUninstall { app { id } userErrors { code } } }",
        Value::Null,
    );
    assert_eq!(
        answer["data"]["appUninstall"]["userErrors"],
        json!([]),
        "{answer}"
    );
    assert_eq!(shop.count("ScriptTag"), 0);
    assert_eq!(shop.count("WebPixel"), 0);
    assert!(super::storefront_scripts(&shop).is_empty());
    let topics: Vec<String> = topics(&mut shop)
        .into_iter()
        .filter(|topic| !topic.starts_with("lsf/"))
        .collect();
    assert_eq!(topics, ["app/uninstalled"]);
    // Nothing left to remove: the counts say so.
    assert_eq!(super::remove_on_uninstall(&mut shop), (0, 0));

    // Installed again (by the records alone here): the app creates its web pixel again.
    let again = run(
        &mut shop,
        CREATE_PIXEL,
        json!({"settings": {"accountID": "123"}}),
    );
    assert_eq!(
        again["data"]["webPixelCreate"]["userErrors"],
        json!([]),
        "{again}"
    );
}
