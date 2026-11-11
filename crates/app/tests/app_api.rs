//! The app family end to end, through `AdminApi::handle` as `lsf` serves it: the billing calls
//! of `@shopify/shopify-api` 15.0.1 word for word, the confirmation page and its redirect, app
//! discounts, staged uploads and bulk operations, each with its webhook delivered to a listener
//! of the test, signed.

mod support;

use std::collections::HashMap;
use std::time::Duration;

use lsf_app::admin::api::{AdminApi, AdminOptions, HttpRequest, HttpResponse, ThrottleSetting};
use lsf_app::admin::webhooks::DeliveryOptions;
use serde_json::{Value, json};
use support::json_of;
use support::receiver::Receiver;

const SECRET: &str = "test-secret";
const GRAPHQL: &str = "/admin/api/2026-10/graphql.json";
const HOST: &str = "127.0.0.1:9292";

fn api(store: &lsf_core::Store, app: Option<lsf_app::config::App>) -> AdminApi {
    AdminApi::new(
        store,
        AdminOptions {
            app,
            client_secret: SECRET.to_string(),
            app_url: None,
            throttle: ThrottleSetting::Off,
            delivery: DeliveryOptions {
                timeout: Duration::from_secs(3),
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(7),
        },
    )
}

fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> HttpRequest {
    let path = path.strip_prefix(&format!("http://{HOST}")).unwrap_or(path);
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let mut all: HashMap<String, String> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.to_string()))
        .collect();
    all.insert("host".into(), HOST.into());
    HttpRequest {
        method: method.to_string(),
        path: path.to_string(),
        query: form_urlencoded::parse(query.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect(),
        headers: all,
        body: body.to_vec(),
    }
}

