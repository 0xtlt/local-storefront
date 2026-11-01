//! Segments, their members and saved searches, through queries over the demo store; and the
//! helpers the tests of the other modules of this area share (`demo`, `run`, `data`, `topics`).

use serde_json::{Value, json};

use crate::domains::test_support;
pub(crate) use crate::domains::test_support::run;
use crate::state::{Clock, ShopState};

/// The demo store on a frozen clock (it runs on the system's).
pub(crate) fn demo() -> ShopState {
    test_support::frozen(test_support::demo())
}

/// The data of a field of an answer that has no GraphQL error.
pub(crate) fn data<'a>(answer: &'a Value, name: &str) -> &'a Value {
    assert!(answer.get("errors").is_none(), "{answer}");
    &answer["data"][name]
}

/// The topics of the events emitted since the last call, `lsf/changed` left out.
pub(crate) fn topics(shop: &mut ShopState) -> Vec<String> {
    test_support::topics(shop)
        .into_iter()
        .filter(|topic| topic != crate::domains::app::CHANGED)
        .collect()
}

/// The GID of a demo customer, by email.
pub(crate) fn customer(shop: &ShopState, email: &str) -> String {
    shop.keys("Customer")
        .find(|key| shop.get(key).unwrap()["email"] == json!(email))
        .unwrap_or_else(|| panic!("no customer {email}"))
        .to_string()
}

const CREATE: &str = "mutation($name: String!, $query: String!) {
    segmentCreate(name: $name, query: $query) {
        segment { id name query creationDate lastEditDate } userErrors { field message } } }";

fn members(shop: &mut ShopState, arguments: &str) -> Value {
    let answer = run(
        shop,
        &format!(
            "{{ customerSegmentMembers(first: 10, {arguments}) {{
                totalCount
                statistics {{ attributeStatistics(attributeName: \"amount_spent\") {{ sum average }} }}
                edges {{ node {{ id displayName firstName numberOfOrders amountSpent {{ amount }}
                    defaultEmailAddress {{ emailAddress }} mergeable {{ isMergeable }} }} }}
            }} }}"
        ),
        json!({}),
    );
    assert!(answer["extensions"].get("lsf").is_none(), "{answer}");
    data(&answer, "customerSegmentMembers").clone()
}

