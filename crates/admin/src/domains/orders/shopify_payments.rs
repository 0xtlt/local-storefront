//! Shopify Payments: the shop's account, the balance transactions of the money it processed,
//! its payouts, and the disputes buyers' banks open.
//!
//! As shopify.dev's reference of 2026-10 describes them (`queries/shopifyPaymentsAccount` and
//! its examples, `objects/ShopifyPaymentsDispute`, `mutations/disputeEvidenceUpdate`,
//! `mutations/shopifyPaymentsPayoutAlternateCurrencyCreate`), on a local shop:
//!
//! - **The account** is active, in the shop's country and currency, paid out daily to one
//!   validated bank account *(chosen)*.
//! - **What it processes** *(chosen: lsf has no other payment provider)*: every sale, capture and
//!   refund that is not through a manual gateway (`manualPaymentGateway`, `manual`, store credit,
//!   gift cards): the checkout's card payments, `shopify_payments` (mandates), the orders of the
//!   store data. Each is a `ShopifyPaymentsBalanceTransaction` (`CHARGE` with a fee of 2.9 % +
//!   0.30 in the shop's currency, Shopify's published online rate of its Basic plan in the
//!   United States *(chosen)*; `REFUND` without one).
//! - **Payouts**: once the shop's day is over, what it processed that day is paid out the next
//!   day (`PAID`, a `DEPOSIT`, or a `WITHDRAWAL` when it is negative), with its `TRANSFER`
//!   balance transaction; `balance` is what was not paid out yet. [`tick`] makes them, before
//!   every request and when the clock moves.
//! - **Disputes**: a buyer's bank opens one with `POST /__lsf/admin/disputes` (a chargeback by
//!   default, `NEEDS_RESPONSE`, evidence due in 7 days *(chosen)*: the amount and Shopify's 15.00
//!   chargeback fee leave the balance) and decides it with `POST /__lsf/admin/disputes/<n>/won`
//!   or `/lost` (won, the amount comes back). Events: `disputes/create`, `disputes/update`.
//!   `disputeEvidenceUpdate` fills the evidence (its files are dispute file uploads, or files of
//!   the shop, which become one), until it is submitted (`submitEvidence`: `UNDER_REVIEW`,
//!   `evidenceSentOn`) or due.
//! - **`shopifyPaymentsPayoutAlternateCurrencyCreate`** pays out at once what the account holds
//!   in a currency. lsf keeps every amount in the shop's currency, so that is the only one with a
//!   balance: another currency answers `ALTERNATE_CURRENCY_PAYOUT_FAILED_NO_ELIGIBLE_BALANCE`.
//!
//! Messages: the descriptions of the error codes, where a code has one; lsf's wording otherwise.

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use serde_json::{Map, Value, json};

use super::model::{self, field_cents, int, text};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::sort::SortValue;
use crate::resolve::{Mutation, Registry, ShopResolver, UserError, payload};
use crate::search::SearchValue;
use crate::state::{Record, ShopState, gid, value};

