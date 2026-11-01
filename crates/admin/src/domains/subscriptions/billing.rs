//! Billing: `subscriptionBillingAttemptCreate`, `subscriptionBillingCycleCharge`,
//! `subscriptionBillingCycleBulkSearch`, `subscriptionBillingCycleBulkCharge`; the
//! `subscriptionBillingCycleBulkResults` query; the completion of a 3D Secure challenge.
//!
//! As shopify.dev's reference and its guide "Build a subscription contract" (step 4) describe
//! them:
//!
//! - An attempt bills a cycle: the one `billingCycleSelector` names, else the one `originTime`
//!   falls in, else the current one (see [`cycles::due`]). Its `idempotencyKey` makes it run
//!   once: the same key on the same contract answers the first attempt again.
//! - Refused (`BillingAttemptUserError`): a contract that is paused (`CONTRACT_PAUSED`),
//!   cancelled or expired (`CONTRACT_TERMINATED`), a skipped cycle (`BILLING_CYCLE_SKIPPED`), a
//!   cycle out of range, an `originTime` before the contract or outside the cycle;
//!   `subscriptionBillingCycleCharge` more than 24 hours before the cycle's
//!   `billingAttemptExpectedDate` (`BILLING_CYCLE_CHARGE_BEFORE_EXPECTED_DATE`).
//! - "Billing attempts are processed asynchronously": the mutation answers the attempt pending
//!   (`ready: false`, `SubscriptionBillingAttemptPendingState`), as Shopify's examples do; lsf
//!   processes it within the request, so the next query reads its outcome. Like bulk
//!   operations, it is done before the answer.
//! - **Success** creates the order through the orders family ([`orders::insert_order`]): the
//!   lines of the cycle's contract (its edit, or the contract) at their current prices, their
//!   discount allocations, the delivery method's address and price less its discounts, the tax
//!   at the store data's rate, `sourceName: subscription_contract`, the line items' selling
//!   plans, a sale of the total through the method's gateway; the stock is committed at the
//!   location that can give every line (`orders::model::fulfilling_location`), as an order that
//!   claims inventory does. Then `orders/create`,
//!   `order_transactions/create` and `tender_transactions/create`, `orders/paid`, and
//!   `subscription_billing_attempts/success`.
//!   The contract's `orders` list it, `lastPaymentStatus` is `SUCCEEDED`, each discount that
//!   applied counts a use (`usageCount`, up to `recurringCycleLimit`).
//! - **Failure** (`SubscriptionBillingAttemptFailedState` and the deprecated `errorCode`,
//!   `errorMessage`, `processingError`): stock short for a variant that does not sell when out of
//!   stock, unless `ALLOW_OVERSELLING` (`INSUFFICIENT_INVENTORY`, with the variants); no payment
//!   method (`PAYMENT_METHOD_NOT_SPECIFIED`), a revoked one (`PAYMENT_METHOD_NOT_FOUND`), an
//!   expired card (`EXPIRED_CARD`), or the outcome of the method's test card (see
//!   [`super::payment_methods`]). `lastPaymentStatus` is `FAILED`, `lastBillingAttemptErrorType`
//!   says which kind; `subscription_billing_attempts/failure`. The contract's status is the
//!   app's to change (dunning), as on Shopify.
//! - **A 3D Secure challenge** (`SubscriptionBillingAttemptActionRequiredState`, its
//!   `nextActionUrl` a page of lsf, `status: OFF_SESSION_REJECTED` until the customer opens it,
//!   then `ON_SESSION_CHALLENGED`): `subscription_billing_attempts/challenged`; completing the
//!   challenge on the page ([`complete_challenge`]) bills the attempt (`/success`), failing it
//!   fails the attempt with `AUTHENTICATION_FAILED` (`/failure`): "The
//!   subscription_billing_attempts/success and subscription_billing_attempts/failure webhooks
//!   aren't triggered until the challenge is completed".
//! - `SKIP_PAYMENT_AND_CREATE_UNPAID_ORDER` creates the order unpaid, whatever the method.
//! - The bulk mutations find the cycles whose `billingAttemptExpectedDate` falls in the range
//!   (one week at most, ending no more than 24 hours ahead) and match the filters, charge them
//!   (`BulkCharge`), and answer a `Job` already done, whose cycles
//!   `subscriptionBillingCycleBulkResults(jobId:)` lists.

use chrono::{Datelike, Duration};
use serde_json::{Map, Value, json};

use super::cycles::{self, Cycle};
use super::drafts::cycle_contract;
use super::model::{self, ATTEMPT, CONTRACT, Pointer, attempt_error, lines, object, text};
use crate::domains::catalog::gids;
use crate::domains::customers;
use crate::domains::orders::{
    self, FinancialStatus, LineDraft, OrderDraft, ShippingDraft, model as orders_model,
};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, gid_number, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("subscriptionBillingAttemptCreate", attempt_create)
        .mutation("subscriptionBillingCycleCharge", cycle_charge)
        .mutation("subscriptionBillingCycleBulkSearch", |m, ctx| {
            bulk(m, ctx, false)
        })
        .mutation("subscriptionBillingCycleBulkCharge", |m, ctx| {
            bulk(m, ctx, true)
        })
        .query("subscriptionBillingCycleBulkResults", bulk_results);
}

/// How an attempt is made.
pub struct AttemptSpec<'a> {
    pub contract: &'a str,
    pub cycle: Cycle,
    pub idempotency_key: String,
    pub origin_time: Option<String>,
    /// `ALLOW_OVERSELLING`: the order is made whatever the stock.
    pub oversell: bool,
    /// `SKIP_PAYMENT_AND_CREATE_UNPAID_ORDER`.
    pub skip_payment: bool,
    /// Where the challenge page is served.
    pub origin: &'a str,
}

