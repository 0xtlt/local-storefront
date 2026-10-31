//! The rules of the resolver, through queries over the demo store.

use serde_json::{Value, json};

use super::{Registry, ShopResolver};
use crate::auth::tokens::{AccessMode, Grant, GrantSource};
use crate::graphql::{self, Request};
use crate::state::ShopState;

fn demo() -> ShopState {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    ShopState::seeded(&store, None)
}

/// The answer to a query, `extensions` without `cost`.
fn run(shop: &mut ShopState, query: &str) -> Value {
    run_as(shop, query, None)
}

fn run_as(shop: &mut ShopState, query: &str, grant: Option<&Grant>) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    if let Some(grant) = grant {
        resolver = resolver.with_grant(grant);
    }
    let mut answer = graphql::run(
        crate::schema::admin(),
        &mut resolver,
        &Request::new(query),
        None,
    );
    answer.extensions.extend(resolver.extensions());
    answer.extensions.remove("cost");
    answer.to_json()
}

fn grant(scopes: &[&str]) -> Grant {
    Grant {
        shop: "local-supply-co.myshopify.com".into(),
        client_id: "test".into(),
        scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
        mode: AccessMode::Offline,
        user: None,
        issued_at: chrono::Utc::now(),
        expires_at: None,
        source: GrantSource::Install,
    }
}