pub fn register(registry: &mut Registry) {
    for field in [
        "accountOpenerName",
        "country",
        "defaultCurrency",
        "payoutStatementDescriptor",
        "chargeStatementDescriptor",
        "chargeStatementDescriptors",
    ] {
        registry.field("ShopifyPaymentsAccount", field, |r, ctx, _object| {
            r.convert(ctx, &account_field(r, ctx.field_name()), false)
        });
    }
    for field in ["country", "currency"] {
        registry.field("ShopifyPaymentsBankAccount", field, |r, ctx, _object| {
            r.convert(ctx, &account_field(r, ctx.field_name()), false)
        });
    }
    registry
        .mutation("disputeEvidenceUpdate", evidence_update)
        .mutation(
            "shopifyPaymentsPayoutAlternateCurrencyCreate",
            alternate_currency_payout,
        )
        .field("ShopifyPaymentsAccount", "balance", |r, ctx, _object| {
            let state = r.state();
            let currency = model::currency(state);
            let pending: i64 = unpaid(state).iter().map(|(_, net)| net).sum();
            r.convert(ctx, &json!([value::money_v2(pending, &currency)]), false)
        })
        .field("ShopifyPaymentsAccount", "balanceTransactions", |r, ctx, _object| {
            let state = r.state();
            let hide = ctx.arg_bool("hideTransfers") == Some(true);
            let items: Vec<Value> = state
                .keys("ShopifyPaymentsBalanceTransaction")
                .filter(|key| {
                    !hide
                        || state
                            .get(key)
                            .and_then(|record| text(record, "type"))
                            != Some("TRANSFER")
                })
                .map(|key| json!(key))
                .collect();
            r.connection(ctx, items)
        })
        .field("ShopifyPaymentsAccount", "payouts", |r, ctx, _object| {
            let state = r.state();
            let kind = ctx.arg_str("transactionType");
            let items: Vec<Value> = state
                .keys("ShopifyPaymentsPayout")
                .filter(|key| {
                    kind.is_none_or(|kind| {
                        state
                            .get(key)
                            .and_then(|record| text(record, "transactionType"))
                            == Some(kind)
                    })
                })
                .map(|key| json!(key))
                .collect();
            r.connection(ctx, items)
        })
        .field("ShopifyPaymentsAccount", "disputes", |r, ctx, _object| {
            let items = r.records_of("ShopifyPaymentsDispute");
            r.connection(ctx, items)
        })
        .field("Order", "disputes", |r, ctx, object| {
            let state = r.state();
            let order = r
                .map_of(object)
                .and_then(|order| text(order, "id"))
                .unwrap_or_default()
                .to_string();
            let summaries: Vec<Value> = state
                .all("ShopifyPaymentsDispute")
                .filter(|dispute| text(dispute, "order") == Some(order.as_str()))
                .map(|dispute| {
                    json!({"id": dispute.get("id"), "initiatedAs": dispute.get("type"), "status": dispute.get("status")})
                })
                .collect();
            r.convert(ctx, &Value::Array(summaries), false)
        })
        .sort_key("ShopifyPaymentsBalanceTransaction", "PROCESSED_AT", |r, item| {
            SortValue::of(r.item_map(item).and_then(|record| record.get("transactionDate")))
        })
        .sort_key("ShopifyPaymentsBalanceTransaction", "AMOUNT", |r, item| {
            money_sort(r, item, "amount")
        })
        .sort_key("ShopifyPaymentsBalanceTransaction", "FEE", |r, item| {
            money_sort(r, item, "fee")
        })
        .sort_key("ShopifyPaymentsBalanceTransaction", "NET", |r, item| {
            money_sort(r, item, "net")
        })
        .sort_key("ShopifyPaymentsBalanceTransaction", "TRANSACTION_TYPE", |r, item| {
            SortValue::of(r.item_map(item).and_then(|record| record.get("type")))
        })
        .sort_key("ShopifyPaymentsBalanceTransaction", "PAYOUT_STATUS", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|record| record.get("associatedPayout"))
                    .and_then(|payout| payout.get("status")),
            )
        })
        .sort_key("ShopifyPaymentsBalanceTransaction", "ORDER_NAME", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|record| record.get("associatedOrder"))
                    .and_then(|order| order.get("name")),
            )
        })
        .sort_key("ShopifyPaymentsPayout", "AMOUNT", |r, item| money_sort(r, item, "net"))
        .search_field("ShopifyPaymentsBalanceTransaction", "transaction_type", |_r, _item, record| {
            Some(keyword(text(record, "type")))
        })
        .search_field("ShopifyPaymentsBalanceTransaction", "payout_status", |_r, _item, record| {
            Some(keyword(
                record
                    .get("associatedPayout")
                    .and_then(|payout| payout.get("status"))
                    .and_then(Value::as_str),
            ))
        })
        .search_field("ShopifyPaymentsBalanceTransaction", "processed_at", |_r, _item, record| {
            Some(date(text(record, "transactionDate")))
        })
        .search_field("ShopifyPaymentsPayout", "amount", |_r, _item, record| {
            Some(vec![SearchValue::Number(
                model::cents(record.get("net")) as f64 / 100.0,
            )])
        });
}

fn keyword(value: Option<&str>) -> Vec<SearchValue> {
    value
        .map(|value| SearchValue::Keyword(value.to_ascii_lowercase()))
        .into_iter()
        .collect()
}

fn date(value: Option<&str>) -> Vec<SearchValue> {
    value.and_then(SearchValue::date).into_iter().collect()
}

fn money_sort(r: &ShopResolver<'_>, item: &Value, field: &str) -> SortValue {
    SortValue::of(
        r.item_map(item)
            .and_then(|record| record.get(field))
            .and_then(|money| money.get("amount")),
    )
}

/// The account's GID: a shop has one.
pub const ACCOUNT: &str = "gid://shopify/ShopifyPaymentsAccount/1";
const BANK_ACCOUNT: &str = "gid://shopify/ShopifyPaymentsBankAccount/1";