/// What a charge came to.
enum Outcome {
    Paid,
    Unpaid,
    Challenge,
    /// The `SubscriptionBillingAttemptErrorCode` and the variants short of stock.
    Failed(String, Vec<String>),
}

/// The inventory errors of `SubscriptionBillingAttemptErrorCode`.
const INVENTORY_ERRORS: [&str; 2] = ["INSUFFICIENT_INVENTORY", "INVENTORY_ALLOCATIONS_NOT_FOUND"];

/// The general errors of `SubscriptionBillingAttemptGeneralErrorCode`; besides them and
/// [`INVENTORY_ERRORS`], the codes are payment errors.
const GENERAL_ERRORS: [&str; 10] = [
    "CUSTOMER_INVALID",
    "CUSTOMER_NOT_FOUND",
    "FREE_GIFT_CARD_NOT_ALLOWED",
    "INVALID_BILLING_ADDRESS",
    "MERCHANT_ACCOUNT_ERROR",
    "NON_TEST_ORDER_LIMIT_REACHED",
    "PAYMENT_METHOD_INCOMPATIBLE_WITH_GATEWAY_CONFIG",
    "PAYMENT_METHOD_NOT_FOUND",
    "PAYMENT_METHOD_NOT_SPECIFIED",
    "PAYMENT_PROVIDER_IS_NOT_ENABLED",
];

