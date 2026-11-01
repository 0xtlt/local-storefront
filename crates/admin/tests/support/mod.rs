//! Helpers of the integration tests.

#![allow(dead_code)]

pub mod receiver;
pub mod shape;

use std::path::{Path, PathBuf};

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

/// The app of `tests/fixtures/app`.
pub fn fixture_app() -> lsf_checkout::app::App {
    lsf_checkout::app::load(&fixture("app")).expect("the fixture app loads")
}

/// The JSON body of an answer of the Admin API.
pub fn json_of(response: &lsf_admin::api::HttpResponse) -> serde_json::Value {
    serde_json::from_slice(&response.body)
        .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&response.body)))
}
