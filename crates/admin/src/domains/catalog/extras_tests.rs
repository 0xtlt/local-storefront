//! The catalog's extras over the demo store: the deprecated publish mutations, collection
//! duplicates and sources, bundles, combined listings and product feeds. Each mutation has a
//! success test and a refused input that changes nothing.

use serde_json::json;

use super::mutation_tests::{demo, ok, product, run, run_with, topics, unchanged};

#[test]
fn product_publish_publishes_on_the_online_store_as_publishable_publish_does() {
    let mut shop = demo();
    let online_store = super::online_store_publication_gid();
    let created = run(
        &mut shop,
        r#"mutation { productCreate(product: {title: "Sun hat"}) { product { id } } }"#,
    );
    let hat = created["data"]["productCreate"]["product"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    shop.take_events();
    let published = run_with(
        &mut shop,
        r#"mutation($id: ID!, $publication: ID!) { productPublish(input: {id: $id, productPublications: [{publicationId: $publication}]}) {
            product { publishedAt onlineStoreUrl publishedOnPublication(publicationId: $publication) }
            productPublications { isPublished publishDate channel { name handle } product { id } }
            shop { name } userErrors { field message } } }"#,
        json!({"id": hat, "publication": online_store}),
    );
    let payload = ok(&published, "productPublish");
    assert_eq!(payload["product"]["publishedAt"], json!(shop.timestamp()));
    assert_eq!(payload["product"]["publishedOnPublication"], json!(true));
    assert_eq!(
        payload["product"]["onlineStoreUrl"],
        json!("https://local-supply-co.myshopify.com/products/sun-hat")
    );
    assert_eq!(
        payload["productPublications"],
        json!([{"isPublished": true, "publishDate": shop.timestamp(),
            "channel": {"name": "Online Store", "handle": "online_store"}, "product": {"id": hat}}])
    );
    assert_eq!(
        topics(&mut shop),
        ["product_publications/create", "products/update"]
    );

    // The deprecated `channelHandle` names the Online Store too; unpublishing clears the date.
    let unpublished = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productUnpublish(input: {id: $id, productPublications: [{channelHandle: "online_store"}]}) {
            product { publishedAt onlineStoreUrl } userErrors { field message } } }"#,
        json!({"id": hat}),
    );
    assert_eq!(
        ok(&unpublished, "productUnpublish")["product"],
        json!({"publishedAt": null, "onlineStoreUrl": null})
    );
    assert_eq!(
        topics(&mut shop),
        ["product_publications/delete", "products/update"]
    );
}

#[test]
fn product_publish_refuses_an_unknown_product_or_publication_and_changes_nothing() {
    let mut shop = demo();
    let mug = product(&shop, "ceramic-mug");
    let before = shop.clone();
    let refused = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productUnpublish(input: {id: $id, productPublications: [
            {publicationId: "gid://shopify/Publication/1"}, {channelHandle: "nowhere"}, {}]}) {
            product { id } userErrors { field message } } }"#,
        json!({"id": mug}),
    );
    assert_eq!(
        refused["data"]["productUnpublish"],
        json!({"product": null, "userErrors": [
            {"field": ["input", "productPublications", "0", "publicationId"], "message": "Publication does not exist or is not publishable"},
            {"field": ["input", "productPublications", "1", "channelHandle"], "message": "Channel does not exist or is not publishable"},
            {"field": ["input", "productPublications", "2", "publicationId"], "message": "PublicationId cannot be empty"},
        ]})
    );
    let unknown = run(
        &mut shop,
        r#"mutation { productPublish(input: {id: "gid://shopify/Product/1", productPublications: []}) {
            product { id } productPublications { isPublished } userErrors { field message } } }"#,
    );
    assert_eq!(
        unknown["data"]["productPublish"],
        json!({"product": null, "productPublications": null,
            "userErrors": [{"field": ["input", "id"], "message": "Product does not exist"}]})
    );
    unchanged(
        &mut shop,
        &before,
        &[&mug, &super::online_store_publication_gid()],
    );
}

#[test]
fn collection_publish_and_unpublish_change_the_collections_publications() {
    let mut shop = demo();
    let online_store = super::online_store_publication_gid();
    let created = run(
        &mut shop,
        r#"mutation { collectionCreate(input: {title: "New in"}) { collection { id } } }"#,
    );
    let new_in = created["data"]["collectionCreate"]["collection"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    shop.take_events();
    let published = run_with(
        &mut shop,
        r#"mutation($id: ID!, $publication: ID!) { collectionPublish(input: {id: $id, collectionPublications: [{publicationId: $publication}]}) {
            collection { publishedOnPublication(publicationId: $publication) }
            collectionPublications { isPublished publication { name } collection { id } channel { name } }
            userErrors { field message } } }"#,
        json!({"id": new_in, "publication": online_store}),
    );
    let payload = ok(&published, "collectionPublish");
    assert_eq!(payload["collection"]["publishedOnPublication"], json!(true));
    assert_eq!(
        payload["collectionPublications"],
        json!([{"isPublished": true, "publication": {"name": "Online Store"},
            "collection": {"id": new_in}, "channel": {"name": "Online Store"}}])
    );
    assert_eq!(
        topics(&mut shop),
        ["collection_publications/create", "collections/update"]
    );
    let unpublished = run_with(
        &mut shop,
        r#"mutation($id: ID!, $publication: ID!) { collectionUnpublish(input: {id: $id, collectionPublications: [{publicationId: $publication}]}) {
            collection { publishedOnPublication(publicationId: $publication) } userErrors { field message } } }"#,
        json!({"id": new_in, "publication": online_store}),
    );
    assert_eq!(
        ok(&unpublished, "collectionUnpublish")["collection"]["publishedOnPublication"],
        json!(false)
    );
    assert_eq!(
        topics(&mut shop),
        ["collection_publications/delete", "collections/update"]
    );

    // An unknown collection: refused, nothing changed.
    let before = shop.clone();
    let refused = run_with(
        &mut shop,
        r#"mutation($publication: ID!) { collectionPublish(input: {id: "gid://shopify/Collection/1", collectionPublications: [{publicationId: $publication}]}) {
            collection { id } userErrors { field message } } }"#,
        json!({"publication": online_store}),
    );
    assert_eq!(
        refused["data"]["collectionPublish"]["userErrors"],
        json!([{"field": ["input", "id"], "message": "Collection does not exist"}])
    );
    unchanged(&mut shop, &before, &[&new_in, &online_store]);
}