fn send(
    api: &AdminApi,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> HttpResponse {
    api.handle(&request(method, path, headers, body))
        .unwrap_or_else(|| panic!("{path} is not routed"))
}

fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// A GraphQL request with the default token of the default shop.
fn graphql(api: &AdminApi, query: &str, variables: Value) -> Value {
    let token = api.token(&api.default_shop());
    let body = json!({"query": query, "variables": variables}).to_string();
    let response = send(
        api,
        "POST",
        GRAPHQL,
        &[
            ("content-type", "application/json"),
            ("x-shopify-access-token", &token),
        ],
        body.as_bytes(),
    );
    assert_eq!(response.status, 200);
    let answer = json_of(&response);
    assert!(answer.get("errors").is_none(), "{answer}");
    answer
}

/// lsf's page of a charge (`/__lsf/admin/charges/<n>?shop=`): where its `confirmationUrl`, on
/// the shop's admin as Shopify writes it, leads once opened on lsf's origin.
fn charge_page(api: &AdminApi, confirmation: &str) -> String {
    let path = confirmation.replacen(&format!("https://{}", api.default_shop()), "", 1);
    let answer = send(api, "GET", &path, &[], b"");
    assert_eq!(answer.status, 302, "{confirmation}");
    header(&answer, "location").unwrap().to_string()
}

/// Subscribes the receiver to a topic.
fn subscribe(api: &AdminApi, topic: &str, receiver: &Receiver) {
    let answer = graphql(
        api,
        "mutation($topic: WebhookSubscriptionTopic!, $uri: String!) { webhookSubscriptionCreate(topic: $topic, webhookSubscription: {uri: $uri}) { userErrors { message } } }",
        json!({"topic": topic, "uri": format!("{}/webhooks", receiver.url)}),
    );
    assert_eq!(
        answer["data"]["webhookSubscriptionCreate"]["userErrors"],
        json!([])
    );
}

// The GraphQL of `@shopify/shopify-api` 15.0.1, `lib/billing/{request,check,cancel,
// create-usage-record,update-usage-subscription-capped-amount,types}.ts`, as it sends it.

const APP_SUBSCRIPTION_FRAGMENT: &str = r#"
  fragment AppSubscriptionFragment on AppSubscription {
    id
    name
    test
    status
    trialDays
    createdAt
    currentPeriodEnd
    returnUrl
    lineItems {
      id
      plan {
        pricingDetails {
          ... on AppRecurringPricing {
            price {
              amount
              currencyCode
            }
            interval
            discount {
              durationLimitInIntervals
              remainingDurationInIntervals
              priceAfterDiscount {
                amount
              }
              value {
                ... on AppSubscriptionDiscountAmount {
                  amount {
                    amount
                    currencyCode
                  }
                }
                ... on AppSubscriptionDiscountPercentage {
                  percentage
                }
              }
            }
          }
          ... on AppUsagePricing {
            balanceUsed {
              amount
              currencyCode
            }
            cappedAmount {
              amount
              currencyCode
            }
            terms
          }
        }
      }
    }
  }
"#;

const RECURRING_PURCHASE_MUTATION: &str = r#"
  mutation AppSubscriptionCreate(
    $name: String!
    $returnUrl: URL!
    $test: Boolean
    $trialDays: Int
    $replacementBehavior: AppSubscriptionReplacementBehavior
    $lineItems: [AppSubscriptionLineItemInput!]!
  ) {
    appSubscriptionCreate(
      name: $name
      returnUrl: $returnUrl
      test: $test
      trialDays: $trialDays
      replacementBehavior: $replacementBehavior
      lineItems: $lineItems
    ) {
      appSubscription {
        ...AppSubscriptionFragment
      }
      confirmationUrl
      userErrors {
        field
        message
      }
    }
  }
"#;

const HAS_PAYMENTS_QUERY: &str = r#"
  query appSubscription($endCursor: String) {
    currentAppInstallation {
      activeSubscriptions {
        ...AppSubscriptionFragment
      }
      oneTimePurchases(first: 250, sortKey: CREATED_AT, after: $endCursor) {
        edges {
          node {
            id
            name
            test
            status
          }
        }
        pageInfo {
          hasNextPage
          endCursor
        }
      }
    }
  }
"#;

const CANCEL_MUTATION: &str = r#"
  mutation appSubscriptionCancel($id: ID!, $prorate: Boolean) {
    appSubscriptionCancel(id: $id, prorate: $prorate) {
      appSubscription {
        ...AppSubscriptionFragment
      }
      userErrors {
        field
        message
      }
    }
  }
"#;

const CREATE_USAGE_RECORD_MUTATION: &str = r#"
mutation appUsageRecordCreate($description: String!, $price: MoneyInput!, $subscriptionLineItemId: ID!) {
  appUsageRecordCreate(description: $description, price: $price, subscriptionLineItemId: $subscriptionLineItemId) {
    userErrors {
      field
      message
    }
    appUsageRecord {
      id
      description
      idempotencyKey
      price {
        amount
        currencyCode
      }
      subscriptionLineItem {
        id
        plan {
          pricingDetails {
            ... on AppUsagePricing {
              balanceUsed {
                amount
                currencyCode
              }
              cappedAmount {
                amount
                currencyCode
              }
              terms
            }
          }
        }
      }
    }
  }
}
"#;

#[test]
fn the_billing_helpers_of_shopify_api_work_against_lsf() {
    let receiver = Receiver::start(SECRET);
    let api = api(&support::demo_store(), None);
    subscribe(&api, "APP_SUBSCRIPTIONS_UPDATE", &receiver);

    // `billing.request`: a plan of a recurring and a usage line, as `requestSubscriptionPayment`
    // builds its variables.
    let requested = graphql(
        &api,
        &format!("{APP_SUBSCRIPTION_FRAGMENT}{RECURRING_PURCHASE_MUTATION}"),
        json!({
            "name": "Pro plan",
            "trialDays": 3,
            "replacementBehavior": null,
            "returnUrl": "http://localhost:3000/app?shop=x.myshopify.com",
            "test": true,
            "lineItems": [
                {"plan": {"appRecurringPricingDetails": {"interval": "EVERY_30_DAYS", "price": {"amount": 9.99, "currencyCode": "USD"}}}},
                {"plan": {"appUsagePricingDetails": {"terms": "1 USD per 100 emails", "cappedAmount": {"amount": 30, "currencyCode": "USD"}}}},
            ],
        }),
    );
    let data = &requested["data"]["appSubscriptionCreate"];
    assert_eq!(data["userErrors"], json!([]));
    let confirmation = data["confirmationUrl"].as_str().unwrap().to_string();
    // Shopify's, on the shop's admin, which lsf's admin serves on its origin.
    let shop = api.default_shop();
    let prefix = format!("https://{shop}/admin/charges/");
    assert!(confirmation.starts_with(&prefix), "{confirmation}");
    assert!(
        confirmation.contains(
            "/RecurringApplicationCharge/confirm_recurring_application_charge?signature="
        ),
        "{confirmation}"
    );
    // The same page on lsf's origin, for a test that opens it itself: followed as it is, it
    // leads to the charge page, as Shopify's does through lsf's admin.
    let local = requested["extensions"]["lsf"]["localConfirmationUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("{requested}"))
        .to_string();
    assert!(
        local.starts_with(&format!("http://{HOST}/admin/charges/")),
        "{local}"
    );
    assert!(local.ends_with(&format!("&shop={shop}")), "{local}");
    let followed = send(&api, "GET", &local, &[], b"");
    assert_eq!(followed.status, 302, "{local}");
    assert_eq!(
        header(&followed, "location"),
        Some(charge_page(&api, &confirmation).as_str()),
        "{local}"
    );
    let on_lsf = send(
        &api,
        "GET",
        &confirmation.replacen(&format!("https://{shop}"), "", 1),
        &[],
        b"",
    );
    assert_eq!(
        on_lsf.status,
        302,
        "{}",
        String::from_utf8_lossy(&on_lsf.body)
    );
    let confirmation = header(&on_lsf, "location").unwrap().to_string();
    assert!(
        confirmation.starts_with("/__lsf/admin/charges/"),
        "{confirmation}"
    );
    let id = data["appSubscription"]["id"].as_str().unwrap().to_string();
    let usage_line = data["appSubscription"]["lineItems"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // The merchant's page, then the approval: back to the app with `charge_id`.
    let page = send(&api, "GET", &confirmation, &[], b"");
    assert_eq!(page.status, 200);
    let html = String::from_utf8_lossy(&page.body);
    assert!(html.contains("Pro plan") && html.contains("Approve") && html.contains("9.99 USD"));
    let number = confirmation
        .split("/charges/")
        .nth(1)
        .and_then(|rest| rest.split('?').next())
        .unwrap()
        .to_string();
    let approved = send(
        &api,
        "POST",
        &format!("/__lsf/admin/charges/{number}/approve"),
        &[("content-type", "application/x-www-form-urlencoded")],
        format!("shop={shop}").as_bytes(),
    );
    assert_eq!(approved.status, 303);
    assert_eq!(
        header(&approved, "location"),
        Some(format!("http://localhost:3000/app?shop=x.myshopify.com&charge_id={number}").as_str())
    );
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "app_subscriptions/update");
    assert_eq!(
        payload["app_subscription"]["admin_graphql_api_id"],
        json!(id)
    );
    assert_eq!(payload["app_subscription"]["status"], json!("ACTIVE"));
    assert_eq!(payload["app_subscription"]["price"], json!("9.99"));
    assert_eq!(payload["app_subscription"]["capped_amount"], json!("30.0"));
    assert_eq!(
        payload["app_subscription"]["interval"],
        json!("every_30_days")
    );

    // `billing.check`: the active subscription meets the criteria.
    let checked = graphql(
        &api,
        &format!("{APP_SUBSCRIPTION_FRAGMENT}{HAS_PAYMENTS_QUERY}"),
        json!({"endCursor": null}),
    );
    let installation = &checked["data"]["currentAppInstallation"];
    assert_eq!(installation["activeSubscriptions"][0]["id"], json!(id));
    assert_eq!(
        installation["activeSubscriptions"][0]["status"],
        json!("ACTIVE")
    );
    assert_eq!(
        installation["oneTimePurchases"]["pageInfo"]["hasNextPage"],
        json!(false)
    );

    // `billing.createUsageRecord`: it sends `idempotencyKey` among the variables, which the
    // operation does not declare.
    let usage = graphql(
        &api,
        CREATE_USAGE_RECORD_MUTATION,
        json!({"description": "100 emails", "price": {"amount": 1, "currencyCode": "USD"},
            "subscriptionLineItemId": usage_line, "idempotencyKey": "k1"}),
    );
    assert_eq!(
        usage["data"]["appUsageRecordCreate"]["appUsageRecord"]["subscriptionLineItem"]["plan"]["pricingDetails"]
            ["balanceUsed"],
        json!({"amount": "1.0", "currencyCode": "USD"})
    );

    // `billing.cancel`, `prorate` true by default.
    let cancelled = graphql(
        &api,
        &format!("{APP_SUBSCRIPTION_FRAGMENT}{CANCEL_MUTATION}"),
        json!({"id": id, "prorate": true}),
    );
    assert_eq!(
        cancelled["data"]["appSubscriptionCancel"]["appSubscription"]["status"],
        json!("CANCELLED")
    );
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "app_subscriptions/update");
    assert_eq!(payload["app_subscription"]["status"], json!("CANCELLED"));

    // The page of a charge decided: nothing to decide; the endpoint says so.
    let again = send(
        &api,
        "POST",
        &format!("/__lsf/admin/charges/{number}/decline?shop={shop}"),
        &[],
        b"",
    );
    assert_eq!(again.status, 409);
    assert_eq!(
        send(&api, "GET", "/__lsf/admin/charges/9999", &[], b"").status,
        404
    );
}

#[test]
fn a_one_time_purchase_is_approved_through_the_endpoint_and_its_webhook_delivered() {
    let receiver = Receiver::start(SECRET);
    let api = api(&support::demo_store(), None);
    subscribe(&api, "APP_PURCHASES_ONE_TIME_UPDATE", &receiver);
    let created = graphql(
        &api,
        r#"mutation test($name: String!, $price: MoneyInput!, $returnUrl: URL!, $test: Boolean) {
            appPurchaseOneTimeCreate(name: $name, price: $price, returnUrl: $returnUrl, test: $test) {
              appPurchaseOneTime { id name test } confirmationUrl userErrors { field message } } }"#,
        json!({"name": "Setup", "returnUrl": "http://localhost:3000/", "test": true, "price": {"amount": 25, "currencyCode": "USD"}}),
    );
    let confirmation = created["data"]["appPurchaseOneTimeCreate"]["confirmationUrl"]
        .as_str()
        .unwrap()
        .to_string();
    let path = charge_page(&api, &confirmation).replace("?", "/approve?");
    let approved = send(&api, "POST", &path, &[], b"");
    assert_eq!(approved.status, 200);
    let answer = json_of(&approved);
    assert_eq!(answer["charge"]["status"], json!("ACTIVE"));
    assert!(answer["redirect"].as_str().unwrap().contains("charge_id="));
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "app_purchases_one_time/update");
    assert_eq!(payload["app_purchase_one_time"]["status"], json!("ACTIVE"));
    assert_eq!(payload["app_purchase_one_time"]["name"], json!("Setup"));
}