fn emails(connection: &Value) -> Vec<String> {
    connection["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| {
            edge["node"]["defaultEmailAddress"]["emailAddress"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn a_segment_is_created_updated_and_deleted() {
    let mut shop = demo();
    let created = run(
        &mut shop,
        CREATE,
        json!({"name": "VIP spenders", "query": "amount_spent > 10 AND customer_tags CONTAINS 'vip'"}),
    );
    let segment = data(&created, "segmentCreate");
    assert_eq!(segment["userErrors"], json!([]), "{created}");
    assert_eq!(segment["segment"]["name"], "VIP spenders");
    assert_eq!(segment["segment"]["creationDate"], "2026-10-31T12:00:00Z");
    let id = segment["segment"]["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("gid://shopify/Segment/"));
    assert_eq!(topics(&mut shop), ["segments/create"]);

    let listed = run(
        &mut shop,
        "{ segments(first: 5, query: \"name:VIP\") { nodes { id } } segmentsCount { count } }",
        json!({}),
    );
    assert_eq!(data(&listed, "segments")["nodes"][0]["id"], json!(id));
    assert_eq!(data(&listed, "segmentsCount")["count"], 1);

    let updated = run(
        &mut shop,
        "mutation($id: ID!) { segmentUpdate(id: $id, name: \"Big spenders\", query: \"number_of_orders >= 1\") {
            segment { name query } userErrors { field message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        data(&updated, "segmentUpdate")["segment"],
        json!({"name": "Big spenders", "query": "number_of_orders >= 1"})
    );
    assert_eq!(topics(&mut shop), ["segments/update"]);
    let found = members(&mut shop, &format!("segmentId: \"{id}\""));
    assert_eq!(found["totalCount"], 2, "{found}");

    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { segmentDelete(id: $id) { deletedSegmentId userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        data(&deleted, "segmentDelete")["deletedSegmentId"],
        json!(id)
    );
    assert_eq!(topics(&mut shop), ["segments/delete"]);
    assert_eq!(shop.count("Segment"), 0);
}

#[test]
fn a_bad_segment_changes_nothing() {
    let mut shop = demo();
    run(
        &mut shop,
        CREATE,
        json!({"name": "Taken", "query": "number_of_orders = 0"}),
    );
    shop.take_events();
    let before = shop.picture();
    for (name, query, field, message) in [
        (
            "Mine",
            "total_spent > 100",
            "query",
            "Query Line 1 Column 0: 'total_spent' filter cannot be found.",
        ),
        (
            "Mine",
            "products_purchased = 1 AND amount_spent MATCHES (id = 1)",
            "query",
            "Query Line 1 Column 0: '=' is not an operator of 'products_purchased'.",
        ),
        ("", "number_of_orders = 1", "name", "Name can't be blank"),
        (
            "Taken",
            "number_of_orders = 1",
            "name",
            "Name has already been taken",
        ),
    ] {
        let answer = run(&mut shop, CREATE, json!({"name": name, "query": query}));
        let payload = data(&answer, "segmentCreate");
        assert_eq!(payload["segment"], Value::Null, "{answer}");
        assert_eq!(
            payload["userErrors"][0],
            json!({"field": [field], "message": message}),
            "{query}"
        );
    }
    let missing = run(
        &mut shop,
        "mutation { segmentDelete(id: \"gid://shopify/Segment/999\") { deletedSegmentId userErrors { field message } } }",
        json!({}),
    );
    assert_eq!(
        data(&missing, "segmentDelete")["userErrors"],
        json!([{"field": ["id"], "message": "Segment does not exist"}])
    );
    assert_eq!(shop.picture(), before);
    assert!(topics(&mut shop).is_empty());
}

#[test]
fn a_date_offset_past_the_calendar_is_refused_and_a_stored_one_breaks_nothing() {
    let mut shop = demo();
    let before = shop.picture();
    for query in [
        "last_order_date > -99999999999d",
        "customer_added_date < +9999999w",
        "first_order_date = -10001y",
        "orders_placed MATCHES (date > -99999999999m)",
    ] {
        let answer = run(&mut shop, CREATE, json!({"name": "Far", "query": query}));
        let payload = data(&answer, "segmentCreate");
        assert_eq!(payload["segment"], Value::Null, "{answer}");
        let message = payload["userErrors"][0]["message"].as_str().unwrap();
        assert!(
            message.starts_with("Query Line 1 Column ")
                && message.ends_with("is not a valid value."),
            "{message}"
        );
    }
    assert_eq!(shop.picture(), before);
    let members = run(
        &mut shop,
        "{ customerSegmentMembers(first: 1, query: \"last_order_date > -99999999999d\") { totalCount } }",
        json!({}),
    );
    assert!(members.get("errors").is_some(), "{members}");

    // The furthest offset lsf takes is evaluated.
    let answer = run(
        &mut shop,
        CREATE,
        json!({"name": "Ever", "query": "last_order_date > -10000y AND customer_added_date < +3660000d"}),
    );
    assert_eq!(
        data(&answer, "segmentCreate")["userErrors"],
        json!([]),
        "{answer}"
    );

    // A segment the parser would refuse, as an older lsf stored it: the customers' mutations
    // still answer, and the segment keeps no one.
    let mut record = crate::state::Record::new();
    record.insert("__typename".into(), json!("Segment"));
    record.insert("name".into(), json!("Broken"));
    record.insert("query".into(), json!("last_order_date > -99999999999d"));
    shop.insert(record);
    shop.take_events();
    let jane = customer(&shop, "jane.doe@example.com");
    let added = run(
        &mut shop,
        "mutation($id: ID!) { tagsAdd(id: $id, tags: [\"far\"]) { userErrors { message } } }",
        json!({"id": jane}),
    );
    assert_eq!(data(&added, "tagsAdd")["userErrors"], json!([]), "{added}");
}

#[test]
fn members_are_the_customers_a_query_keeps() {
    let mut shop = demo();
    let found = members(
        &mut shop,
        "query: \"number_of_orders >= 1\", sortKey: \"amount_spent\", reverse: true",
    );
    assert_eq!(
        emails(&found),
        ["jane.doe@example.com", "camille.martin@example.com"]
    );
    assert_eq!(found["totalCount"], 2);
    let jane = shop
        .get(&customer(&shop, "jane.doe@example.com"))
        .unwrap()
        .clone();
    let camille = shop
        .get(&customer(&shop, "camille.martin@example.com"))
        .unwrap()
        .clone();
    let spent = |record: &Value| {
        record["amountSpent"]["amount"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
    };
    let sum = spent(&json!(jane)) + spent(&json!(camille));
    let statistics = &found["statistics"]["attributeStatistics"];
    assert!(
        (statistics["sum"].as_f64().unwrap() - sum).abs() < 1e-6,
        "{found}"
    );
    assert!((statistics["average"].as_f64().unwrap() - sum / 2.0).abs() < 1e-6);
    let id = jane["id"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(
        found["edges"][0]["node"]["id"],
        format!("gid://shopify/CustomerSegmentMember/{id}")
    );
    assert_eq!(
        emails(&members(
            &mut shop,
            "query: \"customer_tags CONTAINS 'VIP' OR customer_countries CONTAINS 'FR'\""
        )),
        ["jane.doe@example.com", "camille.martin@example.com"]
    );
    assert_eq!(
        emails(&members(
            &mut shop,
            "query: \"email_subscription_status = 'SUBSCRIBED'\""
        )),
        ["jane.doe@example.com"]
    );
    // A bad query is an error of the field.
    let answer = run(
        &mut shop,
        "{ customerSegmentMembers(first: 1, query: \"number_of_orders >\") { totalCount } }",
        json!({}),
    );
    assert_eq!(
        answer["errors"][0]["message"],
        "Query Line 1 Column 0: the query ends before the clause does."
    );
}

#[test]
fn a_members_query_is_a_job_done_at_once() {
    let mut shop = demo();
    let created = run(
        &mut shop,
        "mutation { customerSegmentMembersQueryCreate(input: {query: \"number_of_orders = 0\"}) {
            customerSegmentMembersQuery { id done currentCount } userErrors { field message code } } }",
        json!({}),
    );
    let payload = data(&created, "customerSegmentMembersQueryCreate");
    assert_eq!(payload["userErrors"], json!([]));
    let query = &payload["customerSegmentMembersQuery"];
    assert_eq!(query["done"], true);
    assert_eq!(query["currentCount"], 2);
    let id = query["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("gid://shopify/CustomerSegmentMembersQuery/"));
    let read = run(
        &mut shop,
        "query($id: ID!) { customerSegmentMembersQuery(id: $id) { id currentCount done } }",
        json!({"id": id}),
    );
    assert_eq!(
        data(&read, "customerSegmentMembersQuery")["currentCount"],
        2
    );
    let found = members(&mut shop, &format!("queryId: \"{id}\""));
    assert_eq!(
        emails(&found),
        ["new.customer@example.com", "alex.morgan@example.com"]
    );

    let before = shop.picture();
    let refused = run(
        &mut shop,
        "mutation { customerSegmentMembersQueryCreate(input: {query: \"number_of_orderz = 0\"}) {
            customerSegmentMembersQuery { id } userErrors { field message code } } }",
        json!({}),
    );
    assert_eq!(
        data(&refused, "customerSegmentMembersQueryCreate")["userErrors"],
        json!([{"field": ["input", "query"], "message": "Query Line 1 Column 0: 'number_of_orderz' filter cannot be found.", "code": "INVALID"}])
    );
    assert_eq!(shop.picture(), before);
}

#[test]
fn a_customer_who_changes_joins_and_leaves_segments() {
    let mut shop = demo();
    run(
        &mut shop,
        CREATE,
        json!({"name": "Wholesale", "query": "customer_tags CONTAINS 'wholesale'"}),
    );
    let segment = shop.keys("Segment").next().unwrap().to_string();
    shop.take_events();
    let jane = customer(&shop, "jane.doe@example.com");
    let membership = |shop: &mut ShopState| {
        let answer = run(
            shop,
            "query($customer: ID!, $segments: [ID!]!) { customerSegmentMembership(customerId: $customer, segmentIds: $segments) { memberships { segmentId isMember } } }",
            json!({"customer": jane, "segments": [segment]}),
        );
        data(&answer, "customerSegmentMembership")["memberships"][0]["isMember"].clone()
    };
    assert_eq!(membership(&mut shop), false);

    let added = run(
        &mut shop,
        "mutation($id: ID!) { tagsAdd(id: $id, tags: [\"wholesale\"]) { userErrors { message } } }",
        json!({"id": jane}),
    );
    assert_eq!(data(&added, "tagsAdd")["userErrors"], json!([]));
    let events = shop.take_events();
    let joined: Vec<&crate::state::Event> = events
        .iter()
        .filter(|event| event.topic == "customer.joined_segment")
        .collect();
    assert_eq!(joined.len(), 1, "{events:?}");
    let payload = crate::webhooks::payload::payload(
        "customer.joined_segment",
        joined[0].snapshot.as_ref().unwrap(),
        &crate::api::samples::ShopLookup(&shop),
    )
    .unwrap();
    assert_eq!(payload["customer_id"], json!(jane));
    assert_eq!(payload["segment_id"], json!(segment));
    assert_eq!(payload["shop_id"], json!(shop.shop_gid()));
    assert_eq!(membership(&mut shop), true);

    run(
        &mut shop,
        "mutation($id: ID!) { tagsRemove(id: $id, tags: [\"wholesale\"]) { userErrors { message } } }",
        json!({"id": jane}),
    );
    let topics = topics(&mut shop);
    assert!(
        topics.contains(&"customer.left_segment".to_string()),
        "{topics:?}"
    );
}

#[test]
fn filters_and_their_values_are_listed() {
    let mut shop = demo();
    let answer = run(
        &mut shop,
        "{ segmentFilters(first: 50) { nodes { __typename localizedName queryName multiValue
            ... on SegmentEventFilter { parameters { queryName parameterType } } } }
           segmentFilterSuggestions(first: 5, search: \"email\") { nodes { queryName } }
           segmentValueSuggestions(first: 5, search: \"subs\", filterQueryName: \"email_subscription_status\") { nodes { localizedValue queryName } }
           tags: segmentValueSuggestions(first: 5, search: \"\", filterQueryName: \"customer_tags\") { nodes { queryName } } }",
        json!({}),
    );
    let filters = data(&answer, "segmentFilters")["nodes"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(filters.len(), super::filters::FILTERS.len());
    assert!(filters.iter().any(|filter| filter
        == &json!({"__typename": "SegmentFloatFilter", "localizedName": "Amount spent", "queryName": "amount_spent", "multiValue": false})));
    let purchased = filters
        .iter()
        .find(|filter| filter["queryName"] == "products_purchased")
        .unwrap();
    assert_eq!(purchased["__typename"], "SegmentEventFilter");
    assert_eq!(
        purchased["parameters"][0],
        json!({"queryName": "id", "parameterType": "ID"})
    );
    let suggested: Vec<&str> = data(&answer, "segmentFilterSuggestions")["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["queryName"].as_str().unwrap())
        .collect();
    assert!(
        suggested.contains(&"customer_email_domain"),
        "{suggested:?}"
    );
    assert!(suggested.contains(&"email_subscription_status"));
    // The guide's example: "subs" finds the subscribed states.
    assert_eq!(
        data(&answer, "segmentValueSuggestions")["nodes"][0],
        json!({"localizedValue": "Not subscribed", "queryName": "NOT_SUBSCRIBED"})
    );
    let tags: Vec<&str> = answer["data"]["tags"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["queryName"].as_str().unwrap())
        .collect();
    assert_eq!(tags, ["vip", "wholesale", "newsletter"]);
    assert!(answer["extensions"].get("lsf").is_none(), "{answer}");
}

#[test]
fn saved_searches_are_kept_by_resource_and_filter_connections() {
    let mut shop = demo();
    let create = "mutation($input: SavedSearchCreateInput!) { savedSearchCreate(input: $input) {
        savedSearch { id name query resourceType searchTerms filters { key value } } userErrors { field message } } }";
    let created = run(
        &mut shop,
        create,
        json!({"input": {"name": "VIP", "query": "tag:vip", "resourceType": "CUSTOMER"}}),
    );
    let search = &data(&created, "savedSearchCreate")["savedSearch"];
    assert_eq!(search["filters"], json!([{"key": "tag", "value": "vip"}]));
    assert_eq!(search["searchTerms"], "");
    let id = search["id"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["customer_groups/create"]);
    let products = run(
        &mut shop,
        create,
        json!({"input": {"name": "Foo Products", "query": "foo", "resourceType": "PRODUCT"}}),
    );
    assert_eq!(
        data(&products, "savedSearchCreate")["savedSearch"]["searchTerms"],
        "foo"
    );
    assert!(
        topics(&mut shop)
            .iter()
            .all(|topic| topic.starts_with("saved_searches/"))
    );
    let orders = run(
        &mut shop,
        create,
        json!({"input": {"name": "Jane's", "query": "email:jane.doe@example.com", "resourceType": "ORDER"}}),
    );
    let order_search = data(&orders, "savedSearchCreate")["savedSearch"]["id"].clone();
    shop.take_events();

    let listed = run(
        &mut shop,
        "query($orders: ID!) { customerSavedSearches(first: 5) { nodes { name } }
            productSavedSearches(first: 5) { nodes { name } }
            orders(first: 5, savedSearchId: $orders) { nodes { email } }
            segmentMigrations(first: 5) { nodes { savedSearchId segmentId } } }",
        json!({"orders": order_search}),
    );
    assert_eq!(
        data(&listed, "customerSavedSearches")["nodes"],
        json!([{"name": "VIP"}])
    );
    assert_eq!(
        data(&listed, "productSavedSearches")["nodes"],
        json!([{"name": "Foo Products"}])
    );
    assert_eq!(
        data(&listed, "orders")["nodes"],
        json!([{"email": "jane.doe@example.com"}, {"email": "jane.doe@example.com"}])
    );
    assert_eq!(
        data(&listed, "segmentMigrations")["nodes"],
        json!([{"savedSearchId": id, "segmentId": null}])
    );

    let updated = run(
        &mut shop,
        "mutation($id: ID!) { savedSearchUpdate(input: {id: $id, query: \"tag:wholesale\"}) { savedSearch { query } userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        data(&updated, "savedSearchUpdate")["savedSearch"]["query"],
        "tag:wholesale"
    );
    assert_eq!(topics(&mut shop), ["customer_groups/update"]);

    let before = shop.picture();
    let refused = run(
        &mut shop,
        create,
        json!({"input": {"name": "VIP", "query": "x", "resourceType": "CUSTOMER"}}),
    );
    assert_eq!(
        data(&refused, "savedSearchCreate")["userErrors"],
        json!([{"field": ["input", "name"], "message": "Name has already been taken"}])
    );
    assert_eq!(shop.picture(), before);

    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { savedSearchDelete(input: {id: $id}) { deletedSavedSearchId userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        data(&deleted, "savedSearchDelete")["deletedSavedSearchId"],
        json!(id)
    );
    assert_eq!(topics(&mut shop), ["customer_groups/delete"]);
}

/// On the demo store, the root fields of segments, marketing, analytics, privacy, merges and
/// abandoned checkouts answer their fields from what lsf keeps or computes: none is a default.
#[test]
fn the_queries_of_the_area_answer_without_defaults() {
    let mut shop = demo();
    let jane = customer(&shop, "jane.doe@example.com");
    let camille = customer(&shop, "camille.martin@example.com");
    run(
        &mut shop,
        CREATE,
        json!({"name": "Buyers", "query": "number_of_orders >= 1"}),
    );
    let segment = shop.keys("Segment").next().unwrap().to_string();
    run(
        &mut shop,
        "mutation { savedSearchCreate(input: {name: \"VIP\", query: \"tag:vip\", resourceType: CUSTOMER}) { userErrors { message } } }",
        json!({}),
    );
    let query = run(
        &mut shop,
        "mutation { customerSegmentMembersQueryCreate(input: {query: \"number_of_orders = 0\"}) { customerSegmentMembersQuery { id } } }",
        json!({}),
    );
    let members_query =
        data(&query, "customerSegmentMembersQueryCreate")["customerSegmentMembersQuery"]["id"]
            .clone();
    run(
        &mut shop,
        "mutation { marketingActivityUpsertExternal(input: {remoteId: \"ad-1\", title: \"Ad\", remoteUrl: \"https://ads.example.com\",
            tactic: AD, marketingChannelType: SOCIAL, status: ACTIVE, utm: {campaign: \"c\", medium: \"m\", source: \"s\"},
            budget: {budgetType: DAILY, total: {amount: \"5\", currencyCode: USD}}, adSpend: {amount: \"1\", currencyCode: USD}}) { userErrors { message } } }",
        json!({}),
    );
    let activity = shop.keys("MarketingActivity").next().unwrap().to_string();
    let event = shop.keys("MarketingEvent").next().unwrap().to_string();
    run(
        &mut shop,
        "mutation { analyticsTargetCreate(input: {name: \"Q4\", metric: \"orders\", expectedValue: \"10\", startDate: \"2026-10-01\", endDate: \"2026-12-31\"}) { userErrors { message } }
            analyticsAnnotationCreate(input: {type: \"campaign\", title: \"Launch\", startedAt: \"2026-10-01T00:00:00Z\"}) { userErrors { message } } }",
        json!({}),
    );
    let new = customer(&shop, "new.customer@example.com");
    let merged = run(
        &mut shop,
        "mutation($one: ID!, $two: ID!) { customerMerge(customerOneId: $one, customerTwoId: $two) { job { id } } }",
        json!({"one": new, "two": camille}),
    );
    let job = data(&merged, "customerMerge")["job"]["id"].clone();
    // An abandoned checkout of Jane's.
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let product = shop.get(&variant).unwrap()["product"]
        .as_str()
        .unwrap()
        .to_string();
    let result = lsf_checkout::CheckoutResult {
        lines: vec![lsf_checkout::ResultLine {
            variant_id: crate::state::parse_gid(&variant).unwrap().1,
            product_id: crate::state::parse_gid(&product).unwrap().1,
            title: "Line".into(),
            quantity: 1,
            unit_price: 1000,
            subtotal: 1000,
            total: 1000,
            ..Default::default()
        }],
        cost: lsf_checkout::Cost {
            subtotal: 1000,
            total: 1000,
            currency_code: "USD".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let checkout = crate::domains::checkouts::record_checkout(
        &mut shop,
        &crate::domains::checkouts::Checkout {
            cart_token: "c1",
            generation: 0,
            result: &result,
            customer_id: None,
            email: Some("jane.doe@example.com"),
            phone: None,
            shipping_address: None,
            note: "",
            attributes: &[],
            payment: None,
            locale: Some("en"),
        },
    )
    .unwrap();
    let later = shop.now() + chrono::Duration::minutes(11);
    shop.set_clock(Clock::Frozen(later));
    let abandonment = shop.get(&checkout).unwrap()["lsfAbandonment"].clone();

    let answer = run(
        &mut shop,
        "query($segment: ID!, $customer: ID!, $query: ID!, $activity: ID!, $event: ID!, $abandonment: ID!, $checkout: ID!, $job: ID!, $other: ID!) {
          segments(first: 5) { nodes { id name query creationDate lastEditDate } }
          segment(id: $segment) { id name query }
          segmentsCount { count precision }
          customerSegmentMembers(first: 5, segmentId: $segment) { totalCount statistics { attributeStatistics(attributeName: \"amount_spent\") { average sum } }
            nodes: edges { node { id displayName firstName lastName amountSpent { amount } numberOfOrders lastOrderId note
              defaultEmailAddress { emailAddress marketingState } mergeable { isMergeable errorFields reason } } } }
          customerSegmentMembership(customerId: $customer, segmentIds: [$segment]) { memberships { segmentId isMember } }
          customerSegmentMembersQuery(id: $query) { id currentCount done }
          segmentFilters(first: 5) { nodes { localizedName queryName multiValue } }
          segmentFilterSuggestions(first: 5, search: \"order\") { nodes { localizedName queryName } }
          segmentValueSuggestions(first: 5, search: \"\", filterQueryName: \"customer_countries\") { nodes { localizedValue queryName } }
          segmentMigrations(first: 5) { nodes { id savedSearchId segmentId } }
          customerSavedSearches(first: 5) { nodes { id name query resourceType searchTerms filters { key value } legacyResourceId } }
          marketingActivities(first: 5) { nodes { id title status statusLabel statusBadgeTypeV2 tactic marketingChannelType sourceAndMedium
            isExternal inMainWorkflowVersion createdAt updatedAt app { id } budget { budgetType total { amount } } adSpend { amount }
            utmParameters { campaign medium source } hierarchyLevel parentActivityId urlParameterValue } }
          marketingActivity(id: $activity) { id title marketingEvent { id } }
          marketingEvent(id: $event) { id type remoteId startedAt manageUrl utmCampaign utmMedium utmSource sourceAndMedium marketingChannelType app { id } description }
          marketingEvents(first: 5) { nodes { id type } }
          abandonedCheckouts(first: 5) { nodes { id name abandonedCheckoutUrl createdAt updatedAt completedAt note taxesIncluded discountCodes
            customAttributes { key value } customer { id } subtotalPriceSet { shopMoney { amount } } totalPriceSet { shopMoney { amount } }
            totalLineItemsPriceSet { shopMoney { amount } } totalDiscountSet { shopMoney { amount } } totalTaxSet { shopMoney { amount } }
            lineItems(first: 5) { nodes { id title quantity sku product { id } variant { id } originalUnitPriceSet { shopMoney { amount } }
              originalTotalPriceSet { shopMoney { amount } } discountedTotalPriceSet { shopMoney { amount } } customAttributes { key } } } } }
          abandonedCheckoutsCount { count }
          abandonment(id: $abandonment) { id abandonmentType mostRecentStep createdAt customer { id } app { id } emailState emailSentAt
            customerHasNoOrderSinceAbandonment customerHasNoDraftOrderSinceAbandonment daysSinceLastAbandonmentEmail
            hoursSinceLastAbandonedCheckout inventoryAvailable isFromOnlineStore isFromShopApp isFromShopPay isFromCustomStorefront
            isMostSignificantAbandonment lastBrowseAbandonmentDate lastCartAbandonmentDate lastCheckoutAbandonmentDate
            abandonedCheckoutPayload { id } productsAddedToCart(first: 5) { nodes { quantity product { id } } } }
          abandonmentByAbandonedCheckoutId(abandonedCheckoutId: $checkout) { id }
          customerMergePreview(customerOneId: $other, customerTwoId: $customer) { resultingCustomerId customerMergeErrors { message } }
          customerMergeJobStatus(jobId: $job) { jobId status resultingCustomerId customerMergeErrors { message } }
          analyticsTargets(first: 5) { nodes { id name metric expectedValue currencyCode startDate endDate filters shopifyqlQuery createdAt updatedAt } }
          shop { analyticsAnnotations(first: 5) { nodes { id type title startedAt endedAt description source createdAt updatedAt createdByApp { id } } } }
          shopifyqlQuery(query: \"FROM sales SHOW total_sales, orders TIMESERIES month SINCE -3m\") { parseErrors parseWarnings analyticsTargets { id }
            tableData { columns { name dataType displayName columnOrigin } rows rowMetadata { rawResourceIds } } }
          consentPolicy { id } consentPolicyRegions { countryCode }
          privacySettings { banner { enabled autoManaged } dataSaleOptOutPage { autoManaged } privacyPolicy { autoManaged supportedLocales } }
        }",
        json!({"segment": segment, "customer": jane, "query": members_query, "activity": activity, "event": event,
            "abandonment": abandonment, "checkout": checkout, "job": job, "other": camille}),
    );
    assert!(answer.get("errors").is_none(), "{answer}");
    assert_eq!(answer["extensions"].get("lsf"), None, "{answer}");
    assert_eq!(answer["data"]["abandonedCheckoutsCount"]["count"], 1);
}