/// The handles of a collection's products, in order.
fn handles(shop: &crate::state::ShopState, collection: &str) -> Vec<String> {
    super::gids(shop.get(collection).unwrap().get("products"))
        .iter()
        .map(|product| {
            shop.get(product).unwrap()["handle"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

/// The GID of the collection of a handle.
fn collection(shop: &crate::state::ShopState, handle: &str) -> String {
    shop.keys("Collection")
        .find(|key| shop.get(key).unwrap()["handle"] == json!(handle))
        .unwrap_or_else(|| panic!("no collection {handle}"))
        .to_string()
}

#[test]
fn a_shareable_source_is_linked_into_collections_which_follow_it() {
    let mut shop = demo();
    let mug = product(&shop, "ceramic-mug");
    let created = run_with(
        &mut shop,
        r#"mutation($input: CollectionCreateConditionsSourceInput!) { collectionConditionsSourceCreate(input: $input) {
            source { id title description shareable targetType app { id }
                inclusion { matchType conditions { __typename id ... on CollectionSourceInclusionConditionProductTag { relation values matchType } }
                    selections(first: 10) { nodes { product { handle } } } }
                products(first: 10) { nodes { handle } } }
            userErrors { field message } } }"#,
        json!({"input": {
            "title": "Wool source", "description": "Warm things.",
            "inclusion": {"matchType": "ALL",
                "conditions": [{"productTag": {"relation": "TAGGED_WITH", "values": ["wool"], "matchType": "ANY"}}],
                "selections": [{"productId": mug}]},
        }}),
    );
    let source = &ok(&created, "collectionConditionsSourceCreate")["source"];
    let id = source["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("gid://shopify/CollectionConditionsSource/"));
    assert_eq!(source["shareable"], json!(true));
    assert_eq!(source["targetType"], json!("PRODUCTS"));
    let condition = &source["inclusion"]["conditions"][0];
    assert_eq!(
        condition["__typename"],
        json!("CollectionSourceInclusionConditionProductTag")
    );
    assert!(
        condition["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/CollectionSourceInclusionConditionProductTag/")
    );
    assert_eq!(
        source["inclusion"]["selections"]["nodes"],
        json!([{"product": {"handle": "ceramic-mug"}}])
    );
    let mut members: Vec<&str> = source["products"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["handle"].as_str().unwrap())
        .collect();
    members.sort_unstable();
    assert_eq!(members, ["ceramic-mug", "merino-crew-socks", "wool-beanie"]);
    shop.take_events();

    // Linked into a collection: its products are the source's.
    let linked = run_with(
        &mut shop,
        r#"mutation($id: ID!) { collectionCreate(collection: {title: "Winter", sources: [{shareableSource: {sourceId: $id}}]}) {
            collection { id sources { __typename id } productsCount { count } } userErrors { field message } } }"#,
        json!({"id": id}),
    );
    let winter = &ok(&linked, "collectionCreate")["collection"];
    assert_eq!(
        winter["sources"],
        json!([{"__typename": "CollectionConditionsSource", "id": id}])
    );
    assert_eq!(winter["productsCount"]["count"], json!(3));
    let winter = winter["id"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["collections/create"]);

    // The source changes: the collection follows.
    let updated = run_with(
        &mut shop,
        r#"mutation($input: CollectionUpdateConditionsSourceInput!) { collectionConditionsSourceUpdate(input: $input) {
            source { title } userErrors { field message } } }"#,
        json!({"input": {"id": id, "title": "Warm", "inclusion": {"selectionsToRemove": [{"productId": mug}]}}}),
    );
    assert_eq!(
        ok(&updated, "collectionConditionsSourceUpdate")["source"]["title"],
        json!("Warm")
    );
    let mut now = handles(&shop, &winter);
    now.sort_unstable();
    assert_eq!(now, ["merino-crew-socks", "wool-beanie"]);
    assert_eq!(
        topics(&mut shop),
        ["collection_sources/update", "collections/update"]
    );
    // A product that gets the tag joins.
    let tote = product(&shop, "canvas-tote-bag");
    run_with(
        &mut shop,
        r#"mutation($id: ID!) { tagsAdd(id: $id, tags: ["wool"]) { userErrors { message } } }"#,
        json!({"id": tote}),
    );
    assert!(handles(&shop, &winter).contains(&"canvas-tote-bag".to_string()));
    shop.take_events();

    // `collectionConditionsSources(appId:)` and `…ByApp` list it.
    let app = shop.keys("App").next().map(str::to_string);
    if let Some(app) = app {
        let listed = run_with(
            &mut shop,
            r#"query($app: ID!) { collectionConditionsSources(appId: $app, first: 5) { nodes { id } }
                collectionConditionsSourcesByApp(first: 5) { nodes { app { id } sources(first: 5) { nodes { title } } } } }"#,
            json!({"app": app}),
        );
        assert_eq!(
            listed["data"]["collectionConditionsSources"]["nodes"],
            json!([{"id": id}])
        );
        assert_eq!(
            listed["data"]["collectionConditionsSourcesByApp"]["nodes"][0]["sources"]["nodes"],
            json!([{"title": "Warm"}])
        );
    }

    // Deleted: unlinked from the collection, which has no member left.
    let deleted = run_with(
        &mut shop,
        r#"mutation($id: ID!) { collectionConditionsSourceDelete(id: $id) { deletedId userErrors { field message } } }"#,
        json!({"id": id}),
    );
    assert_eq!(
        ok(&deleted, "collectionConditionsSourceDelete")["deletedId"],
        json!(id)
    );
    assert!(!shop.contains(&id));
    assert!(handles(&shop, &winter).is_empty());
    assert_eq!(
        topics(&mut shop),
        ["collection_sources/delete", "collections/update"]
    );
}

#[test]
fn a_source_with_wrong_input_is_refused_and_nothing_changes() {
    let mut shop = demo();
    let before = shop.clone();
    let refused = run(
        &mut shop,
        r#"mutation { collectionConditionsSourceCreate(input: {title: " ", inclusion: {
            conditions: [{productTag: {relation: TAGGED_WITH, values: [], matchType: ANY}}],
            selections: [{productId: "gid://shopify/Product/1"}]}}) { source { id } userErrors { field message } } }"#,
    );
    assert_eq!(
        refused["data"]["collectionConditionsSourceCreate"],
        json!({"source": null, "userErrors": [
            {"field": ["input", "title"], "message": "Title can't be blank"},
            {"field": ["input", "inclusion", "conditions", "0", "productTag", "values"], "message": "Values can't be blank"},
            {"field": ["input", "inclusion", "selections", "0", "productId"], "message": "Product does not exist"},
        ]})
    );
    for mutation in [
        r#"mutation { collectionConditionsSourceUpdate(input: {id: "gid://shopify/CollectionConditionsSource/1", title: "x"}) { source { id } userErrors { field message } } }"#,
        r#"mutation { collectionConditionsSourceDelete(id: "gid://shopify/CollectionConditionsSource/1") { deletedId userErrors { field message } } }"#,
    ] {
        let answer = run(&mut shop, mutation);
        let payload = answer["data"].as_object().unwrap().values().next().unwrap();
        assert_eq!(
            payload["userErrors"][0]["message"],
            json!("Source does not exist"),
            "{answer}"
        );
    }
    unchanged(&mut shop, &before, &[]);
    assert_eq!(shop.count("CollectionConditionsSource"), 0);
}

