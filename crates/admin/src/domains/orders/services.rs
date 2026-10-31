//! Fulfillment services and their requests: `fulfillmentServiceCreate`,
//! `fulfillmentServiceUpdate`, `fulfillmentServiceDelete`, and
//! `fulfillmentOrderSubmitFulfillmentRequest`, `fulfillmentOrderAcceptFulfillmentRequest`,
//! `fulfillmentOrderRejectFulfillmentRequest`, `fulfillmentOrderSubmitCancellationRequest`,
//! `fulfillmentOrderAcceptCancellationRequest`, `fulfillmentOrderRejectCancellationRequest`,
//! `fulfillmentOrderClose`.
//!
//! As shopify.dev describes them (the reference of 2026-10 and the tutorial "Build for
//! fulfillment services", `apps/build/orders-fulfillment/fulfillment-service-apps`):
//!
//! - A fulfillment service has a location of its own (`isFulfillmentService`), named after it:
//!   `locations/create`, `locations/update`, `locations/delete` follow it. Deleting one moves its
//!   open fulfillment orders to `destinationLocationId` (else the shop's first location); lsf
//!   drops the stock of its location rather than transfer it.
//! - The merchant (or an app acting for them) submits a fulfillment order assigned to the
//!   service's location (some of its line items: the others stay unsubmitted on a new one); the
//!   service accepts it (`IN_PROGRESS`) or rejects it (`OPEN` again). The merchant may ask to
//!   cancel an accepted one; the service accepts (the fulfillment order closes and its units are
//!   on a new open one) or rejects. The service closes one it cannot complete (`INCOMPLETE`).
//!   Each request is a `FulfillmentOrderMerchantRequest` of `merchantRequests`.
//! - Events: the `fulfillment_orders/…` topic of each step (`fulfillment_request_submitted`,
//!   `…_accepted`, `…_rejected`, `cancellation_request_submitted`, `…_accepted`, `…_rejected`,
//!   `fulfillment_service_failed_to_complete`). lsf does not call the service's `callbackUrl`
//!   (`fulfillment_order_notification`): the service polls `assignedFulfillmentOrders`.
//!
//! The messages are lsf's wording: the reference shows none.

use serde_json::{Value, json};

use super::fulfillments::split_off;
use super::model::{self, int, text};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{ShopState, value};
use crate::webhooks::payload::FULFILLMENT_ORDER_EVENT;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("fulfillmentServiceCreate", service_create)
        .mutation("fulfillmentServiceUpdate", service_update)
        .mutation("fulfillmentServiceDelete", service_delete)
        .mutation(
            "fulfillmentOrderSubmitFulfillmentRequest",
            submit_fulfillment_request,
        )
        .mutation("fulfillmentOrderAcceptFulfillmentRequest", |m, ctx| {
            respond(
                m,
                ctx,
                &["SUBMITTED"],
                "ACCEPTED",
                Some("IN_PROGRESS"),
                "fulfillment_orders/fulfillment_request_accepted",
            )
        })
        .mutation("fulfillmentOrderRejectFulfillmentRequest", |m, ctx| {
            respond(
                m,
                ctx,
                &["SUBMITTED"],
                "REJECTED",
                Some("OPEN"),
                "fulfillment_orders/fulfillment_request_rejected",
            )
        })
        .mutation(
            "fulfillmentOrderSubmitCancellationRequest",
            submit_cancellation_request,
        )
        .mutation(
            "fulfillmentOrderAcceptCancellationRequest",
            accept_cancellation,
        )
        .mutation("fulfillmentOrderRejectCancellationRequest", |m, ctx| {
            respond(
                m,
                ctx,
                &["CANCELLATION_REQUESTED"],
                "CANCELLATION_REJECTED",
                None,
                "fulfillment_orders/cancellation_request_rejected",
            )
        })
        .mutation("fulfillmentOrderClose", |m, ctx| {
            respond(
                m,
                ctx,
                &["ACCEPTED", "CANCELLATION_REJECTED"],
                "CLOSED",
                Some("INCOMPLETE"),
                "fulfillment_orders/fulfillment_service_failed_to_complete",
            )
        });
}

/// A fulfillment service of the shop.
fn find_service(state: &ShopState, id: &str) -> Result<String, UserError> {
    if state.type_of(id) == Some("FulfillmentService") {
        Ok(id.to_string())
    } else {
        Err(UserError::new(
            &["id"],
            "Fulfillment service could not be found.",
        ))
    }
}

