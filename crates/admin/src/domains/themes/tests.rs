//! Themes and their files, over the demo store with the fixture theme of lsf-core as the local
//! theme: what the Admin API answers, and the overlay the storefront serves.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::zip;
use crate::domains::storefront::tests::{run, shop, topics};
use crate::state::{Blob, ShopState};

/// A copy of lsf-core's fixture theme in a directory of the test's, so that a test that
/// would write to it (none should) cannot change the fixture.
fn theme_directory(name: &str) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
    let target = std::env::temp_dir().join(format!("lsf-themes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&target);
    copy(&source, &target);
    target
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            copy(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

fn shop_with_theme(name: &str) -> (ShopState, PathBuf) {
    let mut shop = shop();
    let root = theme_directory(name);
    super::attach(&mut shop, &root);
    (shop, root)
}

/// Every file of a directory, by relative path, with its content.
fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    fn walk(root: &Path, directory: &Path, files: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let relative = path.strip_prefix(root).unwrap().display().to_string();
                files.push((relative, std::fs::read(&path).unwrap()));
            }
        }
    }
    walk(root, root, &mut files);
    files.sort();
    files
}

const UPSERT: &str = "mutation($id: ID!, $files: [OnlineStoreThemeFilesUpsertFileInput!]!) {
    themeFilesUpsert(themeId: $id, files: $files) {
        upsertedThemeFiles { filename checksumMd5 size }
        job { id done }
        userErrors { field message code filename }
    }
}";

