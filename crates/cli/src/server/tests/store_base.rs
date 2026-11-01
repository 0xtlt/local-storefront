//! What store sync builds on: the data the files gave when the store was loaded, never the
//! files on disk again. `--static` reads them once; without `--watch`, a file changed on disk
//! is not seen until a reload, by the storefront or by what store sync builds for it.

use std::path::Path;

use lsf_core::theme::Revalidate;
use serde_json::{Value as Json, json};

use super::super::{ServeOptions, ServerState};
use super::{ScenarioData, admin_query, body_json, get, request, retitle, text, title_on_page};
use crate::app::App;

/// A server over a copy of the demo store's files, with the Admin API, that does not watch
/// them: what `lsf app dev --static --data <dir>` serves.
fn static_server(data: &Path) -> ServerState {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
    let app = App::open(&theme, Some(data), Revalidate::Never, &[]).expect("the theme");
    ServerState::new(
        app,
        ServeOptions {
            admin: Some(Default::default()),
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
}

fn product_count(state: &ServerState) -> Json {
    body_json(&get(state, "/__lsf/status", "s"))["counts"]["products"].clone()
}

fn admin_product_count(state: &ServerState) -> Json {
    admin_query(state, "{ productsCount { count } }")["data"]["productsCount"]["count"].clone()
}

#[test]
fn a_static_server_builds_what_the_admin_api_changed_on_the_files_it_read() {
    let data = ScenarioData::new("static-base", &[]);
    let state = static_server(&data.0);
    assert_eq!(product_count(&state), json!(8));
    assert_eq!(admin_product_count(&state), json!(8));

    // A product file added on disk after start: `--static` never reads the files again.
    std::fs::write(
        data.0.join("products/added-on-disk.json"),
        r#"{"title": "Added On Disk", "price": 900}"#,
    )
    .unwrap();
    retitle(&state, "ceramic-mug", "Stoneware Mug");
    assert_eq!(title_on_page(&state, "ceramic-mug", "s"), "Stoneware Mug");
    // The storefront counted 9 products here, the Admin API 8.
    assert_eq!(product_count(&state), json!(8));
    assert_eq!(admin_product_count(&state), json!(8));
    assert_eq!(get(&state, "/products/added-on-disk", "s").status, 404);

    // A session with data of its own is built on the same files.
    let reply = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"data": {"products": [{"title": "Only Here", "price": "5.00"}]}}),
        "own",
    ));
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(get(&state, "/products/only-here", "own").status, 200);
    assert_eq!(get(&state, "/products/added-on-disk", "own").status, 404);
    retitle(&state, "wool-beanie", "Wool Hat");
    let page = text(&get(&state, "/products/wool-beanie", "own"));
    assert!(page.contains("<h1>Wool Hat</h1>"), "{page}");
    assert_eq!(get(&state, "/products/added-on-disk", "own").status, 404);
}

#[test]
fn a_file_changed_on_disk_without_a_reload_does_not_leak_into_the_synced_store() {
    let data = ScenarioData::new("unreloaded-base", &[]);
    let state = static_server(&data.0);
    assert_eq!(
        title_on_page(&state, "canvas-tote-bag", "s"),
        "Canvas Tote Bag"
    );

    // The file of another product changes on disk; nothing reloads it.
    let tote = data.0.join("products/canvas-tote-bag.json");
    let content = std::fs::read_to_string(&tote).unwrap();
    std::fs::write(
        &tote,
        content.replace("\"Canvas Tote Bag\"", "\"Edited On Disk\""),
    )
    .unwrap();
    retitle(&state, "ceramic-mug", "Stoneware Mug");
    assert_eq!(title_on_page(&state, "ceramic-mug", "s"), "Stoneware Mug");
    // The overlay was computed against the store the files gave at start: the tote is as it was.
    assert_eq!(
        title_on_page(&state, "canvas-tote-bag", "s"),
        "Canvas Tote Bag"
    );
    let loaded = state.loaded();
    assert_eq!(loaded.synced.generation, 1);
    assert!(
        loaded.synced.diagnostics.is_empty(),
        "{}",
        loaded.synced.diagnostics
    );

    // A reload reads the files as they are now.
    state.reload();
    assert_eq!(
        title_on_page(&state, "canvas-tote-bag", "s"),
        "Edited On Disk"
    );
}
