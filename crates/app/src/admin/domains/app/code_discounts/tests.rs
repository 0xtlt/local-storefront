//! Codes of Shopify's own discount types, made with the Admin API, applied by the local checkout
//! of the demo store, and the orders placed with them.

use serde_json::{Value, json};

use super::{Buyer, for_checkout};
use crate::admin::domains::orders::{PlacedOrder, record_placed_order};
use crate::admin::domains::test_support::{demo, demo_store, run};
use crate::admin::state::ShopState;
use crate::checkout::pipeline::compute;
use crate::checkout::{
    Address, AllocationMethod, CheckoutConfig, CheckoutResult, CheckoutState, DiscountClass,
    DiscountTarget, DiscountValue, ShippingRate, TargetSelection,
};
use crate::config::{App, Selection};

/// Variants of the demo store.
const TEE: u64 = 7082666010490; // 32.00
const TOTE: u64 = 4381608573867; // 24.00
const MUG: u64 = 4340730137827; // 22.00

fn product_of(store: &lsf_core::Store, variant: u64) -> String {
    let (product, _) = store.variant(variant).expect("a variant of the demo store");
    format!("gid://shopify/Product/{}", product.id)
}

/// Makes a discount with `mutation` (`discountCodeBasicCreate`, `…FreeShippingCreate`) and its
/// `input` argument (`basicCodeDiscount`, `freeShippingCodeDiscount`).
fn create(shop: &mut ShopState, mutation: &str, argument: &str, input: Value) -> String {
    let answer = run(
        shop,
        &format!(
            "mutation($input: {}) {{ {mutation}({argument}: $input) {{ codeDiscountNode {{ id }} userErrors {{ field message }} }} }}",
            if argument == "basicCodeDiscount" {
                "DiscountCodeBasicInput!"
            } else {
                "DiscountCodeFreeShippingInput!"
            }
        ),
        json!({"input": input}),
    );
    let payload = &answer["data"][mutation];
    assert_eq!(payload["userErrors"], json!([]), "{answer}");
    payload["codeDiscountNode"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{answer}"))
        .to_string()
}

fn basic(shop: &mut ShopState, input: Value) -> String {
    create(shop, "discountCodeBasicCreate", "basicCodeDiscount", input)
}

fn free_shipping(shop: &mut ShopState, input: Value) -> String {
    create(
        shop,
        "discountCodeFreeShippingCreate",
        "freeShippingCodeDiscount",
        input,
    )
}

/// A buyer at a US address who entered `codes`.
fn entering(codes: &[&str], email: Option<&str>) -> CheckoutState {
    CheckoutState {
        email: email.map(str::to_string),
        shipping_address: Some(Address {
            country_code: Some("US".into()),
            ..Address::default()
        }),
        discount_codes: codes.iter().map(|code| code.to_string()).collect(),
        ..CheckoutState::default()
    }
}

/// The checkout of the demo store for a cart, the codes the buyer entered, and who they are:
/// no Function runs, two shipping rates (4.90 and 12.90), the first selected.
fn checkout(
    shop: &ShopState,
    lines: &[(u64, u32)],
    state: &CheckoutState,
    customer_id: Option<u64>,
) -> CheckoutResult {
    let store = demo_store();
    let mut session = lsf_core::Session::initial(&store);
    session.customer_id = customer_id;
    session.cart_lines = lines
        .iter()
        .map(|(variant_id, quantity)| lsf_core::store::CartLine {
            variant_id: *variant_id,
            quantity: *quantity,
            properties: Default::default(),
            selling_plan: None,
        })
        .collect();
    let rate = |handle: &str, price| ShippingRate {
        handle: handle.into(),
        title: handle.into(),
        price,
        description: None,
        kind: Default::default(),
    };
    let config = CheckoutConfig {
        shipping_rates: vec![rate("standard", 490), rate("express", 1290)],
        code_discounts: for_checkout(
            shop,
            &Buyer {
                customer_id,
                email: state.email.as_deref(),
            },
        ),
        ..CheckoutConfig::default()
    };
    compute(
        &App::default(),
        &Selection::none(),
        &store,
        &session,
        state,
        &config,
    )
}

/// Each code entered: whether it applies, and why not.
fn statuses(result: &CheckoutResult) -> Vec<(String, bool, Option<String>)> {
    result
        .discount_codes
        .iter()
        .map(|status| {
            (
                status.code.clone(),
                status.applicable,
                status.reason.clone(),
            )
        })
        .collect()
}

/// Why the only code entered was refused.
fn refusal(result: &CheckoutResult) -> String {
    let status = &result.discount_codes[0];
    assert!(!status.applicable, "{:?}", result.discount_codes);
    assert!(result.discounts.is_empty());
    status.reason.clone().expect("a refused code says why")
}

fn place(shop: &mut ShopState, result: &CheckoutResult, email: Option<&str>) -> String {
    try_place(shop, result, email)
        .expect("the codes the checkout applied are still the buyer's")
        .id
}

/// Records the order of a checkout, or why a code it applied is no longer the buyer's.
fn try_place(
    shop: &mut ShopState,
    result: &CheckoutResult,
    email: Option<&str>,
) -> Result<crate::admin::domains::orders::RecordedOrder, super::Refused> {
    record_placed_order(
        shop,
        &PlacedOrder {
            id: None,
            number: None,
            processed_at: "2026-05-01T09:30:00Z".parse().unwrap(),
            result,
            customer_id: None,
            company_location_id: None,
            email,
            phone: None,
            shipping_address: None,
            note: None,
            attributes: &[],
            payment: Some("Credit card"),
            metafields: &[],
        },
    )
}

#[test]
fn a_percentage_off_the_order_is_shared_between_its_lines() {
    let mut shop = demo();
    basic(
        &mut shop,
        json!({"title": "Spring", "code": "SPRING10", "customerGets": {"value": {"percentage": 0.1}, "items": {"all": true}}}),
    );
    // Entered in another case, as Shopify takes it.
    let result = checkout(
        &shop,
        &[(TEE, 1), (MUG, 2)],
        &entering(&["spring10"], None),
        None,
    );
    assert_eq!(statuses(&result), [("spring10".to_string(), true, None)]);
    // 32.00 + 44.00 = 76.00, 10 % off, shared between the two lines.
    assert_eq!(result.discounts.len(), 1);
    let discount = &result.discounts[0];
    assert_eq!(discount.code.as_deref(), Some("SPRING10"));
    assert_eq!(discount.title, "Spring");
    assert_eq!(discount.class, DiscountClass::Order);
    assert_eq!(discount.amount, 760);
    assert_eq!(discount.function, "");
    assert_eq!(discount.details.value, DiscountValue::Percentage(10.0));
    assert_eq!(discount.details.allocation_method, AllocationMethod::Across);
    assert_eq!(discount.details.target_selection, TargetSelection::All);
    assert_eq!(result.lines[0].discount_allocations[0].amount, 320);
    assert_eq!(result.lines[1].discount_allocations[0].amount, 440);
    assert_eq!(result.cost.discounts, 760);
    assert_eq!(result.cost.total, 7600 - 760 + 490);

    // Without the code, nothing.
    let without = checkout(&shop, &[(TEE, 1), (MUG, 2)], &entering(&[], None), None);
    assert!(without.discounts.is_empty());
}

#[test]
fn an_amount_comes_off_listed_products_once_or_off_each_item() {
    let mut shop = demo();
    let store = demo_store();
    basic(
        &mut shop,
        json!({"title": "Tee", "code": "TEE5", "customerGets": {
            "value": {"discountAmount": {"amount": "5.00", "appliesOnEachItem": false}},
            "items": {"products": {"productsToAdd": [product_of(&store, TEE)]}}}}),
    );
    basic(
        &mut shop,
        json!({"title": "Mugs", "code": "MUG3", "customerGets": {
            "value": {"discountAmount": {"amount": "3.00", "appliesOnEachItem": true}},
            "items": {"products": {"productVariantsToAdd": [format!("gid://shopify/ProductVariant/{MUG}")]}}}}),
    );

    // 5.00 once, off the tees only, whatever their number.
    let tee = checkout(
        &shop,
        &[(TEE, 2), (TOTE, 1)],
        &entering(&["TEE5"], None),
        None,
    );
    assert_eq!(tee.discounts.len(), 1, "{:?}", tee.discount_codes);
    assert_eq!(tee.discounts[0].class, DiscountClass::Product);
    assert_eq!(tee.discounts[0].amount, 500);
    assert_eq!(tee.discounts[0].details.value, DiscountValue::Fixed(500));
    assert_eq!(
        tee.discounts[0].details.allocation_method,
        AllocationMethod::Across
    );
    assert_eq!(
        tee.discounts[0].details.target_selection,
        TargetSelection::Entitled
    );
    assert_eq!(tee.lines[0].total, 6400 - 500);
    assert_eq!(tee.lines[1].total, 2400);

    // 3.00 off each of the two mugs.
    let mugs = checkout(
        &shop,
        &[(TEE, 1), (MUG, 2)],
        &entering(&["MUG3"], None),
        None,
    );
    assert_eq!(mugs.discounts[0].amount, 600);
    assert_eq!(
        mugs.discounts[0].details.allocation_method,
        AllocationMethod::Each
    );
    assert_eq!(mugs.lines[0].total, 3200);
    assert_eq!(mugs.lines[1].total, 4400 - 600);

    // A cart without the items it applies to does not take the code.
    let none = checkout(&shop, &[(TOTE, 1)], &entering(&["TEE5"], None), None);
    assert_eq!(
        refusal(&none),
        "the cart has none of the items it applies to"
    );
}

#[test]
fn a_code_off_a_collection_applies_to_the_products_it_has() {
    let mut shop = demo();
    let store = demo_store();
    let collection = store
        .collections
        .iter()
        .find(|collection| {
            collection.products.iter().any(|index| {
                store.products[*index]
                    .variants
                    .iter()
                    .any(|variant| variant.id == TEE)
            }) && collection.products.iter().all(|index| {
                store.products[*index]
                    .variants
                    .iter()
                    .all(|variant| variant.id != TOTE)
            })
        })
        .expect("a collection of the demo store has the tee and not the tote");
    basic(
        &mut shop,
        json!({"title": "Collection", "code": "COLL20", "customerGets": {
            "value": {"percentage": 0.2},
            "items": {"collections": {"add": [format!("gid://shopify/Collection/{}", collection.id)]}}}}),
    );
    let result = checkout(
        &shop,
        &[(TEE, 1), (TOTE, 1)],
        &entering(&["COLL20"], None),
        None,
    );
    assert_eq!(result.discounts.len(), 1, "{:?}", result.discount_codes);
    assert_eq!(result.lines[0].total, 3200 - 640);
    assert_eq!(result.lines[1].total, 2400);
}

#[test]
fn a_code_is_refused_outside_its_dates_and_past_its_usage_limit() {
    let mut shop = demo();
    let all = json!({"value": {"percentage": 0.1}, "items": {"all": true}});
    basic(
        &mut shop,
        json!({"title": "Later", "code": "LATER", "startsAt": "2099-01-01T00:00:00Z", "customerGets": all}),
    );
    basic(
        &mut shop,
        json!({"title": "Gone", "code": "GONE", "startsAt": "2020-01-01T00:00:00Z",
               "endsAt": "2020-02-01T00:00:00Z", "customerGets": all}),
    );
    basic(
        &mut shop,
        json!({"title": "Once", "code": "ONCE", "usageLimit": 1, "customerGets": all}),
    );
    let cart = [(TEE, 1)];
    assert_eq!(
        refusal(&checkout(&shop, &cart, &entering(&["LATER"], None), None)),
        "it is scheduled: it starts at 2099-01-01T00:00:00Z"
    );
    assert_eq!(
        refusal(&checkout(&shop, &cart, &entering(&["GONE"], None), None)),
        "it expired at 2020-02-01T00:00:00Z"
    );
    // A code no discount has is not applicable, and lsf has nothing to say of it.
    let unknown = checkout(&shop, &cart, &entering(&["NOPE"], None), None);
    assert_eq!(statuses(&unknown), [("NOPE".to_string(), false, None)]);

    // The first order uses the only use.
    let first = checkout(&shop, &cart, &entering(&["ONCE"], None), None);
    assert_eq!(first.discounts.len(), 1);
    place(&mut shop, &first, Some("first@example.com"));
    assert_eq!(
        refusal(&checkout(&shop, &cart, &entering(&["ONCE"], None), None)),
        "it reached its usage limit: used 1 times of 1"
    );
}

#[test]
fn a_code_needs_its_minimum_on_the_items_it_applies_to() {
    let mut shop = demo();
    let store = demo_store();
    basic(
        &mut shop,
        json!({"title": "Fifty", "code": "FIFTY", "minimumRequirement": {"subtotal": {"greaterThanOrEqualToSubtotal": "50.00"}},
               "customerGets": {"value": {"percentage": 0.1}, "items": {"all": true}}}),
    );
    basic(
        &mut shop,
        json!({"title": "Three tees", "code": "THREE", "minimumRequirement": {"quantity": {"greaterThanOrEqualToQuantity": "3"}},
               "customerGets": {"value": {"percentage": 0.1},
                                "items": {"products": {"productsToAdd": [product_of(&store, TEE)]}}}}),
    );
    assert_eq!(
        refusal(&checkout(
            &shop,
            &[(TEE, 1)],
            &entering(&["FIFTY"], None),
            None
        )),
        "its minimum purchase is 50.00 USD, and the items it applies to come to 32.00 USD after the product discounts"
    );
    let enough = checkout(
        &shop,
        &[(TEE, 1), (TOTE, 1)],
        &entering(&["FIFTY"], None),
        None,
    );
    assert_eq!(enough.discounts[0].amount, 560);

    // Three items, of the tee: the totes do not count.
    assert_eq!(
        refusal(&checkout(
            &shop,
            &[(TEE, 2), (TOTE, 3)],
            &entering(&["THREE"], None),
            None
        )),
        "its minimum quantity is 3 items, and the cart has 2 of those it applies to"
    );
    let three = checkout(&shop, &[(TEE, 3)], &entering(&["THREE"], None), None);
    assert_eq!(three.discounts[0].amount, 960);
}

#[test]
fn a_code_for_listed_customers_or_once_per_customer_follows_the_buyer() {
    let mut shop = demo();
    let customer = shop
        .keys("Customer")
        .next()
        .expect("the demo store has customers")
        .to_string();
    let email = shop.get(&customer).unwrap()["email"]
        .as_str()
        .unwrap()
        .to_string();
    let id = crate::admin::state::parse_gid(&customer).unwrap().1;
    let all = json!({"value": {"percentage": 0.1}, "items": {"all": true}});
    basic(
        &mut shop,
        json!({"title": "VIP", "code": "VIP", "context": {"customers": {"add": [customer]}}, "customerGets": all}),
    );
    basic(
        &mut shop,
        json!({"title": "Welcome", "code": "WELCOME", "appliesOncePerCustomer": true, "customerGets": all}),
    );
    let cart = [(TEE, 1)];

    // A guest is not the customer; the customer, logged in or by their email, is.
    assert_eq!(
        refusal(&checkout(
            &shop,
            &cart,
            &entering(&["VIP"], Some("guest@example.com")),
            None
        )),
        "it is for the customers it lists, and a buyer who is not a customer of the shop is not one of them"
    );
    assert_eq!(
        checkout(&shop, &cart, &entering(&["VIP"], None), Some(id))
            .discounts
            .len(),
        1
    );
    assert_eq!(
        checkout(
            &shop,
            &cart,
            &entering(&["VIP"], Some(&email.to_uppercase())),
            None
        )
        .discounts
        .len(),
        1
    );

    // Once per customer: the email that placed an order with it cannot use it again; another
    // buyer can.
    let welcome = checkout(
        &shop,
        &cart,
        &entering(&["WELCOME"], Some("new@example.com")),
        None,
    );
    assert_eq!(welcome.discounts.len(), 1);
    place(&mut shop, &welcome, Some("new@example.com"));
    assert_eq!(
        refusal(&checkout(
            &shop,
            &cart,
            &entering(&["WELCOME"], Some("NEW@example.com")),
            None
        )),
        "it applies once per customer, and the buyer has used it"
    );
    assert_eq!(
        checkout(
            &shop,
            &cart,
            &entering(&["WELCOME"], Some("other@example.com")),
            None
        )
        .discounts
        .len(),
        1
    );
}

#[test]
fn a_code_for_segments_applies_to_their_members() {
    let mut shop = demo();
    let created = run(
        &mut shop,
        "mutation { segmentCreate(name: \"Everyone\", query: \"number_of_orders >= 0\") { segment { id } userErrors { message } } }",
        Value::Null,
    );
    let segment = created["data"]["segmentCreate"]["segment"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"))
        .to_string();
    basic(
        &mut shop,
        json!({"title": "Members", "code": "MEMBERS", "context": {"customerSegments": {"add": [segment]}},
               "customerGets": {"value": {"percentage": 0.1}, "items": {"all": true}}}),
    );
    let customer = shop.keys("Customer").next().unwrap().to_string();
    let id = crate::admin::state::parse_gid(&customer).unwrap().1;
    let cart = [(TEE, 1)];
    assert_eq!(
        checkout(&shop, &cart, &entering(&["MEMBERS"], None), Some(id))
            .discounts
            .len(),
        1
    );
    assert_eq!(
        refusal(&checkout(&shop, &cart, &entering(&["MEMBERS"], None), None)),
        "it is for the customers of the segments it lists, and a buyer who is not a customer of the shop is in none of them"
    );
}

#[test]
fn one_code_of_shopifys_types_applies_per_checkout() {
    let mut shop = demo();
    let all = json!({"value": {"percentage": 0.1}, "items": {"all": true}});
    basic(
        &mut shop,
        json!({"title": "A", "code": "AAA", "customerGets": all}),
    );
    free_shipping(&mut shop, json!({"title": "Ship", "code": "SHIP"}));
    let result = checkout(&shop, &[(TEE, 1)], &entering(&["AAA", "SHIP"], None), None);
    assert_eq!(
        statuses(&result),
        [
            ("AAA".to_string(), true, None),
            (
                "SHIP".to_string(),
                false,
                Some(
                    "only one code of a discount of Shopify's own types applies to a checkout, and `AAA` does"
                        .to_string()
                )
            ),
        ]
    );
    assert_eq!(result.discounts.len(), 1);
    assert_eq!(result.cost.shipping, 490);
}

#[test]
fn free_shipping_makes_the_rates_up_to_its_maximum_free_where_it_ships() {
    let mut shop = demo();
    free_shipping(
        &mut shop,
        json!({"title": "Free shipping", "code": "FREESHIP"}),
    );
    free_shipping(
        &mut shop,
        json!({"title": "Cheap free", "code": "CHEAP", "maximumShippingPrice": "5.00"}),
    );
    free_shipping(
        &mut shop,
        json!({"title": "France", "code": "FRANCE", "destination": {"countries": {"add": ["FR"]}}}),
    );
    let cart = [(TEE, 1)];

    let free = checkout(&shop, &cart, &entering(&["FREESHIP"], None), None);
    assert_eq!(free.cost.shipping, 0);
    assert_eq!(free.cost.total, 3200);
    let discount = &free.discounts[0];
    assert_eq!(discount.class, DiscountClass::Shipping);
    assert_eq!(discount.amount, 490);
    assert_eq!(discount.code.as_deref(), Some("FREESHIP"));
    assert_eq!(discount.details.target_type, DiscountTarget::ShippingLine);
    assert_eq!(discount.details.value, DiscountValue::Percentage(100.0));
    assert_eq!(discount.details.allocation_method, AllocationMethod::Each);
    // Every rate is free.
    assert!(
        free.delivery_groups[0]
            .options
            .iter()
            .all(|option| option.cost == 0)
    );

    // Up to 5.00: the standard rate is free, the express one keeps its price.
    let mut express = entering(&["CHEAP"], None);
    express
        .selected_delivery
        .insert("group-1".into(), "express".into());
    let cheap = checkout(&shop, &cart, &express, None);
    assert!(cheap.discount_codes[0].applicable);
    assert_eq!(cheap.cost.shipping, 1290);
    let costs: Vec<i64> = cheap.delivery_groups[0]
        .options
        .iter()
        .map(|option| option.cost)
        .collect();
    assert_eq!(costs, [0, 1290]);

    // To France only: refused for an address in the United States, taken before an address.
    assert_eq!(
        refusal(&checkout(&shop, &cart, &entering(&["FRANCE"], None), None)),
        "it does not apply to the country of the shipping address (US)"
    );
    let before = CheckoutState {
        discount_codes: vec!["FRANCE".into()],
        ..CheckoutState::default()
    };
    assert!(checkout(&shop, &cart, &before, None).discount_codes[0].applicable);
}

#[test]
fn the_order_carries_the_code_its_webhooks_say_it_and_the_code_is_used_once_more() {
    let mut shop = demo();
    basic(
        &mut shop,
        json!({"title": "Spring", "code": "SPRING10", "customerGets": {"value": {"percentage": 0.1}, "items": {"all": true}}}),
    );
    let result = checkout(
        &shop,
        &[(TEE, 1), (MUG, 2)],
        &entering(&["SPRING10"], None),
        None,
    );
    let order = place(&mut shop, &result, Some("buyer@example.com"));

    let answer = run(
        &mut shop,
        r#"query($id: ID!) { order(id: $id) {
            discountCodes
            totalDiscountsSet { shopMoney { amount } }
            discountApplications(first: 5) { nodes {
                __typename allocationMethod targetSelection targetType
                value { ... on PricingPercentageValue { percentage } }
                ... on DiscountCodeApplication { code } } } } }"#,
        json!({"id": order}),
    );
    let data = &answer["data"]["order"];
    assert_eq!(data["discountCodes"], json!(["SPRING10"]), "{answer}");
    assert_eq!(data["totalDiscountsSet"]["shopMoney"]["amount"], "7.6");
    assert_eq!(
        data["discountApplications"]["nodes"],
        json!([{"__typename": "DiscountCodeApplication", "allocationMethod": "ACROSS",
                "targetSelection": "ALL", "targetType": "LINE_ITEM",
                "value": {"percentage": 10.0}, "code": "SPRING10"}]),
        "{answer}"
    );

    for topic in ["orders/create", "orders/paid"] {
        let payload = crate::admin::webhooks::payload::payload(
            topic,
            shop.get(&order).unwrap(),
            &crate::admin::api::samples::ShopLookup(&shop),
        )
        .unwrap();
        assert_eq!(
            payload["discount_codes"],
            json!([{"code": "SPRING10", "amount": "7.60", "type": "percentage"}]),
            "{topic}"
        );
        assert_eq!(payload["discount_applications"][0]["type"], "discount_code");
        assert_eq!(payload["discount_applications"][0]["code"], "SPRING10");
        assert_eq!(
            payload["discount_applications"][0]["value_type"],
            "percentage"
        );
    }

    let used = run(
        &mut shop,
        r#"{ codeDiscountNodeByCode(code: "spring10") { codeDiscount { ... on DiscountCodeBasic {
            asyncUsageCount codes(first: 1) { nodes { code asyncUsageCount } } } } } }"#,
        Value::Null,
    );
    assert_eq!(
        used["data"]["codeDiscountNodeByCode"]["codeDiscount"],
        json!({"asyncUsageCount": 1, "codes": {"nodes": [{"code": "SPRING10", "asyncUsageCount": 1}]}}),
        "{used}"
    );
}

