//! Every record the seed makes is in the shape of its GraphQL type.

mod support;

use lsf_admin::state::ShopState;
use support::shape::Schema;

fn check(state: &ShopState) {
    let Some(schema) = Schema::load() else {
        return;
    };
    let problems = schema.check(state);
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn the_records_of_the_demo_store_are_in_the_shape_of_their_types() {
    check(&ShopState::seeded(&support::demo_store(), None));
}

#[test]
fn the_records_of_the_rich_fixture_are_in_the_shape_of_their_types() {
    let app = support::fixture_app();
    check(&ShopState::seeded(
        &support::fixture_store("rich"),
        Some(&app),
    ));
}

#[test]
fn the_checker_finds_what_is_wrong() {
    let Some(schema) = Schema::load() else {
        return;
    };
    let mut state = ShopState::seeded(&support::demo_store(), None);
    let product = state.keys("Product").next().unwrap().to_string();
    let variant = state.keys("ProductVariant").next().unwrap().to_string();
    let record = state.get_mut(&product).unwrap();
    record.insert("colour".into(), "red".into());
    record.insert("status".into(), "LIVE".into());
    record.insert("createdAt".into(), "2024-01-01".into());
    record.insert("featuredMedia".into(), variant.clone().into());
    record.insert(
        "variants".into(),
        serde_json::json!(["gid://shopify/ProductVariant/1"]),
    );
    record.insert("title".into(), serde_json::Value::Null);
    let record = state.get_mut(&variant).unwrap();
    record.insert("price".into(), "19.9".into());
    record.insert(
        "selectedOptions".into(),
        serde_json::json!([{"name": "Size"}]),
    );
    let problems = schema.check(&state).join("\n");
    for expected in [
        ".colour: `Product` has no field `colour`",
        ".status: \"LIVE\" is not a value of `ProductStatus`",
        ".createdAt: \"2024-01-01\" is not a `DateTime`",
        ".featuredMedia: gid://shopify/ProductVariant/",
        ".variants[0]: gid://shopify/ProductVariant/1 is no record of the shop",
        ".title: null for a non-null field",
        ".price: \"19.9\" is not a `Money`",
    ] {
        assert!(
            problems.contains(expected),
            "{expected} not in:\n{problems}"
        );
    }
}

#[test]
fn the_records_of_a_created_shop_and_of_a_placed_order_are_in_shape() {
    let store = support::demo_store();
    let mut shops = lsf_admin::state::Shops::new(&store, None);
    let shop = shops.create("test-1").unwrap();
    let tee = store.product_by_handle("organic-cotton-t-shirt").unwrap();
    let variant = &tee.variants[0];
    let result = lsf_checkout::CheckoutResult {
        lines: vec![lsf_checkout::ResultLine {
            id: "gid://shopify/CartLine/1".into(),
            variant_id: variant.id,
            product_id: tee.id,
            title: tee.title.clone(),
            quantity: 1,
            unit_price: variant.price,
            subtotal: variant.price,
            total: variant.price,
            ..Default::default()
        }],
        ..Default::default()
    };
    lsf_admin::domains::orders::record_placed_order(
        shop,
        &lsf_admin::domains::orders::PlacedOrder {
            id: None,
            number: Some(1005),
            processed_at: "2026-05-01T09:30:00Z".parse().unwrap(),
            result: &result,
            customer_id: store.customers.first().map(|customer| customer.id),
            company_location_id: None,
            email: None,
            phone: None,
            shipping_address: None,
            note: None,
            attributes: &[],
            payment: None,
        },
    );
    check(shop);
}
