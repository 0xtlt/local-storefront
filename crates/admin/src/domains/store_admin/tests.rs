//! Redirects in bulk and their imports, saved searches, policies, standard definitions and file
//! acknowledgements, through queries over the demo store.

use serde_json::{Value, json};

use crate::domains::storefront::tests::{run, shop, topics};
use crate::state::{Blob, ShopState};

fn redirect(shop: &mut ShopState, path: &str, target: &str) -> String {
    let answer = run(
        shop,
        "mutation($input: UrlRedirectInput!) { urlRedirectCreate(urlRedirect: $input) { urlRedirect { id } userErrors { message } } }",
        json!({"input": {"path": path, "target": target}}),
    );
    answer["data"]["urlRedirectCreate"]["urlRedirect"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{answer}"))
        .to_string()
}

fn paths(shop: &mut ShopState) -> Vec<String> {
    let answer = run(
        shop,
        "{ urlRedirects(first: 50) { nodes { path } } }",
        json!({}),
    );
    answer["data"]["urlRedirects"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["path"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn redirects_are_deleted_by_ids_by_search_by_saved_search_and_all() {
    let mut shop = shop();
    let old = redirect(&mut shop, "/old", "/new");
    redirect(&mut shop, "/sale/summer", "/collections/summer");
    redirect(&mut shop, "/sale/winter", "/collections/winter");
    redirect(&mut shop, "/blog-old", "/blogs/news");
    shop.take_events();

    let by_ids = run(
        &mut shop,
        "mutation($ids: [ID!]!) { urlRedirectBulkDeleteByIds(ids: $ids) { job { id done } userErrors { field message code } } }",
        json!({"ids": [old]}),
    );
    assert_eq!(
        by_ids["data"]["urlRedirectBulkDeleteByIds"]["job"]["done"], true,
        "{by_ids}"
    );
    assert_eq!(topics(&mut shop), ["url_redirects/delete"]);
    let empty = run(
        &mut shop,
        "mutation { urlRedirectBulkDeleteByIds(ids: []) { job { id } userErrors { field code } } }",
        json!({}),
    );
    assert_eq!(
        empty["data"]["urlRedirectBulkDeleteByIds"]["userErrors"][0]["code"],
        "IDS_EMPTY"
    );

    let bad = run(
        &mut shop,
        "mutation { urlRedirectBulkDeleteBySearch(search: \"colour:red\") { job { id } userErrors { field code } } }",
        json!({}),
    );
    assert_eq!(
        bad["data"]["urlRedirectBulkDeleteBySearch"]["userErrors"][0]["code"],
        "INVALID_SEARCH_ARGUMENT",
        "{bad}"
    );
    assert_eq!(
        paths(&mut shop).len(),
        3,
        "a refused search deletes nothing"
    );
    let by_search = run(
        &mut shop,
        "mutation { urlRedirectBulkDeleteBySearch(search: \"path:/sale/summer\") { job { done } userErrors { message } } }",
        json!({}),
    );
    assert_eq!(
        by_search["data"]["urlRedirectBulkDeleteBySearch"]["job"]["done"],
        true
    );
    assert_eq!(paths(&mut shop), ["/sale/winter", "/blog-old"]);

    let saved = run(
        &mut shop,
        "mutation { savedSearchCreate(input: {name: \"Winter\", query: \"path:/sale/winter\", resourceType: URL_REDIRECT}) { savedSearch { id name query searchTerms filters { key value } resourceType } userErrors { field message } } }",
        json!({}),
    );
    let search = &saved["data"]["savedSearchCreate"]["savedSearch"];
    assert_eq!(
        search["filters"],
        json!([{"key": "path", "value": "/sale/winter"}]),
        "{saved}"
    );
    let listed = run(
        &mut shop,
        "{ urlRedirectSavedSearches(first: 5) { nodes { name } } productSavedSearches(first: 5) { nodes { name } } }",
        json!({}),
    );
    assert_eq!(
        listed["data"]["urlRedirectSavedSearches"]["nodes"],
        json!([{"name": "Winter"}])
    );
    assert_eq!(listed["data"]["productSavedSearches"]["nodes"], json!([]));
    let by_saved = run(
        &mut shop,
        "mutation($id: ID!) { urlRedirectBulkDeleteBySavedSearch(savedSearchId: $id) { job { done } userErrors { code } } }",
        json!({"id": search["id"]}),
    );
    assert_eq!(
        by_saved["data"]["urlRedirectBulkDeleteBySavedSearch"]["job"]["done"],
        true
    );
    assert_eq!(paths(&mut shop), ["/blog-old"]);
    let missing = run(
        &mut shop,
        "mutation { urlRedirectBulkDeleteBySavedSearch(savedSearchId: \"gid://shopify/SavedSearch/999\") { job { done } userErrors { code } } }",
        json!({}),
    );
    assert_eq!(
        missing["data"]["urlRedirectBulkDeleteBySavedSearch"]["userErrors"][0]["code"],
        "SAVED_SEARCH_NOT_FOUND"
    );

    let all = run(
        &mut shop,
        "mutation { urlRedirectBulkDeleteAll { job { id done } userErrors { message } } }",
        json!({}),
    );
    assert_eq!(all["data"]["urlRedirectBulkDeleteAll"]["job"]["done"], true);
    assert!(paths(&mut shop).is_empty());
    let job = all["data"]["urlRedirectBulkDeleteAll"]["job"]["id"]
        .as_str()
        .unwrap();
    let read = run(
        &mut shop,
        "query($id: ID!) { job(id: $id) { done } }",
        json!({"id": job}),
    );
    assert_eq!(read["data"]["job"]["done"], true, "{read}");
}

#[test]
fn a_redirect_csv_is_previewed_then_imported_once() {
    let mut shop = shop();
    redirect(&mut shop, "/b", "/old-target");
    let key = "tmp/1/redirects/abc/redirects.csv";
    shop.put_blob(
        key,
        Blob::new(
            "Redirect from,Redirect to\n/a,/products/a\n/b,/products/b\n\"/c\",\"/c\"\n,/x\n/d,https://example.com/d\n",
            "text/csv",
        ),
    );
    let url = format!("https://shopify-staged-uploads.storage.googleapis.com/{key}");
    let created = run(
        &mut shop,
        "mutation($url: URL!) { urlRedirectImportCreate(url: $url) { urlRedirectImport { id count finished previewRedirects { path target } } userErrors { field message code } } }",
        json!({"url": url}),
    );
    let import = &created["data"]["urlRedirectImportCreate"]["urlRedirectImport"];
    if import.is_null() {
        // The bucket URL is lsf's own: read it from a target of `stagedUploadsCreate`.
        panic!("{created}");
    }
    assert_eq!(import["count"], 5);
    assert_eq!(import["finished"], false);
    assert_eq!(
        import["previewRedirects"],
        json!([{"path": "/a", "target": "/products/a"}, {"path": "/b", "target": "/products/b"}, {"path": "/c", "target": "/c"}])
    );
    let id = import["id"].as_str().unwrap().to_string();
    let submitted = run(
        &mut shop,
        "mutation($id: ID!) { urlRedirectImportSubmit(id: $id) { job { done } userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        submitted["data"]["urlRedirectImportSubmit"]["job"]["done"], true,
        "{submitted}"
    );
    let read = run(
        &mut shop,
        "query($id: ID!) { urlRedirectImport(id: $id) { finished finishedAt createdCount updatedCount failedCount } }",
        json!({"id": id}),
    );
    let import = &read["data"]["urlRedirectImport"];
    assert_eq!(
        (
            &import["createdCount"],
            &import["updatedCount"],
            &import["failedCount"]
        ),
        (&json!(2), &json!(1), &json!(2)),
        "{read}"
    );
    assert_eq!(import["finished"], true);
    assert!(paths(&mut shop).contains(&"/d".to_string()));
    let twice = run(
        &mut shop,
        "mutation($id: ID!) { urlRedirectImportSubmit(id: $id) { job { done } userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        twice["data"]["urlRedirectImportSubmit"]["userErrors"][0]["code"],
        "ALREADY_IMPORTED"
    );
}

#[test]
fn a_file_that_is_no_redirect_csv_is_not_imported() {
    let mut shop = shop();
    let key = "tmp/1/redirects/abc/other.csv";
    shop.put_blob(key, Blob::new("Handle,Title\nshirt,Shirt\n", "text/csv"));
    let answer = run(
        &mut shop,
        "mutation($url: URL!) { urlRedirectImportCreate(url: $url) { urlRedirectImport { id } userErrors { field code } } }",
        json!({"url": format!("https://shopify-staged-uploads.storage.googleapis.com/{key}")}),
    );
    assert_eq!(
        answer["data"]["urlRedirectImportCreate"]["userErrors"],
        json!([{"field": ["url"], "code": "NOT_FOUND"}]),
        "{answer}"
    );
    assert_eq!(shop.count("UrlRedirectImport"), 0);
}

#[test]
fn saved_searches_are_named_once_per_resource_type() {
    let mut shop = shop();
    let create = "mutation($input: SavedSearchCreateInput!) { savedSearchCreate(input: $input) { savedSearch { id searchTerms } userErrors { field message } } }";
    let first = run(
        &mut shop,
        create,
        json!({"input": {"name": "Shirts", "query": "shirt tag:summer", "resourceType": "PRODUCT"}}),
    );
    let id = first["data"]["savedSearchCreate"]["savedSearch"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        first["data"]["savedSearchCreate"]["savedSearch"]["searchTerms"],
        "shirt"
    );
    let taken = run(
        &mut shop,
        create,
        json!({"input": {"name": "shirts", "query": "x", "resourceType": "PRODUCT"}}),
    );
    assert_eq!(
        taken["data"]["savedSearchCreate"]["userErrors"],
        json!([{"field": ["input", "name"], "message": "Name has already been taken"}])
    );
    let other_type = run(
        &mut shop,
        create,
        json!({"input": {"name": "Shirts", "query": "x", "resourceType": "COLLECTION"}}),
    );
    assert!(other_type["data"]["savedSearchCreate"]["savedSearch"]["id"].is_string());
    let updated = run(
        &mut shop,
        "mutation($id: ID!) { savedSearchUpdate(input: {id: $id, query: \"tag:winter\"}) { savedSearch { name query searchTerms filters { key value } } userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        updated["data"]["savedSearchUpdate"]["savedSearch"],
        json!({"name": "Shirts", "query": "tag:winter", "searchTerms": "", "filters": [{"key": "tag", "value": "winter"}]})
    );
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { savedSearchDelete(input: {id: $id}) { deletedSavedSearchId shop { id } userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        deleted["data"]["savedSearchDelete"]["deletedSavedSearchId"],
        json!(id)
    );
    assert_eq!(shop.count("SavedSearch"), 1);
}

#[test]
fn a_policy_is_updated_or_created_and_listed_by_the_shop() {
    let mut shop = shop();
    let update = "mutation($policy: ShopPolicyInput!) { shopPolicyUpdate(shopPolicy: $policy) { shopPolicy { id type title body url } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        update,
        json!({"policy": {"type": "REFUND_POLICY", "body": "<p>30 days.</p>"}}),
    );
    let policy = &answer["data"]["shopPolicyUpdate"]["shopPolicy"];
    assert_eq!(policy["body"], "<p>30 days.</p>", "{answer}");
    assert_eq!(policy["title"], "Refund policy");
    assert!(
        policy["url"]
            .as_str()
            .unwrap()
            .ends_with("/policies/refund-policy")
    );
    let created = run(
        &mut shop,
        update,
        json!({"policy": {"type": "LEGAL_NOTICE", "body": "Imprint"}}),
    );
    let legal = &created["data"]["shopPolicyUpdate"]["shopPolicy"];
    assert_eq!(legal["title"], "Legal notice", "{created}");
    let listed = run(
        &mut shop,
        "{ shop { shopPolicies { type body } } }",
        json!({}),
    );
    assert!(
        listed["data"]["shop"]["shopPolicies"]
            .as_array()
            .unwrap()
            .contains(&json!({"type": "LEGAL_NOTICE", "body": "Imprint"})),
        "{listed}"
    );
    let before = shop.count("ShopPolicy");
    let too_big = run(
        &mut shop,
        update,
        json!({"policy": {"type": "TERMS_OF_SALE", "body": "x".repeat(600 * 1024)}}),
    );
    assert_eq!(
        too_big["data"]["shopPolicyUpdate"]["userErrors"][0]["code"],
        "TOO_BIG"
    );
    assert_eq!(shop.count("ShopPolicy"), before);
}

#[test]
fn a_standard_metafield_definition_is_enabled_from_its_template() {
    let mut shop = shop();
    let templates = run(
        &mut shop,
        "{ standardMetafieldDefinitionTemplates(first: 50) { nodes { id name namespace key ownerTypes type { name category } validations { name type value } } } }",
        json!({}),
    );
    let nodes = templates["data"]["standardMetafieldDefinitionTemplates"]["nodes"]
        .as_array()
        .unwrap();
    assert_eq!(nodes.len(), 16, "{templates}");
    assert_eq!(nodes[1]["name"], "Care guide");
    assert_eq!(
        nodes[1]["validations"],
        json!([{"name": "max", "type": "number_integer", "value": "500"}])
    );
    assert!(templates.get("extensions").is_none(), "{templates}");

    let not_activated = |shop: &mut ShopState| {
        let left = run(
            shop,
            "{ standardMetafieldDefinitionTemplates(first: 50, excludeActivated: true) { nodes { key } } }",
            json!({}),
        );
        left["data"]["standardMetafieldDefinitionTemplates"]["nodes"]
            .as_array()
            .unwrap()
            .len()
    };
    // The store data's metafields come with definitions: some templates are activated already.
    let before = not_activated(&mut shop);
    let enable = "mutation($id: ID, $ownerType: MetafieldOwnerType!) { standardMetafieldDefinitionEnable(id: $id, ownerType: $ownerType, pin: false) { createdDefinition { id name namespace key description standardTemplate { id } } userErrors { field message code } } }";
    let wrong_owner = run(
        &mut shop,
        enable,
        json!({"id": "gid://shopify/StandardMetafieldDefinitionTemplate/2", "ownerType": "CUSTOMER"}),
    );
    assert_eq!(
        wrong_owner["data"]["standardMetafieldDefinitionEnable"]["userErrors"][0]["code"],
        "INVALID"
    );
    let unknown = run(
        &mut shop,
        enable,
        json!({"id": "gid://shopify/StandardMetafieldDefinitionTemplate/999", "ownerType": "PRODUCT"}),
    );
    assert_eq!(
        unknown["data"]["standardMetafieldDefinitionEnable"]["userErrors"][0]["code"],
        "TEMPLATE_NOT_FOUND"
    );
    assert!(topics(&mut shop).is_empty());

    let answer = run(
        &mut shop,
        enable,
        json!({"id": "gid://shopify/StandardMetafieldDefinitionTemplate/2", "ownerType": "PRODUCT"}),
    );
    let created = &answer["data"]["standardMetafieldDefinitionEnable"]["createdDefinition"];
    assert_eq!(
        (&created["name"], &created["namespace"], &created["key"]),
        (
            &json!("Care guide"),
            &json!("descriptors"),
            &json!("care_guide")
        ),
        "{answer}"
    );
    assert_eq!(
        created["description"],
        "Instructions for taking care of a product or apparel"
    );
    assert_eq!(
        created["standardTemplate"]["id"],
        "gid://shopify/StandardMetafieldDefinitionTemplate/2"
    );
    assert_eq!(topics(&mut shop), ["metafield_definitions/create"]);
    let again = run(
        &mut shop,
        enable,
        json!({"id": "gid://shopify/StandardMetafieldDefinitionTemplate/2", "ownerType": "PRODUCT"}),
    );
    assert_eq!(
        again["data"]["standardMetafieldDefinitionEnable"]["userErrors"][0]["code"], "TAKEN",
        "{again}"
    );
    assert_eq!(not_activated(&mut shop), before - 1);
}

#[test]
fn the_standard_product_review_definition_is_enabled() {
    let mut shop = shop();
    let enable = "mutation($type: String!) { standardMetaobjectDefinitionEnable(type: $type) { metaobjectDefinition { id type name fieldDefinitions { key required type { name } } standardTemplate { type name } } userErrors { field message code } } }";
    let unknown = run(&mut shop, enable, json!({"type": "recipe"}));
    assert_eq!(
        unknown["data"]["standardMetaobjectDefinitionEnable"]["userErrors"][0]["code"],
        "RECORD_NOT_FOUND"
    );
    let answer = run(&mut shop, enable, json!({"type": "product_review"}));
    let definition = &answer["data"]["standardMetaobjectDefinitionEnable"]["metaobjectDefinition"];
    assert_eq!(definition["type"], "product_review", "{answer}");
    assert_eq!(
        definition["fieldDefinitions"][0],
        json!({"key": "rating", "required": true, "type": {"name": "rating"}})
    );
    assert_eq!(definition["standardTemplate"]["name"], "Product Review");
    let twice = run(&mut shop, enable, json!({"type": "product_review"}));
    assert_eq!(
        twice["data"]["standardMetaobjectDefinitionEnable"]["userErrors"][0]["code"], "TAKEN",
        "{twice}"
    );
}

#[test]
fn a_failed_file_is_acknowledged_back_to_ready() {
    let mut shop = shop();
    let file = shop
        .keys("MediaImage")
        .chain(shop.keys("GenericFile"))
        .next()
        .expect("the demo store has a file")
        .to_string();
    let acknowledge = "mutation($ids: [ID!]!) { fileAcknowledgeUpdateFailed(fileIds: $ids) { files { id fileStatus } userErrors { field code } } }";
    let ready = run(&mut shop, acknowledge, json!({"ids": [file]}));
    assert_eq!(
        ready["data"]["fileAcknowledgeUpdateFailed"]["userErrors"][0]["code"], "NON_READY_STATE",
        "{ready}"
    );
    shop.get_mut(&file)
        .unwrap()
        .insert("fileStatus".into(), json!("FAILED"));
    let missing = run(
        &mut shop,
        acknowledge,
        json!({"ids": [file, "gid://shopify/MediaImage/1"]}),
    );
    assert_eq!(
        missing["data"]["fileAcknowledgeUpdateFailed"]["userErrors"][0]["code"],
        "FILE_DOES_NOT_EXIST"
    );
    assert_eq!(
        shop.get(&file).unwrap()["fileStatus"],
        json!("FAILED"),
        "nothing changed"
    );
    let answer = run(&mut shop, acknowledge, json!({"ids": [file]}));
    assert_eq!(
        answer["data"]["fileAcknowledgeUpdateFailed"]["files"],
        json!([{"id": file, "fileStatus": "READY"}]),
        "{answer}"
    );
    assert_eq!(shop.get(&file).unwrap()["fileErrors"], Value::Array(vec![]));
}