#[test]
fn a_free_shipping_order_says_its_code_is_shipping() {
    let mut shop = demo();
    free_shipping(
        &mut shop,
        json!({"title": "Free shipping", "code": "FREESHIP"}),
    );
    let result = checkout(&shop, &[(TEE, 1)], &entering(&["FREESHIP"], None), None);
    let order = place(&mut shop, &result, Some("buyer@example.com"));
    let payload = crate::admin::webhooks::payload::payload(
        "orders/create",
        shop.get(&order).unwrap(),
        &crate::admin::api::samples::ShopLookup(&shop),
    )
    .unwrap();
    assert_eq!(
        payload["discount_codes"],
        json!([{"code": "FREESHIP", "amount": "4.90", "type": "shipping"}])
    );
    assert_eq!(
        payload["discount_applications"][0]["target_type"],
        "shipping_line"
    );
}

/// The `asyncUsageCount` of the discount that has `code`.
fn uses(shop: &mut ShopState, code: &str) -> Value {
    let used = run(
        shop,
        r#"query($code: String!) { codeDiscountNodeByCode(code: $code) { codeDiscount {
            ... on DiscountCodeBasic { asyncUsageCount }
            ... on DiscountCodeFreeShipping { asyncUsageCount } } } }"#,
        json!({"code": code}),
    );
    used["data"]["codeDiscountNodeByCode"]["codeDiscount"]["asyncUsageCount"].clone()
}

