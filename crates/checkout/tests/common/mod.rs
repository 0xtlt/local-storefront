#![allow(dead_code)]

use std::path::{Path, PathBuf};

use lsf_checkout::app::{self, App};
use lsf_checkout::{CheckoutConfig, CheckoutResult, Metafield, PaymentMethodConfig, ShippingRate};
use lsf_core::store::CartLine;
use lsf_core::store::load::{DataSource, LoadOptions, load};
use lsf_core::{Session, Store};

/// Variants of the demo store.
pub const TEE: u64 = 7082666010490; // 32.00
pub const TOTE: u64 = 4381608573867; // 24.00
pub const MUG: u64 = 4340730137827; // 22.00
pub const GIFT_CARD: u64 = 6085636638502; // 25.00, no shipping

pub fn fixture_app_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app")
}

pub fn app() -> App {
    app::load(&fixture_app_dir()).expect("the fixture app loads")
}

pub fn store() -> Store {
    load(&DataSource::Demo, &LoadOptions::default()).0
}

pub fn session(store: &Store, lines: &[(u64, u32)]) -> Session {
    let mut session = Session::initial(store);
    session.cart_lines = lines
        .iter()
        .map(|(variant_id, quantity)| CartLine {
            variant_id: *variant_id,
            quantity: *quantity,
            properties: Default::default(),
            selling_plan: None,
        })
        .collect();
    session
}

pub fn config() -> CheckoutConfig {
    let rate = |handle: &str, title: &str, price| ShippingRate {
        handle: handle.to_string(),
        title: title.to_string(),
        price,
        description: None,
        kind: Default::default(),
    };
    let method = |handle: &str, name: &str| PaymentMethodConfig {
        handle: handle.to_string(),
        name: name.to_string(),
    };
    CheckoutConfig {
        shipping_rates: vec![
            rate("standard", "Standard", 490),
            rate("express", "Express", 1290),
        ],
        payment_methods: vec![
            method("card", "Credit card"),
            method("cod", "Cash on Delivery"),
            method("bank", "Bank Deposit"),
        ],
        ..CheckoutConfig::default()
    }
}

/// The metafield every fixture Function reads its settings from.
pub fn settings(value: serde_json::Value) -> Vec<Metafield> {
    vec![Metafield {
        namespace: "$app:config".to_string(),
        key: "settings".to_string(),
        kind: "json".to_string(),
        value: value.to_string(),
    }]
}

pub fn run_for<'a>(result: &'a CheckoutResult, handle: &str) -> &'a lsf_checkout::FunctionRun {
    result
        .runs
        .iter()
        .find(|run| run.handle == handle)
        .unwrap_or_else(|| {
            panic!(
                "no run of {handle}: {:?}",
                result.runs.iter().map(|r| &r.handle).collect::<Vec<_>>()
            )
        })
}
