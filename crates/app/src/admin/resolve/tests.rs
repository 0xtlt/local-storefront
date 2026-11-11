//! The rules of the resolver, through queries over the demo store.

use serde_json::{Value, json};

use super::{Registry, ShopResolver};
use crate::admin::auth::tokens::{AccessMode, Grant, GrantSource};
use crate::admin::graphql::{self, Request};
use crate::admin::state::ShopState;

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
        crate::admin::schema::admin(),
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
fn the_registry_names_only_what_a_version_has() {
    assert_eq!(Registry::standard().check_versions(), Vec::<String>::new());
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
            crate::admin::state::parse_gid(node["id"].as_str().unwrap())
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
        r#"{ productByHandle(handle: "wool-beanie") { title category { id } descriptionPlainSummary description events(first: 1) { nodes { id } } } }"#,
    );
    let product = &answer["data"]["productByHandle"];
    assert_eq!(product["category"], Value::Null);
    // A deprecated field reads the one that replaced it ("Use `description` instead."), with
    // what it said of itself: on one line, truncated to 60 characters.
    assert_ne!(product["description"], json!(""));
    assert_eq!(
        product["descriptionPlainSummary"],
        json!(super::plain_summary(
            product["description"].as_str().unwrap()
        ))
    );
    assert!(
        product["descriptionPlainSummary"]
            .as_str()
            .unwrap()
            .chars()
            .count()
            <= 60
    );
    assert_eq!(product["events"]["nodes"], json!([]));
    assert_eq!(
        answer["extensions"]["lsf"],
        json!({
            "defaulted": ["productByHandle.category"],
            "defaultedFields": ["Product.category"],
        })
    );
}

/// Every deprecated field Shopify replaced by another of its object reads the other's value,
/// not a default: the money fields of the line items, drafts, orders and transactions, the
/// counts, the URLs.
#[test]
fn deprecated_fields_read_their_successors() {
    let mut shop = demo();
    let answer = run(
        &mut shop,
        r#"{ orders(first: 1) { nodes { totalPrice totalPriceSet { shopMoney { amount } } subtotalPrice totalTax totalDiscounts
            totalShippingPrice totalReceived totalRefunded netPayment totalCapturable cartDiscountAmount
            lineItems(first: 1) { nodes { originalUnitPrice originalUnitPriceSet { shopMoney { amount } } originalTotal discountedTotal
                discountedUnitPrice totalDiscount unfulfilledDiscountedTotal unfulfilledOriginalTotal } }
            transactions(first: 1) { amount amountV2 { amount } amountSet { shopMoney { amount } } } } }
          products(first: 1) { nodes { totalVariants variantsCount { count } bodyHtml descriptionHtml } } }"#,
    );
    assert!(
        answer["extensions"]["lsf"].get("defaulted").is_none(),
        "{answer}"
    );
    let order = &answer["data"]["orders"]["nodes"][0];
    let money = |amount: &Value| {
        crate::admin::state::value::money(crate::admin::state::value::cents(Some(amount)))
    };
    assert_eq!(
        order["totalPrice"],
        json!(money(&order["totalPriceSet"]["shopMoney"]["amount"]))
    );
    let line = &order["lineItems"]["nodes"][0];
    assert_eq!(
        line["originalUnitPrice"],
        json!(money(&line["originalUnitPriceSet"]["shopMoney"]["amount"]))
    );
    let transaction = &order["transactions"][0];
    assert_eq!(
        transaction["amountV2"]["amount"],
        transaction["amountSet"]["shopMoney"]["amount"]
    );
    let product = &answer["data"]["products"]["nodes"][0];
    assert_eq!(product["totalVariants"], product["variantsCount"]["count"]);
    assert_eq!(product["bodyHtml"], product["descriptionHtml"]);
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

    // A mutation no domain registered (an empty registry: which ones are simulated changes).
    let empty = Registry::new();
    let mut resolver = ShopResolver::new(&mut shop, &empty);
    let unknown = graphql::run(
        crate::admin::schema::admin(),
        &mut resolver,
        &Request::new(
            r#"mutation { productBundleCreate(input: {title: "X", components: []}) { productBundleOperation { id } } }"#,
        ),
        None,
    )
    .to_json();
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