/// The `discountCodes` of an order, and the `discount_codes` of its `orders/create`.
fn order_codes(shop: &ShopState, order: &str) -> (Value, Value) {
    let record = shop.get(order).unwrap();
    let payload = crate::admin::webhooks::payload::payload(
        "orders/create",
        record,
        &crate::admin::api::samples::ShopLookup(shop),
    )
    .unwrap();
    (
        record["discountCodes"].clone(),
        payload["discount_codes"].clone(),
    )
}

#[test]
fn a_limited_code_two_checkouts_priced_before_either_completed_is_redeemed_once() {
    let mut shop = demo();
    basic(
        &mut shop,
        json!({"title": "Once", "code": "ONCE", "usageLimit": 1,
               "customerGets": {"value": {"percentage": 0.1}, "items": {"all": true}}}),
    );
    let cart = [(TEE, 1)];
    // Both buyers see the code applied: neither order is placed yet.
    let first = checkout(
        &shop,
        &cart,
        &entering(&["ONCE"], Some("first@example.com")),
        None,
    );
    let second = checkout(
        &shop,
        &cart,
        &entering(&["ONCE"], Some("second@example.com")),
        None,
    );
    assert_eq!(first.discounts.len(), 1);
    assert_eq!(second.discounts.len(), 1);

    place(&mut shop, &first, Some("first@example.com"));
    let orders = shop.keys("Order").count();
    // The second order, recorded after the first took the only use, is not placed.
    assert_eq!(
        try_place(&mut shop, &second, Some("second@example.com")),
        Err(super::Refused {
            code: "ONCE".into(),
            reason: "it reached its usage limit: used 1 times of 1".into(),
        })
    );
    assert_eq!(shop.keys("Order").count(), orders);
    assert_eq!(uses(&mut shop, "ONCE"), 1);
}

