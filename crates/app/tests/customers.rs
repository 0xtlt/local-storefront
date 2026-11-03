//! Customers and companies end to end, through `AdminApi::handle`: a backend that manages
//! customers (a CRM, a loyalty program, B2B onboarding) creates and changes them, and receives
//! the webhooks of its subscriptions over a real socket, signed.

mod support;

use std::time::Duration;

use lsf_app::admin::api::{AdminApi, AdminOptions, ThrottleSetting};
use lsf_app::admin::webhooks::DeliveryOptions;
use serde_json::{Value, json};
use support::receiver::Receiver;

const SECRET: &str = "test-secret";

fn demo_api() -> AdminApi {
    AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: None,
            client_secret: SECRET.to_string(),
            app_url: None,
            throttle: ThrottleSetting::Store,
            delivery: DeliveryOptions {
                timeout: Duration::from_secs(3),
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(7),
        },
    )
}

/// The `data` of a GraphQL request with the shop's default token, which must have no error.
fn graphql(api: &AdminApi, query: &str, variables: Value) -> Value {
    let token = api.token(&api.default_shop());
    let response = api
        .handle(&support::graphql_request(&token, query, variables))
        .expect("the Admin API is routed");
    assert_eq!(response.status, 200);
    let answer: Value = serde_json::from_slice(&response.body).unwrap();
    assert!(answer.get("errors").is_none(), "{answer}");
    answer["data"].clone()
}

/// Subscribes the backend at `url` to `topic` (`CUSTOMERS_CREATE`).
fn subscribe(api: &AdminApi, topic: &str, url: &str) {
    let data = graphql(
        api,
        "mutation($topic: WebhookSubscriptionTopic!, $uri: String!) {
            webhookSubscriptionCreate(topic: $topic, webhookSubscription: {uri: $uri}) {
                webhookSubscription { id } userErrors { field message } } }",
        json!({"topic": topic, "uri": format!("{url}/webhooks")}),
    );
    assert_eq!(
        data["webhookSubscriptionCreate"]["userErrors"],
        json!([]),
        "{data}"
    );
}