/// The demo's products and twelve more, four of them titled the same, to page over.
fn shop_of_products() -> ShopState {
    let mut shop = demo();
    for index in 0..12 {
        let title = if index % 3 == 0 {
            "Twin".to_string()
        } else {
            format!("Product {index:02}")
        };
        let created = run(
            &mut shop,
            &format!(
                r#"mutation {{ productCreate(product: {{title: "{title}"}}) {{ userErrors {{ message }} }} }}"#
            ),
        );
        assert_eq!(
            created["data"]["productCreate"]["userErrors"],
            json!([]),
            "{created}"
        );
    }
    shop
}

/// A page of products: the ids, and the cursors of its ends.
fn product_page(shop: &mut ShopState, arguments: &str) -> (Vec<String>, String, String) {
    let answer = run(
        shop,
        &format!(
            "{{ products({arguments}) {{ nodes {{ id }} pageInfo {{ startCursor endCursor }} }} }}"
        ),
    );
    let page = &answer["data"]["products"];
    let ids = page["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("a page: {answer}"))
        .iter()
        .map(|node| node["id"].as_str().unwrap().to_string())
        .collect();
    let cursor = |name: &str| page["pageInfo"][name].as_str().unwrap_or("").to_string();
    (ids, cursor("startCursor"), cursor("endCursor"))
}

fn delete_products(shop: &mut ShopState, ids: &[String]) {
    for id in ids {
        let deleted = run(
            shop,
            &format!(
                r#"mutation {{ productDelete(input: {{id: "{id}"}}) {{ deletedProductId userErrors {{ message }} }} }}"#
            ),
        );
        assert_eq!(
            deleted["data"]["productDelete"]["deletedProductId"],
            json!(id),
            "{deleted}"
        );
    }
}

#[test]
fn a_cursor_whose_item_was_deleted_resumes_right_after_it() {
    // Shopify's cursors hold the sort value and the id of their item: a page deleted while
    // paging does not make the next one skip ahead. lsf's fell back to the position, and
    // answered products 11 to 15 where Shopify answers 6 to 10.
    for (sort, reverse) in [
        ("ID", false),
        ("ID", true),
        ("TITLE", false),
        ("TITLE", true),
        ("CREATED_AT", false),
        ("UPDATED_AT", true),
        ("PRODUCT_TYPE", false),
    ] {
        let arguments = format!("sortKey: {sort}, reverse: {reverse}");
        let case = format!("{sort}, reverse: {reverse}");
        let mut shop = shop_of_products();
        let (all, ..) = product_page(&mut shop, &format!("first: 250, {arguments}"));
        assert!(all.len() >= 15, "{case}: {} products", all.len());

        // Forward: the first page is deleted, the next one starts after it.
        let (first, _, end) = product_page(&mut shop, &format!("first: 5, {arguments}"));
        assert_eq!(first, all[..5], "{case}");
        delete_products(&mut shop, &first);
        let (next, ..) = product_page(
            &mut shop,
            &format!("first: 5, after: \"{end}\", {arguments}"),
        );
        assert_eq!(next, all[5..10], "{case}: forward");

        // Backward: the last page is deleted, the one before it ends before it.
        let mut shop = shop_of_products();
        let len = all.len();
        let (last, start, _) = product_page(&mut shop, &format!("last: 5, {arguments}"));
        assert_eq!(last, all[len - 5..], "{case}");
        delete_products(&mut shop, &last);
        let (previous, ..) = product_page(
            &mut shop,
            &format!("last: 5, before: \"{start}\", {arguments}"),
        );
        assert_eq!(previous, all[len - 10..len - 5], "{case}: backward");

        // A cursor whose item is still there resumes after it, as before.
        let mut shop = shop_of_products();
        let (_, _, end) = product_page(&mut shop, &format!("first: 7, {arguments}"));
        let (next, ..) = product_page(
            &mut shop,
            &format!("first: 3, after: \"{end}\", {arguments}"),
        );
        assert_eq!(next, all[7..10], "{case}: kept");
    }
}

#[test]
fn a_deleted_cursor_resumes_within_a_tie_by_id() {
    // Four products titled "Twin": the cursor of the second, deleted, resumes at the third.
    let mut shop = shop_of_products();
    let (twins, ..) = product_page(
        &mut shop,
        "first: 250, sortKey: TITLE, query: \"title:Twin\"",
    );
    assert_eq!(twins.len(), 4);
    for reverse in [false, true] {
        let mut shop = shop_of_products();
        let arguments = format!("sortKey: TITLE, reverse: {reverse}, query: \"title:Twin\"");
        let expected: Vec<String> = if reverse {
            twins.iter().rev().cloned().collect()
        } else {
            twins.clone()
        };
        let (first, _, end) = product_page(&mut shop, &format!("first: 2, {arguments}"));
        assert_eq!(first, expected[..2]);
        delete_products(&mut shop, &first[1..]);
        let (next, ..) = product_page(
            &mut shop,
            &format!("first: 2, after: \"{end}\", {arguments}"),
        );
        assert_eq!(next, expected[2..], "reverse: {reverse}");
        let (_, start, _) = product_page(&mut shop, &format!("last: 1, {arguments}"));
        delete_products(&mut shop, &expected[3..]);
        let (previous, ..) = product_page(
            &mut shop,
            &format!("last: 5, before: \"{start}\", {arguments}"),
        );
        assert_eq!(previous, [expected[0].clone(), expected[2].clone()]);
    }
}

#[test]
fn a_cursor_of_another_sort_key_or_another_connection_falls_back_to_its_place() {
    let mut shop = shop_of_products();
    let (by_id, ..) = product_page(&mut shop, "first: 250");
    let (by_title, ..) = product_page(&mut shop, "first: 250, sortKey: TITLE");
    // A cursor of the `TITLE` order whose item is gone, in the `ID` order: its position.
    let (_, _, end) = product_page(&mut shop, "first: 3, sortKey: TITLE");
    delete_products(&mut shop, &by_title[2..3]);
    let (next, ..) = product_page(&mut shop, &format!("first: 2, after: \"{end}\""));
    let remaining: Vec<&String> = by_id.iter().filter(|id| **id != by_title[2]).collect();
    assert_eq!(next.iter().collect::<Vec<_>>(), remaining[2..4]);
    // A cursor lsf did not make is still Shopify's error.
    let malformed = run(
        &mut shop,
        "{ products(first: 1, after: \"bm90IGEgY3Vyc29y\") { nodes { id } } }",
    );
    assert_eq!(malformed["errors"][0]["message"], json!("Malformed cursor"));
}

/// The title a test adapter gives `productCreate`, and the one it reads `Product.title` as.
fn shouted(arguments: &mut serde_json::Map<String, Value>) {
    if let Some(Value::String(title)) = arguments
        .get_mut("product")
        .and_then(|product| product.get_mut("title"))
    {
        *title = title.to_uppercase();
    }
}

fn whispered(product: &crate::admin::state::Record) -> Option<Value> {
    let title = product.get("title")?.as_str()?;
    Some(json!(title.to_lowercase()))
}

/// What a version that gave `productCreate` its title in another shape, and read `Product.title`
/// differently, would declare.
static ADAPTERS: &[crate::admin::version::Adapter] = &[
    crate::admin::version::Adapter::Arguments {
        type_name: "Mutation",
        field: "productCreate",
        write: shouted,
    },
    crate::admin::version::Adapter::Field {
        type_name: "Product",
        field: "title",
        read: whispered,
    },
];

#[test]
fn a_version_adapts_the_arguments_it_is_given_and_the_fields_it_reads() {
    let mut shop = demo();
    let mut resolver = ShopResolver::new(&mut shop, Registry::standard()).with_adapters(ADAPTERS);
    let answer = graphql::run(
        crate::admin::schema::admin(),
        &mut resolver,
        &Request::new(
            "mutation { productCreate(product: {title: \"Rain Jacket\"}) {
                product { id title handle } userErrors { message } } }",
        ),
        None,
    )
    .to_json();
    let product = &answer["data"]["productCreate"]["product"];
    // The mutation got the adapted title, and the field reads through the adapter.
    assert_eq!(product["handle"], json!("rain-jacket"), "{answer}");
    assert_eq!(product["title"], json!("rain jacket"));
    let gid = product["id"].as_str().unwrap().to_string();
    assert_eq!(shop.get(&gid).unwrap()["title"], json!("RAIN JACKET"));
    // Without the adapters (the default version), the record reads as it is kept.
    let read = run(
        &mut shop,
        &format!("{{ product(id: \"{gid}\") {{ title }} }}"),
    );
    assert_eq!(read["data"]["product"]["title"], json!("RAIN JACKET"));
}

