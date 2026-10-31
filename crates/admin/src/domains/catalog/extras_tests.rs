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
