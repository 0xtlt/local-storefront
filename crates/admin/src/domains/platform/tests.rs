//! The app platform through queries over the demo store and the app of
//! `tests/fixtures/platform-app`. What the tokens do with the effects (an uninstall, a
//! delegate token) is tested through `AdminApi` in `tests/platform.rs`.

use serde_json::{Value, json};

use super::access::{AuthEffect, effects};
use crate::domains::storefront::tests::{run, shop, topics};
use crate::state::ShopState;

/// Grants the installation scopes beyond its required ones, as an approval of optional scopes
/// does.
fn grant(shop: &mut ShopState, scopes: &[&str]) {
    let installation = shop.keys("AppInstallation").next().unwrap().to_string();
    let record = shop.get_mut(&installation).unwrap();
    let mut handles = record["accessScopes"].as_array().unwrap().clone();
    for scope in scopes {
        handles.push(json!({"handle": scope}));
    }
    record.insert("accessScopes".into(), Value::Array(handles));
}

fn installation_scopes(shop: &mut ShopState) -> Vec<String> {
    let answer = run(
        shop,
        "{ currentAppInstallation { accessScopes { handle } } }",
        json!({}),
    );
    answer["data"]["currentAppInstallation"]["accessScopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|scope| scope["handle"].as_str().unwrap().to_string())
        .collect()
}

const REVOKE: &str = "mutation($scopes: [String!]!) {
    appRevokeAccessScopes(scopes: $scopes) { revoked { handle description } userErrors { field message code } }
}";

#[test]
fn optional_scopes_are_revoked_and_the_tokens_hear_of_it() {
    let mut shop = shop();
    grant(&mut shop, &["write_customers", "read_discounts"]);
    let answer = run(
        &mut shop,
        REVOKE,
        json!({"scopes": ["read_discounts", "write_orders"]}),
    );
    let payload = &answer["data"]["appRevokeAccessScopes"];
    assert_eq!(payload["userErrors"], json!([]), "{answer}");
    // `write_orders` was declared but never granted: nothing to revoke.
    assert_eq!(
        payload["revoked"],
        json!([{"handle": "read_discounts", "description": "Read discounts"}])
    );
    let scopes = installation_scopes(&mut shop);
    assert!(!scopes.contains(&"read_discounts".to_string()));
    assert!(scopes.contains(&"write_customers".to_string()));
    let events = shop.take_events();
    assert_eq!(events[0].topic, "app/scopes_update");
    assert_eq!(effects(&events), [AuthEffect::Scopes(scopes)]);
}

#[test]
fn required_undeclared_implied_and_unknown_scopes_are_not_revoked() {
    let mut shop = shop();
    grant(&mut shop, &["write_customers", "read_customers"]);
    let before = installation_scopes(&mut shop);
    for (scopes, code) in [
        (json!(["read_products"]), "CANNOT_REVOKE_REQUIRED_SCOPES"),
        (json!(["write_themes"]), "CANNOT_REVOKE_UNDECLARED_SCOPES"),
        (json!(["read_customers"]), "CANNOT_REVOKE_UNDECLARED_SCOPES"),
        (json!(["read_nonsense_thing"]), "UNKNOWN_SCOPES"),
        (json!(["nonsense"]), "UNKNOWN_SCOPES"),
    ] {
        let answer = run(&mut shop, REVOKE, json!({"scopes": scopes}));
        let payload = &answer["data"]["appRevokeAccessScopes"];
        assert_eq!(payload["revoked"], Value::Null);
        assert_eq!(payload["userErrors"][0]["code"], code, "{scopes}: {answer}");
        assert_eq!(payload["userErrors"][0]["field"], json!(["scopes"]));
    }
    assert_eq!(installation_scopes(&mut shop), before);
    assert!(topics(&mut shop).is_empty());
}

