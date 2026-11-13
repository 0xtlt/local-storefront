//! Store sync through the handlers: what the backend changes through the Admin API is what the
//! storefront and the checkout serve next. Every change is a GraphQL mutation, sent as a backend
//! sends it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsf_app::admin::domains::catalog;
use lsf_core::theme::Revalidate;
use serde_json::{Value as Json, json};

use super::super::reply::Reply;
use super::super::{ServeOptions, ServerState};
use super::{
    ScenarioData, admin_query, admin_request, admin_server, body_json, buyer_details,
    checkout_server_with, copy_directory, fixture_app, get, header, log_in_as, operations, post,
    request, retitle, text, title_on_page,
};
use crate::app::App;

/// The fixture theme's server, serving the Admin API and the checkout of the fixture app, which
/// asks for no scope: the backend gets those its mutations need.
fn checkout_admin_server() -> ServerState {
    let state = checkout_server_with(fixture_app(), &[], false, None, Some(Default::default()));
    let installed = admin_request(
        &state,
        "POST",
        "/__lsf/admin/install",
        r#"{"scopes": "write_products,write_publications,write_customers,write_discounts,write_validations,write_payment_customizations"}"#,
    );
    assert_eq!(installed.status, 200, "{}", text(&installed));
    state
}

/// A mutation (with its variables) that must answer neither an error nor a user error: its
/// payload.
fn mutate(state: &ServerState, query: &str, variables: Json) -> Json {
    let reply = admin_request(
        state,
        "POST",
        "/admin/api/2026-10/graphql.json",
        &json!({ "query": query, "variables": variables }).to_string(),
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let answer = body_json(&reply);
    assert!(answer.get("errors").is_none(), "{answer}");
    let payload = answer["data"]
        .as_object()
        .and_then(|data| data.values().next())
        .cloned()
        .unwrap_or_default();
    assert_eq!(payload["userErrors"], json!([]), "{answer}");
    payload
}

fn product_gid(state: &ServerState, handle: &str) -> String {
    catalog::product_gid(
        state
            .loaded()
            .store
            .product_by_handle(handle)
            .expect("a product"),
    )
}

/// Puts `session` in a cart of `items`, without data of its own, and returns the path of its
/// checkout.
fn cart_checkout(state: &ServerState, session: &str, items: Json) -> String {
    let reply = state.dispatch(&request(
        "PUT",
        "/__lsf/session",
        json!({"cart": {"items": items}}),
        session,
    ));
    assert_eq!(reply.status, 200, "{}", text(&reply));
    header(&get(state, "/checkout", session), "location")
        .expect("a location")
        .to_string()
}

/// The run of a Function in the checkout's JSON.
fn run_of<'a>(checkout: &'a Json, handle: &str) -> &'a Json {
    checkout["runs"]
        .as_array()
        .expect("runs")
        .iter()
        .find(|run| run["handle"] == handle)
        .unwrap_or_else(|| panic!("no run of {handle}: {checkout}"))
}

fn sync_status(state: &ServerState) -> Json {
    body_json(&admin_request(state, "GET", "/__lsf/admin", ""))["storeSync"].clone()
}

#[test]
fn a_product_the_backend_creates_is_on_the_storefront() {
    let state = admin_server();
    let frontpage = catalog::collection_gid(
        state
            .loaded()
            .store
            .collection_by_handle("frontpage")
            .unwrap(),
    );
    let created = admin_query(
        &state,
        &format!(
            r#"mutation {{ productCreate(product: {{title: "Wool Hat", collectionsToJoin: ["{frontpage}"]}}) {{ product {{ id handle }} userErrors {{ message }} }} }}"#
        ),
    );
    let product = &created["data"]["productCreate"]["product"];
    assert_eq!(product["handle"], "wool-hat", "{created}");
    let id = product["id"].as_str().unwrap().to_string();
    // `productCreate` does not publish it on the Online Store, as on Shopify.
    assert_eq!(get(&state, "/products/wool-hat", "sync").status, 404);

    // `publishablePublish` on the Online Store.
    let publications = admin_query(&state, "{ publications(first: 5) { nodes { id name } } }");
    let online_store = publications["data"]["publications"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|publication| publication["name"] == "Online Store")
        .map(|publication| publication["id"].clone())
        .unwrap_or_else(|| panic!("the Online Store: {publications}"));
    mutate(
        &state,
        "mutation($id: ID!, $publication: ID!) { publishablePublish(id: $id, input: [{publicationId: $publication}]) { userErrors { field message } } }",
        json!({"id": id, "publication": online_store}),
    );
    assert_eq!(title_on_page(&state, "wool-hat", "sync"), "Wool Hat");
    // Last of the collection's five, on its second page of three.
    let collection = text(&get(&state, "/collections/frontpage?page=2", "sync"));
    assert!(
        collection.contains("<h1>Home page (5/5)</h1>") && collection.contains(">Wool Hat</a>"),
        "the collection lists it: {collection}"
    );
    // With the id the Admin API gave it: the backend and the storefront name it alike.
    let loaded = state.loaded();
    let hat = loaded.store.product_by_handle("wool-hat").unwrap();
    assert_eq!(catalog::product_gid(hat), id);
    // The files are as they were.
    assert!(loaded.base.product_by_handle("wool-hat").is_none());
}

#[test]
fn a_product_the_backend_sets_to_draft_leaves_the_storefront() {
    let state = admin_server();
    assert_eq!(title_on_page(&state, "ceramic-mug", "sync"), "Ceramic Mug");
    let id = product_gid(&state, "ceramic-mug");
    let updated = admin_query(
        &state,
        &format!(
            r#"mutation {{ productUpdate(product: {{id: "{id}", status: DRAFT}}) {{ product {{ status }} }} }}"#
        ),
    );
    assert_eq!(
        updated["data"]["productUpdate"]["product"]["status"],
        "DRAFT"
    );
    assert_eq!(get(&state, "/products/ceramic-mug", "sync").status, 404);
    assert!(!text(&get(&state, "/collections/frontpage", "sync")).contains("Ceramic Mug"));
}

