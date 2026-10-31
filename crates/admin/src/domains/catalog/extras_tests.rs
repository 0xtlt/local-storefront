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
