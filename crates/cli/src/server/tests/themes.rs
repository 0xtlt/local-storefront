//! What the storefront serves of the themes, script tags and mobile apps the backend changes
//! through the Admin API: the theme directory with the Admin API's files on top (never written
//! to), another theme once published, the script tags `content_for_header` loads, the
//! association files of the shop's mobile apps.

use std::path::Path;

use serde_json::{Value as Json, json};

use super::super::ServerState;
use super::{admin_query, admin_request, admin_server, body_json, get, text};

fn mutate(state: &ServerState, query: &str, variables: Json) -> Json {
    let reply = admin_request(
        state,
        "POST",
        "/admin/api/2026-10/graphql.json",
        &json!({ "query": query, "variables": variables }).to_string(),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let answer = body_json(&reply);
    assert!(answer.get("errors").is_none(), "{answer}");
    answer
}

fn fixture_file(path: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../theme/tests/fixtures/theme")
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

/// Uninstalling the app removes its script tags, as Shopify does: the storefront stops loading
/// them at once, and a reinstall does not bring them back (the app creates them again).
#[test]
fn uninstalling_the_app_removes_its_script_tags_from_the_storefront() {
    let state = admin_server();
    mutate(
        &state,
        "mutation { scriptTagCreate(input: {src: \"https://js.example.org/bar.js\", displayScope: ONLINE_STORE}) { scriptTag { id } userErrors { message } } }",
        json!({}),
    );
    assert!(text(&get(&state, "/", "visitor")).contains("asyncLoad"));

    let uninstalled = admin_request(&state, "POST", "/__lsf/admin/uninstall", "{}");
    assert_eq!(uninstalled.status, 200, "{}", text(&uninstalled));
    let answer = body_json(&uninstalled);
    assert_eq!(answer["removedScriptTags"], 1, "{answer}");
    assert_eq!(answer["removedWebPixels"], 0, "{answer}");
    let home = text(&get(&state, "/", "visitor"));
    assert!(!home.contains("asyncLoad"), "{home}");

    let installed = admin_request(&state, "POST", "/__lsf/admin/install", "{}");
    assert_eq!(installed.status, 200, "{}", text(&installed));
    assert!(!text(&get(&state, "/", "visitor")).contains("asyncLoad"));
    let tags = admin_query(&state, "{ scriptTags(first: 5) { nodes { id } } }");
    assert_eq!(tags["data"]["scriptTags"]["nodes"], json!([]), "{tags}");
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

/// A reload of the store data (`--watch`, `POST /__lsf/reload`) reseeds the shops, which drops
/// the Admin API's files and script tags: the storefront kept serving them, while the Admin API
/// reported the directory.
#[test]
fn a_reload_of_the_store_data_serves_the_directory_again() {
    let state = admin_server();
    mutate(
        &state,
        UPSERT,
        json!({"files": [
            {"filename": "templates/index.json", "body": {"type": "TEXT", "value": "{\"sections\": {\"main\": {\"type\": \"admin-banner\", \"settings\": {}}}, \"order\": [\"main\"]}"}},
            {"filename": "sections/admin-banner.liquid", "body": {"type": "TEXT", "value": "<p>Written by the Admin API</p>"}},
        ]}),
    );
    mutate(
        &state,
        "mutation { scriptTagCreate(input: {src: \"https://js.example.org/bar.js\", displayScope: ONLINE_STORE}) { scriptTag { id } userErrors { message } } }",
        json!({}),
    );
    let home = text(&get(&state, "/", "visitor"));
    assert!(home.contains("Written by the Admin API"), "{home}");
    assert!(home.contains("asyncLoad"));

    state.reload();

    let home = text(&get(&state, "/", "visitor"));
    assert!(home.contains("Stored title"), "{home}");
    assert!(!home.contains("Written by the Admin API"), "{home}");
    assert!(!home.contains("asyncLoad"), "{home}");
    // As the Admin API reports it.
    let answer = admin_query(
        &state,
        "{ theme(id: \"gid://shopify/OnlineStoreTheme/1\") { files(filenames: [\"sections/admin-banner.liquid\"], first: 1) { nodes { filename } } } scriptTags(first: 5) { nodes { id } } }",
    );
    assert_eq!(
        answer["data"]["theme"]["files"]["nodes"],
        json!([]),
        "{answer}"
    );
    assert_eq!(answer["data"]["scriptTags"]["nodes"], json!([]), "{answer}");
}

/// The `Shopify.theme` line of a page: what `content_for_header` says of the theme served.
fn shopify_theme(page: &str) -> Json {
    let line = page
        .lines()
        .find_map(|line| line.trim().strip_prefix("Shopify.theme = "))
        .unwrap_or_else(|| panic!("no Shopify.theme in {page}"));
    serde_json::from_str(line.trim_end_matches(';')).unwrap()
}

/// `Shopify.theme.id` stayed 1, and its name the directory's, while another theme was published.
#[test]
fn shopify_theme_names_the_published_theme() {
    let state = admin_server();
    let local = shopify_theme(&text(&get(&state, "/", "visitor")));
    assert_eq!(local["id"], 1);
    assert_eq!(local["role"], "main");
    let schema_name = local["schema_name"].clone();

    let duplicated = mutate(
        &state,
        "mutation { themeDuplicate(id: \"gid://shopify/OnlineStoreTheme/1\", name: \"Sale\") { newTheme { id } userErrors { message } } }",
        json!({}),
    );
    let id = duplicated["data"]["themeDuplicate"]["newTheme"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let number: u64 = id.rsplit('/').next().unwrap().parse().unwrap();
    mutate(
        &state,
        "mutation($id: ID!) { themePublish(id: $id) { theme { role } userErrors { message } } }",
        json!({"id": id}),
    );
    let home = text(&get(&state, "/", "visitor"));
    let published = shopify_theme(&home);
    assert_eq!(published["id"], number, "{published}");
    assert_eq!(published["name"], "Sale");
    assert_eq!(published["role"], "main");
    assert_eq!(
        published["schema_name"], schema_name,
        "the files' theme_info"
    );
    let assets = format!("/cdn/shop/t/{number}/assets/");
    assert!(home.contains(&assets), "{home}");
    assert_eq!(
        get(&state, &format!("{assets}base.css"), "visitor").status,
        200,
        "the CDN serves the published theme's assets under its id"
    );

    // The local theme published again, renamed: id 1, the name the Admin API gave it.
    mutate(
        &state,
        "mutation { themePublish(id: \"gid://shopify/OnlineStoreTheme/1\") { theme { role } userErrors { message } } }",
        json!({}),
    );
    assert_eq!(shopify_theme(&text(&get(&state, "/", "visitor"))), local);
    mutate(
        &state,
        "mutation { themeUpdate(id: \"gid://shopify/OnlineStoreTheme/1\", input: {name: \"Renamed\"}) { theme { name } userErrors { message } } }",
        json!({}),
    );
    let renamed = shopify_theme(&text(&get(&state, "/", "visitor")));
    assert_eq!(renamed["id"], 1);
    assert_eq!(renamed["name"], "Renamed");
}