#[test]
fn uninstalling_cancels_the_active_subscription() {
    let api = api(&support::demo_store(), None);
    let created = graphql(
        &api,
        &format!("{APP_SUBSCRIPTION_FRAGMENT}{RECURRING_PURCHASE_MUTATION}"),
        json!({"name": "Basic", "returnUrl": "http://localhost:3000/", "lineItems": [
            {"plan": {"appRecurringPricingDetails": {"price": {"amount": 5, "currencyCode": "USD"}}}}]}),
    );
    let confirmation = charge_page(
        &api,
        created["data"]["appSubscriptionCreate"]["confirmationUrl"]
            .as_str()
            .unwrap(),
    )
    .replace('?', "/approve?");
    assert_eq!(send(&api, "POST", &confirmation, &[], b"").status, 200);
    assert_eq!(
        send(&api, "POST", "/__lsf/admin/uninstall", &[], b"").status,
        200
    );
    assert_eq!(
        send(&api, "POST", "/__lsf/admin/install", &[], b"").status,
        200
    );
    let checked = graphql(
        &api,
        "{ currentAppInstallation { activeSubscriptions { id } allSubscriptions(first: 5) { nodes { status } } } }",
        Value::Null,
    );
    assert_eq!(
        checked["data"]["currentAppInstallation"],
        json!({"activeSubscriptions": [], "allSubscriptions": {"nodes": [{"status": "CANCELLED"}]}})
    );
}

