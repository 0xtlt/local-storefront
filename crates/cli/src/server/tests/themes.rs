//! What the storefront serves of the themes, script tags and mobile apps the backend changes
//! through the Admin API: the theme directory with the Admin API's files on top (never written
//! to), another theme once published, the script tags `content_for_header` loads, the
//! association files of the shop's mobile apps.

use std::path::Path;

use serde_json::{Value as Json, json};

use super::super::ServerState;
use super::{admin_query, admin_request, admin_server, get, text};

fn mutate(state: &ServerState, query: &str, variables: Json) -> Json {
    let reply = admin_request(
        state,
        "POST",
        "/admin/api/2026-10/graphql.json",
        &json!({ "query": query, "variables": variables }).to_string(),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let answer: Json = serde_json::from_slice(&reply.body).unwrap();
    assert!(answer.get("errors").is_none(), "{answer}");
    answer
}

fn fixture_file(path: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../core/tests/fixtures/theme")
            .join(path),
    )
    .unwrap()
}

const UPSERT: &str = "mutation($files: [OnlineStoreThemeFilesUpsertFileInput!]!) {
    themeFilesUpsert(themeId: \"gid://shopify/OnlineStoreTheme/1\", files: $files) {
        upsertedThemeFiles { filename } userErrors { message }
    }
}";

#[test]
fn the_storefront_serves_the_files_the_admin_api_writes_over_the_theme_directory() {
    let state = admin_server();
    let home = text(&get(&state, "/", "visitor"));
    assert!(home.contains("Stored title"), "{home}");
    let index = fixture_file("templates/index.json");

    let answer = mutate(
        &state,
        UPSERT,
        json!({"files": [
            {"filename": "templates/index.json", "body": {"type": "TEXT", "value": "{\"sections\": {\"main\": {\"type\": \"admin-banner\", \"settings\": {}}}, \"order\": [\"main\"]}"}},
            {"filename": "sections/admin-banner.liquid", "body": {"type": "TEXT", "value": "<p class=\"admin\">Written by the Admin API</p>"}},
            {"filename": "assets/notes.txt", "body": {"type": "BASE64", "value": "bm90ZXM="}},
        ]}),
    );
    assert_eq!(
        answer["data"]["themeFilesUpsert"]["userErrors"],
        json!([]),
        "{answer}"
    );
    let home = text(&get(&state, "/", "visitor"));
    assert!(home.contains("Written by the Admin API"), "{home}");
    assert!(!home.contains("Stored title"));
    let asset = get(&state, "/cdn/shop/t/1/assets/notes.txt", "visitor");
    assert_eq!(asset.status, 200);
    assert_eq!(text(&asset), "notes");
    assert_eq!(
        fixture_file("templates/index.json"),
        index,
        "the directory is not written to"
    );

    // A reset puts the storefront back to the directory.
    let reset = admin_request(&state, "DELETE", "/__lsf/admin", "");
    assert_eq!(reset.status, 200);
    let home = text(&get(&state, "/", "visitor"));
    assert!(home.contains("Stored title"), "{home}");
    assert_eq!(
        get(&state, "/cdn/shop/t/1/assets/notes.txt", "visitor").status,
        404
    );
}

#[test]
fn a_published_theme_is_served_instead_of_the_directory_until_the_local_one_is_published_again() {
    let state = admin_server();
    // A theme made from the local one, edited, published.
    let duplicated = mutate(
        &state,
        "mutation { themeDuplicate(id: \"gid://shopify/OnlineStoreTheme/1\", name: \"Sale\") { newTheme { id } userErrors { message } } }",
        json!({}),
    );
    let id = duplicated["data"]["themeDuplicate"]["newTheme"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    mutate(
        &state,
        "mutation($id: ID!) { themeFilesUpsert(themeId: $id, files: [{filename: \"sections/announcement.liquid\", body: {type: TEXT, value: \"<p>Sale theme</p>\"}}]) { userErrors { message } } }",
        json!({"id": id}),
    );
    assert!(
        !text(&get(&state, "/", "visitor")).contains("Sale theme"),
        "not published yet"
    );
    mutate(
        &state,
        "mutation($id: ID!) { themePublish(id: $id) { theme { role } userErrors { message } } }",
        json!({"id": id}),
    );
    let home = text(&get(&state, "/", "visitor"));
    assert!(home.contains("Sale theme"), "{home}");
    assert!(
        home.contains("Stored title"),
        "the copy has the directory's other files"
    );
    mutate(
        &state,
        "mutation { themePublish(id: \"gid://shopify/OnlineStoreTheme/1\") { theme { role } userErrors { message } } }",
        json!({}),
    );
    assert!(!text(&get(&state, "/", "visitor")).contains("Sale theme"));
}

#[test]
fn script_tags_are_loaded_by_the_storefront_once_created() {
    let state = admin_server();
    assert!(!text(&get(&state, "/", "visitor")).contains("asyncLoad"));
    let created = mutate(
        &state,
        "mutation { scriptTagCreate(input: {src: \"https://js.example.org/bar.js\", displayScope: ONLINE_STORE}) { scriptTag { id } userErrors { message } } }",
        json!({}),
    );
    let id = created["data"]["scriptTagCreate"]["scriptTag"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let home = text(&get(&state, "/", "visitor"));
    assert!(home.contains("function asyncLoad()"), "{home}");
    assert!(
        home.contains(r#"https:\/\/js.example.org\/bar.js?shop="#),
        "{home}"
    );
    mutate(
        &state,
        "mutation($id: ID!) { scriptTagDelete(id: $id) { deletedScriptTagId userErrors { message } } }",
        json!({"id": id}),
    );
    assert!(!text(&get(&state, "/", "visitor")).contains("asyncLoad"));
}

#[test]
fn the_storefront_serves_the_association_files_of_the_shops_mobile_apps() {
    let state = admin_server();
    assert_eq!(
        get(&state, "/.well-known/assetlinks.json", "visitor").status,
        404
    );
    mutate(
        &state,
        "mutation { mobilePlatformApplicationCreate(input: {android: {applicationId: \"com.example.shop\", appLinksEnabled: true, sha256CertFingerprints: [\"A1:B2\"]}}) { mobilePlatformApplication { __typename } userErrors { message } } }",
        json!({}),
    );
    let reply = get(&state, "/.well-known/assetlinks.json", "visitor");
    assert_eq!(reply.status, 200);
    let file: Json = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(file[0]["target"]["package_name"], "com.example.shop");
    // The theme's pages are not the Admin API's.
    let answer = admin_query(&state, "{ themes(first: 1) { nodes { role } } }");
    assert_eq!(answer["data"]["themes"]["nodes"][0]["role"], "MAIN");
}
