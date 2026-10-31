//! The contracts of an order placed at the local checkout: "After a customer purchases a
//! subscription product or variant at checkout, Shopify generates a subscription contract and
//! shares the subscription contract with your app using a webhook" (shopify.dev, "About
//! subscription contracts").
//!
//! For each recurring selling plan the order's lines were bought with, a contract: the lines of
//! the plan (their price at checkout, the plan's pricing policies as the line's pricing policy),
//! the plan's billing and delivery policies, the order's shipping address and delivery option
//! (its price on the first contract), the customer, a payment method vaulted from the checkout's
//! payment, `originOrder` the order, `nextBillingDate` the end of the first cycle. The order's
//! line items name their selling plan (`LineItem.sellingPlan`). Events:
//! `customer_payment_methods/create` for a method made, `subscription_contracts/create`.
//!
//! A guest order (no customer) makes no contract: Shopify requires a customer account to buy a
//! subscription. The vaulted method is a card that succeeds (Visa ending in 4242, lsf's
//! convention), a PayPal billing agreement when the payment's name says PayPal, a Shop Pay
//! agreement when it says Shop Pay; the customer's method of an earlier checkout with the same
//! payment is reused.

use chrono::Datelike;
use serde_json::{Value, json};

use super::contracts::commit_new;
use super::cycles;
use super::model::{self, PAYMENT_METHOD, text};
use super::payment_methods::create_method;
use crate::domains::catalog::gids;
use crate::domains::orders::PlacedOrder;
use crate::state::{Record, ShopState, value};

/// The contracts of the subscription lines of an order of the checkout. Answers their GIDs.
pub fn contracts_from_order(
    state: &mut ShopState,
    order: &str,
    placed: &PlacedOrder<'_>,
) -> Vec<String> {
    let Some(order_record) = state.get(order).cloned() else {
        return Vec::new();
    };
    let Some(customer) = text(&order_record, "customer").map(str::to_string) else {
        return Vec::new();
    };
    let line_items = gids(order_record.get("lineItems"));
    // The recurring plans, in the order of their first line.
    let mut plans: Vec<String> = Vec::new();
    for line in &placed.result.lines {
        if let Some(plan) = &line.selling_plan
            && plan.recurring_deliveries
            && state.type_of(&plan.id) == Some("SellingPlan")
            && !plans.contains(&plan.id)
        {
            plans.push(plan.id.clone());
        }
    }
    if plans.is_empty() {
        return Vec::new();
    }
    // Every line item bought with a plan names it.
    for (position, line) in placed.result.lines.iter().enumerate() {
        if let (Some(plan), Some(item)) = (&line.selling_plan, line_items.get(position))
            && let Some(record) = state.get_mut(item)
        {
            record.insert(
                "sellingPlan".into(),
                json!({"name": plan.name, "sellingPlanId": plan.id}),
            );
        }
    }
    let currency = text(&order_record, "currencyCode")
        .unwrap_or("USD")
        .to_string();
    let method = vaulted_method(state, &customer, placed.payment);
    let shipping = placed.result.delivery_groups.iter().find_map(|group| {
        let handle = group.selected.as_ref()?;
        group
            .options
            .iter()
            .find(|option| &option.handle == handle)
            .map(|option| (option.clone(), group.selected_cost()))
    });
    let address = order_record
        .get("shippingAddress")
        .filter(|address| !address.is_null())
        .cloned()
        .map(model::located);
    let mut made = Vec::new();
    for (position, plan) in plans.iter().enumerate() {
        let plan_record = state.get(plan).cloned().unwrap_or_default();
        let mut lines = Vec::new();
        for line in placed.result.lines.iter().filter(|line| {
            line.selling_plan
                .as_ref()
                .is_some_and(|known| &known.id == plan)
        }) {
            let variant = crate::state::gid("ProductVariant", line.variant_id);
            let Some(fields) = model::variant_fields(state, &variant) else {
                continue;
            };
            let base = fields
                .get("lsfPrice")
                .and_then(Value::as_i64)
                .unwrap_or(line.unit_price);
            let id = model::uuid_gid(state, "SubscriptionLine");
            let input = value::object(json!({
                "sellingPlanId": plan,
                "sellingPlanName": line.selling_plan.as_ref().map(|plan| plan.name.clone()),
                "customAttributes": line.attributes.iter().map(|attribute| json!({"key": attribute.key, "value": attribute.value})).collect::<Vec<_>>(),
            }));
            let mut made_line = model::line_record(
                state,
                id,
                fields,
                i64::from(line.quantity),
                line.unit_price,
                &input,
                &currency,
            );
            made_line["pricingPolicy"] = pricing_policy(&plan_record, base, &currency);
            lines.push(made_line);
        }
        if lines.is_empty() {
            continue;
        }
        let delivery_method = address.as_ref().map(|address| {
            json!({
                "__typename": "SubscriptionDeliveryMethodShipping",
                "address": address,
                "shippingOption": {
                    "code": shipping.as_ref().map(|(option, _)| option.handle.clone()),
                    "title": shipping.as_ref().map(|(option, _)| option.title.clone()),
                    "presentmentTitle": shipping.as_ref().map(|(option, _)| option.title.clone()),
                    "description": shipping.as_ref().and_then(|(option, _)| option.description.clone()),
                    "carrierService": null,
                },
            })
        });
        let delivery_price = if position == 0 {
            shipping.as_ref().map_or(0, |(_, cost)| *cost)
        } else {
            0
        };
        let billing = plan_record
            .get("billingPolicy")
            .cloned()
            .unwrap_or(Value::Null);
        let delivery = plan_record
            .get("deliveryPolicy")
            .cloned()
            .unwrap_or(Value::Null);
        let interval = billing.get("interval").cloned().unwrap_or(json!("MONTH"));
        let count = billing.get("intervalCount").cloned().unwrap_or(json!(1));
        let record: Record = value::object(json!({
            "status": "ACTIVE",
            "customer": customer,
            "customerPaymentMethod": method,
            "currencyCode": currency,
            "billingPolicy": {
                "interval": interval,
                "intervalCount": count,
                "anchors": billing.get("anchors").cloned().unwrap_or(json!([])),
                "minCycles": billing.get("minCycles"),
                "maxCycles": billing.get("maxCycles"),
                "cadence": {"count": count, "unit": interval},
            },
            "deliveryPolicy": {
                "interval": delivery.get("interval").cloned().unwrap_or(interval.clone()),
                "intervalCount": delivery.get("intervalCount").cloned().unwrap_or(count.clone()),
                "anchors": delivery.get("anchors").cloned().unwrap_or(json!([])),
                "multiFulfillment": null,
            },
            "deliveryMethod": delivery_method,
            "deliveryPrice": model::money(delivery_price, &currency),
            "lines": lines,
            "discounts": [],
            "customAttributes": [],
            "note": null,
            "nextBillingDate": null,
        }));
        let contract = commit_new(state, &record, Some(order));
        if let Some(saved) = state.get(&contract).cloned()
            && let Some(cycle) = cycles::cycle(state, &saved, 1)
        {
            let next = value::date_time(cycles::expected_date(&saved, &cycle));
            if let Some(saved) = state.get_mut(&contract) {
                saved.insert("nextBillingDate".into(), json!(next));
            }
        }
        made.push(contract);
    }
    made
}