fn titles(page: &Value) -> Vec<String> {
    page["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("a page: {page}"))
        .iter()
        .map(|node| node["title"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_registry_names_only_what_the_schema_has() {
    assert_eq!(
        Registry::standard().check(crate::schema::admin()),
        Vec::<String>::new()
    );
}

#[test]
fn root_connections_sort_by_id_and_page_with_cursors() {
    let mut shop = demo();
    let all = run(&mut shop, "{ products(first: 250) { nodes { id title } } }");
    let ids: Vec<u64> = all["data"]["products"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| {
            crate::state::parse_gid(node["id"].as_str().unwrap())
                .unwrap()
                .1
        })
        .collect();
    assert!(ids.len() > 4);
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "{ids:?}");

    let first = run(
        &mut shop,
        "{ products(first: 2) { edges { cursor node { title } } pageInfo { hasNextPage hasPreviousPage startCursor endCursor } } }",
    );
    let page = &first["data"]["products"];
    assert_eq!(page["pageInfo"]["hasNextPage"], json!(true));
    assert_eq!(page["pageInfo"]["hasPreviousPage"], json!(false));
    assert_eq!(page["pageInfo"]["endCursor"], page["edges"][1]["cursor"]);
    let after = page["pageInfo"]["endCursor"].as_str().unwrap();
    let next = run(
        &mut shop,
        &format!(
            "{{ products(first: 2, after: \"{after}\") {{ nodes {{ title }} pageInfo {{ hasPreviousPage }} }} }}"
        ),
    );
    let all_titles = titles(&all["data"]["products"]);
    assert_eq!(titles(&next["data"]["products"]), all_titles[2..4]);
    assert_eq!(
        next["data"]["products"]["pageInfo"]["hasPreviousPage"],
        json!(true)
    );
    // `last` with `before`, and `reverse`.
    let before = page["edges"][1]["cursor"].as_str().unwrap();
    let back = run(
        &mut shop,
        &format!("{{ products(last: 5, before: \"{before}\") {{ nodes {{ title }} }} }}"),
    );
    assert_eq!(titles(&back["data"]["products"]), all_titles[..1]);
    let reversed = run(
        &mut shop,
        "{ products(first: 1, reverse: true) { nodes { title } } }",
    );
    assert_eq!(
        titles(&reversed["data"]["products"]),
        [all_titles.last().unwrap().clone()]
    );
}

#[test]
fn connections_answer_shopifys_errors() {
    let mut shop = demo();
    let none = run(&mut shop, "{ products { nodes { title } } }");
    assert_eq!(none["data"], Value::Null);
    assert_eq!(
        none["errors"][0]["message"],
        json!("you must provide one of first or last")
    );
    assert_eq!(none["errors"][0]["path"], json!(["products"]));
    let over = run(&mut shop, "{ products(first: 251) { nodes { title } } }");
    let message = over["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.starts_with("first cannot exceed 250. To query larger amounts"),
        "{message}"
    );
    let malformed = run(
        &mut shop,
        "{ products(first: 1, after: \"nope\") { nodes { title } } }",
    );
    assert_eq!(malformed["errors"][0]["message"], json!("Malformed cursor"));
    assert_eq!(malformed["errors"][0]["extensions"], json!({}));
    // A nested connection too, even an empty one.
    let nested = run(
        &mut shop,
        "{ products(first: 1) { nodes { variants { nodes { id } } } } }",
    );
    assert_eq!(
        nested["errors"][0]["path"],
        json!(["products", "nodes", 0, "variants"])
    );
}

#[test]
fn search_sort_and_counts() {
    let mut shop = demo();
    let sale = run(
        &mut shop,
        "{ products(first: 10, query: \"tag:sale\", sortKey: TITLE) { nodes { title } } productsCount(query: \"tag:sale\") { count precision } }",
    );
    assert_eq!(
        titles(&sale["data"]["products"]),
        ["Linen Overshirt", "Scented Candle"]
    );
    assert_eq!(
        sale["data"]["productsCount"],
        json!({"count": 2, "precision": "EXACT"})
    );
    assert!(sale.get("extensions").is_none(), "{sale}");

    let not_sale = run(
        &mut shop,
        "{ products(first: 50, query: \"tag_not:sale AND product_type:Apparel\") { nodes { title } } }",
    );
    assert_eq!(
        titles(&not_sale["data"]["products"]),
        ["Organic Cotton T-Shirt"]
    );
    let words = run(
        &mut shop,
        "{ products(first: 50, query: \"title:wool\") { nodes { title } } }",
    );
    assert_eq!(titles(&words["data"]["products"]), ["Wool Beanie"]);
    let free = run(
        &mut shop,
        "{ products(first: 50, query: \"beanie\") { nodes { title } } }",
    );
    assert_eq!(titles(&free["data"]["products"]), ["Wool Beanie"]);

    // An unknown field is ignored, with Shopify's warning.
    let unknown = run(
        &mut shop,
        "{ products(first: 1, query: \"colour:blue\") { nodes { title } } }",
    );
    assert_eq!(
        unknown["data"]["products"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        unknown["extensions"]["search"],
        json!([{
            "path": ["products"],
            "query": "colour:blue",
            "parsed": {"field": "colour", "match_all": "blue"},
            "warnings": [{"field": "colour", "message": "Invalid search field for this query."}],
        }])
    );
    let limited = run(&mut shop, "{ productsCount(limit: 2) { count precision } }");
    assert_eq!(
        limited["data"]["productsCount"],
        json!({"count": 2, "precision": "AT_LEAST"})
    );
    let by_title = run(
        &mut shop,
        "{ products(first: 2, sortKey: TITLE, reverse: true) { nodes { title } } }",
    );
    assert_eq!(
        titles(&by_title["data"]["products"]),
        ["Wool Beanie", "Scented Candle"]
    );
}

#[test]
fn records_by_id_handle_identifier_and_node() {
    let mut shop = demo();
    let tote = "gid://shopify/Product/1169463630974";
    let answer = run(
        &mut shop,
        &format!(
            r#"{{
              product(id: "{tote}") {{ title legacyResourceId variantsCount {{ count precision }} }}
              missing: product(id: "gid://shopify/Product/1") {{ title }}
              wrongType: product(id: "gid://shopify/Collection/1") {{ title }}
              productByHandle(handle: "wool-beanie") {{ title }}
              productByIdentifier(identifier: {{handle: "canvas-tote-bag"}}) {{ title }}
              node(id: "{tote}") {{ __typename ... on Product {{ handle }} }}
              nodes(ids: ["{tote}", "gid://shopify/Product/2"]) {{ id }}
              shop {{ name }}
            }}"#
        ),
    );
    let data = &answer["data"];
    assert_eq!(
        data["product"],
        json!({"title": "Canvas Tote Bag", "legacyResourceId": "1169463630974", "variantsCount": {"count": 1, "precision": "EXACT"}})
    );
    assert_eq!(data["missing"], Value::Null);
    assert_eq!(data["wrongType"], Value::Null);
    assert_eq!(data["productByHandle"]["title"], json!("Wool Beanie"));
    assert_eq!(
        data["productByIdentifier"]["title"],
        json!("Canvas Tote Bag")
    );
    assert_eq!(
        data["node"],
        json!({"__typename": "Product", "handle": "canvas-tote-bag"})
    );
    assert_eq!(data["nodes"], json!([{"id": tote}, null]));
    assert_eq!(data["shop"]["name"], json!("Local Supply Co."));
}

#[test]
fn metafields_and_computed_fields() {
    let mut shop = demo();
    let answer = run(
        &mut shop,
        r#"{ productByHandle(handle: "organic-cotton-t-shirt") {
            care: metafield(namespace: "custom", key: "care") { value type }
            none: metafield(key: "care") { value }
            reviews: metafields(first: 10, namespace: "reviews") { nodes { key } }
            keyed: metafields(first: 10, keys: ["custom.care", "reviews.rating_count"]) { nodes { key } }
            defaultCursor
        } }"#,
    );
    let product = &answer["data"]["productByHandle"];
    assert_eq!(
        product["care"]["value"],
        json!("Machine wash cold. Tumble dry low.")
    );
    assert_eq!(product["none"], Value::Null);
    assert_eq!(
        product["reviews"]["nodes"],
        json!([{"key": "rating"}, {"key": "rating_count"}])
    );
    assert_eq!(
        product["keyed"]["nodes"],
        json!([{"key": "care"}, {"key": "rating_count"}])
    );
    // The default cursor pages from the record on.
    let cursor = product["defaultCursor"].as_str().unwrap();
    let next = run(
        &mut shop,
        &format!("{{ products(first: 1, after: \"{cursor}\") {{ nodes {{ id }} }} }}"),
    );
    let ids = run(
        &mut shop,
        "{ products(first: 250) { nodes { id handle } } }",
    );
    let nodes = ids["data"]["products"]["nodes"].as_array().unwrap();
    let at = nodes
        .iter()
        .position(|node| node["handle"] == json!("organic-cotton-t-shirt"))
        .unwrap();
    assert_eq!(
        next["data"]["products"]["nodes"][0]["id"],
        nodes[at + 1]["id"]
    );
}