#[test]
fn a_collection_is_the_union_of_its_sources() {
    let mut shop = demo();
    let frontpage = collection(&shop, "frontpage");
    let tee = product(&shop, "organic-cotton-t-shirt");
    let created = run_with(
        &mut shop,
        r#"mutation($frontpage: ID!, $tee: ID!) { collectionCreate(collection: {title: "Edit", sources: [
            {source: {title: "Sale, not tops", inclusion: {conditions: [{productTag: {relation: TAGGED_WITH, values: ["sale"], matchType: ANY}}]},
                exclusion: {conditions: [{productTag: {relation: TAGGED_WITH, values: ["tops"], matchType: ANY}}]}}},
            {subCollections: {title: "Front", collectionIds: [$frontpage]}},
            {source: {title: "Cheap", inclusion: {conditions: [{variantPrice: {relation: LESS_THAN, value: {amount: "20.00", currencyCode: USD}}}],
                selections: [{productId: $tee}]}}}
        ]}) { collection { id sources { __typename title } } userErrors { field message } } }"#,
        json!({"frontpage": frontpage, "tee": tee}),
    );
    let edit = &ok(&created, "collectionCreate")["collection"];
    assert_eq!(
        edit["sources"],
        json!([{"__typename": "CollectionConditionsSource", "title": "Sale, not tops"},
            {"__typename": "CollectionSubCollectionsSource", "title": "Front"},
            {"__typename": "CollectionConditionsSource", "title": "Cheap"}])
    );
    let edit = edit["id"].as_str().unwrap().to_string();
    let mut members = handles(&shop, &edit);
    members.sort_unstable();
    // The candle (sale), the frontpage's four, the socks (16.00).
    let mut frontpage_products = handles(&shop, &frontpage);
    frontpage_products.extend([
        "scented-candle".to_string(),
        "merino-crew-socks".to_string(),
    ]);
    frontpage_products.sort_unstable();
    frontpage_products.dedup();
    assert_eq!(members, frontpage_products);
    assert!(!members.contains(&"wool-beanie".to_string()));

    // A sub-collection of a collection that has one is refused (depth one), as itself.
    shop.take_events();
    let before = shop.clone();
    let chained = run_with(
        &mut shop,
        r#"mutation($edit: ID!) { collectionCreate(collection: {title: "Chain", sources: [{subCollections: {title: "x", collectionIds: [$edit]}}]}) {
            collection { id } userErrors { field message } } }"#,
        json!({"edit": edit}),
    );
    assert_eq!(
        chained["data"]["collectionCreate"]["userErrors"],
        json!([{"field": ["sources", "0", "subCollections", "collectionIds", "0"],
            "message": "The referenced collection itself owns a sub-collection source (chains of depth > 1 are not supported)."}])
    );
    unchanged(&mut shop, &before, &[&edit]);
    let eligibility = run_with(
        &mut shop,
        r#"query($edit: ID!, $frontpage: ID!) { edit: collection(id: $edit) { subCollectionEligibility { inclusion { eligible ineligibleReason } } }
            front: collection(id: $frontpage) { subCollectionEligibility { inclusion { eligible } } activeOperations { duplicate { collectionRole } } } }"#,
        json!({"edit": edit, "frontpage": frontpage}),
    );
    assert_eq!(
        eligibility["data"],
        json!({"edit": {"subCollectionEligibility": {"inclusion": {"eligible": false, "ineligibleReason": "CHAIN_REFERENCE"}}},
            "front": {"subCollectionEligibility": {"inclusion": {"eligible": true}}, "activeOperations": {"duplicate": []}}})
    );
}

#[test]
fn a_legacy_collection_reads_as_one_source_and_keeps_it_when_it_gets_more() {
    let mut shop = demo();
    let frontpage = collection(&shop, "frontpage");
    let read = run_with(
        &mut shop,
        r#"query($id: ID!) { collection(id: $id) { sources { __typename title ... on CollectionConditionsSource { shareable
            inclusion { matchType selections(first: 10) { nodes { product { handle } } } } } } } }"#,
        json!({"id": frontpage}),
    );
    let source = &read["data"]["collection"]["sources"][0];
    assert_eq!(source["title"], json!("Home page"));
    assert_eq!(source["shareable"], json!(false));
    assert_eq!(
        source["inclusion"]["selections"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let before = handles(&shop, &frontpage);
    shop.take_events();
    let updated = run_with(
        &mut shop,
        r#"mutation($id: ID!) { collectionUpdate(collection: {id: $id, sourcesToCreate: [
            {source: {title: "Wool", inclusion: {conditions: [{productTag: {relation: TAGGED_WITH, values: ["wool"], matchType: ANY}}]}}}]}) {
            collection { sources { title } } job { done } userErrors { field message } } }"#,
        json!({"id": frontpage}),
    );
    let payload = ok(&updated, "collectionUpdate");
    assert_eq!(
        payload["collection"]["sources"],
        json!([{"title": "Home page"}, {"title": "Wool"}])
    );
    assert_eq!(payload["job"]["done"], json!(true));
    let after = handles(&shop, &frontpage);
    assert!(before.iter().all(|handle| after.contains(handle)));
    assert!(after.contains(&"wool-beanie".to_string()));
    assert_eq!(topics(&mut shop), ["collections/update"]);
}

#[test]
fn collection_duplicate_copies_a_collection_and_its_publications() {
    let mut shop = demo();
    let frontpage = collection(&shop, "frontpage");
    let online_store = super::online_store_publication_gid();
    let duplicated = run_with(
        &mut shop,
        r#"mutation($id: ID!, $publication: ID!) { collectionDuplicate(input: {collectionId: $id, newTitle: "Home page copy"}) {
            collection { id title handle descriptionHtml publishedOnPublication(publicationId: $publication) productsCount { count } }
            job { id } userErrors { field message code } } }"#,
        json!({"id": frontpage, "publication": online_store}),
    );
    let payload = ok(&duplicated, "collectionDuplicate");
    let copy = &payload["collection"];
    assert_eq!(copy["title"], json!("Home page copy"));
    assert_eq!(copy["handle"], json!("home-page-copy"));
    assert_eq!(
        copy["descriptionHtml"],
        json!("<p>A few favourites to get you started.</p>")
    );
    assert_eq!(copy["publishedOnPublication"], json!(true));
    assert_eq!(copy["productsCount"]["count"], json!(4));
    assert_eq!(payload["job"], json!(null));
    let copy = copy["id"].as_str().unwrap().to_string();
    assert_eq!(handles(&shop, &copy), handles(&shop, &frontpage));
    assert_eq!(
        topics(&mut shop),
        [
            "collections/create",
            "collection_publications/create",
            "collections/update"
        ]
    );
    // Without its publications.
    let unpublished = run_with(
        &mut shop,
        r#"mutation($id: ID!, $publication: ID!) { collectionDuplicate(input: {collectionId: $id, newTitle: "Second copy", copyPublications: false}) {
            collection { publishedOnPublication(publicationId: $publication) } userErrors { field message } } }"#,
        json!({"id": frontpage, "publication": online_store}),
    );
    assert_eq!(
        ok(&unpublished, "collectionDuplicate")["collection"]["publishedOnPublication"],
        json!(false)
    );
    shop.take_events();

    // The codes' descriptions: a title taken, an unknown collection; nothing changes.
    let before = shop.clone();
    let taken = run_with(
        &mut shop,
        r#"mutation($id: ID!) { collectionDuplicate(input: {collectionId: $id, newTitle: "Home page copy"}) {
            collection { id } userErrors { field message code } } }"#,
        json!({"id": frontpage}),
    );
    assert_eq!(
        taken["data"]["collectionDuplicate"],
        json!({"collection": null, "userErrors": [{"field": ["input", "newTitle"],
            "message": "The URL handle generated from the new title is already in use. Please choose a different title.", "code": "TAKEN"}]})
    );
    let missing = run(
        &mut shop,
        r#"mutation { collectionDuplicate(input: {collectionId: "gid://shopify/Collection/1", newTitle: "x"}) { userErrors { field code } } }"#,
    );
    assert_eq!(
        missing["data"]["collectionDuplicate"]["userErrors"],
        json!([{"field": ["input", "collectionId"], "code": "COLLECTION_NOT_FOUND"}])
    );
    unchanged(&mut shop, &before, &[&frontpage, &online_store]);
    assert_eq!(shop.count("Collection"), before.count("Collection"));
}

/// The variants of a product of the demo store, by handle.
fn variant_ids(shop: &crate::state::ShopState, handle: &str) -> Vec<String> {
    super::mutation_tests::variants(shop, &product(shop, handle))
}