#[test]
fn a_customer_the_backend_creates_can_log_in() {
    let state = checkout_admin_server();
    mutate(
        &state,
        r#"mutation($input: CustomerInput!) { customerCreate(input: $input) { customer { id } userErrors { field message } } }"#,
        json!({"input": {
            "firstName": "Noa", "lastName": "Lee", "email": "noa@example.com", "tags": ["vip"],
            "emailMarketingConsent": {"marketingState": "SUBSCRIBED"},
            "addresses": [{"firstName": "Noa", "lastName": "Lee", "address1": "1 Rue de la Paix",
                "city": "Lyon", "countryCode": "FR", "zip": "69001"}],
        }}),
    );

    // The storefront logs them in.
    let login = get(
        &state,
        "/__lsf/login?customer=noa@example.com&return_to=/account",
        "noa",
    );
    assert_eq!(header(&login, "location"), Some("/account"));
    assert_eq!(
        body_json(&get(&state, "/__lsf/session", "noa"))["customer"],
        "noa@example.com"
    );
    // So does the checkout, with their address.
    let path = cart_checkout(
        &state,
        "co-noa",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
    );
    let checkout = log_in_as(&state, &path, "co-noa", json!("noa@example.com"));
    assert_eq!(checkout["buyer"]["customer"]["email"], "noa@example.com");
    assert_eq!(checkout["shippingAddress"]["city"], "Lyon", "{checkout}");
}

/// The fixture theme, copied with a template that prints metafields: `?view=metafields`.
struct MetafieldTheme(PathBuf);

impl MetafieldTheme {
    fn new() -> MetafieldTheme {
        let directory = std::env::temp_dir().join(format!("lsf-sync-theme-{}", std::process::id()));
        std::fs::remove_dir_all(&directory).ok();
        copy_directory(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../theme/tests/fixtures/theme"),
            &directory,
        );
        std::fs::write(
            directory.join("templates/product.metafields.liquid"),
            "<p id=\"mf\">care={{ product.metafields.custom.care.value }}|banner={{ shop.metafields.custom.banner }}</p>",
        )
        .unwrap();
        MetafieldTheme(directory)
    }

    fn server(&self) -> ServerState {
        let app = App::open(&self.0, None, Revalidate::Never, &[]).expect("the theme");
        ServerState::new(
            app,
            ServeOptions {
                polaris: Default::default(),
                admin: Some(Default::default()),
                live_reload: false,
                watch: false,
                quiet: true,
                compress: false,
                minify: false,
                throttle: Default::default(),
                timings: false,
                customer: None,
                checkout: None,
            },
        )
        .0
    }
}

