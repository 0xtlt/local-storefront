//! A tax app: its configuration (`taxAppConfigure`), the merchant turning it on, and the tax
//! summaries it asks for (`taxSummaryCreate`).
//!
//! As shopify.dev's reference of 2026-10 describes them (`mutations/taxAppConfigure`,
//! `enums/TaxPartnerState`, `mutations/taxSummaryCreate`):
//!
//! - `taxAppConfigure(ready:)` sets the app's tax configuration `READY` ("configured, but not used
//!   for tax calculations") or back to `PENDING` ("not configured"). Once the merchant made it
//!   `ACTIVE` ("to be used for tax calculations": `POST /__lsf/admin/tax-app/activate`, the
//!   admin's switch), the app cannot change it (`TAX_PARTNER_ALREADY_ACTIVE`, "Unable to update
//!   already active tax partner."). The app is the shop's tax service: `tax_services/create` the
//!   first time, `tax_services/update` after, with whether it is active.
//! - `taxSummaryCreate` enqueues the order given, else the orders processed between `startTime`
//!   and `endTime`, and answers them (`enqueuedOrders`); none is `ORDER_NOT_FOUND`. The summaries
//!   themselves are delivered to `tax_summaries/create`, a topic the 2026-10 enum does not have:
//!   no subscription can receive it, so lsf records the request on each order only.
//!
//! lsf does not check that the caller is a tax calculations app, nor computes taxes with it: the
//! checkout's rate (`checkout.tax_rate`) stays the shop's.

use serde_json::{Value, json};

use super::model::text;
use crate::admin::domains::app::CHANGED;
use crate::admin::graphql::{FieldContext, FieldError};
use crate::admin::resolve::{Mutation, Registry, UserError, payload};
use crate::admin::state::{ShopState, gid_number, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("taxAppConfigure", configure)
        .mutation("taxSummaryCreate", summary_create);
}

/// The key of the app's tax configuration: not a record of a GID (its type has no id).
fn key(state: &ShopState) -> String {
    let app = state.app_id();
    format!("TaxAppConfiguration/{app}")
}

/// The app's tax configuration's state: `PENDING` until it is configured.
pub fn tax_state(state: &ShopState) -> String {
    state
        .get(&key(state))
        .and_then(|record| text(record, "state"))
        .unwrap_or("PENDING")
        .to_string()
}

/// Sets the app's tax configuration to `next` and fires `tax_services/create` (the first time)
/// or `tax_services/update`.
fn set_state(state: &mut ShopState, next: &str) {
    let key = key(state);
    let app = state.app_gid().map(str::to_string);
    let app_record = app
        .as_deref()
        .and_then(|app| state.get(app))
        .cloned()
        .unwrap_or_default();
    let name = text(&app_record, "title").unwrap_or("Tax app").to_string();
    let url = app_record
        .get("installation")
        .and_then(Value::as_str)
        .and_then(|installation| state.get(installation))
        .and_then(|installation| text(installation, "launchUrl"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://{}/admin/apps", state.domain()));
    let created = !state.contains(&key);
    let number = app.as_deref().map_or(0, gid_number);
    let record = value::object(json!({
        "__typename": "TaxAppConfiguration",
        "state": next,
        "lsfService": {"id": number, "name": name, "url": url, "active": next == "ACTIVE"},
    }));
    match state.get_mut(&key) {
        Some(existing) => existing.extend(record),
        None => {
            state.insert_as(key.clone(), record);
        }
    }
    state.emit(
        if created {
            "tax_services/create"
        } else {
            "tax_services/update"
        },
        &key,
    );
}

/// `taxAppConfigure`.
fn configure(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    if tax_state(mutation.state()) == "ACTIVE" {
        let error = UserError::new(&["ready"], "Unable to update already active tax partner.")
            .code("TAX_PARTNER_ALREADY_ACTIVE");
        return Ok(payload(json!({"taxAppConfiguration": null}), &[error]));
    }
    let next = if ctx.arg_bool("ready") == Some(true) {
        "READY"
    } else {
        "PENDING"
    };
    set_state(mutation.state_mut(), next);
    Ok(payload(
        json!({"taxAppConfiguration": {"state": next}}),
        &[],
    ))
}

/// The merchant turns the tax app on (`ACTIVE`): the app must be `READY`. `Err` with why.
pub fn activate(state: &mut ShopState) -> Result<(), String> {
    match tax_state(state).as_str() {
        "READY" => {
            set_state(state, "ACTIVE");
            Ok(())
        }
        "ACTIVE" => Err("the tax app is active already".to_string()),
        _ => Err(
            "the tax app is not configured: it calls taxAppConfigure(ready: true) first"
                .to_string(),
        ),
    }
}

/// `taxSummaryCreate`.
fn summary_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"enqueuedOrders": null}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    let state = mutation.state();
    let time = |name: &str| ctx.arg_str(name).and_then(value::parse_date_time);
    let orders: Vec<String> = if let Some(order) = ctx.arg_str("orderId") {
        if state.type_of(order) != Some("Order") {
            return failed(&["orderId"], "No order was found.", "ORDER_NOT_FOUND");
        }
        vec![order.to_string()]
    } else {
        let (Some(start), Some(end)) = (time("startTime"), time("endTime")) else {
            return failed(
                &["orderId"],
                "An order ID, or a start time and an end time, is required.",
                "GENERAL_ERROR",
            );
        };
        state
            .keys("Order")
            .filter(|key| {
                state
                    .get(key)
                    .and_then(|order| text(order, "processedAt"))
                    .and_then(value::parse_date_time)
                    .is_some_and(|at| at >= start && at <= end)
            })
            .map(str::to_string)
            .collect()
    };
    if orders.is_empty() {
        return failed(&["startTime"], "No order was found.", "ORDER_NOT_FOUND");
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    for order in &orders {
        if let Some(record) = state.get_mut(order) {
            record.insert("lsfTaxSummaryRequestedAt".into(), json!(now));
        }
        state.emit(CHANGED, order);
    }
    Ok(payload(json!({"enqueuedOrders": orders}), &[]))
}