#[test]
fn an_app_discount_created_by_the_backend_is_announced_and_listed() {
    let receiver = Receiver::start(SECRET);
    let api = api(
        &support::fixture_store("rich"),
        Some(support::fixture_app()),
    );
    subscribe(&api, "DISCOUNTS_CREATE", &receiver);
    subscribe(&api, "DISCOUNTS_DELETE", &receiver);
    let created = graphql(
        &api,
        r#"mutation($discount: DiscountAutomaticAppInput!) { discountAutomaticAppCreate(automaticAppDiscount: $discount) {
            automaticAppDiscount { discountId title status } userErrors { field message } } }"#,
        json!({"discount": {"title": "Bulk 10%", "functionHandle": "volume-discount", "discountClasses": ["PRODUCT"],
            "metafields": [{"namespace": "$app:config", "key": "settings", "type": "json", "value": "{\"percent\":10}"}]}}),
    );
    let discount = &created["data"]["discountAutomaticAppCreate"]["automaticAppDiscount"];
    let node = discount["discountId"].as_str().unwrap().to_string();
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "discounts/create");
    assert_eq!(
        payload,
        json!({"admin_graphql_api_id": node, "title": "Bulk 10%", "status": "ACTIVE",
            "created_at": "2026-03-15T10:00:00Z", "updated_at": "2026-03-15T10:00:00Z"})
    );
    let listed = graphql(
        &api,
        "{ discountNodes(first: 10, query: \"title:Bulk\") { nodes { id metafields(first: 1) { nodes { key jsonValue } } } } }",
        Value::Null,
    );
    assert_eq!(
        listed["data"]["discountNodes"]["nodes"],
        json!([{"id": node.replace("DiscountAutomaticNode", "DiscountNode"),
            "metafields": {"nodes": [{"key": "settings", "jsonValue": {"percent": 10}}]}}])
    );
    graphql(
        &api,
        &format!(
            r#"mutation {{ discountAutomaticDelete(id: "{node}") {{ deletedAutomaticDiscountId }} }}"#
        ),
        Value::Null,
    );
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "discounts/delete");
    assert_eq!(payload["admin_graphql_api_id"], json!(node));
    assert_eq!(payload["deleted_at"], json!("2026-03-15T10:00:00Z"));
}

