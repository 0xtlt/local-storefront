//! Store sync as a server runs it, through `AdminApi`: a change of the default shop is counted at
//! once and projected when the server asks, the other shops never sync, a reset and a reseed put
//! the store back to the files.

mod support;

use std::sync::Arc;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest};
use lsf_admin::domains::catalog;
use lsf_core::Store;
use serde_json::{Value, json};

const GRAPHQL: &str = "/admin/api/2026-10/graphql.json";

fn api() -> (AdminApi, Arc<Store>) {
    let base = Arc::new(support::demo_store());
    let api = AdminApi::new(
        &base,
        AdminOptions {
            client_secret: "test-secret".to_string(),
            token_seed: Some(7),
            ..AdminOptions::default()
        },
    );
    (api, base)
}

fn request(api: &AdminApi, method: &str, path: &str, token: Option<&str>, body: Value) -> Value {
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    if let Some(token) = token {
        headers.push(("x-shopify-access-token".to_string(), token.to_string()));
    }
    let response = api
        .handle(&HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            headers: headers.into_iter().collect(),
            body: if body.is_null() {
                Vec::new()
            } else {
                body.to_string().into_bytes()
            },
            ..HttpRequest::default()
        })
        .expect("routed");
    serde_json::from_slice(&response.body).expect("JSON")
}

fn graphql(api: &AdminApi, token: &str, query: &str) -> Value {
    request(api, "POST", GRAPHQL, Some(token), json!({ "query": query }))
}

#[test]
fn changes_of_the_default_shop_are_projected_when_the_server_asks() {
    let (api, base) = api();
    let token = api.token(&api.default_shop());
    // Off until the server turns it on.
    assert!(api.synced().is_none());
    assert_eq!(
        request(&api, "GET", "/__lsf/admin", None, Value::Null)["storeSync"]["enabled"],
        false
    );
    api.sync_store(base.clone());
    let synced = api.synced().expect("on");
    assert_eq!((synced.generation, synced.overlay.is_none()), (0, true));

    // The first request does what the clock passed since the seed (the demo store's orders of
    // past days are paid out), a change like any other. A query then writes nothing: nothing to
    // count.
    graphql(&api, &token, "{ shop { name } }");
    let changes = api.store_changes();
    graphql(&api, &token, "{ products(first: 2) { nodes { title } } }");
    assert_eq!(api.store_changes(), changes);

    // A mutation is counted at once, and projected only when asked.
    let created = graphql(
        &api,
        &token,
        r#"mutation { productCreate(product: {title: "Wool Hat", status: ACTIVE}) { product { id } } }"#,
    );
    assert!(
        created["data"]["productCreate"]["product"]["id"].is_string(),
        "{created}"
    );
    assert!(api.store_changes() > changes);
    let status = request(&api, "GET", "/__lsf/admin", None, Value::Null);
    assert_eq!(status["storeSync"]["pending"], true, "{status}");
    let synced = api.synced().unwrap();
    assert_eq!(synced.generation, 1);
    assert_eq!(synced.changes, api.store_changes());
    let overlay = synced.overlay.clone().expect("an overlay");
    let products = overlay.data["products"].as_array().unwrap();
    assert_eq!(products.len(), 1);
    assert_eq!(products[0]["handle"], "wool-hat");
    // Asked again without a change: the same overlay, not made again.
    let again = api.synced().unwrap();
    assert_eq!(again.generation, 1);
    assert!(Arc::ptr_eq(again.overlay.as_ref().unwrap(), &overlay));
    let status = request(&api, "GET", "/__lsf/admin", None, Value::Null);
    assert_eq!(status["storeSync"]["pending"], false);
    assert_eq!(status["storeSync"]["overlay"]["written"]["products"], 1);

    // A change the store data does not show is counted, but makes the same overlay.
    let tee = catalog::product_gid(&base.products[0]);
    api.change_shop(&api.default_shop(), |state| {
        if let Some(record) = state.get_mut(&tee) {
            record.insert("totalInventory".to_string(), json!(12345));
        }
    });
    let unchanged = api.synced().unwrap();
    assert_eq!(unchanged.generation, 1);

    // What the server built is in the status.
    api.sync_built(1, std::time::Duration::from_millis(3), &Default::default());
    let status = request(&api, "GET", "/__lsf/admin", None, Value::Null);
    assert_eq!(status["storeSync"]["built"]["generation"], 1, "{status}");
    assert_eq!(status["storeSync"]["built"]["diagnostics"], json!([]));

    // A reset puts the store back to the files.
    request(&api, "DELETE", "/__lsf/admin", None, Value::Null);
    let reset = api.synced().unwrap();
    assert_eq!((reset.generation, reset.overlay.is_none()), (2, true));
}

#[test]
fn other_shops_never_sync() {
    let (api, base) = api();
    api.sync_store(base);
    let created = request(
        &api,
        "POST",
        "/__lsf/admin/shops",
        None,
        json!({"shop": "test-1"}),
    );
    let token = created["token"].as_str().expect("a token").to_string();
    let changes = api.store_changes();
    graphql(
        &api,
        &token,
        r#"mutation { productCreate(product: {title: "Isolated"}) { product { id } } }"#,
    );
    assert_eq!(api.store_changes(), changes);
    assert!(api.synced().unwrap().overlay.is_none());
}

#[test]
fn a_reseed_moves_the_projection_to_the_new_files() {
    let (api, base) = api();
    api.sync_store(base.clone());
    let token = api.token(&api.default_shop());
    graphql(
        &api,
        &token,
        r#"mutation { productCreate(product: {title: "Wool Hat"}) { product { id } } }"#,
    );
    assert!(api.synced().unwrap().overlay.is_some());
    // The files changed: the shops are seeded again, and sync goes on over the new store.
    let reseeded = api.reseed(&base);
    assert_eq!(reseeded.dropped, [api.default_shop()]);
    api.sync_store(base);
    let synced = api.synced().unwrap();
    assert!(synced.overlay.is_none());
    assert_eq!(synced.generation, 2);
}