#[test]
fn a_once_per_customer_code_two_checkouts_of_one_buyer_priced_is_redeemed_once() {
    let mut shop = demo();
    basic(
        &mut shop,
        json!({"title": "Welcome", "code": "WELCOME", "appliesOncePerCustomer": true,
               "customerGets": {"value": {"percentage": 0.1}, "items": {"all": true}}}),
    );
    let cart = [(TEE, 1)];
    let buyer = entering(&["WELCOME"], Some("twice@example.com"));
    let first = checkout(&shop, &cart, &buyer, None);
    let second = checkout(&shop, &cart, &buyer, None);
    place(&mut shop, &first, Some("twice@example.com"));
    assert_eq!(
        try_place(&mut shop, &second, Some("TWICE@example.com")).map(|_| ()),
        Err(super::Refused {
            code: "WELCOME".into(),
            reason: "it applies once per customer, and the buyer has used it".into(),
        })
    );
    assert_eq!(uses(&mut shop, "WELCOME"), 1);
}

#[test]
fn a_free_shipping_code_over_its_maximum_rate_is_valid_and_not_used() {
    let mut shop = demo();
    free_shipping(
        &mut shop,
        json!({"title": "Cheap free", "code": "CHEAP", "maximumShippingPrice": "5.00", "usageLimit": 1}),
    );
    let mut express = entering(&["CHEAP"], None);
    express
        .selected_delivery
        .insert("group-1".into(), "express".into());
    let result = checkout(&shop, &[(TEE, 1)], &express, None);
    // Valid, as the page shows it, but nothing comes off the 12.90 rate.
    assert!(result.discount_codes[0].applicable);
    assert_eq!(result.cost.shipping, 1290);
    assert!(result.discounts.is_empty());

    let order = place(&mut shop, &result, Some("buyer@example.com"));
    assert_eq!(order_codes(&shop, &order), (json!([]), json!([])));
    assert_eq!(uses(&mut shop, "CHEAP"), 0);
}

