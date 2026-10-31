//! The deprecated fulfillment mutations, which run their successors': `fulfillmentCreateV2`
//! ("Use `fulfillmentCreate` instead.") and `fulfillmentTrackingInfoUpdateV2` ("Use
//! `fulfillmentTrackingInfoUpdate` instead."). Their arguments and payloads have the shape of the
//! successors' (`FulfillmentV2Input` is `FulfillmentInput` field for field, each payload a
//! `fulfillment` and `userErrors`), so the successor runs as it is, with its validation, its
//! changes and its webhooks (`fulfillments/create`, `orders/fulfilled`, …).

use serde_json::Value;

use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, mutation};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("fulfillmentCreateV2", |m, ctx| {
            successor(m, ctx, "fulfillmentCreate")
        })
        .mutation("fulfillmentTrackingInfoUpdateV2", |m, ctx| {
            successor(m, ctx, "fulfillmentTrackingInfoUpdate")
        });
}

/// Runs the mutation that replaced this one, with the same arguments.
fn successor(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    name: &str,
) -> Result<Value, FieldError> {
    match m.registry().mutation_fn(name) {
        Some(run) => run(m, ctx),
        None => Err(mutation::not_simulated(name)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domains::catalog::mutation_tests::{demo, ok, run, run_with, topics, unchanged};

    /// The first fulfillment order of an order of the demo store.
    fn fulfillment_order(shop: &mut crate::state::ShopState, name: &str) -> serde_json::Value {
        let order = shop
            .keys("Order")
            .find(|key| shop.get(key).unwrap()["name"] == json!(name))
            .unwrap()
            .to_string();
        let answer = run_with(
            shop,
            r#"query($id: ID!) { order(id: $id) { fulfillmentOrders(first: 1) { nodes { id } } } }"#,
            json!({"id": order}),
        );
        answer["data"]["order"]["fulfillmentOrders"]["nodes"][0]["id"].clone()
    }

    #[test]
    fn the_v2_fulfillment_mutations_run_their_successors() {
        let mut shop = demo();
        let fulfillment_order = fulfillment_order(&mut shop, "#1003");
        shop.take_events();
        let created = run_with(
            &mut shop,
            r#"mutation($input: FulfillmentV2Input!) { fulfillmentCreateV2(fulfillment: $input) {
                fulfillment { id status trackingInfo { company number } } userErrors { field message } } }"#,
            json!({"input": {"lineItemsByFulfillmentOrder": [{"fulfillmentOrderId": fulfillment_order}],
                "trackingInfo": {"company": "UPS", "number": "1Z1"}}}),
        );
        let fulfillment = &ok(&created, "fulfillmentCreateV2")["fulfillment"];
        assert_eq!(fulfillment["status"], json!("SUCCESS"));
        assert_eq!(
            fulfillment["trackingInfo"],
            json!([{"company": "UPS", "number": "1Z1"}])
        );
        assert!(topics(&mut shop).contains(&"fulfillments/create".to_string()));
        let tracked = run_with(
            &mut shop,
            r#"mutation($id: ID!) { fulfillmentTrackingInfoUpdateV2(fulfillmentId: $id, trackingInfoInput: {company: "DHL", number: "A1"}) {
                fulfillment { trackingInfo { company number } } userErrors { field message } } }"#,
            json!({"id": fulfillment["id"]}),
        );
        assert_eq!(
            ok(&tracked, "fulfillmentTrackingInfoUpdateV2")["fulfillment"]["trackingInfo"],
            json!([{"company": "DHL", "number": "A1"}])
        );
        assert_eq!(topics(&mut shop), ["fulfillments/update"]);
    }

    #[test]
    fn a_v2_fulfillment_mutation_refuses_what_its_successor_refuses() {
        let mut shop = demo();
        let before = shop.clone();
        let refused = run(
            &mut shop,
            r#"mutation { fulfillmentTrackingInfoUpdateV2(fulfillmentId: "gid://shopify/Fulfillment/1", trackingInfoInput: {number: "A1"}) {
                fulfillment { id } userErrors { field message } } }"#,
        );
        let payload = &refused["data"]["fulfillmentTrackingInfoUpdateV2"];
        assert_eq!(payload["fulfillment"], json!(null));
        assert!(
            !payload["userErrors"].as_array().unwrap().is_empty(),
            "{refused}"
        );
        let refused = run(
            &mut shop,
            r#"mutation { fulfillmentCreateV2(fulfillment: {lineItemsByFulfillmentOrder: [{fulfillmentOrderId: "gid://shopify/FulfillmentOrder/1"}]}) {
                fulfillment { id } userErrors { field message } } }"#,
        );
        assert!(
            !refused["data"]["fulfillmentCreateV2"]["userErrors"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{refused}"
        );
        unchanged(&mut shop, &before, &[]);
    }
}