/// The fee of a charge: 2.9 % + 0.30 *(chosen)*.
fn charge_fee(amount: i64) -> i64 {
    if amount <= 0 {
        0
    } else {
        (amount * 29 + 500) / 1000 + 30
    }
}

/// Shopify's chargeback fee, in the shop's currency *(15.00, its fee in the United States)*.
const CHARGEBACK_FEE: i64 = 1500;

/// Seeds the account and its bank account. What they take from the shop (its name, country and
/// currency) is read when they are, the shop being seeded after the orders.
pub fn seed(state: &mut ShopState) {
    state.insert(value::object(json!({
        "__typename": "ShopifyPaymentsBankAccount",
        "id": BANK_ACCOUNT,
        "accountNumberLastDigits": "6789",
        "bankName": "Local Bank",
        "status": "VALIDATED",
        "createdAt": "2024-01-01T12:00:00Z",
        "payouts": [],
    })));
    state.insert(value::object(json!({
        "__typename": "ShopifyPaymentsAccount",
        "id": ACCOUNT,
        "activated": true,
        "onboardable": false,
        "payoutSchedule": {"interval": "DAILY", "monthlyAnchor": null, "weeklyAnchor": null},
        "bankAccounts": [BANK_ACCOUNT],
    })));
}

/// What the account and its bank account take from the shop: `(name, country, currency)`.
fn shop_facts(state: &ShopState) -> (String, String, String) {
    let shop = state.shop().cloned().unwrap_or_default();
    let country = shop
        .get("billingAddress")
        .and_then(|address| address.get("countryCodeV2"))
        .and_then(Value::as_str)
        .unwrap_or("US")
        .to_string();
    (
        text(&shop, "name").unwrap_or("Shop").to_string(),
        country,
        model::currency(state),
    )
}

/// The statement descriptors of a shop's name: its first 10 and 22 letters and digits, in
/// capitals, as card statements show them *(chosen)*.
fn descriptors(name: &str) -> (String, String) {
    let descriptor: String = name
        .to_ascii_uppercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ')
        .collect();
    (
        descriptor
            .chars()
            .take(10)
            .collect::<String>()
            .trim()
            .to_string(),
        descriptor
            .chars()
            .take(22)
            .collect::<String>()
            .trim()
            .to_string(),
    )
}

/// The fields of the account and its bank account that come from the shop.
fn account_field(r: &ShopResolver<'_>, field: &str) -> Value {
    let (name, country, currency) = shop_facts(r.state());
    let (prefix, default) = descriptors(&name);
    match field {
        "accountOpenerName" => json!(name),
        "country" => json!(country),
        "currency" | "defaultCurrency" => json!(currency),
        "payoutStatementDescriptor" => json!(prefix),
        "chargeStatementDescriptor" => json!(default),
        _ => json!({
            "__typename": "ShopifyPaymentsDefaultChargeStatementDescriptor",
            "default": default,
            "prefix": prefix,
        }),
    }
}

/// Whether Shopify Payments processed a transaction: one that moved money through a gateway that
/// is not manual.
pub fn processed(transaction: &Record) -> bool {
    transaction
        .get("manualPaymentGateway")
        .and_then(Value::as_bool)
        != Some(true)
        && !matches!(
            text(transaction, "gateway")
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("manual" | "shopify_store_credit" | "gift_card" | "cash" | "other")
        )
}

/// Records the balance transaction of an order transaction Shopify Payments processed: a
/// `CHARGE` for a sale or a capture, a `REFUND` for a refund. Called for every transaction that
/// moved money (`payments::record_tender`).
pub fn record_balance(state: &mut ShopState, transaction: &str) {
    let Some(record) = state.get(transaction).cloned() else {
        return;
    };
    if !processed(&record) {
        return;
    }
    let (kind, sign) = match text(&record, "kind") {
        Some("SALE" | "CAPTURE") => ("CHARGE", 1),
        Some("REFUND") => ("REFUND", -1),
        _ => return,
    };
    let amount = field_cents(&record, "amountSet") * sign;
    let fee = if kind == "CHARGE" {
        charge_fee(amount)
    } else {
        0
    };
    let order = text(&record, "order")
        .and_then(|order| state.get(order))
        .cloned();
    let number = model::number(transaction);
    let at = record
        .get("processedAt")
        .or_else(|| record.get("createdAt"))
        .cloned()
        .unwrap_or(Value::Null);
    insert_balance(
        state,
        Balance {
            kind,
            source: kind,
            amount,
            fee,
            at,
            order: order.as_ref(),
            source_id: Some(number),
            test: record.get("test").and_then(Value::as_bool).unwrap_or(false),
            reason: None,
        },
    );
}