#[test]
fn a_write_scope_is_downgraded_to_its_read_scope_in_2027_01() {
    let mut shop = shop();
    grant(&mut shop, &["write_customers", "write_orders"]);
    let version = crate::version::ApiVersion::get("2027-01").unwrap();
    let mut resolver =
        crate::resolve::ShopResolver::new(&mut shop, crate::resolve::Registry::standard())
            .with_version(version);
    let request = crate::graphql::Request::new(
        "mutation { appDowngradeAccessScopes(scopes: [\"write_customers\", \"write_products\"]) { downgraded { handle } userErrors { code } } }",
    );
    let answer = crate::graphql::run(version.schema(), &mut resolver, &request, None).to_json();
    assert_eq!(
        answer["data"]["appDowngradeAccessScopes"]["userErrors"][0]["code"],
        "CANNOT_DOWNGRADE_REQUIRED_SCOPES",
        "{answer}"
    );
    let mut resolver =
        crate::resolve::ShopResolver::new(&mut shop, crate::resolve::Registry::standard())
            .with_version(version);
    let request = crate::graphql::Request::new(
        "mutation { appDowngradeAccessScopes(scopes: [\"write_customers\"]) { downgraded { handle } userErrors { code } } }",
    );
    let answer = crate::graphql::run(version.schema(), &mut resolver, &request, None).to_json();
    assert_eq!(
        answer["data"]["appDowngradeAccessScopes"]["downgraded"],
        json!([{"handle": "write_customers"}]),
        "{answer}"
    );
    let scopes = installation_scopes(&mut shop);
    assert!(scopes.contains(&"read_customers".to_string()));
    assert!(!scopes.contains(&"write_customers".to_string()));
    assert!(scopes.contains(&"write_orders".to_string()));
}

#[test]
fn the_app_uninstalls_itself() {
    let mut shop = shop();
    let answer = run(
        &mut shop,
        "mutation { appUninstall { app { id title } userErrors { message } } }",
        json!({}),
    );
    assert_eq!(
        answer["data"]["appUninstall"]["app"]["title"], "Platform fixtures",
        "{answer}"
    );
    let events = shop.take_events();
    let topics: Vec<&str> = events.iter().map(|event| event.topic.as_str()).collect();
    assert_eq!(topics, ["app/uninstalled", super::access::AUTH]);
    assert_eq!(effects(&events), [AuthEffect::Uninstall]);
}

#[test]
fn a_delegate_token_takes_a_subset_of_the_parents_scopes() {
    let mut shop = shop();
    let parent = crate::auth::Grant {
        shop: shop.domain().to_string(),
        client_id: "22222222222222222222222222222222".to_string(),
        scopes: vec!["read_products".to_string(), "write_orders".to_string()],
        mode: crate::auth::tokens::AccessMode::Offline,
        user: None,
        issued_at: chrono::Utc::now(),
        expires_at: None,
        source: crate::auth::tokens::GrantSource::Install,
    };
    let query = "mutation($input: DelegateAccessTokenInput!) { delegateAccessTokenCreate(input: $input) { delegateAccessToken { accessToken accessScopes expiresIn } shop { id } userErrors { field message code } } }";
    let run_as = |shop: &mut ShopState, grant: &crate::auth::Grant, input: Value| {
        let mut resolver =
            crate::resolve::ShopResolver::new(shop, crate::resolve::Registry::standard())
                .with_grant(grant);
        let request = crate::graphql::Request::new(query).with_variables(json!({"input": input}));
        crate::graphql::run(crate::schema::admin(), &mut resolver, &request, None).to_json()
    };
    for (input, code) in [
        (json!({"delegateAccessScope": []}), "EMPTY_ACCESS_SCOPE"),
        (
            json!({"delegateAccessScope": ["write_customers"]}),
            "UNKNOWN_SCOPES",
        ),
        (
            json!({"delegateAccessScope": ["read_products"], "expiresIn": 0}),
            "NEGATIVE_EXPIRES_IN",
        ),
    ] {
        let answer = run_as(&mut shop, &parent, input);
        assert_eq!(
            answer["data"]["delegateAccessTokenCreate"]["userErrors"][0]["code"], code,
            "{answer}"
        );
    }
    assert!(shop.take_events().is_empty());
    let answer = run_as(
        &mut shop,
        &parent,
        json!({"delegateAccessScope": ["read_products"], "expiresIn": 3600}),
    );
    let token = &answer["data"]["delegateAccessTokenCreate"]["delegateAccessToken"];
    let secret = token["accessToken"].as_str().unwrap().to_string();
    assert!(
        secret.starts_with("shppa_") && secret.len() == 38,
        "{answer}"
    );
    assert_eq!(token["accessScopes"], json!(["read_products"]));
    assert_eq!(token["expiresIn"], 3600);
    let events = shop.take_events();
    let AuthEffect::Delegate {
        token,
        scopes,
        expires_at,
    } = &effects(&events)[0]
    else {
        panic!("not a delegate effect");
    };
    assert_eq!(
        (token, scopes.as_slice()),
        (&secret, ["read_products".to_string()].as_slice())
    );
    assert!(expires_at.is_some());

    // A delegate cannot make delegates; destroying one is said once.
    let mut delegate = parent.clone();
    delegate.source = crate::auth::tokens::GrantSource::Delegate;
    let answer = run_as(
        &mut shop,
        &delegate,
        json!({"delegateAccessScope": ["read_products"]}),
    );
    assert_eq!(
        answer["data"]["delegateAccessTokenCreate"]["userErrors"][0]["code"],
        "DELEGATE_ACCESS_TOKEN"
    );
    let destroy = "mutation($token: String!) { delegateAccessTokenDestroy(accessToken: $token) { status userErrors { code } } }";
    let destroyed = run(&mut shop, destroy, json!({"token": secret}));
    assert_eq!(
        destroyed["data"]["delegateAccessTokenDestroy"]["status"],
        true
    );
    let again = run(&mut shop, destroy, json!({"token": secret}));
    assert_eq!(
        again["data"]["delegateAccessTokenDestroy"]["userErrors"][0]["code"],
        "ACCESS_TOKEN_NOT_FOUND"
    );
    let parent_token = run(&mut shop, destroy, json!({"token": "shpat_0123"}));
    assert_eq!(
        parent_token["data"]["delegateAccessTokenDestroy"]["userErrors"][0]["code"],
        "CAN_ONLY_DELETE_DELEGATE_TOKENS"
    );
}