/// A product made with `productCreate`: its GID and its one variant's.
fn new_product(shop: &mut crate::state::ShopState, title: &str) -> (String, String) {
    let answer = run_with(
        shop,
        r#"mutation($title: String!) { productCreate(product: {title: $title}) { product { id variants(first: 1) { nodes { id } } } } }"#,
        json!({"title": title}),
    );
    let product = &answer["data"]["productCreate"]["product"];
    (
        product["id"].as_str().unwrap().to_string(),
        product["variants"]["nodes"][0]["id"]
            .as_str()
            .unwrap()
            .to_string(),
    )
}

#[test]
fn a_variant_fixed_bundle_takes_its_price_and_inventory_from_its_components() {
    let mut shop = demo();
    let (gift, parent) = new_product(&mut shop, "Gift set");
    let tote = variant_ids(&shop, "canvas-tote-bag")[0].clone();
    let socks = variant_ids(&shop, "merino-crew-socks")[0].clone();
    shop.take_events();
    let created = run_with(
        &mut shop,
        r#"mutation($input: [ProductVariantRelationshipUpdateInput!]!) { productVariantRelationshipBulkUpdate(input: $input) {
            parentProductVariants { id price requiresComponents inventoryQuantity availableForSale
                productVariantComponents(first: 5) { nodes { id quantity productVariant { id } } } }
            userErrors { field message code } } }"#,
        json!({"input": [{"parentProductId": gift,
            "productVariantRelationshipsToCreate": [{"id": tote, "quantity": 1}, {"id": socks, "quantity": 2}],
            "priceInput": {"calculation": "COMPONENTS_SUM"}}]}),
    );
    let parent_variant =
        &ok(&created, "productVariantRelationshipBulkUpdate")["parentProductVariants"][0];
    assert_eq!(parent_variant["id"], json!(parent));
    // 24.00 + 2 × 16.00.
    assert_eq!(parent_variant["price"], json!("56.00"));
    assert_eq!(parent_variant["requiresComponents"], json!(true));
    // min(40 totes, 60 / 2 socks).
    assert_eq!(parent_variant["inventoryQuantity"], json!(30));
    assert_eq!(parent_variant["availableForSale"], json!(true));
    let components = parent_variant["productVariantComponents"]["nodes"]
        .as_array()
        .unwrap();
    assert_eq!(components.len(), 2);
    assert!(
        components[0]["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/ProductVariantComponent/")
    );
    assert_eq!(components[1]["quantity"], json!(2));
    assert_eq!(topics(&mut shop), ["products/update"]);

    // The components' stock moves the bundle's.
    let level = shop
        .get(shop.get(&socks).unwrap()["inventoryItem"].as_str().unwrap())
        .unwrap()["inventoryLevels"][0]
        .as_str()
        .unwrap()
        .to_string();
    let location = shop.get(&level).unwrap()["location"]
        .as_str()
        .unwrap()
        .to_string();
    let item = shop.get(&socks).unwrap()["inventoryItem"]
        .as_str()
        .unwrap()
        .to_string();
    let adjusted = run_with(
        &mut shop,
        r#"mutation($item: ID!, $location: ID!) { inventorySetQuantities(input: {name: "available", reason: "correction",
            quantities: [{inventoryItemId: $item, locationId: $location, quantity: 5, changeFromQuantity: null}]}) @idempotent(key: "bundle-test") { userErrors { message } } }"#,
        json!({"item": item, "location": location}),
    );
    assert!(adjusted.get("errors").is_none(), "{adjusted}");
    let total: i64 = super::mutation_tests::variants(&shop, &product(&shop, "merino-crew-socks"))
        .iter()
        .take(1)
        .map(|variant| {
            shop.get(variant).unwrap()["inventoryQuantity"]
                .as_i64()
                .unwrap()
        })
        .sum();
    let read = run_with(
        &mut shop,
        r#"query($id: ID!, $socks: ID!) { productVariant(id: $id) { inventoryQuantity availableForSale product { totalInventory hasVariantsThatRequiresComponents productComponents(first: 5) { nodes { product { handle } componentVariantsCount { count } nonComponentVariantsCount { count } } } productComponentsCount { count } } }
            socks: productVariant(id: $socks) { productParents(first: 5) { nodes { title } } } }"#,
        json!({"id": parent, "socks": socks}),
    );
    assert!(read.get("errors").is_none(), "{read}");
    let variant = &read["data"]["productVariant"];
    assert_eq!(variant["inventoryQuantity"], json!(total / 2));
    assert_eq!(variant["product"]["totalInventory"], json!(total / 2));
    assert_eq!(
        variant["product"]["hasVariantsThatRequiresComponents"],
        json!(true)
    );
    assert_eq!(
        variant["product"]["productComponents"]["nodes"],
        json!([{"product": {"handle": "canvas-tote-bag"}, "componentVariantsCount": {"count": 1}, "nonComponentVariantsCount": {"count": 0}},
            {"product": {"handle": "merino-crew-socks"}, "componentVariantsCount": {"count": 1}, "nonComponentVariantsCount": {"count": 1}}])
    );
    assert_eq!(
        variant["product"]["productComponentsCount"]["count"],
        json!(2)
    );
    assert_eq!(
        read["data"]["socks"]["productParents"]["nodes"],
        json!([{"title": "Gift set"}])
    );
    shop.take_events();

    // A quantity updated, a component removed, a fixed price; then all removed.
    let updated = run_with(
        &mut shop,
        r#"mutation($parent: ID!, $tote: ID!, $socks: ID!) { productVariantRelationshipBulkUpdate(input: [{parentProductVariantId: $parent,
            productVariantRelationshipsToUpdate: [{id: $socks, quantity: 1}], productVariantRelationshipsToRemove: [$tote],
            priceInput: {calculation: FIXED, price: "19.99"}}]) {
            parentProductVariants { price productVariantComponents(first: 5) { nodes { quantity productVariant { id } } } } userErrors { field message code } } }"#,
        json!({"parent": parent, "tote": tote, "socks": socks}),
    );
    let parent_variant =
        &ok(&updated, "productVariantRelationshipBulkUpdate")["parentProductVariants"][0];
    assert_eq!(parent_variant["price"], json!("19.99"));
    assert_eq!(
        parent_variant["productVariantComponents"]["nodes"],
        json!([{"quantity": 1, "productVariant": {"id": socks}}])
    );
    let removed = run_with(
        &mut shop,
        r#"mutation($parent: ID!) { productVariantRelationshipBulkUpdate(input: [{parentProductVariantId: $parent, removeAllProductVariantRelationships: true}]) {
            parentProductVariants { requiresComponents productVariantComponents(first: 5) { nodes { id } } } userErrors { field message code } } }"#,
        json!({"parent": parent}),
    );
    assert_eq!(
        ok(&removed, "productVariantRelationshipBulkUpdate")["parentProductVariants"][0],
        json!({"requiresComponents": false, "productVariantComponents": {"nodes": []}})
    );
    assert_eq!(shop.count("ProductVariantComponent"), 0);
}