/// A balance transaction to record.
struct Balance<'a> {
    kind: &'a str,
    source: &'a str,
    amount: i64,
    fee: i64,
    at: Value,
    order: Option<&'a Record>,
    source_id: Option<u64>,
    test: bool,
    reason: Option<&'a str>,
}

fn insert_balance(state: &mut ShopState, balance: Balance) -> String {
    let currency = model::currency(state);
    let id = state.new_gid("ShopifyPaymentsBalanceTransaction");
    state.insert(value::object(json!({
        "__typename": "ShopifyPaymentsBalanceTransaction",
        "id": id,
        "type": balance.kind,
        "sourceType": balance.source,
        "test": balance.test,
        "amount": value::money_v2(balance.amount, &currency),
        "fee": value::money_v2(balance.fee, &currency),
        "net": value::money_v2(balance.amount - balance.fee, &currency),
        "transactionDate": balance.at,
        "sourceId": balance.source_id.map(|id| id.to_string()),
        "sourceOrderTransactionId": if balance.source == "CHARGE" || balance.source == "REFUND" {
            json!(balance.source_id.map(|id| id.to_string()))
        } else {
            Value::Null
        },
        "associatedOrder": balance.order.map(|order| json!({"id": order.get("id"), "name": order.get("name")})),
        "associatedPayout": {"id": null, "status": "PENDING"},
        "adjustmentsOrders": [],
        "adjustmentReason": balance.reason,
    })));
    id
}

/// The balance transactions not paid out yet: `(GID, net cents)`.
fn unpaid(state: &ShopState) -> Vec<(String, i64)> {
    state
        .keys("ShopifyPaymentsBalanceTransaction")
        .filter_map(|key| {
            let record = state.get(key)?;
            (record
                .get("associatedPayout")
                .and_then(|payout| payout.get("id"))
                .is_none_or(Value::is_null)
                && text(record, "type") != Some("TRANSFER"))
            .then(|| (key.to_string(), model::cents(record.get("net"))))
        })
        .collect()
}

/// The shop's time zone, for its days.
fn zone(state: &ShopState) -> chrono_tz::Tz {
    state
        .shop()
        .and_then(|shop| text(shop, "ianaTimezone"))
        .and_then(|zone| zone.parse().ok())
        .unwrap_or(chrono_tz::UTC)
}

/// The shop's day of a balance transaction.
fn day_of(state: &ShopState, record: &Record) -> Option<NaiveDate> {
    let at = DateTime::parse_from_rfc3339(text(record, "transactionDate")?).ok()?;
    Some(at.with_timezone(&zone(state)).date_naive())
}

/// Pays out what the account processed on each day before the shop's day now: one payout per
/// day, issued the next day.
pub fn tick(state: &mut ShopState) {
    let tz = zone(state);
    let today = state.now().with_timezone(&tz).date_naive();
    let mut days: Vec<(NaiveDate, Vec<String>)> = Vec::new();
    for (id, _) in unpaid(state) {
        let Some(day) = state.get(&id).and_then(|record| day_of(state, record)) else {
            continue;
        };
        if day >= today {
            continue;
        }
        match days.iter_mut().find(|(known, _)| *known == day) {
            Some((_, ids)) => ids.push(id),
            None => days.push((day, vec![id])),
        }
    }
    days.sort_by_key(|(day, _)| *day);
    for (day, ids) in days {
        let next = day.succ_opt().unwrap_or(day);
        let issued = tz
            .from_local_datetime(&next.and_hms_opt(0, 0, 0).unwrap_or_default())
            .earliest()
            .map_or_else(|| state.now(), |at| at.with_timezone(&Utc));
        pay_out(state, &ids, issued);
    }
}

