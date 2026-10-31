//! A bundle made through the Admin API, end to end: `productBundleCreate` over the demo store,
//! published on the Online Store, written back to the store data by the store sync, then sold at
//! the local checkout, where a cart transform Function expands its line into its components (the
//! checkout's fixture app, whose `bundle-transform` reads the operations from its settings).
//!
//! The store data has no components: the bundle reaches the storefront as a product of its own,
//! and the checkout sells it as one line unless a cart transform expands it, as on Shopify a
//! customized bundle is expanded by a Function.

mod support;

use std::path::Path;

use lsf_admin::graphql::{self, Request};
use lsf_admin::resolve::{Registry, ShopResolver};
use lsf_admin::state::{ShopState, parse_gid};
use lsf_admin::sync;
use lsf_checkout::app::Selection;
use lsf_checkout::{CheckoutConfig, CheckoutState, Metafield};
use lsf_core::Session;
use lsf_core::store::CartLine;
use lsf_core::store::load::{DataSource, LoadOptions, load_with_overlays};
use serde_json::{Value, json};

fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let mut request = Request::new(query);
    if let Value::Object(variables) = variables {
        request.variables = variables;
    }
    let answer = graphql::run(lsf_admin::schema::admin(), &mut resolver, &request, None).to_json();
    assert!(answer.get("errors").is_none(), "{answer}");
    answer
}

fn number(gid: &Value) -> u64 {
    gid.as_str().and_then(parse_gid).map(|(_, id)| id).unwrap()
}

#[test]
fn a_bundle_made_through_the_admin_api_is_expanded_by_a_cart_transform_at_the_checkout() {
    let store = support::demo_store();
    let mut shop = ShopState::seeded(&store, None);
    let product = |shop: &ShopState, handle: &str| {
        shop.keys("Product")
            .find(|key| shop.get(key).unwrap()["handle"] == json!(handle))
            .unwrap()
            .to_string()
    };
    let mug = product(&shop, "ceramic-mug");
    let tote = product(&shop, "canvas-tote-bag");
    let glaze = shop.get(&mug).unwrap()["options"][0].clone();
    let created = run(
        &mut shop,
        r#"mutation($input: ProductBundleCreateInput!) { productBundleCreate(input: $input) {
            productBundleOperation { product { id variants(first: 1) { nodes { id price availableForSale
                productVariantComponents(first: 5) { nodes { quantity productVariant { id } } } } } } } userErrors { message } } }"#,
        json!({"input": {"title": "Kitchen kit", "components": [
            {"productId": mug, "quantity": 1, "optionSelections": [{"componentOptionId": glaze, "name": "Mug glaze", "values": ["Oat"]}]},
            {"productId": tote, "quantity": 1, "optionSelections": []},
        ]}}),
    );
    let bundle = &created["data"]["productBundleCreate"]["productBundleOperation"]["product"];
    let variant = &bundle["variants"]["nodes"][0];
    // The sum of the mug (22.00) and the tote (24.00).
    assert_eq!(variant["price"], json!("46.00"));
    assert_eq!(variant["availableForSale"], json!(true));
    let components: Vec<u64> = variant["productVariantComponents"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|component| number(&component["productVariant"]["id"]))
        .collect();
    run(
        &mut shop,
        r#"mutation($id: ID!, $publication: ID!) { publishablePublish(id: $id, input: [{publicationId: $publication}]) { userErrors { message } } }"#,
        json!({"id": bundle["id"], "publication": lsf_admin::domains::catalog::online_store_publication_gid()}),
    );

    // The store sync writes the bundle back: a product of the storefront.
    let overlay = sync::overlay(&shop, &store).expect("the bundle syncs");
    let (synced, diagnostics) =
        load_with_overlays(&DataSource::Demo, &LoadOptions::default(), &[overlay]);
    assert!(!diagnostics.has_errors(), "{diagnostics}");
    let kit = synced
        .products
        .iter()
        .find(|product| product.handle == "kitchen-kit")
        .expect("the bundle is on the storefront");
    assert_eq!(kit.variants.len(), 1);
    assert_eq!(kit.variants[0].id, number(&variant["id"]));
    assert_eq!(kit.variants[0].price, 4600);
    assert!(kit.variants[0].available);

    // At the checkout, a cart transform expands it into its components (its settings are the
    // metafields a `cartTransformCreate` gives it, which the sync writes to `checkout.functions`).
    let app = lsf_checkout::app::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../checkout/tests/fixtures/app"),
    )
    .expect("the checkout's fixture app loads");
    let mut session = Session::initial(&synced);
    session.cart_lines = vec![CartLine {
        variant_id: kit.variants[0].id,
        quantity: 1,
        properties: Default::default(),
        selling_plan: None,
    }];
    let mut config = CheckoutConfig::default();
    let items: Vec<Value> = components
        .iter()
        .map(|variant| json!({"variant": variant, "quantity": 1}))
        .collect();
    config.function_metafields.insert(
        "bundle-transform".to_string(),
        vec![Metafield {
            namespace: "$app:config".to_string(),
            key: "settings".to_string(),
            kind: "json".to_string(),
            value: json!({"operations": [{"type": "expand", "line": 1, "items": items}]})
                .to_string(),
        }],
    );
    let result = lsf_checkout::pipeline::compute(
        &app,
        &Selection::only(["bundle-transform"]),
        &synced,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let line = &result.lines[0];
    assert_eq!(line.transformed_by.as_deref(), Some("expand"));
    assert_eq!(line.unit_price, 4600);
    assert_eq!(
        line.components
            .iter()
            .map(|component| component.variant_id)
            .collect::<Vec<_>>(),
        components
    );
}
