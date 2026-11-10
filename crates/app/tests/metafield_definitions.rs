//! Every family's writer of metafields (a product's input, a customer's, an app discount's)
//! checks them against the definition of their owner type, namespace and key as `metafieldsSet`
//! does, links them to it, and never creates a definition: only `metafieldDefinitionCreate` does.

mod support;

use lsf_app::admin::graphql::{self, Request};
use lsf_app::admin::resolve::{Registry, ShopResolver};
use lsf_app::admin::state::ShopState;
use serde_json::{Value, json};

fn rich() -> ShopState {
    let store = support::fixture_store("rich");
    ShopState::seeded(&store, Some(&support::fixture_app()))
}

fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let mut request = Request::new(query);
    if let Value::Object(variables) = variables {
        request.variables = variables;
    }
    let answer = graphql::run(
        lsf_app::admin::schema::admin(),
        &mut resolver,
        &request,
        None,
    )
    .to_json();
    assert!(answer.get("errors").is_none(), "{answer}");
    answer
}

/// The `userErrors` of the payload of `name`.
fn errors(answer: &Value, name: &str) -> Value {
    answer["data"][name]["userErrors"].clone()
}

/// The definitions of an owner type, as `namespace.key`.
fn definitions(shop: &mut ShopState, owner_type: &str) -> Vec<String> {
    let answer = run(
        shop,
        "query($type: MetafieldOwnerType!) { metafieldDefinitions(ownerType: $type, first: 250) { nodes { namespace key } } }",
        json!({ "type": owner_type }),
    );
    answer["data"]["metafieldDefinitions"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| {
            format!(
                "{}.{}",
                node["namespace"].as_str().unwrap(),
                node["key"].as_str().unwrap()
            )
        })
        .collect()
}

/// `custom.rating10`, a `number_integer` of 10 at most, for an owner type.
fn define_rating(shop: &mut ShopState, owner_type: &str) {
    let answer = run(
        shop,
        r#"mutation($type: MetafieldOwnerType!) { metafieldDefinitionCreate(definition: {name: "Rating", namespace: "custom", key: "rating10", ownerType: $type, type: "number_integer", validations: [{name: "max", value: "10"}]}) { userErrors { message } } }"#,
        json!({ "type": owner_type }),
    );
    assert_eq!(errors(&answer, "metafieldDefinitionCreate"), json!([]));
}

const TOO_HIGH: &str = "Value 99 must be less than or equal to 10.";