#[test]
fn a_bulk_import_goes_from_a_staged_upload_to_its_results_file() {
    let receiver = Receiver::start(SECRET);
    let api = api(&support::demo_store(), None);
    subscribe(&api, "BULK_OPERATIONS_FINISH", &receiver);
    let staged = graphql(
        &api,
        r#"mutation { stagedUploadsCreate(input: {resource: BULK_MUTATION_VARIABLES, filename: "bulk_op_vars", mimeType: "text/jsonl", httpMethod: POST}) {
            stagedTargets { url resourceUrl parameters { name value } } userErrors { field message } } }"#,
        Value::Null,
    );
    let target = &staged["data"]["stagedUploadsCreate"]["stagedTargets"][0];
    let url = target["url"].as_str().unwrap();
    // The form of the parameters, the file last, as the clients post it to Google's storage.
    let boundary = "lsfboundary";
    let mut body = Vec::new();
    let mut key = String::new();
    for parameter in target["parameters"].as_array().unwrap() {
        let name = parameter["name"].as_str().unwrap();
        let value = parameter["value"].as_str().unwrap();
        if name == "key" {
            key = value.to_string();
        }
        body.extend(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"vars.jsonl\"\r\nContent-Type: text/jsonl\r\n\r\n")
            .as_bytes(),
    );
    body.extend(
        b"{\"product\":{\"title\":\"Imported A\"}}\n{\"product\":{\"title\":\"Imported B\"}}\n",
    );
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    let uploaded = send(
        &api,
        "POST",
        url,
        &[(
            "content-type",
            &format!("multipart/form-data; boundary={boundary}"),
        )],
        &body,
    );
    assert_eq!(
        uploaded.status,
        201,
        "{}",
        String::from_utf8_lossy(&uploaded.body)
    );
    assert!(String::from_utf8_lossy(&uploaded.body).contains(&key));

    let ran = graphql(
        &api,
        r#"mutation($path: String!) { bulkOperationRunMutation(mutation: "mutation call($product: ProductCreateInput!) { productCreate(product: $product) { product { id title } userErrors { message field } } }", stagedUploadPath: $path) {
            bulkOperation { id status } userErrors { field message code } } }"#,
        json!({ "path": key }),
    );
    let operation = &ran["data"]["bulkOperationRunMutation"]["bulkOperation"];
    assert_eq!(operation["status"], json!("CREATED"));
    let id = operation["id"].as_str().unwrap().to_string();
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "bulk_operations/finish");
    assert_eq!(
        (
            payload["admin_graphql_api_id"].clone(),
            payload["status"].clone(),
            payload["type"].clone()
        ),
        (json!(id), json!("completed"), json!("mutation"))
    );
    let done = graphql(
        &api,
        &format!(
            r#"{{ node(id: "{id}") {{ ... on BulkOperation {{ status objectCount url }} }} }}"#
        ),
        Value::Null,
    );
    let operation = &done["data"]["node"];
    assert_eq!(operation["status"], json!("COMPLETED"));
    assert_eq!(operation["objectCount"], json!("2"));
    let file = send(&api, "GET", operation["url"].as_str().unwrap(), &[], b"");
    assert_eq!(file.status, 200);
    assert_eq!(header(&file, "content-type"), Some("application/jsonl"));
    let lines: Vec<Value> = String::from_utf8_lossy(&file.body)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[1]["data"]["productCreate"]["product"]["title"],
        json!("Imported B")
    );
    assert_eq!(lines[1]["__lineNumber"], json!(1));

    // A PUT target takes the body as the file, and gives it back.
    let put = graphql(
        &api,
        r#"mutation { stagedUploadsCreate(input: {resource: FILE, filename: "notes.txt", mimeType: "text/plain"}) { stagedTargets { url resourceUrl } } }"#,
        Value::Null,
    );
    let target = &put["data"]["stagedUploadsCreate"]["stagedTargets"][0];
    let stored = send(
        &api,
        "PUT",
        target["url"].as_str().unwrap(),
        &[("content-type", "text/plain")],
        b"hello",
    );
    assert_eq!(stored.status, 200);
    let back = send(
        &api,
        "GET",
        target["resourceUrl"].as_str().unwrap(),
        &[],
        b"",
    );
    assert_eq!(back.body, b"hello");
}