#[test]
fn a_free_shipping_code_on_a_cart_without_shipping_is_valid_and_not_used() {
    let mut shop = demo();
    free_shipping(
        &mut shop,
        json!({"title": "Free shipping", "code": "FREESHIP", "usageLimit": 1}),
    );
    let store = demo_store();
    let gift_card = store
        .products
        .iter()
        .find(|product| product.gift_card)
        .expect("the demo store sells a gift card")
        .variants[0]
        .id;
    let result = checkout(
        &shop,
        &[(gift_card, 1)],
        &entering(&["FREESHIP"], None),
        None,
    );
    assert!(result.delivery_groups.is_empty());
    assert!(result.discount_codes[0].applicable);
    assert!(result.discounts.is_empty());

    let order = place(&mut shop, &result, Some("buyer@example.com"));
    assert_eq!(order_codes(&shop, &order), (json!([]), json!([])));
    assert_eq!(uses(&mut shop, "FREESHIP"), 0);
}

#[test]
fn a_once_per_customer_code_renamed_since_the_buyer_used_it_stays_used() {
    let mut shop = demo();
    let node = basic(
        &mut shop,
        json!({"title": "Welcome", "code": "OLD", "appliesOncePerCustomer": true,
               "customerGets": {"value": {"percentage": 0.1}, "items": {"all": true}}}),
    );
    let cart = [(TEE, 1)];
    let used = checkout(
        &shop,
        &cart,
        &entering(&["OLD"], Some("loyal@example.com")),
        None,
    );
    place(&mut shop, &used, Some("loyal@example.com"));

    let renamed = run(
        &mut shop,
        r#"mutation($id: ID!) { discountCodeBasicUpdate(id: $id, basicCodeDiscount: {code: "NEW"}) {
            userErrors { field message } } }"#,
        json!({"id": node}),
    );
    assert_eq!(
        renamed["data"]["discountCodeBasicUpdate"]["userErrors"],
        json!([]),
        "{renamed}"
    );
    assert_eq!(
        refusal(&checkout(
            &shop,
            &cart,
            &entering(&["NEW"], Some("loyal@example.com")),
            None
        )),
        "it applies once per customer, and the buyer has used it"
    );
}

