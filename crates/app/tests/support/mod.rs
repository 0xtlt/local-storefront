//! Helpers of the integration tests.

#![allow(dead_code)]

pub mod receiver;
pub mod shape;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lsf_app::admin::api::{HttpRequest, HttpResponse};
use lsf_core::Store;
use lsf_core::store::load::{DataSource, LoadOptions, load};

/// A fixture of `tests/fixtures`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The demo store of lsf-core.
pub fn demo_store() -> Store {
    store(&DataSource::Demo)
}

/// A store of `tests/fixtures`, which must load without error.
pub fn fixture_store(name: &str) -> Store {
    store(&DataSource::Directory(fixture(name)))
}

fn store(source: &DataSource) -> Store {
    let (store, diagnostics) = load(source, &LoadOptions::default());
    assert!(
        !diagnostics.has_errors(),
        "the store data has errors:\n{diagnostics:?}"
    );
    store
}

/// The app of `tests/fixtures/admin-app`.
pub fn fixture_app() -> lsf_app::config::App {
    lsf_app::config::load(&fixture("admin-app")).expect("the fixture app loads")
}

/// The JSON body of an answer of the Admin API.
pub fn json_of(response: &HttpResponse) -> serde_json::Value {
    serde_json::from_slice(&response.body)
        .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&response.body)))
}

/// A GraphQL request to the endpoint of the default version, with `token`.
pub fn graphql_request(token: &str, query: &str, variables: serde_json::Value) -> HttpRequest {
    HttpRequest {
        method: "POST".to_string(),
        path: format!(
            "/admin/api/{}/graphql.json",
            lsf_app::admin::version::DEFAULT
        ),
        headers: HashMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("x-shopify-access-token".to_string(), token.to_string()),
        ]),
        body: serde_json::json!({"query": query, "variables": variables})
            .to_string()
            .into_bytes(),
        ..HttpRequest::default()
    }
}