/// A payout of balance transactions, at `issued`: the `ShopifyPaymentsPayout`, its `TRANSFER`
/// balance transaction, each transaction's `associatedPayout`. Returns its GID.
fn pay_out(state: &mut ShopState, ids: &[String], issued: DateTime<Utc>) -> String {
    let currency = model::currency(state);
    let money = |cents: i64| value::money_v2(cents, &currency);
    let (mut net, mut charges, mut charges_fee, mut refunds, mut adjustments, mut adjustments_fee) =
        (0, 0, 0, 0, 0, 0);
    for id in ids {
        let Some(record) = state.get(id) else {
            continue;
        };
        let amount = model::cents(record.get("amount"));
        let fee = model::cents(record.get("fee"));
        net += amount - fee;
        match text(record, "type") {
            Some("CHARGE") => {
                charges += amount;
                charges_fee += fee;
            }
            Some("REFUND") => refunds += amount,
            _ => {
                adjustments += amount;
                adjustments_fee += fee;
            }
        }
    }
    let payout = state.new_gid("ShopifyPaymentsPayout");
    let shop = state.shop().cloned().unwrap_or_default();
    let name = text(&shop, "name").unwrap_or("Shop").to_string();
    let address = shop.get("billingAddress").cloned().unwrap_or(Value::Null);
    state.insert(value::object(json!({
        "__typename": "ShopifyPaymentsPayout",
        "id": payout,
        "issuedAt": value::date_time(issued),
        "status": "PAID",
        "transactionType": if net < 0 { "WITHDRAWAL" } else { "DEPOSIT" },
        "net": money(net.abs()),
        "gross": money(net.abs()),
        "externalTraceId": null,
        "bankAccount": BANK_ACCOUNT,
        "businessEntity": {
            "__typename": "BusinessEntity",
            "id": "gid://shopify/BusinessEntity/1",
            "displayName": name,
            "companyName": name,
            "primary": true,
            "archived": false,
            "legalEntityId": null,
            "shopifyPaymentsAccount": ACCOUNT,
            "address": {
                "address1": address.get("address1"),
                "address2": address.get("address2"),
                "city": address.get("city"),
                "province": address.get("province"),
                "zip": address.get("zip"),
                "countryCode": address.get("countryCodeV2").and_then(Value::as_str).unwrap_or("US"),
            },
        },
        "summary": {
            "chargesGross": money(charges),
            "chargesFee": money(charges_fee),
            "refundsFeeGross": money(refunds),
            "refundsFee": money(0),
            "adjustmentsGross": money(adjustments),
            "adjustmentsFee": money(adjustments_fee),
            "advanceGross": money(0),
            "advanceFees": money(0),
            "reservedFundsGross": money(0),
            "reservedFundsFee": money(0),
            "retriedPayoutsGross": money(0),
            "retriedPayoutsFee": money(0),
            "usdcRebateCreditAmount": money(0),
        },
    })));
    model::push_key(state, BANK_ACCOUNT, "payouts", &payout);
    let number = model::number(&payout);
    let transfer = insert_balance(
        state,
        Balance {
            kind: "TRANSFER",
            source: "TRANSFER",
            amount: -net,
            fee: 0,
            at: json!(value::date_time(issued)),
            order: None,
            source_id: Some(number),
            test: false,
            reason: None,
        },
    );
    for id in ids.iter().chain(std::iter::once(&transfer)) {
        if let Some(record) = state.get_mut(id) {
            record.insert(
                "associatedPayout".into(),
                json!({"id": payout, "status": "PAID"}),
            );
        }
    }
    payout
}

// --- Disputes ---

/// A dispute to open on an order.
pub struct NewDispute<'a> {
    pub order: &'a str,
    /// In cents; the order's total received when `None`.
    pub amount: Option<i64>,
    /// `CHARGEBACK` or `INQUIRY`.
    pub kind: &'a str,
    /// A `ShopifyPaymentsDisputeReason`.
    pub reason: &'a str,
}

/// The network reason codes of Visa for the reasons that have one *(chosen: the example's 4837
/// for fraud)*.
fn network_reason(reason: &str) -> Option<&'static str> {
    match reason {
        "FRAUDULENT" => Some("4837"),
        "PRODUCT_NOT_RECEIVED" => Some("13.1"),
        "PRODUCT_UNACCEPTABLE" => Some("13.3"),
        "CREDIT_NOT_PROCESSED" => Some("13.6"),
        "DUPLICATE" => Some("12.6.1"),
        "SUBSCRIPTION_CANCELLED" => Some("13.2"),
        _ => None,
    }
}