#[test]
fn a_variant_relationship_that_is_wrong_is_refused_and_nothing_changes() {
    let mut shop = demo();
    let (_, parent) = new_product(&mut shop, "Gift set");
    let tote = variant_ids(&shop, "canvas-tote-bag")[0].clone();
    shop.take_events();
    let before = shop.clone();
    let refused = run_with(
        &mut shop,
        r#"mutation($parent: ID!, $tote: ID!) { productVariantRelationshipBulkUpdate(input: [
            {parentProductVariantId: $parent, productVariantRelationshipsToCreate: [{id: $parent, quantity: 1}, {id: $tote, quantity: 0}],
                productVariantRelationshipsToRemove: [$tote]},
            {productVariantRelationshipsToCreate: [{id: $tote, quantity: 1}]},
            {parentProductVariantId: $parent, priceInput: {calculation: FIXED}, productVariantRelationshipsToCreate: [{id: $tote, quantity: 1}]}]) {
            parentProductVariants { id } userErrors { field code message } } }"#,
        json!({"parent": parent, "tote": tote}),
    );
    let payload = &refused["data"]["productVariantRelationshipBulkUpdate"];
    assert_eq!(payload["parentProductVariants"], json!(null));
    let codes: Vec<&str> = payload["userErrors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        [
            "PRODUCT_VARIANTS_NOT_COMPONENTS",
            "CIRCULAR_REFERENCE",
            "INVALID_QUANTITY",
            "PARENT_REQUIRED",
            "UPDATE_PARENT_VARIANT_PRICE_REQUIRED"
        ]
    );
    assert_eq!(
        payload["userErrors"][1],
        json!({"field": ["input", "0", "productVariantRelationshipsToCreate", "0"], "code": "CIRCULAR_REFERENCE",
            "message": "A parent product variant cannot contain itself as a component."})
    );
    unchanged(&mut shop, &before, &[&parent, &tote]);
    assert_eq!(shop.count("ProductVariantComponent"), 0);
}

#[test]
fn a_product_fixed_bundle_has_a_variant_per_combination_of_its_components() {
    let mut shop = demo();
    let mug = product(&shop, "ceramic-mug");
    let socks = product(&shop, "merino-crew-socks");
    let glaze = super::gids(shop.get(&mug).unwrap().get("options"))[0].clone();
    let created = run_with(
        &mut shop,
        r#"mutation($input: ProductBundleCreateInput!) { productBundleCreate(input: $input) {
            productBundleOperation { id status product { id } } userErrors { field message } } }"#,
        json!({"input": {"title": "Morning kit", "components": [
            {"productId": mug, "quantity": 1, "optionSelections": [{"componentOptionId": glaze, "name": "Mug glaze", "values": ["Oat", "Slate"]}]},
            {"productId": socks, "optionSelections": [], "quantityOption": {"name": "Pack", "values": [{"name": "One pair", "quantity": 1}, {"name": "Three pairs", "quantity": 3}]}},
        ]}}),
    );
    let operation = &ok(&created, "productBundleCreate")["productBundleOperation"];
    assert_eq!(operation["status"], json!("COMPLETE"));
    let bundle = operation["product"]["id"].as_str().unwrap().to_string();
    let polled = run_with(
        &mut shop,
        r#"query($id: ID!, $bundle: ID!) { productOperation(id: $id) { status product { id } ... on ProductBundleOperation { userErrors { message } } }
            product(id: $bundle) { title options { name values } hasVariantsThatRequiresComponents
                variants(first: 10) { nodes { title price requiresComponents inventoryQuantity
                    productVariantComponents(first: 5) { nodes { quantity productVariant { displayName } } } } }
                bundleComponents(first: 5) { nodes { componentProduct { handle } quantity quantityOption { name values { name quantity } }
                    optionSelections { componentOption { name } parentOption { name } values { value selectionStatus } }
                    componentVariantsCount { count } } } } }"#,
        json!({"id": operation["id"], "bundle": bundle}),
    );
    assert_eq!(
        polled["data"]["productOperation"],
        json!({"status": "COMPLETE", "product": {"id": bundle}, "userErrors": []})
    );
    let product = &polled["data"]["product"];
    assert_eq!(
        product["options"],
        json!([{"name": "Mug glaze", "values": ["Oat", "Slate"]}, {"name": "Pack", "values": ["One pair", "Three pairs"]}])
    );
    let variants = product["variants"]["nodes"].as_array().unwrap();
    assert_eq!(
        variants
            .iter()
            .map(|variant| variant["title"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "Oat / One pair",
            "Oat / Three pairs",
            "Slate / One pair",
            "Slate / Three pairs"
        ]
    );
    // The mug is 22.00, a pair of socks 16.00.
    assert_eq!(variants[1]["price"], json!("70.00"));
    assert!(
        variants
            .iter()
            .all(|variant| variant["requiresComponents"] == json!(true))
    );
    // Slate: 18 mugs; three pairs of 36-40: 60 / 3.
    assert_eq!(variants[3]["inventoryQuantity"], json!(18));
    assert_eq!(
        variants[1]["productVariantComponents"]["nodes"],
        json!([{"quantity": 1, "productVariant": {"displayName": "Ceramic Mug - Oat"}},
            {"quantity": 3, "productVariant": {"displayName": "Merino Crew Socks - 36-40"}}])
    );
    let components = &product["bundleComponents"]["nodes"];
    assert_eq!(
        components[0]["optionSelections"][0],
        json!({"componentOption": {"name": "Glaze"}, "parentOption": {"name": "Mug glaze"}, "values": [
            {"value": "Oat", "selectionStatus": "SELECTED"}, {"value": "Slate", "selectionStatus": "SELECTED"},
            {"value": "Rust", "selectionStatus": "DESELECTED"}]})
    );
    assert_eq!(
        components[1]["quantityOption"],
        json!({"name": "Pack", "values": [{"name": "One pair", "quantity": 1}, {"name": "Three pairs", "quantity": 3}]})
    );
    assert_eq!(components[0]["componentVariantsCount"]["count"], json!(2));
    let events = topics(&mut shop);
    assert_eq!(
        events.first().map(String::as_str),
        Some("inventory_items/create")
    );
    assert!(events.contains(&"products/create".to_string()));

    // Updated: the title, and the mug alone; the variants are made again.
    let updated = run_with(
        &mut shop,
        r#"mutation($input: ProductBundleUpdateInput!) { productBundleUpdate(input: $input) {
            productBundleOperation { status product { title variants(first: 5) { nodes { title price } } } } userErrors { field message } } }"#,
        json!({"input": {"productId": bundle, "title": "Mug kit", "components": [
            {"productId": mug, "quantity": 2, "optionSelections": [{"componentOptionId": glaze, "name": "Glaze", "values": ["Rust"]}]}]}}),
    );
    assert_eq!(
        ok(&updated, "productBundleUpdate")["productBundleOperation"]["product"],
        json!({"title": "Mug kit", "variants": {"nodes": [{"title": "Rust", "price": "44.00"}]}})
    );
    let events = topics(&mut shop);
    assert!(events.contains(&"inventory_items/delete".to_string()));
    assert!(events.contains(&"products/update".to_string()));
}

#[test]
fn a_product_bundle_that_is_wrong_is_refused_and_nothing_changes() {
    let mut shop = demo();
    let mug = product(&shop, "ceramic-mug");
    let glaze = super::gids(shop.get(&mug).unwrap().get("options"))[0].clone();
    let before = shop.clone();
    let refused = run_with(
        &mut shop,
        r#"mutation($input: ProductBundleCreateInput!) { productBundleCreate(input: $input) { productBundleOperation { id } userErrors { field message } } }"#,
        json!({"input": {"title": "", "components": [
            {"productId": mug, "optionSelections": [{"componentOptionId": glaze, "name": "Glaze", "values": ["Teal"]}]},
            {"productId": "gid://shopify/Product/1", "optionSelections": []}]}}),
    );
    assert_eq!(
        refused["data"]["productBundleCreate"],
        json!({"productBundleOperation": null, "userErrors": [
            {"field": ["input", "title"], "message": "Title can't be blank"},
            {"field": ["input", "components", "0", "optionSelections", "0", "values", "0"], "message": "Option value 'Teal' does not exist"},
            {"field": ["input", "components", "1", "productId"], "message": "Product does not exist"},
        ]})
    );
    let not_a_bundle = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productBundleUpdate(input: {productId: $id, title: "x"}) { productBundleOperation { id } userErrors { field message } } }"#,
        json!({"id": mug}),
    );
    assert_eq!(
        not_a_bundle["data"]["productBundleUpdate"]["userErrors"],
        json!([{"field": ["input", "productId"], "message": "Product does not exist"}])
    );
    unchanged(&mut shop, &before, &[&mug]);
    assert_eq!(shop.count("Product"), before.count("Product"));
}