#[test]
fn defaults_are_reported() {
    let mut shop = demo();
    let answer = run(
        &mut shop,
        r#"{ productByHandle(handle: "wool-beanie") { title category { id } descriptionPlainSummary events(first: 1) { nodes { id } } } }"#,
    );
    let product = &answer["data"]["productByHandle"];
    assert_eq!(product["category"], Value::Null);
    assert_eq!(product["descriptionPlainSummary"], json!(""));
    assert_eq!(product["events"]["nodes"], json!([]));
    assert_eq!(
        answer["extensions"]["lsf"],
        json!({
            "defaulted": ["productByHandle.category", "productByHandle.descriptionPlainSummary"],
            "defaultedFields": ["Product.category", "Product.descriptionPlainSummary"],
        })
    );
}

#[test]
fn scopes_are_enforced_field_by_field() {
    let mut shop = demo();
    let orders_only = grant(&["read_orders"]);
    let denied = run_as(
        &mut shop,
        "{ products(first: 1) { nodes { id } } }",
        Some(&orders_only),
    );
    assert_eq!(denied["data"], Value::Null);
    assert_eq!(
        denied["errors"][0]["message"],
        json!("Access denied for products field. Required access: `read_products` access scope.")
    );
    assert_eq!(
        denied["errors"][0]["extensions"]["code"],
        json!("ACCESS_DENIED")
    );
    // A nested field answering a record the token cannot read.
    let nested = run_as(
        &mut shop,
        "{ orders(first: 1) { nodes { name customer { id } } } }",
        Some(&orders_only),
    );
    assert_eq!(
        nested["errors"][0]["path"],
        json!(["orders", "nodes", 0, "customer"])
    );
    assert!(
        nested["errors"][0]["message"]
            .as_str()
            .unwrap()
            .starts_with("Access denied for customer field.")
    );
    assert!(nested["data"]["orders"]["nodes"][0]["name"].is_string());
    // `node` reads what the record's type requires.
    let tote = run_as(
        &mut shop,
        r#"{ node(id: "gid://shopify/Product/1169463630974") { id } }"#,
        Some(&orders_only),
    );
    assert_eq!(
        tote["errors"][0]["extensions"]["code"],
        json!("ACCESS_DENIED")
    );
    // `write_products` covers `read_products`.
    let writer = grant(&["write_products"]);
    let allowed = run_as(
        &mut shop,
        "{ products(first: 1) { nodes { id } } }",
        Some(&writer),
    );
    assert!(allowed.get("errors").is_none(), "{allowed}");
}