/// A buyer's bank opens a dispute on an order: the dispute and its evidence, prefilled from
/// the order; for a chargeback, the amount and the fee leave the balance. `disputes/create`.
pub fn open_dispute(state: &mut ShopState, dispute: &NewDispute) -> Result<String, String> {
    let Some(order) = state
        .get(dispute.order)
        .filter(|_| state.type_of(dispute.order) == Some("Order"))
        .cloned()
    else {
        return Err(format!(
            "{} is not an order of {}",
            dispute.order,
            state.domain()
        ));
    };
    if !matches!(dispute.kind, "CHARGEBACK" | "INQUIRY") {
        return Err("`type` is CHARGEBACK or INQUIRY".to_string());
    }
    let currency = model::currency(state);
    let amount = dispute
        .amount
        .unwrap_or_else(|| field_cents(&order, "netPaymentSet"));
    if amount <= 0 {
        return Err(format!("{} has no payment to dispute", dispute.order));
    }
    let now = state.now();
    let id = state.new_gid("ShopifyPaymentsDispute");
    let evidence = state.new_gid("ShopifyPaymentsDisputeEvidence");
    let fulfillments: Vec<Value> = gids(order.get("fulfillments"))
        .iter()
        .filter_map(|fulfillment| state.get(fulfillment))
        .map(|fulfillment| {
            let tracking = model::array(fulfillment, "trackingInfo")
                .into_iter()
                .next()
                .unwrap_or(Value::Null);
            json!({
                "id": format!("gid://shopify/ShopifyPaymentsDisputeFulfillment/{}", model::number(text(fulfillment, "id").unwrap_or_default())),
                "shippingCarrier": tracking.get("company"),
                "shippingTrackingNumber": tracking.get("number"),
                "shippingDate": text(fulfillment, "createdAt").map(|at| at.chars().take(10).collect::<String>()),
            })
        })
        .collect();
    let customer = text(&order, "customer")
        .and_then(|customer| state.get(customer))
        .cloned();
    let titles: Vec<String> = gids(order.get("lineItems"))
        .iter()
        .filter_map(|line| {
            state
                .get(line)
                .and_then(|line| text(line, "name"))
                .map(str::to_string)
        })
        .collect();
    state.insert(value::object(json!({
        "__typename": "ShopifyPaymentsDisputeEvidence",
        "id": evidence,
        "dispute": id,
        "submitted": false,
        "customerFirstName": customer.as_ref().and_then(|customer| customer.get("firstName")),
        "customerLastName": customer.as_ref().and_then(|customer| customer.get("lastName")),
        "customerEmailAddress": order.get("email"),
        "customerPurchaseIp": null,
        "billingAddress": order.get("billingAddress"),
        "shippingAddress": order.get("shippingAddress"),
        "productDescription": titles.join(", "),
        "fulfillments": fulfillments,
        "disputeFileUploads": [],
        "accessActivityLog": null,
        "cancellationPolicyDisclosure": null,
        "cancellationRebuttal": null,
        "refundPolicyDisclosure": null,
        "refundRefusalExplanation": null,
        "uncategorizedText": null,
        "cancellationPolicyFile": null,
        "customerCommunicationFile": null,
        "refundPolicyFile": null,
        "serviceDocumentationFile": null,
        "shippingDocumentationFile": null,
        "uncategorizedFile": null,
    })));
    state.insert(value::object(json!({
        "__typename": "ShopifyPaymentsDispute",
        "id": id,
        "order": dispute.order,
        "type": dispute.kind,
        "status": "NEEDS_RESPONSE",
        "amount": value::money_v2(amount, &currency),
        "reasonDetails": {"reason": dispute.reason, "networkReasonCode": network_reason(dispute.reason)},
        "initiatedAt": value::date_time(now),
        "evidenceDueBy": value::date_time(now + Duration::days(7)),
        "evidenceSentOn": null,
        "finalizedOn": null,
        "disputeEvidence": evidence,
    })));
    if dispute.kind == "CHARGEBACK" {
        let number = model::number(&id);
        insert_balance(
            state,
            Balance {
                kind: "ADJUSTMENT",
                source: "DISPUTE",
                amount: -amount,
                fee: CHARGEBACK_FEE,
                at: json!(value::date_time(now)),
                order: Some(&order),
                source_id: Some(number),
                test: order.get("test").and_then(Value::as_bool).unwrap_or(false),
                reason: Some("chargeback"),
            },
        );
    }
    state.emit("disputes/create", &id);
    Ok(id)
}