/// A multipart form of a POST target's parameters, then the file, as clients post it to Google's
/// storage.
fn upload_form(target: &Value, file_name: &str, mime: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "lsfboundary";
    let mut body = Vec::new();
    for parameter in target["parameters"].as_array().unwrap() {
        let (name, value) = (
            parameter["name"].as_str().unwrap(),
            parameter["value"].as_str().unwrap(),
        );
        body.extend(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n")
            .as_bytes(),
    );
    body.extend(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[test]
fn a_file_uploaded_to_a_staged_target_is_created_with_its_name_type_and_size() {
    let api = api(&support::demo_store(), None);
    let staged = graphql(
        &api,
        r#"mutation { stagedUploadsCreate(input: [
            {resource: IMAGE, filename: "hero.png", mimeType: "image/png", httpMethod: POST},
            {resource: FILE, filename: "manual.pdf", mimeType: "application/pdf"}
        ]) { stagedTargets { url resourceUrl parameters { name value } } userErrors { field message } } }"#,
        Value::Null,
    );
    let targets = &staged["data"]["stagedUploadsCreate"]["stagedTargets"];
    let (image, manual) = (&targets[0], &targets[1]);

    // The image as a form (its part named as the client's disk names it: the target's name
    // counts), the PDF as the body of a PUT.
    let png = b"\x89PNG\r\n\x1a\n0123456789abcdef";
    let (content_type, form) = upload_form(image, "IMG_0001.PNG", "image/png", png);
    let posted = send(
        &api,
        "POST",
        image["url"].as_str().unwrap(),
        &[("content-type", &content_type)],
        &form,
    );
    assert_eq!(posted.status, 201);
    let pdf = vec![b'%'; 2048];
    let put = send(
        &api,
        "PUT",
        manual["url"].as_str().unwrap(),
        &[("content-type", "application/pdf")],
        &pdf,
    );
    assert_eq!(put.status, 200);

    // A name that does not keep the upload's extension is refused, as for a URL.
    const FILE_CREATE: &str = "mutation($files: [FileCreateInput!]!) { fileCreate(files: $files) {
        files { id fileStatus } userErrors { field message code } } }";
    let mismatched = graphql(
        &api,
        FILE_CREATE,
        json!({"files": [{"originalSource": image["resourceUrl"], "filename": "cover.jpg"}]}),
    );
    assert_eq!(
        mismatched["data"]["fileCreate"]["userErrors"][0]["code"],
        "MISMATCHED_FILENAME_AND_ORIGINAL_SOURCE"
    );
    let created = graphql(
        &api,
        FILE_CREATE,
        json!({"files": [
            {"originalSource": image["resourceUrl"], "alt": "Hero"},
            {"originalSource": manual["resourceUrl"]}
        ]}),
    );
    assert_eq!(created["data"]["fileCreate"]["userErrors"], json!([]));

    // Read back as a backend polls them: by name, ready, with the upload's type and size.
    const FILES: &str = "query($name: String!) { files(first: 5, query: $name) { nodes {
        __typename fileStatus alt
        ... on MediaImage { mimeType image { url } originalSource { url fileSize } }
        ... on GenericFile { mimeType url originalFileSize } } } }";
    let hero = graphql(&api, FILES, json!({"name": "hero.png"}));
    assert_eq!(
        hero["data"]["files"]["nodes"],
        json!([{
            "__typename": "MediaImage", "fileStatus": "READY", "alt": "Hero",
            "mimeType": "image/png", "image": {"url": image["resourceUrl"]},
            "originalSource": {"url": image["resourceUrl"], "fileSize": png.len()},
        }])
    );
    let found = graphql(&api, FILES, json!({"name": "manual.pdf"}));
    assert_eq!(
        found["data"]["files"]["nodes"],
        json!([{
            "__typename": "GenericFile", "fileStatus": "READY", "alt": "",
            "mimeType": "application/pdf", "url": manual["resourceUrl"],
            "originalFileSize": 2048,
        }])
    );
    // Its URL serves the bytes uploaded.
    let back = send(
        &api,
        "GET",
        manual["resourceUrl"].as_str().unwrap(),
        &[],
        b"",
    );
    assert_eq!((back.status, back.body.len()), (200, 2048));
}