#[test]
fn the_local_theme_is_the_main_theme_and_its_files_are_the_directorys() {
    let (mut shop, root) = shop_with_theme("files");
    let answer = run(
        &mut shop,
        "{
            themes(first: 5) { nodes { id name role prefix processing processingFailed themeStoreId createdAt updatedAt } }
            theme(id: \"gid://shopify/OnlineStoreTheme/1\") {
                files(first: 3, filenames: [\"layout/*\", \"templates/index.json\", \"templates/nope.json\"]) {
                    nodes { filename checksumMd5 size contentType createdAt updatedAt body { ... on OnlineStoreThemeFileBodyText { content } } }
                    userErrors { code filename }
                }
            }
        }",
        json!({}),
    );
    assert!(answer.get("extensions").is_none(), "{answer}");
    let theme = &answer["data"]["themes"]["nodes"][0];
    assert_eq!(theme["role"], "MAIN");
    assert_eq!(theme["id"], "gid://shopify/OnlineStoreTheme/1");
    let files = answer["data"]["theme"]["files"]["nodes"]
        .as_array()
        .unwrap();
    let names: Vec<&str> = files
        .iter()
        .map(|file| file["filename"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "layout/bare.liquid",
            "layout/password.liquid",
            "layout/theme.liquid"
        ]
    );
    let theme_liquid = std::fs::read(root.join("layout/theme.liquid")).unwrap();
    assert_eq!(files[2]["size"], json!(theme_liquid.len().to_string()));
    assert_eq!(
        files[2]["checksumMd5"],
        json!(super::files::md5_hex(&theme_liquid))
    );
    assert_eq!(files[2]["contentType"], "text/x-liquid");
    assert_eq!(
        files[2]["body"]["content"],
        json!(String::from_utf8(theme_liquid).unwrap())
    );
    assert_eq!(
        answer["data"]["theme"]["files"]["userErrors"],
        json!([{"code": "NOT_FOUND", "filename": "templates/nope.json"}])
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_upsert_on_the_main_theme_is_an_overlay_the_directory_never_sees() {
    let (mut shop, root) = shop_with_theme("overlay");
    let before = snapshot(&root);
    assert!(
        super::storefront_overlay(&shop).is_none(),
        "the directory alone"
    );
    let answer = run(
        &mut shop,
        UPSERT,
        json!({"id": "gid://shopify/OnlineStoreTheme/1", "files": [
            {"filename": "templates/index.json", "body": {"type": "TEXT", "value": "{ \"sections\": {}, \"order\": [] }"}},
            {"filename": "assets/custom-content.txt", "body": {"type": "BASE64", "value": "Y3VzdG9tIGNvbnRlbnQ="}},
        ]}),
    );
    let payload = &answer["data"]["themeFilesUpsert"];
    assert_eq!(payload["userErrors"], json!([]), "{answer}");
    assert_eq!(payload["upsertedThemeFiles"][1]["size"], "14");
    assert_eq!(payload["job"], Value::Null, "nothing fetched");
    assert_eq!(
        topics(&mut shop),
        ["theme_files/update"],
        "no theme webhook for files"
    );
    assert_eq!(snapshot(&root), before, "the directory is not written to");

    let overlay = super::storefront_overlay(&shop).unwrap();
    assert!(!overlay.exclusive);
    assert_eq!(
        overlay.files["assets/custom-content.txt"]
            .as_ref()
            .unwrap()
            .text
            .as_deref(),
        Some("custom content")
    );
    let read = run(
        &mut shop,
        "{ theme(id: \"gid://shopify/OnlineStoreTheme/1\") { files(filenames: [\"templates/index.json\"]) { nodes { body { ... on OnlineStoreThemeFileBodyText { content } } } } } }",
        json!({}),
    );
    assert_eq!(
        read["data"]["theme"]["files"]["nodes"][0]["body"]["content"],
        "{ \"sections\": {}, \"order\": [] }"
    );

    // A deletion hides the directory's file, a copy writes another.
    let deleted = run(
        &mut shop,
        "mutation { themeFilesDelete(themeId: \"gid://shopify/OnlineStoreTheme/1\", files: [\"templates/404.liquid\"]) { deletedThemeFiles { filename } userErrors { code } } }",
        json!({}),
    );
    assert_eq!(
        deleted["data"]["themeFilesDelete"]["deletedThemeFiles"],
        json!([{"filename": "templates/404.liquid"}])
    );
    let copied = run(
        &mut shop,
        "mutation { themeFilesCopy(themeId: \"gid://shopify/OnlineStoreTheme/1\", files: [{srcFilename: \"templates/index.json\", dstFilename: \"templates/index.alt.json\"}]) { copiedThemeFiles { filename } userErrors { code } } }",
        json!({}),
    );
    assert_eq!(
        copied["data"]["themeFilesCopy"]["copiedThemeFiles"],
        json!([{"filename": "templates/index.alt.json"}])
    );
    let overlay = super::storefront_overlay(&shop).unwrap();
    assert!(overlay.files["templates/404.liquid"].is_none());
    assert!(overlay.files["templates/index.alt.json"].is_some());
    assert_eq!(snapshot(&root), before);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_refused_file_writes_nothing() {
    let (mut shop, root) = shop_with_theme("refused");
    let id = "gid://shopify/OnlineStoreTheme/1";
    for (files, code, field) in [
        (
            json!([{"filename": "secrets/key.txt", "body": {"type": "TEXT", "value": "x"}}]),
            "FILE_VALIDATION_ERROR",
            json!(["files", "0", "filename"]),
        ),
        (
            json!([{"filename": "templates/a.json", "body": {"type": "TEXT", "value": "{ not json"}}]),
            "FILE_VALIDATION_ERROR",
            json!(["files", "0", "body", "value"]),
        ),
        (
            json!([
                {"filename": "snippets/a.liquid", "body": {"type": "TEXT", "value": "a"}},
                {"filename": "snippets/a.liquid", "body": {"type": "TEXT", "value": "b"}},
            ]),
            "DUPLICATE_FILE_INPUT",
            json!(["files", "1"]),
        ),
        (
            json!((0..51).map(|n| json!({"filename": format!("snippets/{n}.liquid"), "body": {"type": "TEXT", "value": "x"}})).collect::<Vec<_>>()),
            "LESS_THAN_OR_EQUAL_TO",
            json!(["files"]),
        ),
    ] {
        let answer = run(&mut shop, UPSERT, json!({"id": id, "files": files}));
        let error = &answer["data"]["themeFilesUpsert"]["userErrors"][0];
        assert_eq!(error["code"], code, "{answer}");
        assert_eq!(error["field"], field);
    }
    let missing = run(
        &mut shop,
        UPSERT,
        json!({"id": "gid://shopify/OnlineStoreTheme/999", "files": []}),
    );
    assert_eq!(
        missing["data"]["themeFilesUpsert"]["userErrors"][0]["code"],
        "NOT_FOUND"
    );
    let layout = run(
        &mut shop,
        "mutation { themeFilesDelete(themeId: \"gid://shopify/OnlineStoreTheme/1\", files: [\"layout/theme.liquid\", \"snippets/nope.liquid\"]) { deletedThemeFiles { filename } userErrors { code filename } } }",
        json!({}),
    );
    assert_eq!(
        layout["data"]["themeFilesDelete"]["userErrors"],
        json!([
            {"code": "FILE_VALIDATION_ERROR", "filename": "layout/theme.liquid"},
            {"code": "NOT_FOUND", "filename": "snippets/nope.liquid"},
        ])
    );
    assert!(super::storefront_overlay(&shop).is_none());
    assert!(topics(&mut shop).is_empty());
    let _ = std::fs::remove_dir_all(root);
}

fn upload(shop: &mut ShopState, key: &str, bytes: Vec<u8>) -> String {
    shop.put_blob(key, Blob::new(bytes, "application/zip"));
    format!("https://shopify-staged-uploads.storage.googleapis.com/{key}")
}

const CREATE: &str = "mutation($source: URL!, $name: String, $role: ThemeRole) {
    themeCreate(source: $source, name: $name, role: $role) {
        theme { id name role prefix processing }
        userErrors { field message code }
    }
}";

#[test]
fn a_theme_is_created_from_a_zip_duplicated_published_and_deleted() {
    let (mut shop, root) = shop_with_theme("lifecycle");
    let archive = zip::stored(&[
        ("comfort/layout/theme.liquid", b"<html><head>{{ content_for_header }}</head><body>Comfort {{ content_for_layout }}</body></html>"),
        ("comfort/templates/index.liquid", b"home"),
        ("comfort/config/settings_schema.json", br#"[{"name": "theme_info", "theme_name": "Comfort"}]"#),
        ("comfort/README.md", b"not a theme file"),
    ]);
    let url = upload(&mut shop, "tmp/1/themes/abc/comfort.zip", archive);
    let created = run(&mut shop, CREATE, json!({"source": url}));
    let theme = &created["data"]["themeCreate"]["theme"];
    assert_eq!(theme["name"], "Comfort", "{created}");
    assert_eq!(theme["role"], "UNPUBLISHED");
    let id = theme["id"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["themes/create"]);
    let files = run(
        &mut shop,
        "query($id: ID!) { theme(id: $id) { files { nodes { filename } } } }",
        json!({"id": id}),
    );
    assert_eq!(
        files["data"]["theme"]["files"]["nodes"],
        json!([{"filename": "config/settings_schema.json"}, {"filename": "layout/theme.liquid"}, {"filename": "templates/index.liquid"}])
    );

    let renamed = run(
        &mut shop,
        "mutation($id: ID!) { themeUpdate(id: $id, input: {name: \"Comfort - Summer\"}) { theme { name } userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        renamed["data"]["themeUpdate"]["theme"]["name"],
        "Comfort - Summer"
    );
    let duplicated = run(
        &mut shop,
        "mutation($id: ID!) { themeDuplicate(id: $id) { newTheme { id name role } userErrors { code } } }",
        json!({"id": id}),
    );
    let copy = &duplicated["data"]["themeDuplicate"]["newTheme"];
    assert_eq!(copy["name"], "Copy of Comfort - Summer");
    let copy_id = copy["id"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["themes/update", "themes/create"]);

    // Published, the theme is what the storefront serves, alone.
    let published = run(
        &mut shop,
        "mutation($id: ID!) { themePublish(id: $id) { theme { id role } userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(published["data"]["themePublish"]["theme"]["role"], "MAIN");
    assert_eq!(topics(&mut shop), ["themes/update", "themes/publish"]);
    let overlay = super::storefront_overlay(&shop).unwrap();
    assert!(overlay.exclusive);
    assert!(overlay.files.contains_key("layout/theme.liquid"));
    let roles = run(
        &mut shop,
        "{ main: themes(first: 5, roles: [MAIN]) { nodes { id } } named: themes(first: 5, names: [\"Copy of Comfort - Summer\"]) { nodes { id } } }",
        json!({}),
    );
    assert_eq!(
        roles["data"]["main"]["nodes"],
        json!([{"id": id}]),
        "{roles}"
    );
    assert_eq!(roles["data"]["named"]["nodes"], json!([{"id": copy_id}]));
    let main = run(
        &mut shop,
        "mutation($id: ID!) { themeDelete(id: $id) { deletedThemeId userErrors { field message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        main["data"]["themeDelete"]["deletedThemeId"],
        Value::Null,
        "the main theme stays"
    );

    // The local theme published again: the directory is served again.
    run(
        &mut shop,
        "mutation { themePublish(id: \"gid://shopify/OnlineStoreTheme/1\") { theme { id } userErrors { code } } }",
        json!({}),
    );
    assert!(super::storefront_overlay(&shop).is_none());
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { themeDelete(id: $id) { deletedThemeId userErrors { message } } }",
        json!({"id": copy_id}),
    );
    assert_eq!(
        deleted["data"]["themeDelete"]["deletedThemeId"],
        json!(copy_id)
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_theme_is_not_created_from_what_is_no_theme() {
    let mut shop = shop();
    let not_zip = upload(&mut shop, "tmp/1/themes/a/a.zip", b"plain".to_vec());
    let empty = upload(&mut shop, "tmp/1/themes/b/b.zip", zip::stored(&[]));
    let no_layout = upload(
        &mut shop,
        "tmp/1/themes/c/c.zip",
        zip::stored(&[("templates/index.liquid", b"home")]),
    );
    for (source, role, code) in [
        (not_zip.clone(), "UNPUBLISHED", "INVALID_ZIP"),
        (empty, "UNPUBLISHED", "ZIP_IS_EMPTY"),
        (no_layout, "UNPUBLISHED", "INVALID_ZIP"),
        (not_zip, "MAIN", "INVALID_THEME_ROLE_FOR_THEME_CREATION"),
    ] {
        let answer = run(&mut shop, CREATE, json!({"source": source, "role": role}));
        assert_eq!(
            answer["data"]["themeCreate"]["userErrors"][0]["code"], code,
            "{answer}"
        );
    }
    assert_eq!(shop.count("OnlineStoreTheme"), 1);
    assert!(topics(&mut shop).is_empty());
    let long = run(
        &mut shop,
        "mutation { themeUpdate(id: \"gid://shopify/OnlineStoreTheme/1\", input: {name: \"012345678901234567890123456789012345678901234567890\"}) { theme { name } userErrors { field code } } }",
        json!({}),
    );
    assert_eq!(
        long["data"]["themeUpdate"]["userErrors"],
        json!([{"field": ["input", "name"], "code": "TOO_LONG"}])
    );
}

#[test]
fn the_local_theme_is_named_by_its_settings_schema() {
    let (mut shop, root) = shop_with_theme("name");
    std::fs::write(
        root.join("config/settings_schema.json"),
        r#"[{"name": "theme_info", "theme_name": "Dawn", "theme_version": "15.0.0"}]"#,
    )
    .unwrap();
    super::attach(&mut shop, &root);
    let answer = run(
        &mut shop,
        "{ theme(id: \"gid://shopify/OnlineStoreTheme/1\") { name } }",
        json!({}),
    );
    assert_eq!(answer["data"]["theme"]["name"], "Dawn");
    assert!(
        !shop.touched(),
        "attaching the directory is no change of the Admin API"
    );
    let _ = std::fs::remove_dir_all(root);
}