#[test]
fn a_once_per_customer_code_deleted_since_the_buyer_used_it_leaves_the_discount_used() {
    let mut shop = demo();
    let node = basic(
        &mut shop,
        json!({"title": "Welcome", "code": "FIRST", "appliesOncePerCustomer": true,
               "customerGets": {"value": {"percentage": 0.1}, "items": {"all": true}}}),
    );
    let added = run(
        &mut shop,
        r#"mutation($id: ID!) { discountRedeemCodeBulkAdd(discountId: $id, codes: [{code: "SECOND"}]) {
            userErrors { field message } } }"#,
        json!({"id": node}),
    );
    assert_eq!(
        added["data"]["discountRedeemCodeBulkAdd"]["userErrors"],
        json!([]),
        "{added}"
    );
    let cart = [(TEE, 1)];
    let used = checkout(
        &shop,
        &cart,
        &entering(&["FIRST"], Some("loyal@example.com")),
        None,
    );
    place(&mut shop, &used, Some("loyal@example.com"));

    let deleted = run(
        &mut shop,
        r#"mutation($id: ID!) { discountCodeRedeemCodeBulkDelete(discountId: $id, search: "code:FIRST") {
            userErrors { field message } } }"#,
        json!({"id": node}),
    );
    assert_eq!(
        deleted["data"]["discountCodeRedeemCodeBulkDelete"]["userErrors"],
        json!([]),
        "{deleted}"
    );
    assert_eq!(
        refusal(&checkout(
            &shop,
            &cart,
            &entering(&["SECOND"], Some("loyal@example.com")),
            None
        )),
        "it applies once per customer, and the buyer has used it"
    );
}