#[test]
fn a_crm_creates_tags_and_subscribes_a_customer() {
    let receiver = Receiver::start(SECRET);
    let api = demo_api();
    for topic in [
        "CUSTOMERS_CREATE",
        "CUSTOMER_TAGS_ADDED",
        "CUSTOMERS_EMAIL_MARKETING_CONSENT_UPDATE",
    ] {
        subscribe(&api, topic, &receiver.url);
    }

    let created = graphql(
        &api,
        "mutation($input: CustomerInput!) { customerCreate(input: $input) { customer { id } userErrors { field message } } }",
        json!({"input": {"email": "lena@example.com", "firstName": "Lena", "lastName": "Petit", "tags": ["crm"],
            "addresses": [{"address1": "9 Quai Saint-Antoine", "city": "Lyon", "countryCode": "FR", "zip": "69002"}]}}),
    );
    let id = created["customerCreate"]["customer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let number = lsf_app::admin::state::parse_gid(&id).unwrap().1;
    let (topic, customer) = receiver.next();
    assert_eq!(topic, "customers/create");
    assert_eq!(customer["id"], json!(number));
    assert_eq!(customer["admin_graphql_api_id"], json!(id));
    assert_eq!(customer["email"], json!("lena@example.com"));
    assert_eq!(customer["state"], json!("disabled"));
    assert_eq!(customer["tags"], json!("crm"));
    assert_eq!(customer["default_address"]["city"], json!("Lyon"));
    assert_eq!(customer["default_address"]["country_code"], json!("FR"));
    assert_eq!(customer["default_address"]["default"], json!(true));
    assert_eq!(
        customer["email_marketing_consent"]["state"],
        json!("not_subscribed")
    );
    // The tags it was created with were added.
    let (topic, tags) = receiver.next();
    assert_eq!(topic, "customer.tags_added");
    assert_eq!(tags["tags"], json!(["crm"]));

    // `tagsAdd` reports the tags it added, not those the customer had.
    graphql(
        &api,
        "mutation($id: ID!) { tagsAdd(id: $id, tags: [\"crm\", \"loyalty-gold\"]) { userErrors { message } } }",
        json!({"id": id}),
    );
    let (topic, tags) = receiver.next();
    assert_eq!(topic, "customer.tags_added");
    assert_eq!(tags["customerId"], json!(id));
    assert_eq!(tags["tags"], json!(["loyalty-gold"]));
    assert!(tags["occurredAt"].as_str().unwrap().ends_with(".000Z"));

    graphql(
        &api,
        "mutation($input: CustomerEmailMarketingConsentUpdateInput!) { customerEmailMarketingConsentUpdate(input: $input) { userErrors { message } } }",
        json!({"input": {"customerId": id, "emailMarketingConsent": {"marketingState": "SUBSCRIBED", "marketingOptInLevel": "CONFIRMED_OPT_IN"}}}),
    );
    let (topic, consent) = receiver.next();
    assert_eq!(topic, "customers_email_marketing_consent/update");
    assert_eq!(
        consent,
        json!({
            "customer_id": number,
            "email_address": "lena@example.com",
            "email_marketing_consent": {
                "state": "subscribed",
                "opt_in_level": "confirmed_opt_in",
                "consent_updated_at": consent["email_marketing_consent"]["consent_updated_at"],
            },
        })
    );
}

#[test]
fn a_b2b_backend_onboards_a_company() {
    let receiver = Receiver::start(SECRET);
    let api = demo_api();
    for topic in [
        "COMPANIES_CREATE",
        "COMPANY_CONTACT_ROLES_ASSIGN",
        "COMPANY_LOCATIONS_DELETE",
    ] {
        subscribe(&api, topic, &receiver.url);
    }
    let created = graphql(
        &api,
        "mutation($input: CompanyCreateInput!) { companyCreate(input: $input) {
            company { id mainContact { id } locations(first: 1) { nodes { id } } } userErrors { field message code } } }",
        json!({"input": {
            "company": {"name": "Postal Cards Inc", "externalId": "PC-1", "note": "Net 30"},
            "companyLocation": {"name": "Ottawa", "shippingAddress": {"address1": "150 Elgin Street", "city": "Ottawa", "zoneCode": "ON", "countryCode": "CA"}},
            "companyContact": {"email": "avery@example.com", "firstName": "Avery", "title": "Buyer"},
        }}),
    );
    let company = &created["companyCreate"]["company"];
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "companies/create");
    assert_eq!(
        payload,
        json!({
            "name": "Postal Cards Inc",
            "note": "Net 30",
            "external_id": "PC-1",
            "main_contact_admin_graphql_api_id": company["mainContact"]["id"],
            "created_at": payload["created_at"],
            "updated_at": payload["updated_at"],
            "customer_since": payload["customer_since"],
            "admin_graphql_api_id": company["id"],
        })
    );
    let (topic, assigned) = receiver.next();
    assert_eq!(topic, "company_contact_roles/assign");
    assert_eq!(
        assigned["company_contact"]["admin_graphql_api_id"],
        company["mainContact"]["id"]
    );
    assert_eq!(assigned["company_contact"]["title"], json!("Buyer"));
    assert_eq!(
        assigned["company_contact"]["company"]["name"],
        json!("Postal Cards Inc")
    );
    assert_eq!(assigned["company_location"]["name"], json!("Ottawa"));
    assert_eq!(
        assigned["company_location"]["shipping_address"]["country_code"],
        json!("CA")
    );
    assert_eq!(
        assigned["company_contact_role"],
        json!({"name": "Ordering only"})
    );

    // Deleting the company reports its location, with the company embedded though it is gone.
    graphql(
        &api,
        "mutation($id: ID!) { companyDelete(id: $id) { userErrors { message } } }",
        json!({"id": company["id"]}),
    );
    let (topic, location) = receiver.next();
    assert_eq!(topic, "company_locations/delete");
    assert_eq!(
        location["admin_graphql_api_id"],
        company["locations"]["nodes"][0]["id"]
    );
    assert_eq!(location["company"]["name"], json!("Postal Cards Inc"));
}