/// The cycle contract an attempt bills (the anchor's, when the cycle is concatenated into
/// another contract's), with the contracts the order is linked to.
fn billed_contract(
    state: &ShopState,
    contract: &str,
    index: i64,
) -> (String, i64, Record, Vec<(String, i64)>) {
    let record = state.get(contract).cloned().unwrap_or_default();
    if let Some(into) =
        cycles::edits(&record, index).and_then(|edits| edits.get("concatenatedInto"))
    {
        let anchor = into
            .get("contract")
            .and_then(Value::as_str)
            .unwrap_or(contract)
            .to_string();
        let anchor_index = into.get("index").and_then(Value::as_i64).unwrap_or(index);
        if anchor != contract && state.contains(&anchor) {
            return billed_contract(state, &anchor, anchor_index);
        }
    }
    let members: Vec<(String, i64)> = cycles::edits(&record, index)
        .and_then(|edits| edits.get("concatenated"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|member| {
            Some((
                member.get("contract")?.as_str()?.to_string(),
                member.get("index")?.as_i64()?,
            ))
        })
        .collect();
    let mut billed = cycle_contract(&record, index);
    for field in ["customer", "currencyCode", "customerPaymentMethod"] {
        if billed.get(field).is_none_or(Value::is_null) {
            billed.insert(
                field.into(),
                record.get(field).cloned().unwrap_or(Value::Null),
            );
        }
    }
    (contract.to_string(), index, billed, members)
}

/// Where the order of a billed contract is fulfilled from: where every line can come from
/// (`orders::model::fulfilling_location`).
fn location_for(state: &ShopState, billed: &Record) -> Option<String> {
    let wanted: Vec<(String, i64)> = lines(billed)
        .iter()
        .filter_map(|line| {
            Some((
                line.get("variantId")?.as_str()?.to_string(),
                line.get("quantity").and_then(Value::as_i64).unwrap_or(0),
            ))
        })
        .collect();
    orders_model::fulfilling_location(state, &wanted)
}

/// The variants of the lines that lack stock: tracked, not sold when out of stock, and with
/// less available at the attempt's location than the line takes (a bundle: one of its components,
/// `orders::model::short_parts`).
fn short_variants(state: &ShopState, billed: &Record, location: Option<&str>) -> Vec<String> {
    lines(billed)
        .iter()
        .filter_map(|line| {
            let variant = line.get("variantId")?.as_str()?;
            let quantity = line.get("quantity").and_then(Value::as_i64).unwrap_or(0);
            let short = !orders_model::short_parts(state, variant, location, quantity).is_empty();
            short.then(|| variant.to_string())
        })
        .collect()
}

/// What charging the contract's payment method does.
fn payment_outcome(state: &ShopState, billed: &Record) -> Outcome {
    let customer = text(billed, "customer").unwrap_or_default();
    if state.type_of(customer) != Some("Customer") {
        return Outcome::Failed("CUSTOMER_NOT_FOUND".into(), Vec::new());
    }
    let Some(method) = text(billed, "customerPaymentMethod").and_then(|method| state.get(method))
    else {
        return Outcome::Failed("PAYMENT_METHOD_NOT_SPECIFIED".into(), Vec::new());
    };
    if method.get("revokedAt").is_some_and(|at| !at.is_null()) {
        return Outcome::Failed("PAYMENT_METHOD_NOT_FOUND".into(), Vec::new());
    }
    let outcome = text(method, "lsfOutcome").unwrap_or("success");
    if outcome == "success" {
        let instrument = method.get("instrument").cloned().unwrap_or(Value::Null);
        let month = instrument.get("expiryMonth").and_then(Value::as_i64);
        let year = instrument.get("expiryYear").and_then(Value::as_i64);
        if let (Some(month), Some(year)) = (month, year) {
            let now = state.now();
            if year * 12 + month < i64::from(now.year()) * 12 + i64::from(now.month()) {
                return Outcome::Failed("EXPIRED_CARD".into(), Vec::new());
            }
        }
        return Outcome::Paid;
    }
    if outcome == "challenge" {
        return Outcome::Challenge;
    }
    Outcome::Failed(outcome.to_string(), Vec::new())
}

/// The gateway of a payment method's charges.
fn gateway(state: &ShopState, billed: &Record) -> String {
    let Some(method) = text(billed, "customerPaymentMethod").and_then(|method| state.get(method))
    else {
        return "manual".to_string();
    };
    if let Some(gateway) = text(method, "lsfGateway") {
        return gateway.to_ascii_lowercase().replace('.', "_");
    }
    match method
        .pointer("/instrument/__typename")
        .and_then(Value::as_str)
    {
        Some("CustomerPaypalBillingAgreement") => "paypal".to_string(),
        Some("CustomerShopPayAgreement") => "shopify_payments".to_string(),
        _ if method.pointer("/instrument/brand").and_then(Value::as_str) == Some("Bogus") => {
            "bogus".to_string()
        }
        _ => "shopify_payments".to_string(),
    }
}

/// The order of a billed cycle, made through the orders family: lines, delivery, tax, the sale.
fn create_order(
    state: &mut ShopState,
    billed: &Record,
    paid: bool,
    location: Option<&str>,
) -> String {
    let currency_lines = lines(billed);
    // A contract in another currency than the shop's bills an order presented in it: its
    // amounts in the shop's currency at lsf's rate (a market's manual rate, else 1.0).
    let currency = text(billed, "currencyCode").unwrap_or_default().to_string();
    let presentment =
        (!currency.is_empty() && currency != state.currency()).then(|| orders_model::Presentment {
            rate_bps: crate::domains::markets::rate_bps(state, &currency).unwrap_or(10_000),
            currency: currency.clone(),
        });
    let shop_cents = |cents: i64| {
        presentment
            .as_ref()
            .map_or(cents, |presentment| presentment.to_shop(cents))
    };
    let (_, _, delivery, delivery_discounts, tax, _) = model::amounts(state, billed);
    let (delivery, delivery_discounts, tax) = (
        shop_cents(delivery),
        shop_cents(delivery_discounts),
        shop_cents(tax),
    );
    let customer = text(billed, "customer").map(str::to_string);
    let customer_record = customer
        .as_deref()
        .and_then(|customer| state.get(customer))
        .cloned();
    let id = state.new_gid("Order");
    let number = orders_model::next_order_number(state);
    let line_drafts: Vec<LineDraft> = currency_lines
        .iter()
        .map(|line| {
            let quantity = line.get("quantity").and_then(Value::as_i64).unwrap_or(0);
            let unit = shop_cents(orders_model::cents(line.get("currentPrice")));
            let discounted = shop_cents(orders_model::cents(line.get("lineDiscountedPrice")));
            let variant = line
                .get("variantId")
                .and_then(Value::as_str)
                .map(str::to_string);
            let inventory_item = variant
                .as_deref()
                .and_then(|variant| state.get(variant))
                .and_then(|record| text(record, "inventoryItem"))
                .map(str::to_string);
            LineDraft {
                product: line
                    .get("productId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                variant,
                inventory_item,
                title: line
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                variant_title: line
                    .get("variantTitle")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                sku: line.get("sku").and_then(Value::as_str).map(str::to_string),
                vendor: None,
                quantity: u32::try_from(quantity.max(0)).unwrap_or(0),
                unit_price: unit,
                discount: (unit * quantity - discounted).max(0),
                requires_shipping: line
                    .get("requiresShipping")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                taxable: line.get("taxable").and_then(Value::as_bool).unwrap_or(true),
                gift_card: false,
                image: line
                    .get("variantImage")
                    .filter(|image| !image.is_null())
                    .cloned(),
                weight: line
                    .get("weight")
                    .filter(|weight| !weight.is_null())
                    .cloned(),
                grams: 0,
                attributes: crate::domains::orders::model::attribute_pairs(
                    line.get("customAttributes"),
                ),
            }
        })
        .collect();
    let method = billed.get("deliveryMethod").cloned().unwrap_or(Value::Null);
    let shipping = (!method.is_null()).then(|| ShippingDraft {
        title: method
            .pointer("/shippingOption/title")
            .or_else(|| method.pointer("/localDeliveryOption/title"))
            .or_else(|| method.pointer("/pickupOption/title"))
            .and_then(Value::as_str)
            .unwrap_or("Shipping")
            .to_string(),
        handle: method
            .pointer("/shippingOption/code")
            .or_else(|| method.pointer("/localDeliveryOption/code"))
            .or_else(|| method.pointer("/pickupOption/code"))
            .and_then(Value::as_str)
            .map(str::to_string),
        original: delivery,
        discounted: delivery - delivery_discounts,
    });
    let address = model::delivery_address(billed).cloned().map(|mut address| {
        address["id"] = json!(format!(
            "gid://shopify/MailingAddress/{}?model_name=Address",
            gid_number(&id)
        ));
        address
    });
    let included = state
        .shop()
        .and_then(|shop| shop.get("taxesIncluded"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let draft = OrderDraft {
        id: id.clone(),
        name: format!("#{number}"),
        number,
        created_at: state.now(),
        customer: customer.clone(),
        email: customer_record
            .as_ref()
            .and_then(|record| text(record, "email"))
            .map(str::to_string),
        phone: customer_record
            .as_ref()
            .and_then(|record| text(record, "phone"))
            .map(str::to_string),
        accepts_marketing: false,
        tax_exempt: customer_record
            .as_ref()
            .and_then(|record| record.get("taxExempt"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        lines: line_drafts,
        shipping_address: address.clone(),
        billing_address: address,
        shipping,
        tax: if included { 0 } else { tax },
        order_discount: 0,
        discount_codes: model::discounts(billed)
            .iter()
            .filter(|discount| model::applies(discount))
            .filter(|discount| {
                discount.get("type").and_then(Value::as_str) == Some("CODE_DISCOUNT")
            })
            .filter_map(|discount| {
                discount
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect(),
        note: text(billed, "note").map(str::to_string),
        attributes: crate::domains::orders::model::attribute_pairs(billed.get("customAttributes")),
        financial: if paid {
            FinancialStatus::Paid
        } else {
            FinancialStatus::Pending
        },
        fulfillment: orders::FulfillmentState::Unfulfilled,
        cancelled: false,
        paid: None,
        gateway: Some(gateway(state, billed)),
        source_name: Some("subscription_contract".to_string()),
        returns: Vec::new(),
        location: location.map(str::to_string),
        presentment,
    };
    let order = orders::insert_order(state, &draft);
    let now = state.timestamp();
    let line_items = state
        .get(&order)
        .map(|record| gids(record.get("lineItems")))
        .unwrap_or_default();
    for (line_item, line) in line_items.iter().zip(&currency_lines) {
        if let Some(record) = state.get_mut(line_item)
            && let Some(name) = line.get("sellingPlanName").and_then(Value::as_str)
        {
            record.insert(
                "sellingPlan".into(),
                json!({"name": name, "sellingPlanId": line.get("sellingPlanId")}),
            );
        }
    }
    if let Some(record) = state.get_mut(&order) {
        record.insert("createdAt".into(), json!(now));
        record.insert("updatedAt".into(), json!(now));
        record.insert("test".into(), json!(false));
        record.insert("tags".into(), json!([]));
    }
    state.emit("orders/create", &order);
    for transaction in state
        .get(&order)
        .map(|record| gids(record.get("transactions")))
        .unwrap_or_default()
    {
        state.emit("order_transactions/create", &transaction);
        // Its tender transaction, as `orderCreate` and the checkout's orders emit it.
        crate::domains::orders::payments::emit_tender(state, &transaction);
    }
    for line_item in &line_items {
        let quantity = state
            .get(line_item)
            .and_then(|record| record.get("quantity"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        orders_model::commit_line(state, line_item, location, quantity);
    }
    orders_model::refresh_order(state, &order);
    if let Some(customer) = &customer {
        customers::refresh_customer(state, customer);
    }
    if paid
        && state
            .get(&order)
            .and_then(|record| text(record, "displayFinancialStatus"))
            == Some("PAID")
    {
        state.emit("orders/paid", &order);
    }
    order
}

/// The failed state of an attempt and its deprecated fields, for an error code.
fn failure_fields(code: &str, variants: &[String]) -> Map<String, Value> {
    let message = model::described("SubscriptionBillingAttemptErrorCode", code);
    let error = if INVENTORY_ERRORS.contains(&code) {
        json!({
            "__typename": "SubscriptionBillingAttemptInventoryError",
            "code": code,
            "insufficientStockProductVariants": variants,
        })
    } else if GENERAL_ERRORS.contains(&code) {
        json!({"__typename": "SubscriptionBillingAttemptGeneralError", "code": code})
    } else {
        json!({"__typename": "SubscriptionBillingAttemptPaymentError", "code": code})
    };
    let processing = if variants.is_empty() {
        json!({"__typename": "SubscriptionBillingAttemptGenericError", "code": code, "message": message})
    } else {
        json!({
            "__typename": "SubscriptionBillingAttemptInsufficientStockProductVariantsError",
            "code": code,
            "message": message,
            "insufficientStockProductVariants": variants,
            "outOfStockProductVariants": variants,
        })
    };
    value::object(json!({
        "state": {"__typename": "SubscriptionBillingAttemptFailedState", "error": error},
        "errorCode": code,
        "errorMessage": message,
        "processingError": processing,
    }))
}

/// The kind of error of a code, as `lastBillingAttemptErrorType` says it.
fn error_type(code: &str) -> &'static str {
    if INVENTORY_ERRORS.contains(&code) {
        "INVENTORY_ERROR"
    } else if code.starts_with("CUSTOMER") {
        "CUSTOMER_ERROR"
    } else if GENERAL_ERRORS.contains(&code) {
        "OTHER"
    } else {
        "PAYMENT_ERROR"
    }
}

/// Makes a billing attempt and processes it. Answers its GID. Emits what its outcome fires.
pub fn attempt(state: &mut ShopState, spec: &AttemptSpec<'_>) -> String {
    let (anchor, anchor_index, billed, members) =
        billed_contract(state, spec.contract, spec.cycle.index);
    let id = state.new_gid(ATTEMPT);
    let now = state.timestamp();
    let mut record = value::object(json!({
        "__typename": ATTEMPT,
        "id": id,
        "subscriptionContract": spec.contract,
        "idempotencyKey": spec.idempotency_key,
        "originTime": spec.origin_time,
        "createdAt": now,
        "completedAt": null,
        "ready": false,
        "state": {"__typename": "SubscriptionBillingAttemptPendingState", "processing": true},
        "order": null,
        "errorCode": null,
        "errorMessage": null,
        "processingError": null,
        "nextActionUrl": null,
        "respectInventoryPolicy": !spec.oversell,
        "transactions": [],
        "paymentGroupId": null,
        "paymentSessionId": null,
        "lsfCycleIndex": spec.cycle.index,
        "lsfAnchor": {"contract": anchor, "index": anchor_index},
        "lsfMembers": members.iter().map(|(contract, index)| json!({"contract": contract, "index": index})).collect::<Vec<_>>(),
        "lsfOversell": spec.oversell,
    }));
    let location = location_for(state, &billed);
    let short = if spec.oversell {
        Vec::new()
    } else {
        short_variants(state, &billed, location.as_deref())
    };
    let outcome = if !short.is_empty() {
        Outcome::Failed("INSUFFICIENT_INVENTORY".into(), short)
    } else if spec.skip_payment {
        Outcome::Unpaid
    } else {
        payment_outcome(state, &billed)
    };
    if let Outcome::Challenge = outcome {
        let url = challenge_url(spec.origin, state.domain(), &id);
        record.insert("ready".into(), json!(true));
        record.insert("nextActionUrl".into(), json!(url));
        record.insert(
            "state".into(),
            json!({
                "__typename": "SubscriptionBillingAttemptActionRequiredState",
                "action": {
                    "__typename": "SubscriptionBillingAttemptPaymentChallenge",
                    "nextActionUrl": url,
                    "status": "OFF_SESSION_REJECTED",
                },
            }),
        );
    }
    state.insert(record);
    for (contract, _) in
        std::iter::once((spec.contract.to_string(), spec.cycle.index)).chain(members.clone())
    {
        if let Some(record) = state.get_mut(&contract) {
            let mut attempts = gids(record.get("billingAttempts"));
            if !attempts.contains(&id) {
                attempts.push(id.clone());
            }
            record.insert("billingAttempts".into(), json!(attempts));
        }
    }
    match outcome {
        Outcome::Challenge => {
            state.emit("subscription_billing_attempts/challenged", &id);
        }
        Outcome::Paid | Outcome::Unpaid => {
            settle_success(
                state,
                &id,
                &billed,
                matches!(outcome, Outcome::Paid),
                location.as_deref(),
            );
        }
        Outcome::Failed(code, variants) => settle_failure(state, &id, &code, &variants),
    }
    id
}

/// Bills a pending attempt: its order, its state, the contracts' payment status and orders,
/// the discounts' uses. Emits `subscription_billing_attempts/success`.
fn settle_success(
    state: &mut ShopState,
    id: &str,
    billed: &Record,
    paid: bool,
    location: Option<&str>,
) {
    let order = create_order(state, billed, paid, location);
    let now = state.timestamp();
    let transactions = state
        .get(&order)
        .map(|record| gids(record.get("transactions")))
        .unwrap_or_default();
    let contracts = attempt_contracts(state, id);
    if let Some(record) = state.get_mut(id) {
        record.insert("ready".into(), json!(true));
        record.insert("completedAt".into(), json!(now));
        record.insert("order".into(), json!(order));
        record.insert("transactions".into(), json!(transactions));
        record.insert(
            "state".into(),
            json!({"__typename": "SubscriptionBillingAttemptSuccessState", "order": order}),
        );
        record.insert("nextActionUrl".into(), Value::Null);
    }
    let used: Vec<String> = model::discounts(billed)
        .iter()
        .filter(|discount| model::applies(discount))
        .filter_map(|discount| {
            discount
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    for contract in contracts {
        let Some(mut record) = state.get(&contract).cloned() else {
            continue;
        };
        let mut orders = gids(record.get("orders"));
        orders.push(order.clone());
        record.insert("orders".into(), json!(orders));
        record.insert("lastPaymentStatus".into(), json!("SUCCEEDED"));
        record.insert("lastBillingAttemptErrorType".into(), Value::Null);
        let discounts: Vec<Value> = model::discounts(&record)
            .into_iter()
            .map(|mut discount| {
                if discount
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| used.iter().any(|known| known == id))
                {
                    let count = discount
                        .get("usageCount")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    discount["usageCount"] = json!(count + 1);
                }
                discount
            })
            .collect();
        record.insert("discounts".into(), Value::Array(discounts));
        model::refresh(state, &mut record);
        state.insert(record);
    }
    state.emit("subscription_billing_attempts/success", id);
}

/// Fails an attempt with a code. Emits `subscription_billing_attempts/failure`.
fn settle_failure(state: &mut ShopState, id: &str, code: &str, variants: &[String]) {
    let now = state.timestamp();
    let contracts = attempt_contracts(state, id);
    if let Some(record) = state.get_mut(id) {
        record.extend(failure_fields(code, variants));
        record.insert("ready".into(), json!(true));
        record.insert("completedAt".into(), json!(now));
        record.insert("nextActionUrl".into(), Value::Null);
    }
    for contract in contracts {
        if let Some(record) = state.get_mut(&contract) {
            record.insert("lastPaymentStatus".into(), json!("FAILED"));
            record.insert(
                "lastBillingAttemptErrorType".into(),
                json!(error_type(code)),
            );
        }
    }
    state.emit("subscription_billing_attempts/failure", id);
}

/// The contracts an attempt bills: its own and those concatenated with it.
fn attempt_contracts(state: &ShopState, id: &str) -> Vec<String> {
    let Some(record) = state.get(id) else {
        return Vec::new();
    };
    let mut contracts = vec![
        text(record, "subscriptionContract")
            .unwrap_or_default()
            .to_string(),
    ];
    if let Some(anchor) = record
        .pointer("/lsfAnchor/contract")
        .and_then(Value::as_str)
        && !contracts.iter().any(|known| known == anchor)
    {
        contracts.push(anchor.to_string());
    }
    for member in record
        .get("lsfMembers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(contract) = member.get("contract").and_then(Value::as_str)
            && !contracts.iter().any(|known| known == contract)
        {
            contracts.push(contract.to_string());
        }
    }
    contracts
}

/// The URL of an attempt's 3D Secure challenge: a page of lsf.
pub fn challenge_url(origin: &str, shop: &str, attempt: &str) -> String {
    format!(
        "{origin}/__lsf/admin/billing-attempts/{}/challenge?shop={shop}",
        gid_number(attempt)
    )
}

/// Why a challenge cannot be completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChallengeError {
    NotFound,
    NotChallenged,
}

/// The customer opened the challenge page: its status is `ON_SESSION_CHALLENGED`.
pub fn open_challenge(state: &mut ShopState, number: u64) -> Result<Record, ChallengeError> {
    let id = crate::state::gid(ATTEMPT, number);
    let record = state.get(&id).cloned().ok_or(ChallengeError::NotFound)?;
    if record.pointer("/state/__typename").and_then(Value::as_str)
        != Some("SubscriptionBillingAttemptActionRequiredState")
    {
        return Ok(record);
    }
    if record
        .pointer("/state/action/status")
        .and_then(Value::as_str)
        != Some("ON_SESSION_CHALLENGED")
        && let Some(record) = state.get_mut(&id)
    {
        record["state"]["action"]["status"] = json!("ON_SESSION_CHALLENGED");
        state.emit(crate::domains::app::CHANGED, &id);
    }
    state.get(&id).cloned().ok_or(ChallengeError::NotFound)
}

/// The customer completed (`passed`) or failed the challenge of an attempt: the attempt bills,
/// or fails with `AUTHENTICATION_FAILED`.
pub fn complete_challenge(
    state: &mut ShopState,
    number: u64,
    passed: bool,
) -> Result<Record, ChallengeError> {
    let id = crate::state::gid(ATTEMPT, number);
    let record = state.get(&id).cloned().ok_or(ChallengeError::NotFound)?;
    if record.pointer("/state/__typename").and_then(Value::as_str)
        != Some("SubscriptionBillingAttemptActionRequiredState")
    {
        return Err(ChallengeError::NotChallenged);
    }
    if passed {
        let anchor = record
            .pointer("/lsfAnchor/contract")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let index = record
            .pointer("/lsfAnchor/index")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let (_, _, billed, _) = billed_contract(state, &anchor, index);
        let location = location_for(state, &billed);
        let oversell = record
            .get("lsfOversell")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let short = if oversell {
            Vec::new()
        } else {
            short_variants(state, &billed, location.as_deref())
        };
        if short.is_empty() {
            settle_success(state, &id, &billed, true, location.as_deref());
        } else {
            settle_failure(state, &id, "INSUFFICIENT_INVENTORY", &short);
        }
    } else {
        settle_failure(state, &id, "AUTHENTICATION_FAILED", &[]);
    }
    state.get(&id).cloned().ok_or(ChallengeError::NotFound)
}

/// The attempt as the mutation answers it: pending, as Shopify's answers are (its outcome is
/// read by the next query).
fn pending(state: &ShopState, id: &str) -> Value {
    let mut record = state.get(id).cloned().unwrap_or_default();
    if record.get("lsfReplayed").is_none() {
        record.insert("ready".into(), json!(false));
        record.insert(
            "state".into(),
            json!({"__typename": "SubscriptionBillingAttemptPendingState", "processing": true}),
        );
        for field in [
            "order",
            "completedAt",
            "errorCode",
            "errorMessage",
            "processingError",
            "nextActionUrl",
        ] {
            record.insert(field.into(), Value::Null);
        }
        record.insert("transactions".into(), json!([]));
    }
    Value::Object(record)
}

/// The checks every attempt makes of its contract and cycle (`field` the argument of the
/// contract): the status, the cycle, the origin time.
fn check_attempt(
    state: &ShopState,
    contract_id: &str,
    contract_field: &str,
    selector: Option<&Map<String, Value>>,
    selector_path: &[&str],
    origin_time: Option<&str>,
    origin_path: &[&str],
) -> Result<Cycle, UserError> {
    let Some(contract) = state.get_of(contract_id, CONTRACT) else {
        return Err(attempt_error(&[contract_field], "CONTRACT_NOT_FOUND"));
    };
    match text(contract, "status") {
        Some("PAUSED") => return Err(attempt_error(&[contract_field], "CONTRACT_PAUSED")),
        Some("CANCELLED" | "EXPIRED") => {
            return Err(attempt_error(&[contract_field], "CONTRACT_TERMINATED"));
        }
        _ => {}
    }
    let origin = match origin_time {
        Some(text) => match model::date_time(text) {
            Some(origin) => Some(origin),
            None => return Err(attempt_error(origin_path, "INVALID")),
        },
        None => None,
    };
    let created = text(contract, "createdAt").and_then(model::date_time);
    if let (Some(origin), Some(created)) = (origin, created)
        && origin < created
    {
        return Err(attempt_error(
            origin_path,
            "ORIGIN_TIME_BEFORE_CONTRACT_CREATION",
        ));
    }
    let cycle = match (selector.filter(|selector| !selector.is_empty()), origin) {
        (Some(selector), _) => {
            let cycle = cycles::select(state, contract, selector)
                .map_err(|miss| attempt_error(selector_path, miss.code()))?;
            if let Some(origin) = origin
                && (origin < cycle.start || origin > cycle.end)
            {
                return Err(attempt_error(origin_path, "ORIGIN_TIME_OUT_OF_RANGE"));
            }
            cycle
        }
        (None, Some(origin)) => cycles::cycle_at(state, contract, origin)
            .map_err(|miss| attempt_error(origin_path, miss.code()))?,
        (None, None) => cycles::due(state, contract),
    };
    if cycles::is_skipped(contract, cycle.index) {
        return Err(attempt_error(selector_path, "BILLING_CYCLE_SKIPPED"));
    }
    Ok(cycle)
}

/// The attempt of a contract made with an idempotency key, when there is one.
fn by_key(state: &ShopState, contract: &str, key: &str) -> Option<String> {
    state
        .all(ATTEMPT)
        .find(|attempt| {
            text(attempt, "subscriptionContract") == Some(contract)
                && text(attempt, "idempotencyKey") == Some(key)
        })
        .and_then(|attempt| text(attempt, "id").map(str::to_string))
}

/// `subscriptionBillingAttemptCreate`.
fn attempt_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"subscriptionBillingAttempt": null});
    let contract = ctx
        .arg_str("subscriptionContractId")
        .unwrap_or_default()
        .to_string();
    let input = object(ctx.arg("subscriptionBillingAttemptInput"));
    let key = text(&input, "idempotencyKey")
        .unwrap_or_default()
        .trim()
        .to_string();
    let state = mutation.state();
    if key.is_empty() {
        return Ok(payload(
            empty,
            &[attempt_error(
                &["subscriptionBillingAttemptInput", "idempotencyKey"],
                "BLANK",
            )],
        ));
    }
    if let Some(existing) = by_key(state, &contract, &key) {
        let mut replayed = state.get(&existing).cloned().unwrap_or_default();
        replayed.insert("lsfReplayed".into(), json!(true));
        return Ok(payload(
            json!({"subscriptionBillingAttempt": Value::Object(replayed)}),
            &[],
        ));
    }
    let selector = input.get("billingCycleSelector").and_then(Value::as_object);
    let cycle = match check_attempt(
        state,
        &contract,
        "subscriptionContractId",
        selector,
        &["subscriptionBillingAttemptInput", "billingCycleSelector"],
        text(&input, "originTime"),
        &["subscriptionBillingAttemptInput", "originTime"],
    ) {
        Ok(cycle) => cycle,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let origin = mutation.origin().to_string();
    let spec = AttemptSpec {
        contract: &contract,
        cycle,
        idempotency_key: key,
        origin_time: text(&input, "originTime")
            .and_then(model::date_time)
            .map(value::date_time),
        oversell: text(&input, "inventoryPolicy") == Some("ALLOW_OVERSELLING"),
        skip_payment: text(&input, "paymentProcessingPolicy")
            == Some("SKIP_PAYMENT_AND_CREATE_UNPAID_ORDER"),
        origin: &origin,
    };
    let state = mutation.state_mut();
    let id = attempt(state, &spec);
    Ok(payload(
        json!({"subscriptionBillingAttempt": pending(state, &id)}),
        &[],
    ))
}

/// The idempotency key lsf gives an attempt Shopify makes itself (a cycle charge, a bulk
/// charge): one per contract, cycle and attempt.
fn generated_key(state: &ShopState, contract: &str, index: i64) -> String {
    let count = state
        .get(contract)
        .map(|record| cycles::attempts(state, record, index).len())
        .unwrap_or(0);
    value::uuid(&format!("billing-cycle-charge/{contract}/{index}/{count}"))
}

/// `subscriptionBillingCycleCharge`: an attempt on a selected cycle, no earlier than a day
/// before its expected date.
fn cycle_charge(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"subscriptionBillingAttempt": null});
    let contract = ctx
        .arg_str("subscriptionContractId")
        .unwrap_or_default()
        .to_string();
    let selector = object(ctx.arg("billingCycleSelector"));
    let state = mutation.state();
    let cycle = match check_attempt(
        state,
        &contract,
        "subscriptionContractId",
        Some(&selector),
        &["billingCycleSelector"],
        None,
        &["originTime"],
    ) {
        Ok(cycle) => cycle,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let record = state.get(&contract).cloned().unwrap_or_default();
    if state.now() < cycles::expected_date(&record, &cycle) - Duration::hours(24) {
        return Ok(payload(
            empty,
            &[attempt_error(
                &["billingCycleSelector"],
                "BILLING_CYCLE_CHARGE_BEFORE_EXPECTED_DATE",
            )],
        ));
    }
    let key = generated_key(state, &contract, cycle.index);
    let origin = mutation.origin().to_string();
    let spec = AttemptSpec {
        contract: &contract,
        cycle,
        idempotency_key: key,
        origin_time: None,
        oversell: ctx.arg_str("inventoryPolicy") == Some("ALLOW_OVERSELLING"),
        skip_payment: ctx.arg_str("paymentProcessingPolicy")
            == Some("SKIP_PAYMENT_AND_CREATE_UNPAID_ORDER"),
        origin: &origin,
    };
    let state = mutation.state_mut();
    let id = attempt(state, &spec);
    Ok(payload(
        json!({"subscriptionBillingAttempt": pending(state, &id)}),
        &[],
    ))
}

/// A `SubscriptionBillingCycleBulkUserError`.
fn bulk_error(field: &[&str], code: &str) -> UserError {
    UserError::new(
        field,
        model::described("SubscriptionBillingCycleBulkUserErrorCode", code),
    )
    .code(code)
}

/// The cycles of the shop's contracts whose expected billing date falls in a range and that
/// match the bulk filters (`billingAttemptStatus`, `billingCycleStatus`, `contractStatus`).
fn bulk_cycles(
    state: &ShopState,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
    filters: &Map<String, Value>,
) -> Vec<(String, i64)> {
    let attempt_status = text(filters, "billingAttemptStatus").unwrap_or("ANY");
    let cycle_statuses = gids(filters.get("billingCycleStatus"));
    let contract_statuses = gids(filters.get("contractStatus"));
    let mut found = Vec::new();
    for contract in state.all(CONTRACT) {
        let id = text(contract, "id").unwrap_or_default();
        if !contract_statuses.is_empty()
            && !contract_statuses
                .iter()
                .any(|status| Some(status.as_str()) == text(contract, "status"))
        {
            continue;
        }
        // The cycles whose expected date can fall in the range: those that end within a cycle
        // of it.
        for cycle in cycles::cycles_between(
            state,
            contract,
            from - Duration::days(366),
            to + Duration::days(366),
        ) {
            let expected = cycles::expected_date(contract, &cycle);
            if expected < from || expected > to || cycles::is_skipped(contract, cycle.index) {
                continue;
            }
            let attempted = !cycles::attempts(state, contract, cycle.index).is_empty();
            let attempt_ok = match attempt_status {
                "HAS_ATTEMPT" => attempted,
                "NO_ATTEMPT" => !attempted,
                _ => true,
            };
            let billed = cycles::is_billed(state, contract, cycle.index);
            let status = if billed { "BILLED" } else { "UNBILLED" };
            let status_ok =
                cycle_statuses.is_empty() || cycle_statuses.iter().any(|known| known == status);
            if attempt_ok && status_ok {
                found.push((id.to_string(), cycle.index));
            }
        }
    }
    found
}

/// `subscriptionBillingCycleBulkSearch` and `subscriptionBillingCycleBulkCharge`: a `Job` done
/// at once, the cycles found kept on it for `subscriptionBillingCycleBulkResults`; the charge
/// bills each (unpaused, not terminated) with an attempt.
fn bulk(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    charge: bool,
) -> Result<Value, FieldError> {
    let empty = json!({"job": null});
    let range = object(ctx.arg("billingAttemptExpectedDateRange"));
    let from = text(&range, "startDate").and_then(model::date_time);
    let to = text(&range, "endDate").and_then(model::date_time);
    let (Some(from), Some(to)) = (from, to) else {
        return Ok(payload(
            empty,
            &[bulk_error(&["billingAttemptExpectedDateRange"], "INVALID")],
        ));
    };
    if from >= to {
        return Ok(payload(
            empty,
            &[bulk_error(
                &["billingAttemptExpectedDateRange"],
                "START_DATE_BEFORE_END_DATE",
            )],
        ));
    }
    if to - from > Duration::weeks(1) {
        return Ok(payload(
            empty,
            &[bulk_error(
                &["billingAttemptExpectedDateRange"],
                "INVALID_DATE_RANGE",
            )],
        ));
    }
    let state = mutation.state();
    if to > state.now() + Duration::hours(24) {
        return Ok(payload(
            empty,
            &[bulk_error(
                &["billingAttemptExpectedDateRange", "endDate"],
                "END_DATE_IN_THE_FUTURE",
            )],
        ));
    }
    let filters = object(ctx.arg("filters"));
    let found = bulk_cycles(state, from, to, &filters);
    let oversell = ctx.arg_str("inventoryPolicy") == Some("ALLOW_OVERSELLING");
    let skip_payment =
        ctx.arg_str("paymentProcessingPolicy") == Some("SKIP_PAYMENT_AND_CREATE_UNPAID_ORDER");
    let origin = mutation.origin().to_string();
    let state = mutation.state_mut();
    if charge {
        for (contract, index) in &found {
            let Some(record) = state.get(contract).cloned() else {
                continue;
            };
            if matches!(
                text(&record, "status"),
                Some("PAUSED" | "CANCELLED" | "EXPIRED")
            ) {
                continue;
            }
            let Some(cycle) = cycles::cycle(state, &record, *index) else {
                continue;
            };
            let key = generated_key(state, contract, *index);
            let spec = AttemptSpec {
                contract,
                cycle,
                idempotency_key: key,
                origin_time: None,
                oversell,
                skip_payment,
                origin: &origin,
            };
            attempt(state, &spec);
        }
    }
    let kind = if charge {
        "subscription_billing_cycle_bulk_charge"
    } else {
        "subscription_billing_cycle_bulk_search"
    };
    let job = crate::resolve::done_job(state, kind);
    let cycles: Vec<Value> = found
        .iter()
        .map(|(contract, index)| json!({"contract": contract, "index": index}))
        .collect();
    if let Some(record) = state.get_mut(&job) {
        record.insert("lsfBillingCycles".into(), Value::Array(cycles));
    }
    state.emit(crate::domains::app::CHANGED, &job);
    Ok(payload(json!({ "job": job }), &[]))
}

/// `subscriptionBillingCycleBulkResults(jobId:)`: the cycles of a bulk job, as they are now.
fn bulk_results(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let job = ctx.arg_str("jobId").unwrap_or_default();
    let state = r.state();
    let items: Vec<Value> = state
        .get(job)
        .and_then(|record| record.get("lsfBillingCycles"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let contract = entry.get("contract")?.as_str()?;
            let index = entry.get("index")?.as_i64()?;
            let record = state.get(contract)?;
            let cycle = cycles::cycle(state, record, index)?;
            Some(cycles::to_value(state, contract, record, &cycle))
        })
        .collect();
    r.connection(ctx, items)
}

/// What the log of `GET /__lsf/admin/subscriptions` says of the charges: each billing attempt,
/// its contract and cycle, its outcome, its order and what was charged.
pub fn charges(state: &ShopState) -> Vec<Value> {
    state
        .all(ATTEMPT)
        .map(|attempt| {
            let order = text(attempt, "order").and_then(|order| state.get(order));
            let contract = text(attempt, "subscriptionContract").and_then(|contract| state.get(contract));
            let method = contract
                .and_then(|contract| text(contract, "customerPaymentMethod"))
                .map(str::to_string);
            json!({
                "attempt": attempt.get("id"),
                "contract": attempt.get("subscriptionContract"),
                "cycleIndex": attempt.get("lsfCycleIndex"),
                "createdAt": attempt.get("createdAt"),
                "state": attempt.pointer("/state/__typename"),
                "errorCode": attempt.get("errorCode"),
                "paymentMethod": method,
                "order": attempt.get("order"),
                "amount": order.and_then(|order| order.pointer("/totalPriceSet/shopMoney")).cloned(),
                "nextActionUrl": attempt.get("nextActionUrl"),
            })
        })
        .collect()
}