#[test]
fn a_combined_listing_points_its_parent_variants_at_its_children() {
    let mut shop = demo();
    let parent = run(
        &mut shop,
        r#"mutation { productCreate(product: {title: "Shorts", combinedListingRole: PARENT}) { product { id combinedListingRole } userErrors { message } } }"#,
    );
    let parent_product = &ok(&parent, "productCreate")["product"];
    assert_eq!(parent_product["combinedListingRole"], json!("PARENT"));
    let parent = parent_product["id"].as_str().unwrap().to_string();
    let (blue, _) = new_product(&mut shop, "Shorts - Blue");
    let (red, _) = new_product(&mut shop, "Shorts - Red");
    shop.take_events();
    let added = run_with(
        &mut shop,
        r#"mutation($parent: ID!, $blue: ID!, $red: ID!) { combinedListingUpdate(parentProductId: $parent,
            productsAdded: [{childProductId: $blue, selectedParentOptionValues: [{name: "Color", value: "Blue"}]},
                {childProductId: $red, selectedParentOptionValues: [{name: "Color", value: "Red"}]}],
            optionsAndValues: [{name: "Color", values: ["Red", "Blue"]}]) {
            product { combinedListingRole options { name values } variants(first: 5) { nodes { title } }
                combinedListing { parentProduct { id } combinedListingChildren(first: 5) { nodes { product { title } parentVariant { selectedOptions { name value } } } } } }
            userErrors { field message code } } }"#,
        json!({"parent": parent, "blue": blue, "red": red}),
    );
    let product = &ok(&added, "combinedListingUpdate")["product"];
    assert_eq!(
        product["options"],
        json!([{"name": "Color", "values": ["Red", "Blue"]}])
    );
    assert_eq!(
        product["variants"]["nodes"],
        json!([{"title": "Blue"}, {"title": "Red"}])
    );
    assert_eq!(
        product["combinedListing"]["parentProduct"]["id"],
        json!(parent)
    );
    assert_eq!(
        product["combinedListing"]["combinedListingChildren"]["nodes"],
        json!([{"product": {"title": "Shorts - Blue"}, "parentVariant": {"selectedOptions": [{"name": "Color", "value": "Blue"}]}},
            {"product": {"title": "Shorts - Red"}, "parentVariant": {"selectedOptions": [{"name": "Color", "value": "Red"}]}}])
    );
    let mut events = topics(&mut shop);
    events.sort();
    events.dedup();
    assert!(events.contains(&"products/update".to_string()));

    // The child reads the listing too; the role is a search filter.
    let read = run_with(
        &mut shop,
        r#"query($blue: ID!) { product(id: $blue) { combinedListingRole combinedListing { parentProduct { title } } }
            products(first: 5, query: "combined_listing_role:PARENT") { nodes { title } } }"#,
        json!({"blue": blue}),
    );
    assert_eq!(
        read["data"]["product"],
        json!({"combinedListingRole": "CHILD", "combinedListing": {"parentProduct": {"title": "Shorts"}}})
    );
    assert_eq!(
        read["data"]["products"]["nodes"],
        json!([{"title": "Shorts"}])
    );

    // Edited and removed: the red child leaves, the blue one becomes navy.
    let changed = run_with(
        &mut shop,
        r#"mutation($parent: ID!, $blue: ID!, $red: ID!) { combinedListingUpdate(parentProductId: $parent, title: "Summer shorts",
            productsEdited: [{childProductId: $blue, selectedParentOptionValues: [{name: "Color", value: "Navy"}]}],
            productsRemovedIds: [$red]) {
            product { title options { values } combinedListing { combinedListingChildren(first: 5) { nodes { product { id } } } } }
            userErrors { field message code } } }"#,
        json!({"parent": parent, "blue": blue, "red": red}),
    );
    let product = &ok(&changed, "combinedListingUpdate")["product"];
    assert_eq!(product["title"], json!("Summer shorts"));
    assert_eq!(product["options"], json!([{"values": ["Navy"]}]));
    assert_eq!(
        product["combinedListing"]["combinedListingChildren"]["nodes"],
        json!([{"product": {"id": blue}}])
    );
    assert_eq!(shop.get(&red).unwrap()["combinedListingRole"], json!(null));
}

#[test]
fn a_combined_listing_update_that_is_wrong_is_refused_and_nothing_changes() {
    let mut shop = demo();
    let mug = product(&shop, "ceramic-mug");
    let parent = run(
        &mut shop,
        r#"mutation { productCreate(product: {title: "Shorts", combinedListingRole: PARENT}) { product { id } } }"#,
    )["data"]["productCreate"]["product"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    shop.take_events();
    let before = shop.clone();
    let not_parent = run_with(
        &mut shop,
        r#"mutation($mug: ID!) { combinedListingUpdate(parentProductId: $mug, productsRemovedIds: []) { product { id } userErrors { field message code } } }"#,
        json!({"mug": mug}),
    );
    assert_eq!(
        not_parent["data"]["combinedListingUpdate"]["userErrors"],
        json!([{"field": ["parentProductId"], "code": "PARENT_PRODUCT_MUST_BE_A_COMBINED_LISTING",
            "message": "Unable to update components for a product that isn't a combined listing."}])
    );
    let wrong = run_with(
        &mut shop,
        r#"mutation($parent: ID!, $mug: ID!) { combinedListingUpdate(parentProductId: $parent, productsAdded: [
            {childProductId: $parent, selectedParentOptionValues: [{name: "Color", value: "Blue"}]},
            {childProductId: $mug, selectedParentOptionValues: []},
            {childProductId: "gid://shopify/Product/1", selectedParentOptionValues: [{name: "Color", value: "Red"}]}]) {
            product { id } userErrors { field code } } }"#,
        json!({"parent": parent, "mug": mug}),
    );
    assert_eq!(
        wrong["data"]["combinedListingUpdate"],
        json!({"product": null, "userErrors": [
            {"field": ["productsAdded", "0"], "code": "CANNOT_HAVE_PARENT_AS_CHILD"},
            {"field": ["productsAdded", "1"], "code": "MUST_HAVE_SELECTED_OPTION_VALUES"},
            {"field": ["productsAdded", "2"], "code": "PRODUCT_NOT_FOUND"},
        ]})
    );
    unchanged(&mut shop, &before, &[&parent, &mug]);
}