#[test]
fn every_writer_checks_metafields_against_their_definition_and_creates_none() {
    let mut shop = rich();
    for owner_type in ["PRODUCT", "CUSTOMER", "DISCOUNT"] {
        define_rating(&mut shop, owner_type);
    }
    let before: Vec<(&str, Vec<String>)> = ["PRODUCT", "CUSTOMER", "DISCOUNT"]
        .into_iter()
        .map(|owner_type| (owner_type, definitions(&mut shop, owner_type)))
        .collect();
    shop.take_events();
    let product = shop
        .keys("Product")
        .find(|key| shop.get(key).unwrap()["handle"] == json!("linen-dress"))
        .unwrap()
        .to_string();
    let customer = shop
        .keys("Customer")
        .find(|key| shop.get(key).unwrap()["email"] == json!("zoe@example.com"))
        .unwrap()
        .to_string();

    // `metafieldsSet` refuses a value over the definition's maximum…
    let set = run(
        &mut shop,
        r#"mutation($owner: ID!) { metafieldsSet(metafields: [{ownerId: $owner, namespace: "custom", key: "rating10", value: "99"}]) { userErrors { field message code } } }"#,
        json!({ "owner": product }),
    );
    assert_eq!(
        errors(&set, "metafieldsSet"),
        json!([{"field": ["metafields", "0", "value"], "message": TOO_HIGH, "code": "LESS_THAN_OR_EQUAL_TO"}])
    );

    // …and so do the product's, the customer's and the app discount's inputs, each with its
    // payload's errors at the path of its input.
    let product_update = run(
        &mut shop,
        r#"mutation($id: ID!) { productUpdate(product: {id: $id, metafields: [{namespace: "custom", key: "rating10", value: "99"}, {namespace: "custom", key: "brandnew", type: "number_integer", value: "abc"}]}) { userErrors { field message } } }"#,
        json!({ "id": product }),
    );
    assert_eq!(
        errors(&product_update, "productUpdate"),
        json!([
            {"field": ["metafields", "0", "value"], "message": TOO_HIGH},
            {"field": ["metafields", "1", "value"], "message": "Value must be an integer."},
        ])
    );
    let customer_update = run(
        &mut shop,
        r#"mutation($id: ID!) { customerUpdate(input: {id: $id, metafields: [{namespace: "custom", key: "rating10", value: "99"}]}) { userErrors { field message } } }"#,
        json!({ "id": customer }),
    );
    assert_eq!(
        errors(&customer_update, "customerUpdate"),
        json!([{"field": ["metafields", "0", "value"], "message": TOO_HIGH}])
    );
    const DISCOUNT: &str = r#"mutation($value: String!) { discountAutomaticAppCreate(automaticAppDiscount: {title: "Rated", functionHandle: "volume-discount", startsAt: "2026-01-01T00:00:00Z", metafields: [{namespace: "custom", key: "rating10", value: $value}]}) { automaticAppDiscount { discountId } userErrors { field message code } } }"#;
    let discount = run(&mut shop, DISCOUNT, json!({ "value": "99" }));
    assert_eq!(
        errors(&discount, "discountAutomaticAppCreate"),
        json!([{"field": ["automaticAppDiscount", "metafields", "0", "value"], "message": TOO_HIGH, "code": "INVALID"}])
    );
    assert!(
        shop.take_events().is_empty(),
        "the failures changed nothing"
    );

    // Values that fit are set, take the definition's type, and are linked to it; a metafield
    // without a definition gets none.
    let product_update = run(
        &mut shop,
        r#"mutation($id: ID!) { productUpdate(product: {id: $id, metafields: [{namespace: "custom", key: "rating10", value: "7"}, {namespace: "custom", key: "brandnew", type: "number_integer", value: "3"}]}) {
            product { rating: metafield(namespace: "custom", key: "rating10") { type definition { key } } brandnew: metafield(namespace: "custom", key: "brandnew") { value definition { key } } } userErrors { message } } }"#,
        json!({ "id": product }),
    );
    assert_eq!(
        product_update["data"]["productUpdate"],
        json!({"product": {
            "rating": {"type": "number_integer", "definition": {"key": "rating10"}},
            "brandnew": {"value": "3", "definition": null},
        }, "userErrors": []})
    );
    let customer_update = run(
        &mut shop,
        r#"mutation($id: ID!) { customerUpdate(input: {id: $id, metafields: [{namespace: "custom", key: "rating10", value: "8"}, {namespace: "custom", key: "cust_new", type: "single_line_text_field", value: "x"}]}) {
            customer { metafield(namespace: "custom", key: "rating10") { definition { key } } } userErrors { message } } }"#,
        json!({ "id": customer }),
    );
    assert_eq!(
        customer_update["data"]["customerUpdate"],
        json!({"customer": {"metafield": {"definition": {"key": "rating10"}}}, "userErrors": []})
    );
    let discount = run(&mut shop, DISCOUNT, json!({ "value": "9" }));
    assert_eq!(errors(&discount, "discountAutomaticAppCreate"), json!([]));

    // Each owner type has its one `rating10`, and nothing more; no definition event was emitted.
    for (owner_type, definitions_before) in before {
        let now = definitions(&mut shop, owner_type);
        assert_eq!(now, definitions_before, "{owner_type}");
        assert_eq!(
            now.iter().filter(|name| *name == "custom.rating10").count(),
            1,
            "{owner_type}: {now:?}"
        );
    }
    assert!(
        shop.take_events()
            .iter()
            .all(|event| !event.topic.starts_with("metafield_definitions/"))
    );
}