const FLOW: &str = "mutation($handle: String, $payload: JSON) { flowTriggerReceive(handle: $handle, payload: $payload) { userErrors { field message } } }";

#[test]
fn a_flow_trigger_is_checked_and_written_to_the_outbox() {
    let mut shop = shop();
    let customer = shop.keys("Customer").next().unwrap().to_string();
    let id = crate::state::parse_gid(&customer).unwrap().1;
    for (handle, payload, field) in [
        ("auction-bid", json!({}), json!(["handle"])),
        (
            "auction-bid-placed",
            json!({"Amount": "30"}),
            json!(["payload", "customer_id"]),
        ),
        (
            "auction-bid-placed",
            json!({"Amount": "30", "customer_id": 999_999_999}),
            json!(["payload", "customer_id"]),
        ),
        (
            "auction-bid-placed",
            json!({"Amount": "x".repeat(50_000), "customer_id": id}),
            json!(["payload"]),
        ),
    ] {
        let answer = run(
            &mut shop,
            FLOW,
            json!({"handle": handle, "payload": payload}),
        );
        assert_eq!(
            answer["data"]["flowTriggerReceive"]["userErrors"][0]["field"], field,
            "{answer}"
        );
    }
    assert!(super::outbox_entries(&shop).is_empty());
    let answer = run(
        &mut shop,
        FLOW,
        json!({"handle": "auction-bid-placed", "payload": {"Amount": "30", "customer_id": id}}),
    );
    assert_eq!(
        answer["data"]["flowTriggerReceive"]["userErrors"],
        json!([])
    );
    let outbox = super::outbox_entries(&shop);
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0]["service"], "flow");
    assert_eq!(outbox[0]["payload"]["handle"], "auction-bid-placed");
    assert_eq!(outbox[0]["payload"]["payload"]["Amount"], "30");
}