/// A channel of the app and its publication.
fn channel(shop: &mut crate::state::ShopState) -> (String, String) {
    let answer = run(
        shop,
        r#"mutation { channelCreate(input: {accountId: "A1", accountName: "Marketplace", specificationHandle: "example-us"}) { channel { id } userErrors { message } } }"#,
    );
    let channel = ok(&answer, "channelCreate")["channel"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let publication = shop.get(&channel).unwrap()["lsfPublication"]
        .as_str()
        .unwrap()
        .to_string();
    shop.take_events();
    (channel, publication)
}

/// Publishes a product on a publication.
fn publish(shop: &mut crate::state::ShopState, product: &str, publication: &str) {
    let answer = run_with(
        shop,
        r#"mutation($id: ID!, $publication: ID!) { publishablePublish(id: $id, input: [{publicationId: $publication}]) { userErrors { message } } }"#,
        json!({"id": product, "publication": publication}),
    );
    ok(&answer, "publishablePublish");
}

/// The payload of an event as the webhooks build it.
fn event_payload(shop: &crate::state::ShopState, event: &crate::state::Event) -> serde_json::Value {
    let record = event
        .snapshot
        .clone()
        .or_else(|| shop.get(&event.gid).cloned())
        .unwrap();
    crate::webhooks::payload::payload(
        &event.topic,
        &record,
        &crate::api::samples::ShopLookup(shop),
    )
    .unwrap()
}

#[test]
fn a_product_feed_syncs_the_products_of_its_channel() {
    let mut shop = demo();
    let (channel, publication) = channel(&mut shop);
    let mug = product(&shop, "ceramic-mug");
    let tote = product(&shop, "canvas-tote-bag");
    publish(&mut shop, &mug, &publication);
    publish(&mut shop, &tote, &publication);
    shop.take_events();
    let created = run(
        &mut shop,
        r#"mutation { productFeedCreate(input: {country: CA, language: EN}) { productFeed { id country language status channelId } userErrors { field message code } } }"#,
    );
    let feed = &ok(&created, "productFeedCreate")["productFeed"];
    assert_eq!(feed["status"], json!("ACTIVE"));
    assert_eq!(feed["channelId"], json!(channel));
    let feed = feed["id"].as_str().unwrap().to_string();
    let events = shop.take_events();
    assert_eq!(events[0].topic, "product_feeds/create");
    assert_eq!(
        event_payload(&shop, &events[0]),
        json!({"id": feed, "country": "CA", "language": "EN", "status": "active"})
    );
    let listed = run(&mut shop, r#"{ productFeeds(first: 5) { nodes { id } } }"#);
    assert_eq!(
        listed["data"]["productFeeds"]["nodes"],
        json!([{"id": feed}])
    );

    // A full sync: one webhook per product of the channel, then the finish.
    let synced = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productFullSync(id: $id) { id userErrors { field message code } } }"#,
        json!({"id": feed}),
    );
    let sync = ok(&synced, "productFullSync")["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(sync.starts_with("gid://shopify/ProductFullSync/"));
    let events = shop.take_events();
    assert_eq!(
        events
            .iter()
            .map(|event| event.topic.as_str())
            .collect::<Vec<_>>(),
        [
            "product_feeds/full_sync",
            "product_feeds/full_sync",
            "product_feeds/full_sync_finish"
        ]
    );
    let first = event_payload(&shop, &events[0]);
    assert_eq!(first["metadata"]["fullSyncId"], json!(sync));
    assert_eq!(first["metadata"]["type"], json!("FULL"));
    assert_eq!(first["productFeed"]["id"], json!(feed));
    assert_eq!(first["product"]["id"], json!(mug));
    assert_eq!(first["product"]["title"], json!("Ceramic Mug"));
    assert_eq!(
        first["product"]["variants"]["edges"][0]["node"]["price"],
        json!({"amount": "22.00", "currencyCode": "USD"})
    );
    let finish = event_payload(&shop, &events[2]);
    assert_eq!(finish["fullSync"]["count"], json!(2));
    assert_eq!(finish["fullSync"]["status"], json!("completed"));

    // A listed product changes: an incremental sync; unlisted: its deletion.
    run_with(
        &mut shop,
        r#"mutation($id: ID!) { productUpdate(product: {id: $id, title: "Big mug"}) { userErrors { message } } }"#,
        json!({"id": mug}),
    );
    let events = shop.take_events();
    let incremental: Vec<&crate::state::Event> = events
        .iter()
        .filter(|event| event.topic == "product_feeds/incremental_sync")
        .collect();
    assert_eq!(incremental.len(), 1);
    let payload = event_payload(&shop, incremental[0]);
    assert_eq!(payload["metadata"]["action"], json!("UPDATE"));
    assert_eq!(payload["product"]["title"], json!("Big mug"));
    run_with(
        &mut shop,
        r#"mutation($id: ID!, $publication: ID!) { publishableUnpublish(id: $id, input: [{publicationId: $publication}]) { userErrors { message } } }"#,
        json!({"id": tote, "publication": publication}),
    );
    let events = shop.take_events();
    let removed = events
        .iter()
        .find(|event| event.topic == "product_feeds/incremental_sync")
        .unwrap();
    let payload = event_payload(&shop, removed);
    assert_eq!(payload["metadata"]["action"], json!("DELETE"));
    assert_eq!(payload["product"]["isPublished"], json!(false));

    // `channelFullSync` syncs the channel's feeds of its countries and languages.
    let channel_sync = run_with(
        &mut shop,
        r#"mutation($id: ID!) { channelFullSync(channelId: $id, country: CA, language: EN) { fullSyncTraceInfo { operationId } userErrors { message } } }"#,
        json!({"id": channel}),
    );
    let operation =
        ok(&channel_sync, "channelFullSync")["fullSyncTraceInfo"][0]["operationId"].clone();
    let events = shop.take_events();
    let finish = events
        .iter()
        .find(|event| event.topic == "product_feeds/full_sync_finish")
        .unwrap();
    assert_eq!(
        event_payload(&shop, finish)["metadata"]["fullSyncId"],
        operation
    );
}

#[test]
fn a_feed_follows_the_languages_of_the_shop_and_wrong_input_is_refused() {
    let mut shop = demo();
    let before = shop.clone();
    // No channel yet.
    let without = run(
        &mut shop,
        r#"mutation { productFeedCreate(input: {country: CA, language: FR}) { productFeed { id } userErrors { field message code } } }"#,
    );
    assert_eq!(
        without["data"]["productFeedCreate"]["userErrors"],
        json!([{"field": ["input", "channelId"], "message": "The input value is invalid.", "code": "INVALID"}])
    );
    for mutation in [
        r#"mutation { productFeedDelete(id: "gid://shopify/ProductFeed/1") { deletedId userErrors { field code } } }"#,
        r#"mutation { productFullSync(id: "gid://shopify/ProductFeed/1") { id userErrors { field code } } }"#,
    ] {
        let answer = run(&mut shop, mutation);
        let payload = answer["data"].as_object().unwrap().values().next().unwrap();
        assert_eq!(
            payload["userErrors"],
            json!([{"field": ["id"], "code": "INVALID"}]),
            "{answer}"
        );
    }
    unchanged(&mut shop, &before, &[]);

    channel(&mut shop);
    let created = run(
        &mut shop,
        r#"mutation { productFeedCreate(input: {country: CA, language: FR}) { productFeed { id status } userErrors { message } } }"#,
    );
    let feed = ok(&created, "productFeedCreate")["productFeed"].clone();
    assert_eq!(feed["status"], json!("ACTIVE"));
    let taken = run(
        &mut shop,
        r#"mutation { productFeedCreate(input: {country: CA, language: FR}) { productFeed { id } userErrors { field message code } } }"#,
    );
    assert_eq!(
        taken["data"]["productFeedCreate"]["userErrors"],
        json!([{"field": ["input"], "message": "The input value is already taken.", "code": "TAKEN"}])
    );
    shop.take_events();
    // French is unpublished: the feed is inactive.
    let disabled = run(
        &mut shop,
        r#"mutation { shopLocaleDisable(locale: "fr") { locale userErrors { message } } }"#,
    );
    ok(&disabled, "shopLocaleDisable");
    assert!(topics(&mut shop).contains(&"product_feeds/update".to_string()));
    assert_eq!(
        shop.get(feed["id"].as_str().unwrap()).unwrap()["status"],
        json!("INACTIVE")
    );
    let deleted = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productFeedDelete(id: $id) { deletedId userErrors { message } } }"#,
        json!({"id": feed["id"]}),
    );
    assert_eq!(ok(&deleted, "productFeedDelete")["deletedId"], feed["id"]);
    assert_eq!(shop.count("ProductFeed"), 0);
}