impl Drop for MetafieldTheme {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
fn metafields_the_backend_sets_are_read_by_liquid() {
    let theme = MetafieldTheme::new();
    let state = theme.server();
    let page = |state: &ServerState| {
        text(&get(
            state,
            "/products/organic-cotton-t-shirt?view=metafields",
            "mf",
        ))
    };
    assert!(
        page(&state).contains("care=Machine wash cold. Tumble dry low.|banner=<"),
        "{}",
        page(&state)
    );
    let tee = product_gid(&state, "organic-cotton-t-shirt");
    let shop = admin_query(&state, "{ shop { id } }")["data"]["shop"]["id"].clone();
    // `metafieldsSet`, on a product (one the data gives, whose definition gives the type) and on
    // the shop (a new one).
    mutate(
        &state,
        r#"mutation($metafields: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $metafields) { metafields { id } userErrors { field message } } }"#,
        json!({"metafields": [
            {"ownerId": tee, "namespace": "custom", "key": "care", "value": "Hand wash"},
            {"ownerId": shop, "namespace": "custom", "key": "banner", "type": "single_line_text_field", "value": "Spring sale"},
        ]}),
    );
    assert!(
        page(&state).contains("care=Hand wash|banner=Spring sale<"),
        "{}",
        page(&state)
    );
}

#[test]
fn a_validation_runs_once_the_backend_creates_it_and_stops_when_it_is_disabled() {
    let state = checkout_admin_server();
    // A shop the app was just installed on has no validation: its Function does not run.
    let validations = admin_query(&state, "{ validations(first: 10) { nodes { id } } }");
    assert_eq!(
        validations["data"]["validations"]["nodes"],
        json!([]),
        "{validations}"
    );
    let three = json!([{"variant": "TEE-BLK-M", "quantity": 3}]);
    let path = cart_checkout(&state, "co-limit-before", three.clone());
    let before = body_json(&get(&state, &format!("{path}.json"), "co-limit-before"));
    assert_eq!(before["errors"], json!([]), "{before}");
    assert!(
        before["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|run| run["handle"] != "quantity-limit"),
        "{before}"
    );
    let idle = |json: &Json| {
        json["idle"]
            .as_array()
            .unwrap()
            .iter()
            .find(|idle| idle["handle"] == "quantity-limit")
            .cloned()
            .unwrap_or_else(|| panic!("quantity-limit is not idle: {json}"))
    };
    assert_eq!(idle(&before)["owner"], "Validation");
    assert_eq!(idle(&before)["enabled"], Json::Null);

    // `validationCreate`, enabled, with the settings in its metafields.
    let created = mutate(
        &state,
        r#"mutation($validation: ValidationCreateInput!) { validationCreate(validation: $validation) { validation { id enabled } userErrors { field message } } }"#,
        json!({"validation": {
            "functionHandle": "quantity-limit", "title": "Limits", "enable": true,
            "metafields": [{"namespace": "$app:config", "key": "settings", "type": "json", "value": "{\"maxQuantity\":2}"}],
        }}),
    );
    let owner = created["validation"]["id"].clone();
    let path = cart_checkout(&state, "co-limit", three.clone());
    let after = body_json(&get(&state, &format!("{path}.json"), "co-limit"));
    assert_eq!(after["errors"][0]["function"], "quantity-limit", "{after}");
    let input = &run_of(&after, "quantity-limit")["input"];
    assert!(
        input.to_string().contains("maxQuantity"),
        "the input reads the settings: {input}"
    );

    // `metafieldsSet` on it changes what the Function reads.
    mutate(
        &state,
        r#"mutation($owner: ID!) { metafieldsSet(metafields: [{ownerId: $owner, namespace: "$app:config", key: "settings", type: "json", value: "{\"maxQuantity\":5}"}]) { userErrors { field message } } }"#,
        json!({ "owner": owner }),
    );
    let path = cart_checkout(&state, "co-limit-5", three.clone());
    let five = body_json(&get(&state, &format!("{path}.json"), "co-limit-5"));
    assert_eq!(five["errors"], json!([]), "{five}");

    // Disabled: it does not run any more.
    mutate(
        &state,
        r#"mutation($id: ID!) { validationUpdate(id: $id, validation: {enable: false, metafields: []}) { validation { enabled } userErrors { field message } } }"#,
        json!({ "id": owner }),
    );
    let path = cart_checkout(&state, "co-limit-off", three);
    let off = body_json(&get(&state, &format!("{path}.json"), "co-limit-off"));
    assert!(
        off["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|run| run["handle"] != "quantity-limit"),
        "{off}"
    );
    assert_eq!(idle(&off)["enabled"], false);
    assert_eq!(idle(&off)["reason"], "its validation is disabled");
}

#[test]
fn a_validation_created_to_block_on_failure_blocks_the_checkout_when_it_fails() {
    let state = checkout_admin_server();
    let created = mutate(
        &state,
        r#"mutation { validationCreate(validation: {functionHandle: "quantity-limit", enable: true, blockOnFailure: true}) { validation { id blockOnFailure } userErrors { field message } } }"#,
        json!({}),
    );
    assert_eq!(created["validation"]["blockOnFailure"], true, "{created}");
    // The checkout reads the shop the backend changed.
    let session = "co-validation-blocks";
    let path = cart_checkout(
        &state,
        session,
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
    );
    assert_eq!(get(&state, &format!("{path}.json"), session).status, 200);
    let (_, store) = state.snapshot(session);
    assert_eq!(
        store.checkout.block_on_failure.get("quantity-limit"),
        Some(&true)
    );
    let config = crate::checkout::config_for(&store, None);
    assert!(config.block_on_failure.contains("quantity-limit"));

    // `validationUpdate` turns it off again.
    mutate(
        &state,
        r#"mutation($id: ID!) { validationUpdate(id: $id, validation: {enable: true, blockOnFailure: false}) { validation { blockOnFailure } userErrors { field message } } }"#,
        json!({ "id": created["validation"]["id"] }),
    );
    let path = cart_checkout(
        &state,
        "co-validation-ignores",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
    );
    assert_eq!(
        get(&state, &format!("{path}.json"), "co-validation-ignores").status,
        200
    );
    let (_, store) = state.snapshot("co-validation-ignores");
    assert!(
        !crate::checkout::config_for(&store, None)
            .block_on_failure
            .contains("quantity-limit")
    );
}

#[test]
fn toggling_a_payment_customization_changes_the_payment_methods_of_the_checkout() {
    let state = checkout_admin_server();
    // The app's setup: a payment customization of its Function, that hides "Bank Deposit".
    let settings = r#"{"hideName":"Bank Deposit","hideAbove":0}"#;
    let names = |state: &ServerState, session: &str| -> Vec<String> {
        let path = cart_checkout(
            state,
            session,
            json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        );
        let json = body_json(&get(state, &format!("{path}.json"), session));
        json["paymentMethods"]
            .as_array()
            .unwrap()
            .iter()
            .map(|method| method["name"].as_str().unwrap().to_string())
            .collect()
    };
    // Not set up yet: every method.
    assert!(names(&state, "co-pay-0").contains(&"Bank Deposit".to_string()));
    let created = mutate(
        &state,
        r#"mutation($customization: PaymentCustomizationInput!) { paymentCustomizationCreate(paymentCustomization: $customization) { paymentCustomization { id enabled } userErrors { field message } } }"#,
        json!({"customization": {
            "functionHandle": "payment-rules", "title": "Hide bank deposit", "enabled": true,
            "metafields": [{"namespace": "$app:config", "key": "settings", "type": "json", "value": settings}],
        }}),
    );
    let id = created["paymentCustomization"]["id"].clone();
    assert!(!names(&state, "co-pay-1").contains(&"Bank Deposit".to_string()));

    // `paymentCustomizationUpdate(enabled: false)`: the method is back.
    mutate(
        &state,
        r#"mutation($id: ID!) { paymentCustomizationUpdate(id: $id, paymentCustomization: {enabled: false}) { paymentCustomization { enabled } userErrors { field message } } }"#,
        json!({ "id": id }),
    );
    assert!(names(&state, "co-pay-2").contains(&"Bank Deposit".to_string()));
    // Enabled again: hidden again, with its settings.
    mutate(
        &state,
        r#"mutation($id: ID!) { paymentCustomizationUpdate(id: $id, paymentCustomization: {enabled: true}) { paymentCustomization { enabled } userErrors { field message } } }"#,
        json!({ "id": id }),
    );
    assert!(!names(&state, "co-pay-3").contains(&"Bank Deposit".to_string()));
    // `paymentCustomizationActivation`, then deleted: back each time.
    mutate(
        &state,
        r#"mutation($ids: [ID!]!) { paymentCustomizationActivation(ids: $ids, enabled: false) { ids userErrors { field message } } }"#,
        json!({ "ids": [id.clone()] }),
    );
    assert!(names(&state, "co-pay-4").contains(&"Bank Deposit".to_string()));
    mutate(
        &state,
        r#"mutation($id: ID!) { paymentCustomizationActivation(ids: [$id], enabled: true) { ids userErrors { field message } } }"#,
        json!({ "id": id }),
    );
    assert!(!names(&state, "co-pay-5").contains(&"Bank Deposit".to_string()));
    mutate(
        &state,
        r#"mutation($id: ID!) { paymentCustomizationDelete(id: $id) { deletedId userErrors { field message } } }"#,
        json!({ "id": id }),
    );
    assert!(names(&state, "co-pay-6").contains(&"Bank Deposit".to_string()));
}

#[test]
fn an_app_discount_the_backend_creates_runs_its_function_at_the_checkout() {
    let state = checkout_admin_server();
    let settings = r#"{"minimumQuantity":3,"percentage":10}"#;
    // `discountAutomaticAppCreate`, with its metafields.
    mutate(
        &state,
        r#"mutation($discount: DiscountAutomaticAppInput!) { discountAutomaticAppCreate(automaticAppDiscount: $discount) { automaticAppDiscount { discountId } userErrors { field message } } }"#,
        json!({"discount": {
            "title": "Volume 10", "functionHandle": "volume-discount", "discountClasses": ["PRODUCT"],
            "startsAt": "2026-01-01T00:00:00Z",
            "metafields": [{"namespace": "$app:config", "key": "settings", "type": "json", "value": settings}],
        }}),
    );
    let path = cart_checkout(
        &state,
        "co-volume",
        json!([{"variant": "TEE-BLK-M", "quantity": 3}]),
    );
    // The checkout's configuration, which comes from the store, has it.
    let config = crate::checkout::config_from_store(&state.loaded().store);
    let discount = config
        .discounts
        .iter()
        .find(|discount| discount.title == "Volume 10")
        .expect("the discount");
    assert_eq!(discount.function, "volume-discount");

    let checkout = body_json(&get(&state, &format!("{path}.json"), "co-volume"));
    assert_eq!(checkout["discounts"][0]["function"], "volume-discount");
    let line = &checkout["lines"][0];
    let subtotal = line["subtotal"]["amount"].as_f64().unwrap();
    let total = line["total"]["amount"].as_f64().unwrap();
    assert!((total - subtotal * 0.9).abs() < 0.01, "{subtotal} {total}");
    // Its Function read the metafields the backend gave it.
    let input = &run_of(&checkout, "volume-discount")["input"];
    assert_eq!(input["discount"]["metafield"]["value"], settings, "{input}");
}

/// A code of Shopify's own discount types that the backend creates (`discountCodeBasicCreate`)
/// is taken at the checkout without a Function: the order placed carries it, its usage count
/// moves, and once its usage limit is reached, the next checkout refuses it.
#[test]
fn a_basic_code_the_backend_creates_applies_at_the_checkout_and_counts_its_uses() {
    let state = checkout_admin_server();
    mutate(
        &state,
        r#"mutation($discount: DiscountCodeBasicInput!) { discountCodeBasicCreate(basicCodeDiscount: $discount) { codeDiscountNode { id } userErrors { field message } } }"#,
        json!({"discount": {
            "title": "Loyalty voucher", "code": "LOYAL-5", "usageLimit": 1,
            "customerGets": {"value": {"discountAmount": {"amount": "5.00"}}, "items": {"all": true}},
        }}),
    );
    let enter = |session: &str| {
        let path = cart_checkout(
            &state,
            session,
            json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        );
        let reply = operations(
            &state,
            &path,
            session,
            json!([{"type": "discountCode", "action": "add", "code": "loyal-5"}]),
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        (path, body_json(&reply))
    };
    let (path, checkout) = enter("co-loyal");
    assert_eq!(
        checkout["discountCodes"],
        json!([{"code": "loyal-5", "applicable": true}]),
        "{checkout}"
    );
    assert_eq!(checkout["discounts"][0]["code"], "LOYAL-5");
    assert_eq!(checkout["discounts"][0]["class"], "ORDER");
    assert_eq!(checkout["cost"]["discounts"]["amount"], 5.0);

    assert_eq!(
        operations(&state, &path, "co-loyal", buyer_details()).status,
        200
    );
    let reply = post(&state, &format!("{path}/complete"), json!({}), "co-loyal");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    let order = body_json(&reply)["order"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // The backend reads the order with `read_orders`.
    let installed = admin_request(
        &state,
        "POST",
        "/__lsf/admin/install",
        r#"{"scopes": "write_discounts,read_orders"}"#,
    );
    assert_eq!(installed.status, 200, "{}", text(&installed));
    let answer = admin_query(
        &state,
        &format!(
            r#"{{ order(id: "{order}") {{ discountCodes discountApplications(first: 1) {{ nodes {{ ... on DiscountCodeApplication {{ code }} }} }} }}
               codeDiscountNodeByCode(code: "LOYAL-5") {{ codeDiscount {{ ... on DiscountCodeBasic {{ asyncUsageCount }} }} }} }}"#
        ),
    );
    assert_eq!(
        answer["data"]["order"]["discountApplications"]["nodes"],
        json!([{"code": "LOYAL-5"}]),
        "{answer}"
    );
    assert_eq!(answer["data"]["order"]["discountCodes"], json!(["loyal-5"]));
    assert_eq!(
        answer["data"]["codeDiscountNodeByCode"]["codeDiscount"]["asyncUsageCount"],
        1
    );

    // Used up: refused, with why.
    let (_, again) = enter("co-loyal-again");
    assert_eq!(again["discountCodes"][0]["applicable"], false, "{again}");
    assert_eq!(
        again["discountCodes"][0]["reason"],
        "it reached its usage limit: used 1 times of 1"
    );
    assert_eq!(again["discounts"], json!([]));
}

/// Two buyers price their orders with the last use of a code; the one whose order is recorded
/// second does not get it: the completion is refused, and the checkout shows the code refused
/// and the price without it.
#[test]
fn the_last_use_of_a_code_goes_to_one_of_two_completions_at_once() {
    let state = checkout_admin_server();
    mutate(
        &state,
        r#"mutation($discount: DiscountCodeBasicInput!) { discountCodeBasicCreate(basicCodeDiscount: $discount) { codeDiscountNode { id } userErrors { field message } } }"#,
        json!({"discount": {
            "title": "Last voucher", "code": "LAST-ONE", "usageLimit": 1,
            "customerGets": {"value": {"discountAmount": {"amount": "5.00"}}, "items": {"all": true}},
        }}),
    );
    let ready = |session: &str| {
        let path = cart_checkout(
            &state,
            session,
            json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        );
        let reply = operations(
            &state,
            &path,
            session,
            json!([{"type": "discountCode", "action": "add", "code": "LAST-ONE"}]),
        );
        assert_eq!(body_json(&reply)["discountCodes"][0]["applicable"], true);
        assert_eq!(
            operations(&state, &path, session, buyer_details()).status,
            200
        );
        path
    };
    let first = ready("co-last-first");
    let second = ready("co-last-second");
    let token = first.rsplit('/').next().unwrap();

    // The second buyer completes after the first completion priced its order, before it is
    // recorded.
    let reply = super::super::checkout::place_order(
        &state,
        &request(
            "POST",
            &format!("{first}/complete"),
            json!({}),
            "co-last-first",
        ),
        "co-last-first",
        token,
        || {},
        || {
            let reply = post(
                &state,
                &format!("{second}/complete"),
                json!({}),
                "co-last-second",
            );
            assert_eq!(reply.status, 200, "{}", text(&reply));
        },
    );
    assert_eq!(reply.status, 422, "{}", text(&reply));
    let checkout = body_json(&reply);
    assert_eq!(checkout["status"], "open");
    assert_eq!(
        checkout["discountCodes"],
        json!([{"code": "LAST-ONE", "applicable": false,
                "reason": "it reached its usage limit: used 1 times of 1"}]),
        "{checkout}"
    );
    assert_eq!(checkout["cost"]["discounts"]["amount"], 0.0);
    assert_eq!(
        checkout["errors"],
        json!([{"message": "LAST-ONE discount code isn't available to you right now",
                "target": "$.cart.discountCodes", "function": null}]),
        "{checkout}"
    );
    assert_eq!(state.placed_orders(), 1);
    let answer = admin_query(
        &state,
        r#"{ codeDiscountNodeByCode(code: "LAST-ONE") { codeDiscount { ... on DiscountCodeBasic { asyncUsageCount } } } }"#,
    );
    assert_eq!(
        answer["data"]["codeDiscountNodeByCode"]["codeDiscount"]["asyncUsageCount"], 1,
        "{answer}"
    );

    // The buyer completes what they now see, without the code's 5.00.
    let reply = post(
        &state,
        &format!("{first}/complete"),
        json!({}),
        "co-last-first",
    );
    assert_eq!(reply.status, 200, "{}", text(&reply));
    assert_eq!(state.placed_orders(), 2);
}

#[test]
fn a_session_with_its_own_data_sees_what_the_backend_changes() {
    let state = admin_server();
    let own_data = |session: &str, title: &str| {
        let reply = state.dispatch(&request(
            "PUT",
            "/__lsf/session",
            json!({"data": {"products": [{"title": title, "price": 1500}]}}),
            session,
        ));
        assert_eq!(reply.status, 200, "{}", text(&reply));
    };
    own_data("own", "Session Scarf");
    retitle(&state, "ceramic-mug", "Stoneware Mug");

    // Its data, on top of what the backend changed.
    assert_eq!(title_on_page(&state, "ceramic-mug", "own"), "Stoneware Mug");
    assert_eq!(
        title_on_page(&state, "session-scarf", "own"),
        "Session Scarf"
    );
    // Another session sees the change, not the data.
    assert_eq!(
        title_on_page(&state, "ceramic-mug", "other"),
        "Stoneware Mug"
    );
    assert_eq!(get(&state, "/products/session-scarf", "other").status, 404);
    // Data set after the change goes on top of it too.
    own_data("late", "Late Scarf");
    assert_eq!(
        title_on_page(&state, "ceramic-mug", "late"),
        "Stoneware Mug"
    );
    assert_eq!(title_on_page(&state, "late-scarf", "late"), "Late Scarf");
    // And again after the next change.
    retitle(&state, "ceramic-mug", "Clay Mug");
    assert_eq!(title_on_page(&state, "ceramic-mug", "own"), "Clay Mug");
    assert_eq!(
        title_on_page(&state, "session-scarf", "own"),
        "Session Scarf"
    );
}

#[test]
fn open_pages_reload_when_the_backend_changes_the_store() {
    let state = admin_server();
    let token = state.observe_changes();
    // A query changes nothing.
    admin_query(&state, "{ shop { name } }");
    assert_eq!(state.observe_changes(), token);

    retitle(&state, "ceramic-mug", "Stoneware Mug");
    let changed = state.observe_changes();
    assert_ne!(changed, token);
    // The store was built before the token changed: the page that reloads finds it.
    assert_eq!(
        state
            .loaded()
            .store
            .product_by_handle("ceramic-mug")
            .unwrap()
            .title,
        "Stoneware Mug"
    );
    assert_eq!(state.observe_changes(), changed);
}

#[test]
fn a_change_of_the_files_and_a_reset_bring_the_files_back() {
    // `--watch` over a data directory of the demo store.
    let data = ScenarioData::new("sync-watch", &[]);
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../theme/tests/fixtures/theme");
    let app = App::open(&theme, Some(&data.0), Revalidate::Never, &[]).expect("the theme");
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            polaris: Default::default(),
            admin: Some(Default::default()),
            live_reload: false,
            watch: true,
            quiet: true,
            compress: false,
            minify: false,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    retitle(&state, "ceramic-mug", "Stoneware Mug");
    assert_eq!(title_on_page(&state, "ceramic-mug", "w"), "Stoneware Mug");
    assert_eq!(sync_status(&state)["generation"], 1);

    // The files change: the shops are seeded again, and the store is the new files.
    let pages = data.0.join("pages.json");
    let content = std::fs::read_to_string(&pages).unwrap();
    std::fs::write(&pages, format!("{content}\n")).unwrap();
    state.observe_changes();
    assert_eq!(title_on_page(&state, "ceramic-mug", "w"), "Ceramic Mug");
    let loaded = state.loaded();
    assert!(Arc::ptr_eq(&loaded.store, &loaded.base));
    assert!(loaded.synced.overlay.is_none());
    assert_eq!(sync_status(&state)["overlay"], Json::Null);

    // `DELETE /__lsf/admin` does as much.
    retitle(&state, "ceramic-mug", "Clay Mug");
    assert_eq!(title_on_page(&state, "ceramic-mug", "w"), "Clay Mug");
    assert_eq!(
        admin_request(&state, "DELETE", "/__lsf/admin", "").status,
        200
    );
    assert_eq!(title_on_page(&state, "ceramic-mug", "w"), "Ceramic Mug");
    let loaded = state.loaded();
    assert!(Arc::ptr_eq(&loaded.store, &loaded.base));
}

/// `--watch`: a checkout scenario added or changed is read again on top of the same store data,
/// and the shops of the Admin API keep what the backend changed (here a payment customization
/// and a product's title); a change of the store's own data seeds them again.
#[test]
fn a_scenario_added_while_watching_keeps_what_the_backend_changed() {
    let data = ScenarioData::new("sync-scenario-watch", &[]);
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../theme/tests/fixtures/theme");
    let app = App::open(&theme, Some(&data.0), Revalidate::Never, &[]).expect("the theme");
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            polaris: Default::default(),
            admin: Some(Default::default()),
            live_reload: false,
            watch: true,
            quiet: true,
            compress: false,
            minify: false,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: Some(crate::checkout::CheckoutOptions {
                app_dir: fixture_app(),
                extensions: Vec::new(),
                variant: crate::checkout::Variant::Standard,
                standalone: false,
                no_functions: false,
            }),
        },
    );
    let installed = admin_request(
        &state,
        "POST",
        "/__lsf/admin/install",
        r#"{"scopes": "write_products,write_payment_customizations"}"#,
    );
    assert_eq!(installed.status, 200, "{}", text(&installed));
    mutate(
        &state,
        r#"mutation($customization: PaymentCustomizationInput!) { paymentCustomizationCreate(paymentCustomization: $customization) { paymentCustomization { id } userErrors { field message } } }"#,
        json!({"customization": {
            "functionHandle": "payment-rules", "title": "Hide bank deposit", "enabled": true,
            "metafields": [{"namespace": "$app:config", "key": "settings", "type": "json", "value": r#"{"hideName":"Bank Deposit","hideAbove":0}"#}],
        }}),
    );
    retitle(&state, "ceramic-mug", "Stoneware Mug");
    let bank = |state: &ServerState, session: &str| -> bool {
        let path = cart_checkout(
            state,
            session,
            json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        );
        body_json(&get(state, &format!("{path}.json"), session))["paymentMethods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|method| method["name"] == "Bank Deposit")
    };
    assert!(!bank(&state, "sw-0"));
    assert_eq!(title_on_page(&state, "ceramic-mug", "sw"), "Stoneware Mug");

    // A scenario added, the README rewritten: the scenarios are read again, nothing else moves.
    std::fs::write(
        data.0.join("checkouts/added.json"),
        r#"{"title": "Added while watching"}"#,
    )
    .unwrap();
    std::fs::write(data.0.join("README.md"), "notes").unwrap();
    state.observe_changes();
    assert!(
        state.loaded().store.scenario("added").is_some(),
        "the new scenario is read"
    );
    assert!(!bank(&state, "sw-1"), "the payment customization is kept");
    assert_eq!(title_on_page(&state, "ceramic-mug", "sw"), "Stoneware Mug");
    assert_eq!(
        get(&state, "/checkout?scenario=added", "sw-2").status,
        302,
        "it opens"
    );
    // Changed: the same.
    std::fs::write(
        data.0.join("checkouts/added.json"),
        r#"{"title": "Changed while watching"}"#,
    )
    .unwrap();
    state.observe_changes();
    assert_eq!(
        state.loaded().store.scenario("added").unwrap().title,
        "Changed while watching"
    );
    assert!(!bank(&state, "sw-3"));

    // The store's own data changes: the shops are seeded again, and the backend's changes go.
    let pages = data.0.join("pages.json");
    let content = std::fs::read_to_string(&pages).unwrap();
    std::fs::write(&pages, format!("{content}\n")).unwrap();
    state.observe_changes();
    assert_eq!(title_on_page(&state, "ceramic-mug", "sw"), "Ceramic Mug");
    assert!(bank(&state, "sw-4"));
}

#[test]
fn an_order_placed_at_the_checkout_does_not_build_the_store_again() {
    let state = checkout_admin_server();
    let admin = state.admin.as_ref().unwrap();
    // The backend changed the store first: the order must not undo it, nor build it again.
    retitle(&state, "ceramic-mug", "Stoneware Mug");
    assert_eq!(get(&state, "/", "co-order").status, 200);
    let before = state.loaded();
    assert_eq!(before.synced.generation, 1);
    let changes = admin.api.store_changes();

    let path = cart_checkout(
        &state,
        "co-order",
        json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
    );
    // A customer of the data, whose orders and amount spent change in the Admin API.
    log_in_as(&state, &path, "co-order", json!("jane.doe@example.com"));
    assert_eq!(
        operations(&state, &path, "co-order", buyer_details()).status,
        200
    );
    let reply: Reply = post(&state, &format!("{path}/complete"), json!({}), "co-order");
    assert_eq!(reply.status, 200, "{}", text(&reply));
    // The order is in the default shop.
    assert!(admin.api.store_changes() > changes);

    // The next page asks for the overlay again, finds the same one, and keeps the store.
    assert_eq!(
        title_on_page(&state, "ceramic-mug", "co-order"),
        "Stoneware Mug"
    );
    let after = state.loaded();
    assert!(Arc::ptr_eq(&before.store, &after.store));
    assert_eq!(after.synced.generation, 1);
    let status = sync_status(&state);
    assert_eq!(
        (&status["generation"], &status["pending"]),
        (&json!(1), &json!(false)),
        "{status}"
    );
}

#[test]
fn what_the_store_data_refuses_of_a_change_is_told_and_the_rest_is_served() {
    let state = admin_server();
    // Two companies of one name: Shopify takes them (only an external id is unique), the store
    // data names a company by its name, and says so. Neither is lost (the overlay once let the
    // second replace the first, silently).
    let twins: Vec<Json> = (0..2)
        .map(|_| {
            mutate(
                &state,
                r#"mutation { companyCreate(input: {company: {name: "Twin Bistro"}}) { company { id } userErrors { field message } } }"#,
                json!({}),
            )["company"]["id"]
                .clone()
        })
        .collect();
    retitle(&state, "ceramic-mug", "Stoneware Mug");
    assert_eq!(title_on_page(&state, "ceramic-mug", "bad"), "Stoneware Mug");
    let loaded = state.loaded();
    assert!(
        !loaded.synced.diagnostics.is_empty(),
        "the store data says something of the overlay"
    );
    let told = &sync_status(&state)["built"]["diagnostics"];
    assert_eq!(
        told.as_array().map(Vec::len),
        Some(loaded.synced.diagnostics.items.len()),
        "{told}"
    );
    assert_eq!(
        told[0]["file"],
        lsf_app::admin::sync::OVERLAY_NAME,
        "{told}"
    );
    assert_eq!(told[0]["code"], "duplicate_company", "{told}");
    // The store, built from the overlay with the problem told, has both.
    let kept: Vec<String> = loaded
        .store
        .companies
        .iter()
        .filter(|company| company.name == "Twin Bistro")
        .map(|company| format!("gid://shopify/Company/{}", company.id))
        .collect();
    assert_eq!(kept, twins);
}

#[test]
fn a_burst_of_mutations_is_built_once_and_pages_see_whole_stores() {
    let state = admin_server();
    // Two products renamed by one request: a page sees both changes or neither.
    let tee = product_gid(&state, "organic-cotton-t-shirt");
    let mug = product_gid(&state, "ceramic-mug");
    let round = |n: usize| {
        let updated = admin_query(
            &state,
            &format!(
                r#"mutation {{ a: productUpdate(product: {{id: "{tee}", title: "Tee {n}"}}) {{ userErrors {{ message }} }} b: productUpdate(product: {{id: "{mug}", title: "Mug {n}"}}) {{ userErrors {{ message }} }} }}"#
            ),
        );
        assert_eq!(updated["data"]["b"]["userErrors"], json!([]), "{updated}");
    };
    for n in 0..5 {
        round(n);
    }
    // Five requests, one overlay, one build: by the page that follows them.
    assert_eq!(title_on_page(&state, "ceramic-mug", "burst"), "Mug 4");
    let status = sync_status(&state);
    assert_eq!(status["generation"], 1, "{status}");
    assert_eq!(status["built"]["generation"], 1, "{status}");

    // Pages rendered while the backend goes on.
    let rounds = |page: &str| -> Option<(String, String)> {
        let number = |name: &str| {
            let start = page.find(&format!(">{name} "))? + name.len() + 2;
            let end = start + page[start..].find('<')?;
            Some(page[start..end].to_string())
        };
        Some((number("Tee")?, number("Mug")?))
    };
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for n in 5..30 {
                round(n);
            }
        });
        for reader in 0..3 {
            let state = &state;
            scope.spawn(move || {
                for _ in 0..30 {
                    let page = text(&get(state, "/collections/frontpage", &format!("r{reader}")));
                    let (tee, mug) = rounds(&page).unwrap_or_else(|| panic!("{page}"));
                    assert_eq!(tee, mug, "a page saw half a request's changes");
                }
            });
        }
    });
    assert_eq!(title_on_page(&state, "ceramic-mug", "burst"), "Mug 29");
}