/// Whether a fulfillment service other than `except` has the name.
fn name_taken(state: &ShopState, name: &str, except: Option<&str>) -> bool {
    state.keys("FulfillmentService").any(|key| {
        Some(key) != except
            && state
                .get(key)
                .and_then(|service| text(service, "serviceName"))
                .is_some_and(|known| known.eq_ignore_ascii_case(name))
    })
}

/// `fulfillmentServiceCreate`.
fn service_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let name = ctx.arg_str("name").unwrap_or_default().trim().to_string();
    let failed = |error: UserError| Ok(payload(json!({"fulfillmentService": null}), &[error]));
    if name.is_empty() {
        return failed(UserError::new(&["name"], "Name can't be blank"));
    }
    if name_taken(mutation.state(), &name, None) {
        return failed(UserError::new(&["name"], "Name has already been taken"));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let country = state
        .shop()
        .and_then(|shop| shop.get("shopAddress"))
        .and_then(|address| address.get("countryCodeV2"))
        .cloned()
        .unwrap_or(Value::Null);
    let service = state.new_gid("FulfillmentService");
    let location = state.new_gid("Location");
    state.insert(value::object(json!({
        "__typename": "Location",
        "id": location,
        "name": name,
        "address": {"address1": null, "address2": null, "city": null, "province": null, "provinceCode": null, "country": null, "countryCode": country, "zip": null, "phone": null, "latitude": null, "longitude": null, "formatted": []},
        "isActive": true,
        "fulfillsOnlineOrders": true,
        "isFulfillmentService": true,
        "fulfillmentService": service,
        "hasActiveInventory": ctx.arg_bool("inventoryManagement").unwrap_or(false),
        "shipsInventory": true,
        "createdAt": now,
        "updatedAt": now,
        "inventoryLevels": [],
        "metafields": [],
        "localPickupSettingsV2": null,
    })));
    state.insert(value::object(json!({
        "__typename": "FulfillmentService",
        "id": service,
        "serviceName": name,
        "handle": lsf_core::util::handleize(&name),
        "callbackUrl": ctx.arg_str("callbackUrl"),
        "inventoryManagement": ctx.arg_bool("inventoryManagement").unwrap_or(false),
        "trackingSupport": ctx.arg_bool("trackingSupport").unwrap_or(false),
        "requiresShippingMethod": ctx.arg_bool("requiresShippingMethod").unwrap_or(true),
        "fulfillmentOrdersOptIn": true,
        "location": location,
        "type": "THIRD_PARTY",
        "createdAt": now,
    })));
    state.emit("locations/create", &location);
    Ok(payload(json!({"fulfillmentService": service}), &[]))
}

/// `fulfillmentServiceUpdate`.
fn service_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| Ok(payload(json!({"fulfillmentService": null}), &[error]));
    let service = match find_service(mutation.state(), ctx.arg_str("id").unwrap_or_default()) {
        Ok(service) => service,
        Err(error) => return failed(error),
    };
    let name = ctx.arg_str("name").map(|name| name.trim().to_string());
    if let Some(name) = &name {
        if name.is_empty() {
            return failed(UserError::new(&["name"], "Name can't be blank"));
        }
        if name_taken(mutation.state(), name, Some(&service)) {
            return failed(UserError::new(&["name"], "Name has already been taken"));
        }
    }
    let state = mutation.state_mut();
    let location = state
        .get(&service)
        .and_then(|record| text(record, "location"))
        .map(str::to_string);
    if let Some(record) = state.get_mut(&service) {
        if let Some(name) = &name {
            record.insert("serviceName".into(), json!(name));
        }
        if let Some(url) = ctx.arg("callbackUrl") {
            record.insert("callbackUrl".into(), url.clone());
        }
        for field in [
            "inventoryManagement",
            "trackingSupport",
            "requiresShippingMethod",
        ] {
            if let Some(flag) = ctx.arg_bool(field) {
                record.insert(field.into(), json!(flag));
            }
        }
    }
    if let (Some(name), Some(location)) = (&name, &location) {
        if let Some(record) = state.get_mut(location) {
            record.insert("name".into(), json!(name));
        }
        state.touch(location);
        state.emit("locations/update", location);
    }
    Ok(payload(json!({"fulfillmentService": service}), &[]))
}