#[test]
fn product_create_update_delete() {
    let mut shop = demo();
    let created = run(
        &mut shop,
        r#"mutation { productCreate(product: {title: "Rain Jacket", tags: ["outer, rain", "Rain"], productOptions: [{name: "Size", values: [{name: "S"}, {name: "M"}]}]}) {
            product { id handle status vendor tags publishedAt totalInventory tracksInventory
              variants(first: 5) { nodes { title price selectedOptions { name value } inventoryItem { tracked inventoryLevels(first: 1) { nodes { location { id } } } } } }
              options { name values } }
            userErrors { field message } shop { name }
        } }"#,
    );
    let payload = &created["data"]["productCreate"];
    assert_eq!(payload["userErrors"], json!([]), "{created}");
    assert_eq!(payload["shop"]["name"], json!("Local Supply Co."));
    let product = &payload["product"];
    assert_eq!(product["handle"], json!("rain-jacket"));
    assert_eq!(product["status"], json!("ACTIVE"));
    assert_eq!(product["vendor"], json!("Local Supply Co."));
    assert_eq!(product["tags"], json!(["outer", "rain"]));
    assert_eq!(product["publishedAt"], Value::Null);
    assert_eq!(
        product["options"],
        json!([{"name": "Size", "values": ["S", "M"]}])
    );
    let variant = &product["variants"]["nodes"][0];
    assert_eq!(variant["title"], json!("S"));
    assert_eq!(variant["price"], json!("0.00"));
    assert_eq!(variant["inventoryItem"]["tracked"], json!(false));
    assert!(variant["inventoryItem"]["inventoryLevels"]["nodes"][0]["location"]["id"].is_string());
    let id = product["id"].as_str().unwrap().to_string();
    let topics: Vec<String> = shop
        .take_events()
        .into_iter()
        .map(|event| event.topic)
        .collect();
    assert_eq!(
        topics,
        [
            "products/create",
            "inventory_items/create",
            "inventory_levels/connect"
        ]
    );

    // The same title again: a free handle.
    let again = run(
        &mut shop,
        r#"mutation { productCreate(product: {title: "Rain Jacket"}) { product { handle variants(first: 1) { nodes { title } } } } }"#,
    );
    assert_eq!(
        again["data"]["productCreate"]["product"]["handle"],
        json!("rain-jacket-1")
    );
    assert_eq!(
        again["data"]["productCreate"]["product"]["variants"]["nodes"][0]["title"],
        json!("Default Title")
    );
    shop.take_events();

    // A failure changes nothing.
    let before = shop.count("Product");
    let blank = run(
        &mut shop,
        r#"mutation { productCreate(product: {title: " "}) { product { id } userErrors { field message } } }"#,
    );
    assert_eq!(
        blank["data"]["productCreate"],
        json!({"product": null, "userErrors": [{"field": ["title"], "message": "Title can't be blank"}]})
    );
    assert_eq!(shop.count("Product"), before);
    assert!(shop.events().is_empty());

    let updated = run(
        &mut shop,
        &format!(
            r#"mutation {{ productUpdate(product: {{id: "{id}", title: "Storm Jacket", status: DRAFT}}) {{ product {{ title status handle variants(first: 1) {{ nodes {{ displayName }} }} }} userErrors {{ message }} }} }}"#
        ),
    );
    assert_eq!(
        updated["data"]["productUpdate"]["product"],
        json!({"title": "Storm Jacket", "status": "DRAFT", "handle": "rain-jacket", "variants": {"nodes": [{"displayName": "Storm Jacket - S"}]}})
    );
    assert_eq!(shop.take_events()[0].topic, "products/update");
    let unknown = run(
        &mut shop,
        r#"mutation { productUpdate(product: {id: "gid://shopify/Product/1", title: "X"}) { product { id } userErrors { field message } } }"#,
    );
    assert_eq!(
        unknown["data"]["productUpdate"]["userErrors"],
        json!([{"field": ["id"], "message": "Product does not exist"}])
    );

    let deleted = run(
        &mut shop,
        &format!(
            r#"mutation {{ productDelete(input: {{id: "{id}"}}) {{ deletedProductId userErrors {{ message }} }} }}"#
        ),
    );
    assert_eq!(
        deleted["data"]["productDelete"]["deletedProductId"],
        json!(id)
    );
    assert!(!shop.contains(&id));
    let events = shop.take_events();
    assert_eq!(events.last().unwrap().topic, "products/delete");
    assert_eq!(
        events.last().unwrap().snapshot.as_ref().unwrap()["title"],
        json!("Storm Jacket")
    );
    assert!(
        events
            .iter()
            .any(|event| event.topic == "inventory_items/delete")
    );
}

