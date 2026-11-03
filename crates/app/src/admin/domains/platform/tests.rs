//! The app platform through queries over the demo store and the app of
//! `tests/fixtures/platform-app`. What the tokens do with the effects (an uninstall, a
//! delegate token) is tested through `AdminApi` in `tests/platform.rs`.

use serde_json::{Value, json};

use super::access::{AuthEffect, effects};
use crate::admin::domains::storefront::tests::{run, shop, topics};
use crate::admin::state::ShopState;

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
    let version = crate::admin::version::ApiVersion::get("2027-01").unwrap();
    let mut resolver = crate::admin::resolve::ShopResolver::new(
        &mut shop,
        crate::admin::resolve::Registry::standard(),
    )
    .with_version(version);
    let request = crate::admin::graphql::Request::new(
        "mutation { appDowngradeAccessScopes(scopes: [\"write_customers\", \"write_products\"]) { downgraded { handle } userErrors { code } } }",
    );
    let answer =
        crate::admin::graphql::run(version.schema(), &mut resolver, &request, None).to_json();
    assert_eq!(
        answer["data"]["appDowngradeAccessScopes"]["userErrors"][0]["code"],
        "CANNOT_DOWNGRADE_REQUIRED_SCOPES",
        "{answer}"
    );
    let mut resolver = crate::admin::resolve::ShopResolver::new(
        &mut shop,
        crate::admin::resolve::Registry::standard(),
    )
    .with_version(version);
    let request = crate::admin::graphql::Request::new(
        "mutation { appDowngradeAccessScopes(scopes: [\"write_customers\"]) { downgraded { handle } userErrors { code } } }",
    );
    let answer =
        crate::admin::graphql::run(version.schema(), &mut resolver, &request, None).to_json();
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
    let parent = crate::admin::auth::Grant {
        shop: shop.domain().to_string(),
        client_id: "22222222222222222222222222222222".to_string(),
        scopes: vec!["read_products".to_string(), "write_orders".to_string()],
        mode: crate::admin::auth::tokens::AccessMode::Offline,
        user: None,
        issued_at: chrono::Utc::now(),
        expires_at: None,
        source: crate::admin::auth::tokens::GrantSource::Install,
    };
    let query = "mutation($input: DelegateAccessTokenInput!) { delegateAccessTokenCreate(input: $input) { delegateAccessToken { accessToken accessScopes expiresIn } shop { id } userErrors { field message code } } }";
    let run_as = |shop: &mut ShopState, grant: &crate::admin::auth::Grant, input: Value| {
        let mut resolver = crate::admin::resolve::ShopResolver::new(
            shop,
            crate::admin::resolve::Registry::standard(),
        )
        .with_grant(grant);
        let request =
            crate::admin::graphql::Request::new(query).with_variables(json!({"input": input}));
        crate::admin::graphql::run(crate::admin::schema::admin(), &mut resolver, &request, None)
            .to_json()
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
    delegate.source = crate::admin::auth::tokens::GrantSource::Delegate;
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
    let id = crate::admin::state::parse_gid(&customer).unwrap().1;
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
    assert!(crate::admin::outbox::entries(&shop, None).is_empty());
    let answer = run(
        &mut shop,
        FLOW,
        json!({"handle": "auction-bid-placed", "payload": {"Amount": "30", "customer_id": id}}),
    );
    assert_eq!(
        answer["data"]["flowTriggerReceive"]["userErrors"],
        json!([])
    );
    let outbox = crate::admin::outbox::entries(&shop, None);
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0]["channel"], "flow");
    assert_eq!(outbox[0]["what"], "flow_trigger");
    assert_eq!(outbox[0]["to"], "auction-bid-placed");
    assert_eq!(outbox[0]["data"]["Amount"], "30");
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

/// The selection of every field of `type_name` that takes no required argument and is not
/// deprecated: leaves by name, objects one level down, connections by their first two nodes.
fn every_field(type_name: &str, depth: usize) -> String {
    let schema = crate::admin::schema::admin();
    let Some(definition) = schema.get(type_name) else {
        return "__typename".to_string();
    };
    if definition.is_abstract() {
        let members: Vec<String> = definition
            .possible_types
            .iter()
            .map(|member| format!("... on {member} {{ {} }}", every_field(member, depth)))
            .collect();
        return format!("__typename {}", members.join(" "));
    }
    let mut parts = vec!["__typename".to_string()];
    for field in definition.fields.values() {
        if field.deprecation.is_some() || field.args.values().any(|arg| arg.is_required()) {
            continue;
        }
        let named = field.ty.named();
        let Some(target) = schema.get(named) else {
            continue;
        };
        if target.is_leaf() {
            parts.push(field.name.clone());
        } else if depth == 0 {
            continue;
        } else if named.ends_with("Connection") && target.field("nodes").is_some() {
            let node = target.field("nodes").unwrap().ty.named().to_string();
            let first = if field.args.contains_key("first") {
                "(first: 2)"
            } else {
                ""
            };
            parts.push(format!(
                "{}{first} {{ nodes {{ {} }} }}",
                field.name,
                every_field(&node, depth - 1)
            ));
        } else {
            parts.push(format!(
                "{} {{ {} }}",
                field.name,
                every_field(named, depth - 1)
            ));
        }
    }
    parts.join(" ")
}