/// `fulfillmentServiceDelete`.
fn service_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| Ok(payload(json!({"deletedId": null}), &[error]));
    let state = mutation.state();
    let service = match find_service(state, ctx.arg_str("id").unwrap_or_default()) {
        Ok(service) => service,
        Err(error) => return failed(error),
    };
    let location = state
        .get(&service)
        .and_then(|record| text(record, "location"))
        .map(str::to_string);
    let destination = match ctx.arg_str("destinationLocationId") {
        Some(id) if state.type_of(id) == Some("Location") && Some(id) != location.as_deref() => {
            Some(id.to_string())
        }
        Some(_) => {
            return failed(UserError::new(
                &["destinationLocationId"],
                "Location could not be found.",
            ));
        }
        None => state
            .keys("Location")
            .find(|key| Some(*key) != location.as_deref())
            .map(str::to_string),
    };
    if ctx.arg_str("inventoryAction") != Some("DELETE") {
        mutation.note("fulfillmentServiceDelete: lsf drops the stock of the service's location rather than transfer or keep it");
    }
    let state = mutation.state_mut();
    if let Some(location) = &location {
        // Its open fulfillment orders move to the destination.
        let moving: Vec<String> = state
            .keys("FulfillmentOrder")
            .filter(|fo| {
                model::assigned_location(state, fo).as_deref() == Some(location.as_str())
                    && !matches!(
                        state.get(fo).and_then(|record| text(record, "status")),
                        Some("CLOSED" | "CANCELLED")
                    )
            })
            .map(str::to_string)
            .collect();
        for fo in moving {
            if let Some(destination) = &destination {
                let assigned = model::assigned_location_value(state, destination);
                if let Some(record) = state.get_mut(&fo) {
                    record.insert("assignedLocation".into(), assigned);
                    record.insert("requestStatus".into(), json!("UNSUBMITTED"));
                }
                state.touch(&fo);
            }
        }
        let levels = state
            .get(location)
            .map(|record| gids(record.get("inventoryLevels")))
            .unwrap_or_default();
        let mut products = Vec::new();
        for level in levels {
            let item = state
                .get(&level)
                .and_then(|record| text(record, "item"))
                .map(str::to_string);
            if let Some(item) = item {
                if let Some(Value::Array(item_levels)) = state
                    .get_mut(&item)
                    .and_then(|record| record.get_mut("inventoryLevels"))
                {
                    item_levels.retain(|known| known.as_str() != Some(level.as_str()));
                }
                let product = state
                    .get(&item)
                    .and_then(|record| text(record, "variant"))
                    .and_then(|variant| state.get(variant))
                    .and_then(|variant| text(variant, "product"))
                    .map(str::to_string);
                products.extend(product);
            }
            state.remove(&level);
        }
        for product in products {
            crate::domains::catalog::refresh_product(state, &product);
        }
        state.delete(location, "locations/delete");
    }
    state.remove(&service);
    Ok(payload(json!({"deletedId": service}), &[]))
}

/// A fulfillment order of the shop, with its status and request status.
fn find_fulfillment_order(
    state: &ShopState,
    id: &str,
) -> Result<(String, String, String), UserError> {
    let record = state
        .get(id)
        .filter(|_| state.type_of(id) == Some("FulfillmentOrder"))
        .ok_or_else(|| UserError::new(&["id"], "Fulfillment order could not be found."))?;
    Ok((
        id.to_string(),
        text(record, "status").unwrap_or("OPEN").to_string(),
        text(record, "requestStatus")
            .unwrap_or("UNSUBMITTED")
            .to_string(),
    ))
}

/// Records a merchant request on a fulfillment order: its GID.
fn merchant_request(
    state: &mut ShopState,
    fulfillment_order: &str,
    kind: &str,
    message: Option<&str>,
    options: Value,
) -> String {
    let now = state.timestamp();
    let id = state.new_gid("FulfillmentOrderMerchantRequest");
    state.insert(value::object(json!({
        "__typename": "FulfillmentOrderMerchantRequest",
        "id": id,
        "fulfillmentOrder": fulfillment_order,
        "kind": kind,
        "message": message,
        "requestOptions": options,
        "responseData": null,
        "sentAt": now,
    })));
    model::push_key(state, fulfillment_order, "merchantRequests", &id);
    id
}

