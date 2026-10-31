//! What lsf-admin covers of the GraphQL Admin API: every mutation of every version it knows is
//! simulated, and every webhook topic has a payload it builds, but for the ones listed here, with
//! why. A mutation or a topic added to the API (a new version) fails these tests until it is
//! simulated or listed.

mod support;

use std::collections::BTreeSet;
use std::time::Duration;

use lsf_admin::api::{AdminApi, AdminOptions, ThrottleSetting};
use lsf_admin::version::ApiVersion;
use lsf_admin::webhooks::DeliveryOptions;
use lsf_admin::webhooks::payload::BUILT_TOPICS;
use lsf_admin::{schema, webhooks};

/// The mutations being written, which answer `LSF_NOT_SIMULATED` until they are. Empty when
/// everything is simulated: take a mutation out once it is.
const NOT_SIMULATED_YET: &[&str] = &[];

/// The topics lsf builds no payload for, and why. It logs them as not sent rather than send a
/// made-up body.
const NOT_BUILT: &[(&str, &str)] = &[
    (
        "returns/process",
        "Shopify publishes no example payload for it (webhook reference, REST reference, dev-mcp)",
    ),
    (
        "shipping_addresses/create",
        "Shopify publishes no example payload for it",
    ),
    (
        "shipping_addresses/update",
        "Shopify publishes no example payload for it",
    ),
];

/// The topics whose payload is being written: empty when every topic with an example is built.
const NOT_BUILT_YET: &[&str] = &[];

fn api() -> AdminApi {
    AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: None,
            client_secret: "coverage".to_string(),
            app_url: None,
            throttle: ThrottleSetting::Store,
            delivery: DeliveryOptions {
                timeout: Duration::from_secs(1),
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(7),
        },
    )
}

#[test]
fn every_mutation_of_every_version_is_simulated() {
    let status = api().status();
    let simulated: BTreeSet<&str> = status["mutations"]
        .as_array()
        .expect("the status lists the mutations")
        .iter()
        .filter_map(|name| name.as_str())
        .collect();
    let mut missing = Vec::new();
    for version in ApiVersion::all() {
        let schema = schema::for_version(version);
        let root = schema
            .mutation_type()
            .and_then(|name| schema.get(name))
            .expect("the schema has mutations");
        for name in root.fields.keys() {
            if !simulated.contains(name.as_str()) && !NOT_SIMULATED_YET.contains(&name.as_str()) {
                missing.push(format!("{version} {name}"));
            }
        }
    }
    assert!(missing.is_empty(), "not simulated: {missing:#?}");
    let done: Vec<&&str> = NOT_SIMULATED_YET
        .iter()
        .filter(|name| simulated.contains(**name))
        .collect();
    assert!(
        done.is_empty(),
        "simulated now, take them out of NOT_SIMULATED_YET: {done:?}"
    );
}

#[test]
fn every_topic_of_every_version_has_a_payload() {
    let built: BTreeSet<&str> = BUILT_TOPICS.iter().copied().collect();
    let listed = |topic: &str| {
        NOT_BUILT.iter().any(|(not_built, _)| *not_built == topic) || NOT_BUILT_YET.contains(&topic)
    };
    let mut missing = Vec::new();
    for version in ApiVersion::all() {
        for topic in webhooks::topics::for_version(version).all() {
            if !built.contains(topic.written.as_str()) && !listed(&topic.written) {
                missing.push(format!("{version} {}", topic.written));
            }
        }
    }
    assert!(missing.is_empty(), "no payload: {missing:#?}");
    let done: Vec<&&str> = NOT_BUILT_YET
        .iter()
        .filter(|topic| built.contains(**topic))
        .collect();
    assert!(
        done.is_empty(),
        "built now, take them out of NOT_BUILT_YET: {done:?}"
    );
}