/// The bank decides a dispute: `WON` (a chargeback's amount comes back), `LOST` or `ACCEPTED`.
/// `disputes/update`.
pub fn decide_dispute(state: &mut ShopState, number: u64, outcome: &str) -> Result<String, String> {
    let id = gid("ShopifyPaymentsDispute", number);
    let Some(record) = state.get(&id).cloned() else {
        return Err(format!("{} has no dispute {number}", state.domain()));
    };
    let status = match outcome.to_ascii_lowercase().as_str() {
        "won" => "WON",
        "lost" => "LOST",
        "accepted" => "ACCEPTED",
        other => return Err(format!("`{other}`: a dispute is won, lost or accepted")),
    };
    if matches!(text(&record, "status"), Some("WON" | "LOST" | "ACCEPTED")) {
        return Err(format!("dispute {number} is decided already"));
    }
    let now = state.timestamp();
    if let Some(dispute) = state.get_mut(&id) {
        dispute.insert("status".into(), json!(status));
        dispute.insert("finalizedOn".into(), json!(now));
    }
    if status == "WON" && text(&record, "type") == Some("CHARGEBACK") {
        let order = text(&record, "order")
            .and_then(|order| state.get(order))
            .cloned();
        insert_balance(
            state,
            Balance {
                kind: "DISPUTE_REVERSAL",
                source: "DISPUTE",
                amount: model::cents(record.get("amount")),
                fee: 0,
                at: json!(now),
                order: order.as_ref(),
                source_id: Some(number),
                test: false,
                reason: Some("chargeback won"),
            },
        );
    }
    state.emit("disputes/update", &id);
    Ok(id)
}

/// The fields of `ShopifyPaymentsDisputeEvidenceUpdateInput` that are text.
const TEXTS: &[&str] = &[
    "accessActivityLog",
    "cancellationPolicyDisclosure",
    "cancellationRebuttal",
    "customerEmailAddress",
    "customerFirstName",
    "customerLastName",
    "refundPolicyDisclosure",
    "refundRefusalExplanation",
    "uncategorizedText",
];

/// The file fields, and the evidence type of each.
const FILES: &[(&str, &str)] = &[
    ("cancellationPolicyFile", "CANCELLATION_POLICY_FILE"),
    ("customerCommunicationFile", "CUSTOMER_COMMUNICATION_FILE"),
    ("refundPolicyFile", "REFUND_POLICY_FILE"),
    ("serviceDocumentationFile", "SERVICE_DOCUMENTATION_FILE"),
    ("shippingDocumentationFile", "SHIPPING_DOCUMENTATION_FILE"),
    ("uncategorizedFile", "UNCATEGORIZED_FILE"),
];

/// A dispute file upload from a file of the shop (a `GenericFile`, a `MediaImage`) or an upload
/// already made.
fn file_upload(state: &mut ShopState, id: &str, kind: &str) -> Option<String> {
    match state.type_of(id) {
        Some("ShopifyPaymentsDisputeFileUpload") => Some(id.to_string()),
        Some("GenericFile" | "MediaImage") => {
            let file = state.get(id)?.clone();
            let url = text(&file, "url")
                .or_else(|| {
                    file.get("image")
                        .and_then(|image| image.get("url"))
                        .and_then(Value::as_str)
                })
                .unwrap_or_default()
                .to_string();
            let name = url
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .split('?')
                .next()
                .unwrap_or_default()
                .to_string();
            let upload = state.new_gid("ShopifyPaymentsDisputeFileUpload");
            state.insert(value::object(json!({
                "__typename": "ShopifyPaymentsDisputeFileUpload",
                "id": upload,
                "disputeEvidenceType": kind,
                "fileSize": int(&file, "originalFileSize"),
                "fileType": file.get("mimeType").and_then(Value::as_str).unwrap_or("application/pdf"),
                "originalFileName": name,
                "url": url,
            })));
            Some(upload)
        }
        _ => None,
    }
}