#[test]
fn a_standalone_app_gets_a_confirmation_url_on_lsf_that_works_as_it_is() {
    let mut app = support::fixture_app();
    app.config.embedded = Some(false);
    let api = api(&support::demo_store(), Some(app));
    // On a shop other than the default one: the URL names it.
    let created = send(
        &api,
        "POST",
        "/__lsf/admin/shops",
        &[("content-type", "application/json")],
        br#"{"shop": "standalone"}"#,
    );
    assert_eq!(
        created.status,
        201,
        "{}",
        String::from_utf8_lossy(&created.body)
    );
    let created = json_of(&created);
    let shop = created["shop"].as_str().unwrap().to_string();
    let token = created["token"].as_str().unwrap().to_string();
    let body = json!({
        "query": r#"mutation($returnUrl: URL!) { appPurchaseOneTimeCreate(name: "Setup", price: {amount: 25, currencyCode: USD}, returnUrl: $returnUrl, test: true) { confirmationUrl userErrors { message } } }"#,
        "variables": {"returnUrl": "http://localhost:3000/"},
    })
    .to_string();
    let answer = json_of(&send(
        &api,
        "POST",
        GRAPHQL,
        &[
            ("content-type", "application/json"),
            ("x-shopify-access-token", &token),
        ],
        body.as_bytes(),
    ));
    let confirmation = answer["data"]["appPurchaseOneTimeCreate"]["confirmationUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("{answer}"))
        .to_string();
    // lsf's origin, Shopify's path, and the shop: no `https://<shop>`, which is Shopify's.
    assert!(
        confirmation.starts_with(&format!("http://{HOST}/admin/charges/")),
        "{confirmation}"
    );
    assert!(confirmation.contains("/ApplicationCharge/confirm_application_charge?signature="));
    assert!(
        confirmation.ends_with(&format!("&shop={shop}")),
        "{confirmation}"
    );
    // Followed as it is, it leads to the charge page of that shop.
    let followed = send(&api, "GET", &confirmation, &[], b"");
    assert_eq!(followed.status, 302);
    let page = header(&followed, "location").unwrap().to_string();
    assert!(page.starts_with("/__lsf/admin/charges/1?shop="), "{page}");
    assert!(page.contains(&shop), "{page}");
    let html = send(&api, "GET", &page, &[], b"");
    assert_eq!(html.status, 200);
    assert!(String::from_utf8_lossy(&html.body).contains("Approve a one-time charge"));

    // An embedded app keeps Shopify's URL, which the library's exit-iframe page requires.
    let api = self::api(&support::demo_store(), Some(support::fixture_app()));
    let answer = graphql(
        &api,
        r#"mutation { appPurchaseOneTimeCreate(name: "Setup", price: {amount: 25, currencyCode: USD}, returnUrl: "http://localhost:3000/", test: true) { confirmationUrl } }"#,
        json!({}),
    );
    let confirmation = answer["data"]["appPurchaseOneTimeCreate"]["confirmationUrl"]
        .as_str()
        .unwrap();
    assert!(
        confirmation.starts_with(&format!("https://{}/admin/charges/", api.default_shop())),
        "{confirmation}"
    );
}