/// A selling plan's pricing policies as a line's `SubscriptionPricingPolicy`: the variant's
/// price as the base, each adjustment after its cycle with the price it computes.
fn pricing_policy(plan: &Record, base: i64, currency: &str) -> Value {
    let policies = plan
        .get("pricingPolicies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if policies.is_empty() {
        return Value::Null;
    }
    let cycle_discounts: Vec<Value> = policies
        .iter()
        .map(|policy| {
            let kind = policy
                .get("adjustmentType")
                .and_then(Value::as_str)
                .unwrap_or("PERCENTAGE");
            let value = policy
                .get("adjustmentValue")
                .cloned()
                .unwrap_or(Value::Null);
            let computed = match kind {
                "PERCENTAGE" => {
                    let percentage = value
                        .get("percentage")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0);
                    base - ((base as f64) * percentage / 100.0).round() as i64
                }
                "FIXED_AMOUNT" => base - crate::domains::orders::model::cents(value.get("amount")),
                _ => crate::domains::orders::model::cents(value.get("amount")),
            };
            json!({
                "adjustmentType": kind,
                "adjustmentValue": value,
                "afterCycle": policy.get("afterCycle").cloned().unwrap_or(json!(0)),
                "computedPrice": model::money(computed.max(0), currency),
            })
        })
        .collect();
    json!({"basePrice": model::money(base, currency), "cycleDiscounts": cycle_discounts})
}

/// The payment method a checkout's payment vaults for a customer: the one an earlier checkout
/// vaulted with the same payment, else a new one.
fn vaulted_method(state: &mut ShopState, customer: &str, payment: Option<&str>) -> String {
    let payment = payment.unwrap_or("Credit card").to_string();
    let reference = format!("checkout/{payment}");
    if let Some(known) = state
        .all(PAYMENT_METHOD)
        .find(|method| {
            text(method, "customer") == Some(customer)
                && text(method, "lsfReference") == Some(reference.as_str())
                && method.get("revokedAt").is_none_or(Value::is_null)
        })
        .and_then(|method| text(method, "id").map(str::to_string))
    {
        return known;
    }
    let lower = payment.to_ascii_lowercase();
    let name = state
        .get(customer)
        .and_then(|record| text(record, "displayName"))
        .unwrap_or_default()
        .to_string();
    let email = state
        .get(customer)
        .and_then(|record| text(record, "email"))
        .map(str::to_string);
    let year = i64::from(state.now().year()) + 3;
    let id = create_method(
        state,
        customer,
        |method| {
            if lower.contains("paypal") {
                json!({
                    "__typename": "CustomerPaypalBillingAgreement",
                    "billingAddress": null,
                    "inactive": false,
                    "paypalAccountEmail": email,
                    "lsfMethod": method,
                })
            } else {
                let shop_pay = lower.contains("shop pay");
                json!({
                    "__typename": if shop_pay { "CustomerShopPayAgreement" } else { "CustomerCreditCard" },
                    "brand": "Visa",
                    "expiryMonth": 12,
                    "expiryYear": year,
                    "firstDigits": "424242",
                    "lastDigits": "4242",
                    "maskedNumber": "•••• •••• •••• 4242",
                    "name": name,
                    "inactive": false,
                    "source": null,
                    "virtualLastDigits": null,
                    "billingAddress": null,
                    "lsfMethod": method,
                })
            }
        },
        "success",
    );
    if let Some(record) = state.get_mut(&id) {
        record.insert("lsfReference".into(), json!(reference));
    }
    id
}