#[test]
fn mobile_applications_are_kept_and_served_as_association_files() {
    let mut shop = shop();
    let android = run(
        &mut shop,
        "mutation { mobilePlatformApplicationCreate(input: {android: {applicationId: \"com.android.package\", appLinksEnabled: true, sha256CertFingerprints: [\"A1:B2:C3:D4\"]}}) { mobilePlatformApplication { __typename ... on AndroidApplication { id applicationId appLinksEnabled sha256CertFingerprints } } userErrors { message } } }",
        json!({}),
    );
    let created = &android["data"]["mobilePlatformApplicationCreate"]["mobilePlatformApplication"];
    assert_eq!(created["__typename"], "AndroidApplication", "{android}");
    assert!(
        created["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/MobilePlatformApplication/")
    );
    let apple = run(
        &mut shop,
        "mutation { mobilePlatformApplicationCreate(input: {apple: {appId: \"ABCDE12345.com.apple.package\", universalLinksEnabled: true, sharedWebCredentialsEnabled: false, appClipsEnabled: true}}) { mobilePlatformApplication { __typename } userErrors { field code } } }",
        json!({}),
    );
    assert_eq!(
        apple["data"]["mobilePlatformApplicationCreate"]["userErrors"][0]["code"], "INVALID",
        "app clips need their id"
    );
    assert_eq!(shop.count("AppleApplication"), 0);
    let links = super::mobile::association_file(&shop, "/.well-known/assetlinks.json").unwrap();
    assert_eq!(
        links[0]["target"]["package_name"], "com.android.package",
        "{links}"
    );
    assert!(
        super::mobile::association_file(&shop, "/.well-known/apple-app-site-association").is_none()
    );
    let listed = run(
        &mut shop,
        "{ mobilePlatformApplications(first: 5) { nodes { __typename } } }",
        json!({}),
    );
    assert_eq!(
        listed["data"]["mobilePlatformApplications"]["nodes"],
        json!([{"__typename": "AndroidApplication"}])
    );
    let id = created["id"].as_str().unwrap();
    let updated = run(
        &mut shop,
        "mutation($id: ID!) { mobilePlatformApplicationUpdate(id: $id, input: {android: {appLinksEnabled: false}}) { mobilePlatformApplication { ... on AndroidApplication { appLinksEnabled applicationId } } userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        updated["data"]["mobilePlatformApplicationUpdate"]["mobilePlatformApplication"],
        json!({"appLinksEnabled": false, "applicationId": "com.android.package"})
    );
    assert!(super::mobile::association_file(&shop, "/.well-known/assetlinks.json").is_none());
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { mobilePlatformApplicationDelete(id: $id) { deletedMobilePlatformApplicationId userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        deleted["data"]["mobilePlatformApplicationDelete"]["deletedMobilePlatformApplicationId"],
        json!(id)
    );
}

#[test]
fn the_checkout_branding_is_merged_and_read_back() {
    let mut shop = shop();
    let update = "mutation($configuration: CheckoutAndAccountsConfigurationInput!) {
        checkoutAndAccountsConfigurationUpdate(id: \"gid://shopify/CheckoutAndAccountsConfiguration/1\", configuration: $configuration) {
            configuration { id name isPublished branding { designTokens { cornerRadius { base small } } } overrides { id name } }
            userErrors { field message code reason }
        }
    }";
    let answer = run(
        &mut shop,
        update,
        json!({"configuration": {"branding": {"designTokens": {"cornerRadius": {"base": 6, "small": 2}}}}}),
    );
    let configuration = &answer["data"]["checkoutAndAccountsConfigurationUpdate"]["configuration"];
    assert_eq!(
        configuration["branding"]["designTokens"]["cornerRadius"],
        json!({"base": 6, "small": 2}),
        "{answer}"
    );
    assert_eq!(configuration["isPublished"], true);
    let missing = run(
        &mut shop,
        "mutation { checkoutAndAccountsConfigurationUpdate(id: \"gid://shopify/CheckoutAndAccountsConfiguration/9\", configuration: {}) { configuration { id } userErrors { field code reason } } }",
        json!({}),
    );
    assert_eq!(
        missing["data"]["checkoutAndAccountsConfigurationUpdate"]["userErrors"],
        json!([{"field": ["id"], "code": "NOT_FOUND", "reason": "not_found"}])
    );

    let upsert = run(
        &mut shop,
        "mutation { checkoutBrandingUpsert(checkoutProfileId: \"gid://shopify/CheckoutProfile/1\", checkoutBrandingInput: {designSystem: {cornerRadius: {base: 4}}}) { checkoutBranding { designSystem { cornerRadius { base } } } userErrors { message } } }",
        json!({}),
    );
    assert_eq!(
        upsert["data"]["checkoutBrandingUpsert"]["checkoutBranding"]["designSystem"]["cornerRadius"]
            ["base"],
        4,
        "{upsert}"
    );
    let read = run(
        &mut shop,
        "{ checkoutProfiles(first: 1) { nodes { id name isPublished } } checkoutBranding(checkoutProfileId: \"gid://shopify/CheckoutProfile/1\") { designSystem { cornerRadius { base } } } }",
        json!({}),
    );
    assert_eq!(
        read["data"]["checkoutProfiles"]["nodes"][0]["name"], "Default",
        "{read}"
    );
    assert_eq!(
        read["data"]["checkoutBranding"]["designSystem"]["cornerRadius"]["base"],
        4
    );
}