/// A product created by a token without `write_publications` is on no channel, and the answer
/// says how to put it on the storefront; with the scope, it says nothing.
#[test]
fn a_product_created_without_the_publication_scopes_says_it_is_not_on_the_storefront() {
    let mut shop = demo();
    let create = r#"mutation { productCreate(product: {title: "Desk lamp"}) { product { id } userErrors { message } } }"#;
    let without = run_as(&mut shop, create, Some(&grant(&["write_products"])));
    let notes = without["extensions"]["lsf"]["notes"].to_string();
    assert!(
        notes.contains("productCreate: the product is published on no sales channel")
            && notes.contains("write_publications"),
        "{without}"
    );
    let with = run_as(
        &mut shop,
        create,
        Some(&grant(&["write_products", "write_publications"])),
    );
    assert!(with["extensions"]["lsf"].get("notes").is_none(), "{with}");
}

/// A deprecated field whose replacement says something else than it did reads the replacement
/// with the difference: the summary of a description is cut to 60 characters, an order's
/// shipping price is before its discounts, a shipping line's discounted price before its taxes.
#[test]
fn deprecated_fields_keep_what_set_them_apart_from_their_successors() {
    use crate::admin::state::value::money_bag;
    let mut shop = demo();
    let created = run(
        &mut shop,
        r#"mutation { productCreate(product: {title: "Long", descriptionHtml: "<p>A long description,</p>\n<p>on two paragraphs, that goes past sixty characters for sure.</p>"}) { product { description descriptionPlainSummary } } }"#,
    );
    let product = &created["data"]["productCreate"]["product"];
    let summary = product["descriptionPlainSummary"].as_str().unwrap();
    assert_eq!(summary.chars().count(), 60, "{summary}");
    assert!(summary.ends_with("..."), "{summary}");
    assert!(!summary.contains('\n'));
    assert!(
        product["description"]
            .as_str()
            .unwrap()
            .starts_with(&summary[..57])
    );
    assert_eq!(super::plain_summary("  Short\n text "), "Short text");

    // An order whose shipping of 10.00 is discounted to 8.00, 1.33 of it taxes included.
    let order = shop.keys("Order").next().unwrap().to_string();
    let currency = shop.currency();
    let record = shop.get_mut(&order).unwrap();
    record.insert("taxesIncluded".into(), json!(true));
    record.insert("totalShippingPriceSet".into(), money_bag(800, &currency));
    record.insert(
        "shippingLines".into(),
        json!([{
            "title": "Standard",
            "originalPriceSet": money_bag(1000, &currency),
            "discountedPriceSet": money_bag(800, &currency),
            "taxLines": [{"title": "VAT", "rate": 0.2, "priceSet": money_bag(133, &currency)}],
        }]),
    );
    let answer = run(
        &mut shop,
        &format!(
            r#"{{ order(id: "{order}") {{ totalShippingPrice totalShippingPriceSet {{ shopMoney {{ amount }} }}
                shippingLines(first: 1) {{ nodes {{ discountedPrice {{ amount currencyCode }} discountedPriceSet {{ shopMoney {{ amount }} }} }} }} }} }}"#
        ),
    );
    let order_answer = &answer["data"]["order"];
    assert_eq!(
        order_answer["totalShippingPrice"],
        json!("10.00"),
        "{answer}"
    );
    assert_eq!(
        order_answer["totalShippingPriceSet"]["shopMoney"]["amount"],
        json!("8.0")
    );
    let line = &order_answer["shippingLines"]["nodes"][0];
    assert_eq!(line["discountedPrice"]["amount"], json!("6.67"), "{answer}");
    assert_eq!(line["discountedPrice"]["currencyCode"], json!(currency));
    // Taxes not included: the discounted price is pre-tax already.
    shop.get_mut(&order)
        .unwrap()
        .insert("taxesIncluded".into(), json!(false));
    let answer = run(
        &mut shop,
        &format!(
            r#"{{ order(id: "{order}") {{ shippingLines(first: 1) {{ nodes {{ discountedPrice {{ amount }} }} }} }} }}"#
        ),
    );
    assert_eq!(
        answer["data"]["order"]["shippingLines"]["nodes"][0]["discountedPrice"]["amount"],
        json!("8.0")
    );
}