#[test]
fn gift_card_product_set_makes_a_gift_card_product_with_its_settings() {
    let mut shop = demo();
    let created = run(
        &mut shop,
        r#"mutation { giftCardProductSet(input: {title: "Store credit", issuanceCurrency: CAD, crossCurrencyRedeemable: true,
            productOptions: [{name: "Denomination", values: [{name: "$25"}, {name: "$50"}]}],
            variants: [{optionValues: [{optionName: "Denomination", name: "$25"}], price: "25.00"},
                {optionValues: [{optionName: "Denomination", name: "$50"}], price: "50.00"}]}) {
            product { id isGiftCard giftCardSettings { issuanceCurrency crossCurrencyRedeemable } variants(first: 5) { nodes { price } } }
            userErrors { field message code } } }"#,
    );
    let card = &ok(&created, "giftCardProductSet")["product"];
    assert_eq!(card["isGiftCard"], json!(true));
    assert_eq!(
        card["giftCardSettings"],
        json!({"issuanceCurrency": "CAD", "crossCurrencyRedeemable": true})
    );
    assert_eq!(
        card["variants"]["nodes"],
        json!([{"price": "25.00"}, {"price": "50.00"}])
    );
    assert!(topics(&mut shop).contains(&"products/create".to_string()));
    let id = card["id"].as_str().unwrap().to_string();

    // Its settings can't change; a product that is no gift card is refused; nothing changes.
    let before = shop.clone();
    let settings = run_with(
        &mut shop,
        r#"mutation($id: ID!) { giftCardProductSet(identifier: {id: $id}, input: {issuanceCurrency: USD}) { product { id } userErrors { field code } } }"#,
        json!({"id": id}),
    );
    assert_eq!(
        settings["data"]["giftCardProductSet"]["userErrors"],
        json!([{"field": ["input", "issuanceCurrency"], "code": "INVALID_INPUT"}])
    );
    let mug = product(&shop, "ceramic-mug");
    let not_gift = run_with(
        &mut shop,
        r#"mutation($id: ID!) { giftCardProductSet(identifier: {id: $id}, input: {title: "Mug"}) { product { id } userErrors { field code } } }"#,
        json!({"id": mug}),
    );
    assert_eq!(
        not_gift["data"]["giftCardProductSet"]["userErrors"],
        json!([{"field": ["identifier"], "code": "PRODUCT_MUST_BE_GIFT_CARD"}])
    );
    unchanged(&mut shop, &before, &[&id, &mug]);
}

#[test]
fn gift_card_notifications_are_logged_and_never_sent() {
    let mut shop = demo();
    let cards: Vec<String> = shop.keys("GiftCard").map(str::to_string).collect();
    let (with_customer, without) = (cards[0].clone(), cards[1].clone());
    let sent = run_with(
        &mut shop,
        r#"mutation($id: ID!) { giftCardSendNotificationToCustomer(id: $id) { giftCard { id } userErrors { field message code } } }"#,
        json!({"id": with_customer}),
    );
    assert_eq!(
        ok(&sent, "giftCardSendNotificationToCustomer")["giftCard"]["id"],
        json!(with_customer)
    );
    let log = shop.get(&with_customer).unwrap()["lsfNotifications"].clone();
    assert_eq!(log[0]["to"], json!("jane.doe@example.com"));
    // The card has a recipient: the customer gets a purchase confirmation.
    assert_eq!(log[0]["kind"], json!("purchase confirmation"));
    let to_recipient = run_with(
        &mut shop,
        r#"mutation($id: ID!) { giftCardSendNotificationToRecipient(id: $id) { giftCard { id } userErrors { message } } }"#,
        json!({"id": with_customer}),
    );
    ok(&to_recipient, "giftCardSendNotificationToRecipient");
    shop.take_events();
    let before = shop.clone();
    let refused = run_with(
        &mut shop,
        r#"mutation($id: ID!) { giftCardSendNotificationToRecipient(id: $id) { giftCard { id } userErrors { field message code } } }"#,
        json!({"id": without}),
    );
    assert_eq!(
        refused["data"]["giftCardSendNotificationToRecipient"]["userErrors"],
        json!([{"field": ["id"], "message": "The recipient could not be found.", "code": "RECIPIENT_NOT_FOUND"}])
    );
    let missing = run(
        &mut shop,
        r#"mutation { giftCardSendNotificationToCustomer(id: "gid://shopify/GiftCard/1") { userErrors { code } } }"#,
    );
    assert_eq!(
        missing["data"]["giftCardSendNotificationToCustomer"]["userErrors"],
        json!([{"code": "GIFT_CARD_NOT_FOUND"}])
    );
    unchanged(&mut shop, &before, &[&without, &with_customer]);
}

/// The root queries of the catalog's extras answer real values on the demo store: none reports
/// a field answered with a default.
#[test]
fn the_extras_queries_answer_without_defaults_on_the_demo_store() {
    let mut shop = demo();
    let tote = product(&shop, "canvas-tote-bag");
    let (_, device) = (
        (),
        crate::domains::pos::device_gid(shop.keys("Location").next().unwrap()),
    );
    let read = run_with(
        &mut shop,
        r#"query($tote: ID!, $device: ID!) {
            productTags(first: 5) { nodes } productTypes(first: 5) { nodes } productVendors(first: 5) { nodes }
            productSavedSearches(first: 5) { nodes { id } }
            productDuplicateJob(id: "gid://shopify/ProductDuplicateJob/1") { id done }
            productFeeds(first: 5) { nodes { id } }
            collectionConditionsSourcesByApp(first: 5) { nodes { app { id } } }
            cashDrawers(first: 5) { nodes { id } }
            cashManagementReasonCodes(first: 3) { nodes { __typename ... on CashManagementDefaultReasonCode { id code } } }
            pointOfSaleDevice(id: $device) { id cashDrawer { id } activePaymentSession { id } fiscalDeviceIdentifier }
            pointOfSaleDevicePaymentSessions(first: 5) { nodes { id } }
            cashTrackingSessions(first: 5) { nodes { id } }
            discountNodes(first: 5, query: "tag:none") { nodes { id } }
            product(id: $tote) { combinedListing { parentProduct { id } } combinedListingRole hasVariantsThatRequiresComponents
                bundleComponents(first: 5) { nodes { quantity } } productComponents(first: 5) { nodes { product { id } } }
                productParents(first: 5) { nodes { id } } sources: collections(first: 1) { nodes { sources { __typename } } }
                variants(first: 1) { nodes { requiresComponents productVariantComponents(first: 5) { nodes { id } } productParents(first: 5) { nodes { id } } } } } }"#,
        json!({"tote": tote, "device": device}),
    );
    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(
        read["extensions"]["lsf"]["defaulted"],
        serde_json::Value::Null,
        "{read}"
    );
    assert_eq!(read["data"]["productDuplicateJob"]["done"], json!(true));
}