#[test]
fn resource_feedback_replaces_the_last_and_refuses_outdated_feedback() {
    let mut shop = shop();
    let create = "mutation($input: ResourceFeedbackCreateInput!) { shopResourceFeedbackCreate(input: $input) { feedback { state messages { message } } userErrors { field message } } }";
    let answer = run(
        &mut shop,
        create,
        json!({"input": {"state": "REQUIRES_ACTION", "feedbackGeneratedAt": "2024-01-02T00:00:00Z", "messages": ["is not connected. Connect your account to use this sales channel."]}}),
    );
    assert_eq!(
        answer["data"]["shopResourceFeedbackCreate"]["feedback"]["messages"][0]["message"],
        "Platform fixtures is not connected. Connect your account to use this sales channel.",
        "{answer}"
    );
    let app = run(&mut shop, "{ app { feedback { state } } }", json!({}));
    assert_eq!(app["data"]["app"]["feedback"]["state"], "REQUIRES_ACTION");
    let outdated = run(
        &mut shop,
        create,
        json!({"input": {"state": "ACCEPTED", "feedbackGeneratedAt": "1998-06-07T10:00:00Z"}}),
    );
    assert_eq!(
        outdated["data"]["shopResourceFeedbackCreate"]["userErrors"],
        json!([{"field": ["input"], "message": "Feedback for a later version of this resource was already accepted."}])
    );
    let accepted = run(
        &mut shop,
        create,
        json!({"input": {"state": "ACCEPTED", "feedbackGeneratedAt": "2024-01-03T00:00:00Z"}}),
    );
    assert_eq!(
        accepted["data"]["shopResourceFeedbackCreate"]["feedback"],
        Value::Null
    );
    let app = run(&mut shop, "{ app { feedback { state } } }", json!({}));
    assert_eq!(app["data"]["app"]["feedback"], Value::Null);

    let product = shop.keys("Product").next().unwrap().to_string();
    let bulk = "mutation($input: [ProductResourceFeedbackInput!]!) { bulkProductResourceFeedbackCreate(feedbackInput: $input) { feedback { productId state messages } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        bulk,
        json!({"input": [{"productId": product, "state": "REQUIRES_ACTION", "feedbackGeneratedAt": "2024-01-02T00:00:00Z", "productUpdatedAt": "2024-01-01T00:00:00Z", "messages": ["Needs a description."]}]}),
    );
    assert_eq!(
        answer["data"]["bulkProductResourceFeedbackCreate"]["feedback"][0]["messages"],
        json!(["Needs a description."]),
        "{answer}"
    );
    let missing = run(
        &mut shop,
        bulk,
        json!({"input": [{"productId": "gid://shopify/Product/1", "state": "ACCEPTED", "feedbackGeneratedAt": "2024-01-02T00:00:00Z", "productUpdatedAt": "2024-01-01T00:00:00Z"}]}),
    );
    assert_eq!(
        missing["data"]["bulkProductResourceFeedbackCreate"]["userErrors"][0]["code"],
        "PRODUCT_NOT_FOUND"
    );
}