#[test]
fn the_queries_of_the_platform_answer_every_main_field_without_a_default() {
    let mut shop = shop();
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../theme/tests/fixtures/theme");
    crate::admin::domains::themes::attach(&mut shop, &root);
    for mutation in [
        "mutation { scriptTagCreate(input: {src: \"https://js.example.org/a.js\"}) { userErrors { message } } }",
        "mutation { webPixelCreate(webPixel: {settings: {accountID: \"1\"}}) { userErrors { message } } }",
        "mutation { serverPixelCreate { userErrors { message } } }",
        "mutation { storefrontAccessTokenCreate(input: {title: \"t\"}) { userErrors { message } } }",
        "mutation { savedSearchCreate(input: {name: \"s\", query: \"path:/a\", resourceType: URL_REDIRECT}) { userErrors { message } } }",
        "mutation { mobilePlatformApplicationCreate(input: {apple: {appId: \"A.b\", universalLinksEnabled: true, sharedWebCredentialsEnabled: false}}) { userErrors { message } } }",
        "mutation { checkoutBrandingUpsert(checkoutProfileId: \"gid://shopify/CheckoutProfile/1\", checkoutBrandingInput: {designSystem: {cornerRadius: {base: 4}}}) { userErrors { message } } }",
    ] {
        let answer = run(&mut shop, mutation, json!({}));
        assert!(answer.get("errors").is_none(), "{answer}");
    }
    shop.put_blob(
        "tmp/1/redirects/x/r.csv",
        crate::admin::state::Blob::new("Redirect from,Redirect to\n/a,/b\n", "text/csv"),
    );
    let import = run(
        &mut shop,
        "mutation { urlRedirectImportCreate(url: \"https://shopify-staged-uploads.storage.googleapis.com/tmp/1/redirects/x/r.csv\") { urlRedirectImport { id } userErrors { message } } }",
        json!({}),
    );
    let import = import["data"]["urlRedirectImportCreate"]["urlRedirectImport"]["id"].clone();
    let one = |type_name: &str| every_field(type_name, 1);
    let query = format!(
        "query($import: ID!) {{
            theme(id: \"gid://shopify/OnlineStoreTheme/1\") {{ {theme} files(first: 2) {{ nodes {{ {file} }} userErrors {{ code filename }} }} }}
            themes(first: 2) {{ nodes {{ {theme} }} }}
            scriptTags(first: 2) {{ nodes {{ {script} }} }}
            webPixel {{ {pixel} }}
            serverPixel {{ {server} }}
            shop {{ storefrontAccessTokens(first: 2) {{ nodes {{ {token} }} }} shopPolicies {{ {policy} }} }}
            urlRedirectImport(id: $import) {{ {import_fields} }}
            urlRedirectSavedSearches(first: 2) {{ nodes {{ {saved} }} }}
            standardMetafieldDefinitionTemplates(first: 2) {{ nodes {{ {template} }} }}
            currentAppInstallation {{ {installation} }}
            app {{ {app} }}
            appByHandle(handle: \"platform-fixtures\") {{ id title }}
            appByKey(apiKey: \"22222222222222222222222222222222\") {{ id title }}
            appInstallation {{ id launchUrl }}
            appInstallations(first: 2) {{ nodes {{ id }} }}
            mobilePlatformApplications(first: 2) {{ nodes {{ {mobile} }} }}
            checkoutProfiles(first: 2) {{ nodes {{ {profile} }} }}
            checkoutProfile(id: \"gid://shopify/CheckoutProfile/1\") {{ id name }}
            checkoutBranding(checkoutProfileId: \"gid://shopify/CheckoutProfile/1\") {{ designSystem {{ cornerRadius {{ base }} }} }}
            checkoutAndAccountsConfigurations(first: 2) {{ nodes {{ id name isPublished createdAt editedAt updatedAt }} }}
            staffMembers(first: 2) {{ nodes {{ {staff} }} }}
        }}",
        theme = every_field("OnlineStoreTheme", 0),
        file = every_field("OnlineStoreThemeFile", 1),
        script = one("ScriptTag"),
        pixel = one("WebPixel"),
        server = one("ServerPixel"),
        token = one("StorefrontAccessToken"),
        policy = every_field("ShopPolicy", 0),
        import_fields = one("UrlRedirectImport"),
        saved = one("SavedSearch"),
        template = one("StandardMetafieldDefinitionTemplate"),
        installation = every_field("AppInstallation", 0),
        app = every_field("App", 0),
        mobile = one("MobilePlatformApplication"),
        profile = one("CheckoutProfile"),
        staff = one("StaffMember"),
    );
    let answer = run(&mut shop, &query, json!({"import": import}));
    assert!(answer.get("errors").is_none(), "{answer}");
    assert!(
        answer["data"]["theme"]["files"]["nodes"][1]["filename"].is_string(),
        "{answer}"
    );
    // The test sees a default when there is one.
    let defaulted = run(&mut shop, "{ shop { features { giftCards } } }", json!({}));
    assert!(
        defaulted["extensions"]["lsf"]["defaultedFields"].is_array(),
        "{defaulted}"
    );
    assert_eq!(
        answer["extensions"]["lsf"]["defaultedFields"],
        Value::Null,
        "{}",
        answer["extensions"]
    );
}