/// `disputeEvidenceUpdate`.
fn evidence_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"disputeEvidence": null}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(evidence) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("ShopifyPaymentsDisputeEvidence"))
        .cloned()
    else {
        return failed(
            &["id"],
            "Dispute evidence could not be found.",
            "DISPUTE_EVIDENCE_NOT_FOUND",
        );
    };
    let dispute_id = text(&evidence, "dispute").unwrap_or_default().to_string();
    let dispute = state.get(&dispute_id).cloned().unwrap_or_default();
    if evidence.get("submitted").and_then(Value::as_bool) == Some(true)
        || text(&dispute, "status") != Some("NEEDS_RESPONSE")
    {
        return failed(
            &["id"],
            "Evidence already accepted.",
            "EVIDENCE_ALREADY_ACCEPTED",
        );
    }
    if DateTime::parse_from_rfc3339(text(&dispute, "evidenceDueBy").unwrap_or_default())
        .is_ok_and(|due| due.with_timezone(&Utc) < mutation.now())
    {
        return failed(&["id"], "Evidence past due date.", "EVIDENCE_PAST_DUE_DATE");
    }
    let input = model::input_of(ctx.arg("input"));
    for (field, _) in FILES {
        if let Some(file) = input.get(*field).filter(|file| !file.is_null()) {
            let file_id = file.get("id").and_then(Value::as_str).unwrap_or_default();
            if !matches!(
                state.type_of(file_id),
                Some("ShopifyPaymentsDisputeFileUpload" | "GenericFile" | "MediaImage")
            ) {
                return failed(
                    &["input", field, "id"],
                    "File upload failed. Please try again.",
                    "FILE_NOT_FOUND",
                );
            }
        }
    }
    let state = mutation.state_mut();
    let mut fields = Map::new();
    for field in TEXTS {
        if let Some(text) = input.get(*field) {
            fields.insert((*field).into(), text.clone());
        }
    }
    if let Some(address) = input
        .get("shippingAddress")
        .filter(|address| !address.is_null())
    {
        let address_id = format!(
            "gid://shopify/MailingAddress/{}?model_name=Address",
            model::number(&id)
        );
        let mut address = address.clone();
        // `country` of the input is a code (the example's "US").
        if address.get("countryCode").is_none()
            && let Some(code) = address
                .get("country")
                .and_then(Value::as_str)
                .filter(|code| code.len() == 2)
        {
            address["countryCode"] = json!(code);
        }
        if let Some(address) = model::mailing_address(&address, &address_id) {
            fields.insert("shippingAddress".into(), address);
        }
    }
    let mut uploads = gids(evidence.get("disputeFileUploads"));
    for (field, kind) in FILES {
        let Some(file) = input.get(*field).filter(|file| !file.is_null()) else {
            continue;
        };
        let file_id = file.get("id").and_then(Value::as_str).unwrap_or_default();
        if file.get("destroy").and_then(Value::as_bool) == Some(true) {
            uploads.retain(|known| known != file_id);
            fields.insert((*field).into(), Value::Null);
            continue;
        }
        if let Some(upload) = file_upload(state, file_id, kind) {
            if !uploads.contains(&upload) {
                uploads.push(upload.clone());
            }
            fields.insert((*field).into(), json!(upload));
        }
    }
    fields.insert("disputeFileUploads".into(), json!(uploads));
    let submit = input.get("submitEvidence").and_then(Value::as_bool) == Some(true);
    if submit {
        fields.insert("submitted".into(), json!(true));
    }
    if let Some(record) = state.get_mut(&id) {
        record.extend(fields);
    }
    if submit {
        let now = state.timestamp();
        if let Some(record) = state.get_mut(&dispute_id) {
            record.insert("status".into(), json!("UNDER_REVIEW"));
            record.insert("evidenceSentOn".into(), json!(now));
        }
    }
    state.emit("disputes/update", &dispute_id);
    Ok(payload(json!({"disputeEvidence": id}), &[]))
}

/// `shopifyPaymentsPayoutAlternateCurrencyCreate`.
fn alternate_currency_payout(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"payout": null, "success": false}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    if let Some(account) = ctx.arg_str("accountId")
        && account != ACCOUNT
    {
        return failed(
            &["accountId"],
            "No Stripe provider account was found.",
            "MISSING_PROVIDER_ACCOUNT",
        );
    }
    let currency = ctx.arg_str("currency").unwrap_or_default().to_string();
    let state = mutation.state();
    let pending = unpaid(state);
    let total: i64 = pending.iter().map(|(_, net)| net).sum();
    if currency != model::currency(state) || total <= 0 {
        return failed(
            &["currency"],
            "Failed to create payout, there is no eligible balance in this currency.",
            "ALTERNATE_CURRENCY_PAYOUT_FAILED_NO_ELIGIBLE_BALANCE",
        );
    }
    let ids: Vec<String> = pending.into_iter().map(|(id, _)| id).collect();
    let state = mutation.state_mut();
    let now = state.now();
    let payout = pay_out(state, &ids, now);
    let remote = format!(
        "po_{}",
        value::uuid(&format!("{}/{payout}", state.domain())).replace('-', "")
    );
    state.emit(crate::domains::app::CHANGED, &payout);
    Ok(payload(
        json!({
            "success": true,
            "payout": {
                "amount": value::money_v2(total, &currency),
                "currency": currency,
                "createdAt": value::date_time(now),
                "arrivalDate": value::date_time(now + Duration::days(1)),
                "remoteId": remote,
            },
        }),
        &[],
    ))
}