/// `fulfillmentOrderSubmitFulfillmentRequest`.
fn submit_fulfillment_request(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| {
        Ok(payload(
            json!({"originalFulfillmentOrder": null, "submittedFulfillmentOrder": null, "unsubmittedFulfillmentOrder": null}),
            &[error],
        ))
    };
    let state = mutation.state();
    let (id, status, request) =
        match find_fulfillment_order(state, ctx.arg_str("id").unwrap_or_default()) {
            Ok(found) => found,
            Err(error) => return failed(error),
        };
    let at_service = model::assigned_location(state, &id)
        .and_then(|location| state.get(&location).cloned())
        .and_then(|location| {
            location
                .get("isFulfillmentService")
                .and_then(Value::as_bool)
        })
        == Some(true);
    if !at_service {
        return failed(UserError::new(
            &["id"],
            "The fulfillment order is not assigned to a fulfillment service's location.",
        ));
    }
    if !matches!(status.as_str(), "OPEN")
        || !matches!(
            request.as_str(),
            "UNSUBMITTED" | "REJECTED" | "CANCELLATION_ACCEPTED"
        )
    {
        return failed(UserError::new(
            &["id"],
            "The fulfillment order cannot be submitted in its current state.",
        ));
    }
    let record = state.get(&id).cloned().unwrap_or_default();
    let items = ctx
        .arg("fulfillmentOrderLineItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let lines = gids(record.get("lineItems"));
    let mut picked: Vec<(String, i64)> = Vec::new();
    for item in &items {
        let line = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        let remaining = state
            .get(line)
            .map_or(0, |line| int(line, "remainingQuantity"));
        if !lines.iter().any(|known| known == line) || quantity <= 0 || quantity > remaining {
            return failed(UserError::new(
                &["fulfillmentOrderLineItems"],
                "The fulfillment order line item quantity is invalid.",
            ));
        }
        picked.push((line.to_string(), quantity));
    }
    let rest: Vec<(String, i64)> = if items.is_empty() {
        Vec::new()
    } else {
        lines
            .iter()
            .filter_map(|line| {
                let remaining = state
                    .get(line)
                    .map_or(0, |item| int(item, "remainingQuantity"));
                let taken: i64 = picked
                    .iter()
                    .filter(|(known, _)| known == line)
                    .map(|(_, q)| q)
                    .sum();
                (remaining - taken > 0).then(|| (line.clone(), remaining - taken))
            })
            .collect()
    };
    if ctx.arg_bool("notifyCustomer") == Some(true) {
        mutation.note("fulfillmentOrderSubmitFulfillmentRequest: lsf sends no email: `notifyCustomer` was ignored");
    }
    let message = ctx.arg_str("message").map(str::to_string);
    let notify = ctx.arg_bool("notifyCustomer").unwrap_or(false);
    let state = mutation.state_mut();
    let order = text(&record, "order").unwrap_or_default().to_string();
    let unsubmitted = (!rest.is_empty()).then(|| {
        let new = split_off(state, &id, &rest, None, true);
        model::push_key(state, &order, "fulfillmentOrders", &new);
        new
    });
    let request_id = merchant_request(
        state,
        &id,
        "FULFILLMENT_REQUEST",
        message.as_deref(),
        json!({"notify_customer": notify}),
    );
    if let Some(record) = state.get_mut(&id) {
        record.insert("requestStatus".into(), json!("SUBMITTED"));
        record.insert(
            FULFILLMENT_ORDER_EVENT.into(),
            json!({"original": id, "submitted": id, "request": request_id, "message": message}),
        );
    }
    state.touch(&id);
    model::refresh_order(state, &order);
    state.emit("fulfillment_orders/fulfillment_request_submitted", &id);
    Ok(payload(
        json!({"originalFulfillmentOrder": id, "submittedFulfillmentOrder": id, "unsubmittedFulfillmentOrder": unsubmitted}),
        &[],
    ))
}

/// A service's answer to a request (or its closing of a fulfillment order): from one of the
/// request statuses `from` to `to`, the status following when `status` says so.
fn respond(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    from: &[&str],
    to: &str,
    status: Option<&str>,
    topic: &str,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| Ok(payload(json!({"fulfillmentOrder": null}), &[error]));
    let (id, current_status, request) =
        match find_fulfillment_order(mutation.state(), ctx.arg_str("id").unwrap_or_default()) {
            Ok(found) => found,
            Err(error) => return failed(error),
        };
    if !from.contains(&request.as_str())
        || matches!(current_status.as_str(), "CLOSED" | "CANCELLED")
    {
        return failed(UserError::new(
            &["id"],
            format!(
                "The fulfillment order's request status is {}: it cannot be {}.",
                request.to_ascii_lowercase(),
                to.to_ascii_lowercase().replace('_', " ")
            ),
        ));
    }
    let message = ctx.arg_str("message").map(str::to_string);
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("requestStatus".into(), json!(to));
        if let Some(status) = status {
            record.insert("status".into(), json!(status));
        }
        record.insert(FULFILLMENT_ORDER_EVENT.into(), json!({"message": message}));
    }
    state.touch(&id);
    if let Some(order) = model::order_of(state, &id) {
        model::refresh_order(state, &order);
    }
    state.emit(topic, &id);
    Ok(payload(json!({"fulfillmentOrder": id}), &[]))
}