#[test]
fn tags_and_mutations_lsf_does_not_simulate() {
    let mut shop = demo();
    let tote = "gid://shopify/Product/1169463630974";
    let added = run(
        &mut shop,
        &format!(
            r#"mutation {{ tagsAdd(id: "{tote}", tags: ["Gift", "bags"]) {{ node {{ ... on Product {{ tags }} }} userErrors {{ message }} }} }}"#
        ),
    );
    assert_eq!(
        added["data"]["tagsAdd"]["node"]["tags"],
        json!(["bags", "cotton", "Gift"])
    );
    assert_eq!(shop.take_events()[0].topic, "products/update");
    let removed = run(
        &mut shop,
        &format!(
            r#"mutation {{ tagsRemove(id: "{tote}", tags: "gift, cotton") {{ node {{ ... on Product {{ tags }} }} }} }}"#
        ),
    );
    assert_eq!(
        removed["data"]["tagsRemove"]["node"]["tags"],
        json!(["bags"])
    );
    // Tagging a product needs `write_products`.
    let reader = grant(&["read_products"]);
    let denied = run_as(
        &mut shop,
        &format!(r#"mutation {{ tagsAdd(id: "{tote}", tags: ["x"]) {{ node {{ id }} }} }}"#),
        Some(&reader),
    );
    assert_eq!(
        denied["errors"][0]["extensions"]["code"],
        json!("ACCESS_DENIED")
    );

    let unknown = run(
        &mut shop,
        r#"mutation { productBundleCreate(input: {title: "X", components: []}) { productBundleOperation { id } } }"#,
    );
    assert_eq!(unknown["data"]["productBundleCreate"], Value::Null);
    assert_eq!(
        unknown["errors"][0]["extensions"]["code"],
        json!("LSF_NOT_SIMULATED")
    );
}

#[test]
fn inventory_quantities_are_those_asked() {
    let mut shop = demo();
    let answer = run(
        &mut shop,
        r#"{ productByHandle(handle: "wool-beanie") { variants(first: 1) { nodes { inventoryItem { inventoryLevels(first: 1) { nodes {
            quantities(names: ["on_hand", "available", "nope"]) { name quantity }
        } } } } } } }"#,
    );
    let quantities = &answer["data"]["productByHandle"]["variants"]["nodes"][0]["inventoryItem"]["inventoryLevels"]
        ["nodes"][0]["quantities"];
    let names: Vec<&str> = quantities
        .as_array()
        .unwrap()
        .iter()
        .map(|quantity| quantity["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["on_hand", "available"]);
    assert_eq!(quantities[0]["quantity"], quantities[1]["quantity"]);
}
