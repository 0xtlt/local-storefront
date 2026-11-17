use lsf_core::Store;
use lsf_core::store::Metafield;
use lsf_core::store::load::{DataSource, LoadOptions, load};
use serde_json::{Value, json};

use super::query;

/// The demo store, with a metafield on the slate mug.
fn store() -> Store {
    let (mut store, _) = load(&DataSource::Demo, &LoadOptions::default());
    let mug = store.product_index("ceramic-mug").unwrap();
    store.products[mug].variants[1].metafields.insert(
        "custom".to_string(),
        [(
            "lead_time".to_string(),
            Metafield {
                kind: "number_integer".to_string(),
                value: json!(3),
            },
        )]
        .into(),
    );
    store
}

fn variant_gid(store: &Store, sku: &str) -> String {
    let variant = store
        .products
        .iter()
        .flat_map(|product| &product.variants)
        .find(|variant| variant.sku == sku)
        .unwrap();
    format!("gid://shopify/ProductVariant/{}", variant.id)
}

fn error_of(answer: &Value) -> &str {
    answer["errors"][0]["message"].as_str().unwrap_or_default()
}

#[test]
fn nodes_answer_the_variants_of_the_lines_as_issue_324_asks() {
    let store = store();
    let ids = [
        variant_gid(&store, "MUG-SLT"),
        variant_gid(&store, "TOTE-NAT"),
    ];
    let answer = query(
        &store,
        "query ($ids: [ID!]!) { nodes(ids: $ids) { ... on ProductVariant { id sku availableForSale } } }",
        Some(&json!({ "ids": ids })),
    );
    assert_eq!(
        answer,
        json!({"data": {"nodes": [
            {"id": ids[0], "sku": "MUG-SLT", "availableForSale": true},
            {"id": ids[1], "sku": "TOTE-NAT", "availableForSale": true},
        ]}})
    );
}

#[test]
fn a_variant_reads_its_product_its_price_its_options_and_its_metafields() {
    let store = store();
    let id = variant_gid(&store, "MUG-SLT");
    let answer = query(
        &store,
        r#"query ($id: ID!) {
          node(id: $id) {
            __typename
            ... on ProductVariant {
              title
              price { amount currencyCode }
              compareAtPrice { amount }
              selectedOptions { name value }
              weightUnit
              lead: metafield(namespace: "custom", key: "lead_time") { type value }
              metafields(identifiers: [{namespace: "custom", key: "lead_time"}, {namespace: "custom", key: "none"}]) { key value }
              product { handle title productType tags care: metafield(namespace: "custom", key: "care") { value } }
            }
          }
        }"#,
        Some(&json!({ "id": id })),
    );
    let currency = store.currency().to_string();
    assert_eq!(
        answer,
        json!({"data": {"node": {
            "__typename": "ProductVariant",
            "title": "Slate",
            "price": {"amount": "22.0", "currencyCode": currency},
            "compareAtPrice": null,
            "selectedOptions": [{"name": "Glaze", "value": "Slate"}],
            "weightUnit": "KILOGRAMS",
            "lead": {"type": "number_integer", "value": "3"},
            "metafields": [{"key": "lead_time", "value": "3"}, null],
            "product": {
                "handle": "ceramic-mug",
                "title": "Ceramic Mug",
                "productType": "Home",
                "tags": ["bestseller", "ceramics", "kitchen"],
                "care": null,
            },
        }}})
    );
}

#[test]
fn a_product_by_handle_or_id_and_its_metafields() {
    let store = store();
    let tee = store.product_by_handle("organic-cotton-t-shirt").unwrap();
    let by_handle = query(
        &store,
        r#"{ product(handle: "organic-cotton-t-shirt") { id availableForSale metafield(namespace: "custom", key: "care") { value } } }"#,
        None,
    );
    assert_eq!(
        by_handle,
        json!({"data": {"product": {
            "id": format!("gid://shopify/Product/{}", tee.id),
            "availableForSale": true,
            "metafield": {"value": "Machine wash cold. Tumble dry low."},
        }}})
    );
    let by_id = query(
        &store,
        &format!(
            r#"{{ product(id: "gid://shopify/Product/{}") {{ handle }} node(id: "gid://shopify/Product/{}") {{ ... on Product {{ title }} }} }}"#,
            tee.id, tee.id
        ),
        None,
    );
    assert_eq!(
        by_id,
        json!({"data": {"product": {"handle": "organic-cotton-t-shirt"}, "node": {"title": "Organic Cotton T-Shirt"}}})
    );
    let missing = query(
        &store,
        r#"{ product(handle: "nothing") { id } node(id: "gid://shopify/ProductVariant/1") { id } }"#,
        None,
    );
    assert_eq!(missing, json!({"data": {"product": null, "node": null}}));
}

#[test]
fn a_field_lsf_does_not_answer_is_an_error_that_names_it() {
    let store = store();
    let id = variant_gid(&store, "MUG-SLT");
    let answer = query(
        &store,
        "query ($id: ID!) { node(id: $id) { ... on ProductVariant { id image { url } } } }",
        Some(&json!({ "id": id })),
    );
    assert!(answer.get("data").is_none(), "{answer}");
    assert!(
        error_of(&answer).contains("unknown field `image` on type `ProductVariant`"),
        "{answer}"
    );
    // The inventory is refused as Shopify refuses it: `api_access` has no inventory scope.
    let answer = query(
        &store,
        "query ($id: ID!) { node(id: $id) { ... on ProductVariant { quantityAvailable } } }",
        Some(&json!({ "id": id })),
    );
    assert!(
        error_of(&answer).contains("Access denied for quantityAvailable field"),
        "{answer}"
    );
    // Another type of object than a product or a variant.
    let answer = query(
        &store,
        r#"{ node(id: "gid://shopify/Collection/1") { id } }"#,
        None,
    );
    assert!(error_of(&answer).contains("is a Collection"), "{answer}");
    let answer = query(&store, "mutation { cartCreate { cart { id } } }", None);
    assert!(error_of(&answer).contains("queries only"), "{answer}");
    let answer = query(&store, "{ nodes(ids: [\"x\"]) { id } }", None);
    assert!(
        error_of(&answer).contains("invalid global id `x`"),
        "{answer}"
    );
}