/// `fulfillmentOrderSubmitCancellationRequest`.
fn submit_cancellation_request(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| Ok(payload(json!({"fulfillmentOrder": null}), &[error]));
    let (id, _, request) =
        match find_fulfillment_order(mutation.state(), ctx.arg_str("id").unwrap_or_default()) {
            Ok(found) => found,
            Err(error) => return failed(error),
        };
    if !matches!(request.as_str(), "ACCEPTED" | "CANCELLATION_REJECTED") {
        return failed(UserError::new(
            &["id"],
            "Only a fulfillment order whose request was accepted can be asked to be cancelled.",
        ));
    }
    let message = ctx.arg_str("message").map(str::to_string);
    let state = mutation.state_mut();
    let request_id = merchant_request(
        state,
        &id,
        "CANCELLATION_REQUEST",
        message.as_deref(),
        json!({}),
    );
    if let Some(record) = state.get_mut(&id) {
        record.insert("requestStatus".into(), json!("CANCELLATION_REQUESTED"));
        record.insert(
            FULFILLMENT_ORDER_EVENT.into(),
            json!({"request": request_id, "message": message}),
        );
    }
    state.touch(&id);
    state.emit("fulfillment_orders/cancellation_request_submitted", &id);
    Ok(payload(json!({"fulfillmentOrder": id}), &[]))
}

/// `fulfillmentOrderAcceptCancellationRequest`: the fulfillment order closes; its units are to
/// fulfill again on a new one at the same location.
fn accept_cancellation(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| Ok(payload(json!({"fulfillmentOrder": null}), &[error]));
    let (id, _, request) =
        match find_fulfillment_order(mutation.state(), ctx.arg_str("id").unwrap_or_default()) {
            Ok(found) => found,
            Err(error) => return failed(error),
        };
    if request != "CANCELLATION_REQUESTED" {
        return failed(UserError::new(
            &["id"],
            "The fulfillment order has no cancellation request to accept.",
        ));
    }
    let message = ctx.arg_str("message").map(str::to_string);
    let state = mutation.state_mut();
    let record = state.get(&id).cloned().unwrap_or_default();
    let rest: Vec<(String, i64)> = gids(record.get("lineItems"))
        .into_iter()
        .filter_map(|line| {
            let remaining = state
                .get(&line)
                .map_or(0, |item| int(item, "remainingQuantity"));
            (remaining > 0).then_some((line, remaining))
        })
        .collect();
    let order = text(&record, "order").unwrap_or_default().to_string();
    if !rest.is_empty() {
        let replacement = split_off(state, &id, &rest, None, true);
        model::push_key(state, &order, "fulfillmentOrders", &replacement);
    }
    if let Some(record) = state.get_mut(&id) {
        record.insert("requestStatus".into(), json!("CANCELLATION_ACCEPTED"));
        record.insert("status".into(), json!("CLOSED"));
        record.insert(FULFILLMENT_ORDER_EVENT.into(), json!({"message": message}));
    }
    state.touch(&id);
    model::refresh_order(state, &order);
    state.emit("fulfillment_orders/cancellation_request_accepted", &id);
    Ok(payload(json!({"fulfillmentOrder": id}), &[]))
}