#[test]
fn apps_installations_and_staff_answer_real_values() {
    let mut shop = shop();
    let answer = run(
        &mut shop,
        "{
            app { title handle apiKey icon { url } banner { url } launchUrl developerName publicCategory pricingDetailsSummary uninstallMessage requestedAccessScopes { handle description } }
            appByHandle(handle: \"platform-fixtures\") { id }
            appByKey(apiKey: \"22222222222222222222222222222222\") { id }
            appInstallations(first: 5) { nodes { id launchUrl app { title } } }
            currentAppInstallation { id launchUrl accessScopes { handle description } }
            staffMembers(first: 5) { nodes { id name email isShopOwner initials avatar { url } privateData { accountSettingsUrl createdAt } } }
            currentStaffMember { id }
        }",
        json!({}),
    );
    assert!(answer.get("errors").is_none(), "{answer}");
    let data = &answer["data"];
    assert_eq!(data["app"]["title"], "Platform fixtures");
    assert_eq!(data["appByHandle"]["id"], data["appByKey"]["id"]);
    assert!(data["appByHandle"]["id"].is_string());
    assert_eq!(
        data["appInstallations"]["nodes"].as_array().unwrap().len(),
        1
    );
    assert_eq!(data["staffMembers"]["nodes"][0]["isShopOwner"], true);
    assert_eq!(
        data["staffMembers"]["nodes"][0]["initials"],
        json!(["S", "O"])
    );
    // An offline token acts for no staff member.
    assert_eq!(data["currentStaffMember"], Value::Null);
    assert_eq!(
        answer["extensions"]["lsf"]["defaulted"],
        Value::Null,
        "{}",
        answer["extensions"]
    );
}

#[test]
fn a_fulfillment_constraint_rule_runs_a_constraint_function_of_the_app() {
    let mut shop = shop();
    let create = "mutation($handle: String, $methods: [DeliveryMethodType!]!) { fulfillmentConstraintRuleCreate(functionHandle: $handle, deliveryMethodTypes: $methods, metafields: [{namespace: \"$app\", key: \"limits\", type: \"json\", value: \"{\\\"max\\\": 3}\"}]) { fulfillmentConstraintRule { id deliveryMethodTypes function { handle apiType } metafield(namespace: \"$app\", key: \"limits\") { value } } userErrors { field code } } }";
    for (handle, code) in [
        (json!(null), "MISSING_FUNCTION_IDENTIFIER"),
        (json!("nope"), "FUNCTION_NOT_FOUND"),
    ] {
        let answer = run(
            &mut shop,
            create,
            json!({"handle": handle, "methods": ["SHIPPING"]}),
        );
        assert_eq!(
            answer["data"]["fulfillmentConstraintRuleCreate"]["userErrors"][0]["code"], code,
            "{answer}"
        );
    }
    assert_eq!(shop.count("FulfillmentConstraintRule"), 0);
    let answer = run(
        &mut shop,
        create,
        json!({"handle": "delivery-limits", "methods": ["SHIPPING", "PICK_UP"]}),
    );
    let rule = &answer["data"]["fulfillmentConstraintRuleCreate"]["fulfillmentConstraintRule"];
    assert_eq!(
        rule["function"]["apiType"], "fulfillment_constraints",
        "{answer}"
    );
    assert_eq!(rule["metafield"]["value"], "{\"max\":3}");
    let again = run(
        &mut shop,
        create,
        json!({"handle": "delivery-limits", "methods": ["SHIPPING"]}),
    );
    assert_eq!(
        again["data"]["fulfillmentConstraintRuleCreate"]["userErrors"][0]["code"],
        "FUNCTION_ALREADY_REGISTERED"
    );
    let id = rule["id"].as_str().unwrap();
    let updated = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentConstraintRuleUpdate(id: $id, deliveryMethodTypes: [LOCAL]) { fulfillmentConstraintRule { deliveryMethodTypes } userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        updated["data"]["fulfillmentConstraintRuleUpdate"]["fulfillmentConstraintRule"]["deliveryMethodTypes"],
        json!(["LOCAL"])
    );
    let listed = run(
        &mut shop,
        "{ fulfillmentConstraintRules { id } }",
        json!({}),
    );
    assert_eq!(
        listed["data"]["fulfillmentConstraintRules"],
        json!([{"id": id}])
    );
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentConstraintRuleDelete(id: $id) { success userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        deleted["data"]["fulfillmentConstraintRuleDelete"]["success"],
        true
    );
    let missing = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentConstraintRuleDelete(id: $id) { success userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        missing["data"]["fulfillmentConstraintRuleDelete"]["userErrors"][0]["code"],
        "NOT_FOUND"
    );
}

#[test]
fn discount_tags_are_added_and_removed_in_bulk() {
    let mut shop = shop();
    let created = run(
        &mut shop,
        "mutation { discountCodeBasicCreate(basicCodeDiscount: {title: \"Ten\", code: \"TEN\", startsAt: \"2024-01-01T00:00:00Z\", customerSelection: {all: true}, customerGets: {value: {percentage: 0.1}, items: {all: true}}}) { codeDiscountNode { id } userErrors { message } } }",
        json!({}),
    );
    let id = created["data"]["discountCodeBasicCreate"]["codeDiscountNode"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"))
        .to_string();
    shop.take_events();
    let add = "mutation($ids: [ID!], $search: String, $tags: [String!]!) { discountBulkTagsAdd(ids: $ids, search: $search, tags: $tags) { job { done } userErrors { field code } } }";
    for (variables, code) in [
        (json!({"tags": ["summer"]}), "MISSING_ARGUMENT"),
        (
            json!({"ids": [id], "search": "x", "tags": ["summer"]}),
            "TOO_MANY_ARGUMENTS",
        ),
        (json!({"ids": [id], "tags": []}), "BLANK"),
    ] {
        let answer = run(&mut shop, add, variables);
        assert_eq!(
            answer["data"]["discountBulkTagsAdd"]["userErrors"][0]["code"], code,
            "{answer}"
        );
    }
    assert!(topics(&mut shop).is_empty());
    let number = id.rsplit('/').next().unwrap();
    let answer = run(
        &mut shop,
        add,
        json!({"ids": [format!("gid://shopify/DiscountNode/{number}")], "tags": ["summer", "vip"]}),
    );
    assert_eq!(
        answer["data"]["discountBulkTagsAdd"]["job"]["done"], true,
        "{answer}"
    );
    assert_eq!(topics(&mut shop), ["discounts/update"]);
    let read = run(
        &mut shop,
        "query($id: ID!) { codeDiscountNode(id: $id) { codeDiscount { ... on DiscountCodeBasic { tags } } } }",
        json!({"id": id}),
    );
    assert_eq!(
        read["data"]["codeDiscountNode"]["codeDiscount"]["tags"],
        json!(["summer", "vip"]),
        "{read}"
    );
    let removed = run(
        &mut shop,
        "mutation { discountBulkTagsRemove(search: \"title:Ten\", tags: [\"vip\"]) { job { done } userErrors { code } } }",
        json!({}),
    );
    assert_eq!(
        removed["data"]["discountBulkTagsRemove"]["job"]["done"],
        true
    );
    let read = run(
        &mut shop,
        "query($id: ID!) { codeDiscountNode(id: $id) { codeDiscount { ... on DiscountCodeBasic { tags } } } }",
        json!({"id": id}),
    );
    assert_eq!(
        read["data"]["codeDiscountNode"]["codeDiscount"]["tags"],
        json!(["summer"])
    );
}

#[test]
fn the_deprecated_upload_targets_are_staged_uploads() {
    let mut shop = shop();
    let one = run(
        &mut shop,
        "mutation { stagedUploadTargetGenerate(input: {filename: \"a.jpg\", mimeType: \"image/jpeg\", resource: IMAGE, httpMethod: POST}) { url parameters { name value } userErrors { field message } } }",
        json!({}),
    );
    let payload = &one["data"]["stagedUploadTargetGenerate"];
    assert_eq!(payload["userErrors"], json!([]), "{one}");
    assert!(
        payload["url"]
            .as_str()
            .unwrap()
            .contains("/__lsf/admin/uploads/")
    );
    assert!(
        payload["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|parameter| parameter["name"] == "key")
    );
    let many = run(
        &mut shop,
        "mutation { stagedUploadTargetsGenerate(input: [{filename: \"a.jpg\", mimeType: \"image/jpeg\", resource: IMAGE}, {filename: \"b.png\", mimeType: \"image/png\", resource: IMAGE}]) { urls { url parameters { name } } userErrors { field message } } }",
        json!({}),
    );
    assert_eq!(
        many["data"]["stagedUploadTargetsGenerate"]["urls"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "{many}"
    );
    let refused = run(
        &mut shop,
        "mutation { stagedUploadTargetsGenerate(input: [{filename: \"a.jpg\", mimeType: \"text/plain\", resource: IMAGE}]) { urls { url } userErrors { field message } } }",
        json!({}),
    );
    assert!(
        !refused["data"]["stagedUploadTargetsGenerate"]["userErrors"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{refused}"
    );
    assert_eq!(
        refused["data"]["stagedUploadTargetsGenerate"]["urls"],
        Value::Null
    );
}
