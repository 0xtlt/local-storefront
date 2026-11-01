//! What the tests of the domains share: the demo store and the rich fixture
//! (`tests/fixtures/rich`) as shops, a request run on a shop, the topics it emitted.

use serde_json::Value;

use crate::graphql::{self, Request};
use crate::resolve::{Registry, ShopResolver};
use crate::state::{Clock, ShopState};

/// The demo store.
pub fn demo_store() -> lsf_core::Store {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    store
}

/// The demo store, seeded.
pub fn demo() -> ShopState {
    ShopState::seeded(&demo_store(), None)
}

/// The rich fixture, seeded.
pub fn rich() -> ShopState {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rich");
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Directory(path),
        &Default::default(),
    );
    ShopState::seeded(&store, None)
}

/// `shop` on a clock frozen at 2026-10-31T12:00:00Z: the demo store and the rich fixture run on
/// the system's, a frozen one gives every change the same time, and the same dates every run.
pub fn frozen(mut shop: ShopState) -> ShopState {
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-31T12:00:00Z").unwrap();
    shop.set_clock(Clock::Frozen(now.with_timezone(&chrono::Utc)));
    shop
}

/// The answer to a request on the shop, `extensions` without `cost`.
pub fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let mut answer = graphql::run(
        crate::schema::admin(),
        &mut resolver,
        &Request::new(query).with_variables(variables),
        None,
    );
    answer.extensions.extend(resolver.extensions());
    answer.extensions.remove("cost");
    answer.to_json()
}

/// The topics of the events emitted since the last call, in order.
pub fn topics(shop: &mut ShopState) -> Vec<String> {
    shop.take_events()
        .into_iter()
        .map(|event| event.topic)
        .collect()
}