/// Uninstalling the app removes its Functions with every object that runs them: the shop's
/// checkout runs none of them, and a reinstalled app finds no owner until it creates one again.
#[test]
fn uninstalling_the_app_stops_its_functions_and_a_reinstall_finds_no_owner() {
    let state = checkout_admin_server();
    let settings = r#"{"hideName":"Bank Deposit","hideAbove":0}"#;
    let customize = |state: &ServerState| {
        mutate(
            state,
            r#"mutation($customization: PaymentCustomizationInput!) { paymentCustomizationCreate(paymentCustomization: $customization) { paymentCustomization { id } userErrors { field message } } }"#,
            json!({"customization": {
                "functionHandle": "payment-rules", "title": "Hide bank deposit", "enabled": true,
                "metafields": [{"namespace": "$app:config", "key": "settings", "type": "json", "value": settings}],
            }}),
        )
    };
    customize(&state);
    mutate(
        &state,
        r#"mutation($discount: DiscountAutomaticAppInput!) { discountAutomaticAppCreate(automaticAppDiscount: $discount) { automaticAppDiscount { discountId } userErrors { field message } } }"#,
        json!({"discount": {
            "title": "Volume 10", "functionHandle": "volume-discount", "discountClasses": ["PRODUCT"],
            "startsAt": "2026-01-01T00:00:00Z",
            "metafields": [{"namespace": "$app:config", "key": "settings", "type": "json", "value": r#"{"minimumQuantity":1,"percentage":10}"#}],
        }}),
    );
    let checkout = |state: &ServerState, session: &str| -> Json {
        let path = cart_checkout(
            state,
            session,
            json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        );
        body_json(&get(state, &format!("{path}.json"), session))
    };
    let bank = |checkout: &Json| {
        checkout["paymentMethods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|method| method["name"] == "Bank Deposit")
    };
    let idle = |checkout: &Json, handle: &str| -> String {
        checkout["idle"]
            .as_array()
            .unwrap()
            .iter()
            .find(|idle| idle["handle"] == handle)
            .map(|idle| idle["reason"].as_str().unwrap().to_string())
            .unwrap_or_else(|| panic!("{handle} ran: {}", checkout["idle"]))
    };
    let set_up = checkout(&state, "co-un-0");
    assert!(!bank(&set_up));
    assert_eq!(set_up["discounts"][0]["function"], "volume-discount");

    let uninstalled = admin_request(&state, "POST", "/__lsf/admin/uninstall", "{}");
    assert_eq!(uninstalled.status, 200, "{}", text(&uninstalled));
    assert_eq!(body_json(&uninstalled)["removedFunctionOwners"], json!(2));
    assert!(state.app_uninstalled());
    let gone = checkout(&state, "co-un-1");
    assert!(bank(&gone), "{}", gone["paymentMethods"]);
    assert_eq!(gone["discounts"], json!([]));
    assert_eq!(gone["runs"], json!([]));
    for handle in ["payment-rules", "volume-discount"] {
        assert!(
            idle(&gone, handle).contains("the app is not installed on the shop"),
            "{}",
            gone["idle"]
        );
    }

    // Installed again: Shopify gives the app no owner back. It creates one.
    let installed = admin_request(
        &state,
        "POST",
        "/__lsf/admin/install",
        r#"{"scopes": "write_products,write_discounts,write_payment_customizations"}"#,
    );
    assert_eq!(installed.status, 200, "{}", text(&installed));
    assert!(!state.app_uninstalled());
    let again = checkout(&state, "co-un-2");
    assert!(bank(&again));
    assert_eq!(again["discounts"], json!([]));
    assert!(
        idle(&again, "payment-rules").contains("the shop has no payment customization"),
        "{}",
        again["idle"]
    );
    customize(&state);
    assert!(!bank(&checkout(&state, "co-un-3")));
}

/// The handles of the UI extensions a page's state loads, and of those it holds back because the
/// app is not installed on the shop.
fn extension_handles(state: &Json) -> (Vec<String>, Vec<String>) {
    let handles = |list: &Json| -> Vec<String> {
        list.as_array()
            .into_iter()
            .flatten()
            .map(|extension| extension["handle"].as_str().unwrap().to_string())
            .collect()
    };
    (
        handles(&state["extensions"]),
        handles(&state["uninstalled"]["extensions"]),
    )
}

/// Uninstalling the app removes its UI extensions from the checkout and the thank-you page, as
/// Shopify removes an uninstalled app's extensions: the state says why, the home page too, and
/// a reinstall brings them back (Shopify restores the checkout's active ones within 24 hours; lsf
/// has no checkout editor, so always).
#[test]
fn uninstalling_the_app_hides_its_ui_extensions_until_it_is_installed_again() {
    let state = checkout_admin_server();
    let both = vec!["gift-message".to_string(), "order-notice".to_string()];
    let open = |state: &ServerState, session: &str| -> (String, Json) {
        let path = cart_checkout(
            state,
            session,
            json!([{"variant": "TEE-BLK-M", "quantity": 1}]),
        );
        let json = body_json(&get(state, &format!("{path}.json"), session));
        (path, json)
    };
    let (_, installed) = open(&state, "ui-un-0");
    assert_eq!(extension_handles(&installed), (both.clone(), vec![]));
    assert_eq!(installed["uninstalled"], Json::Null);
    // An order placed while the app is installed: its thank-you page follows the app.
    let (placed, _) = open(&state, "ui-un-placed");
    assert_eq!(
        operations(&state, &placed, "ui-un-placed", buyer_details()).status,
        200
    );
    let completed = post(
        &state,
        &format!("{placed}/complete"),
        json!({}),
        "ui-un-placed",
    );
    assert_eq!(completed.status, 200, "{}", text(&completed));
    let thanks =
        |state: &ServerState| body_json(&get(state, &format!("{placed}.json"), "ui-un-placed"));
    assert_eq!(thanks(&state)["status"], "completed");
    assert_eq!(extension_handles(&thanks(&state)).0, both);

    let uninstalled = admin_request(&state, "POST", "/__lsf/admin/uninstall", "{}");
    assert_eq!(uninstalled.status, 200, "{}", text(&uninstalled));
    let (path, gone) = open(&state, "ui-un-1");
    assert_eq!(extension_handles(&gone), (vec![], both.clone()));
    let reason = gone["uninstalled"]["reason"].as_str().unwrap();
    assert!(
        reason.starts_with(
            "the app is not installed on the shop: uninstalling it removed its UI extensions"
        ),
        "{reason}"
    );
    // The page itself holds the same state.
    let page = text(&get(&state, &path, "ui-un-1"));
    assert!(page.contains("\"extensions\":[]"), "{page}");
    assert!(
        page.contains("the app is not installed on the shop"),
        "{page}"
    );
    let thanked = thanks(&state);
    assert_eq!(extension_handles(&thanked), (vec![], both.clone()));
    let thank_you_page = text(&get(&state, &format!("{placed}/thank-you"), "ui-un-placed"));
    assert!(
        thank_you_page.contains("\"extensions\":[]"),
        "{thank_you_page}"
    );

    // The home page says it, and marks each extension.
    let home = body_json(&get(&state, "/__lsf/checkouts?format=json", "ui-un-home"));
    assert_eq!(home["installed"], false, "{home}");
    assert!(
        home["uninstalled"]
            .as_str()
            .unwrap()
            .starts_with("the app is not installed on the shop")
    );
    assert!(
        home["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|problem| problem["kind"] == "app"
                && problem["message"]
                    .as_str()
                    .unwrap()
                    .starts_with("the app is not installed on the shop")),
        "{}",
        home["problems"]
    );
    for extension in home["uiExtensions"].as_array().unwrap() {
        assert_eq!(extension["installed"], false, "{extension}");
        assert_eq!(extension["status"], "off", "{extension}");
    }
    let html = text(&get(&state, "/__lsf/checkouts", "ui-un-home"));
    assert!(html.contains("data-banner=\"uninstalled\""), "{html}");
    assert!(
        html.contains("The app is not installed on the shop"),
        "{html}"
    );
    assert!(html.contains("app not installed"), "{html}");

    // Installed again: they render again, the thank-you page's too.
    let installed = admin_request(&state, "POST", "/__lsf/admin/install", "{}");
    assert_eq!(installed.status, 200, "{}", text(&installed));
    let (_, back) = open(&state, "ui-un-2");
    assert_eq!(extension_handles(&back), (both.clone(), vec![]));
    assert_eq!(back["uninstalled"], Json::Null);
    assert_eq!(extension_handles(&thanks(&state)), (both, vec![]));
    let home = body_json(&get(&state, "/__lsf/checkouts?format=json", "ui-un-home"));
    assert_eq!(home["installed"], true);
    assert_eq!(home["uninstalled"], Json::Null);
    assert!(
        !text(&get(&state, "/__lsf/checkouts", "ui-un-home"))
            .contains("data-banner=\"uninstalled\"")
    );
}

#[test]
fn a_url_redirect_the_backend_creates_sends_its_path_to_its_target() {
    let state = admin_server();
    assert_eq!(get(&state, "/old-mug", "sync").status, 404);
    let created = mutate(
        &state,
        "mutation($redirect: UrlRedirectInput!) { urlRedirectCreate(urlRedirect: $redirect) { urlRedirect { id } userErrors { field message } } }",
        json!({"redirect": {"path": "/old-mug", "target": "/products/ceramic-mug"}}),
    );
    let moved = get(&state, "/old-mug", "sync");
    assert_eq!(moved.status, 301);
    assert_eq!(header(&moved, "location"), Some("/products/ceramic-mug"));

    let id = created["urlRedirect"]["id"].as_str().unwrap();
    mutate(
        &state,
        "mutation($id: ID!) { urlRedirectDelete(id: $id) { deletedUrlRedirectId userErrors { field message } } }",
        json!({"id": id}),
    );
    assert_eq!(get(&state, "/old-mug", "sync").status, 404);
}
