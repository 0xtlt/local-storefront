//! The queries of the family that the generic rules do not answer: the search filters and sort
//! keys of `orders` and `draftOrders`, and the fields derived from the state of a fulfillment
//! order or a fulfillment.
//!
//! - **`orders(query:)`**: the filters of shopify.dev's reference (`queries/orders`, 2026-10)
//!   whose value is not a field of the record: `financial_status` (`displayFinancialStatus`, in
//!   lowercase), `fulfillment_status` (`shipped` and `fulfilled` for a fulfilled order,
//!   `partial`, `unshipped` for one that shipped nothing, `unfulfilled` for one with something
//!   left to ship, `on_hold`, `scheduled`), `status` (`open`, `closed`, `cancelled`,
//!   `not_closed`), `name` (with or without `#`), `sku` (of its line items), `gateway`,
//!   `current_total_price`, `return_status`, `delivery_method`, `location_id` and
//!   `fulfillment_location_id`. The others are read generically (`email`, `customer_id`, `tag`,
//!   the dates, `test`, `source_name`, `po_number`, …). A free term searches the name, the
//!   email, the phone, the customer's name and the tags.
//! - **Sort keys** of `OrderSortKeys` and `DraftOrderSortKeys` that are not a field:
//!   `ORDER_NUMBER`, `TOTAL_PRICE`, `CURRENT_TOTAL_PRICE`, `CUSTOMER_NAME`, `FINANCIAL_STATUS`,
//!   `FULFILLMENT_STATUS`, `TOTAL_ITEMS_QUANTITY`, `DESTINATION`; `NUMBER`, `STATUS`,
//!   `TOTAL_PRICE`, `CUSTOMER_NAME` of draft orders.
//! - **`fulfillmentOrders`** lists the open ones unless `includeClosed` (closed and cancelled
//!   ones are closed); **`manualHoldsFulfillmentOrders`** those on hold;
//!   **`assignedFulfillmentOrders`** those assigned to a location of a fulfillment service of
//!   the app, by `assignmentStatus` (the request status) and `locationIds`.
//! - **`FulfillmentOrder.supportedActions`**: what a merchant-managed fulfillment order of its
//!   status allows, as the admin offers it (`CREATE_FULFILLMENT`, `HOLD`, `MOVE` when another
//!   location could take it, `SPLIT` when it has more than one unit left, `RELEASE_HOLD`,
//!   `MARK_AS_OPEN`); `REQUEST_FULFILLMENT` and `CANCEL_FULFILLMENT_ORDER` for one at a
//!   fulfillment service's location.
//! - **`FulfillmentOrder.locationsForMove`**: the other active locations, every line item
//!   movable (lsf does not check their stock).
//! - **`returnableFulfillments(orderId:)`** and **`returnableFulfillment(id:)`**: each successful
//!   fulfillment of the order with what is left to return of each of its lines (shipped less
//!   what returns that were not declined or canceled hold). Its id is the fulfillment's number
//!   as a `ReturnableFulfillment` GID *(chosen)*.

use serde_json::{Value, json};

use super::model::{self, int, text};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::sort::SortValue;
use crate::resolve::{Obj, Registry, ShopResolver};
use crate::search::SearchValue;
use crate::state::{Record, gid, parse_gid};

pub fn register(registry: &mut Registry) {
    registry
        .search_field("Order", "financial_status", |_r, _item, order| {
            Some(keywords(
                text(order, "displayFinancialStatus").map(str::to_ascii_lowercase),
            ))
        })
        .search_field("Order", "fulfillment_status", |_r, _item, order| {
            Some(fulfillment_status_values(order))
        })
        .search_field("Order", "status", |_r, _item, order| {
            Some(status_values(order))
        })
        .search_field("Order", "name", |_r, _item, order| {
            let name = text(order, "name").unwrap_or_default();
            Some(
                [name, name.trim_start_matches('#')]
                    .iter()
                    .filter(|name| !name.is_empty())
                    .map(|name| SearchValue::Keyword(name.to_string()))
                    .collect(),
            )
        })
        .search_field("Order", "sku", |r, _item, order| {
            Some(
                gids(order.get("lineItems"))
                    .iter()
                    .filter_map(|line| r.state().get(line))
                    .filter_map(|line| text(line, "sku"))
                    .map(|sku| SearchValue::Keyword(sku.to_string()))
                    .collect(),
            )
        })
        // "A list of the names of all payment gateways used for the order": those of its
        // transactions, each once, in the order they were used.
        .field("Order", "paymentGatewayNames", |r, _ctx, object| {
            let mut names: Vec<String> = Vec::new();
            for transaction in r
                .map_of(object)
                .map(|order| gids(order.get("transactions")))
                .unwrap_or_default()
                .iter()
                .filter_map(|id| r.state().get(id))
            {
                if let Some(gateway) = text(transaction, "gateway")
                    && !names.iter().any(|known| known == gateway)
                {
                    names.push(gateway.to_string());
                }
            }
            Ok(Output::value(names))
        })
        .search_field("Order", "gateway", |r, _item, order| {
            let mut gateways: Vec<SearchValue> = Vec::new();
            for transaction in gids(order.get("transactions"))
                .iter()
                .filter_map(|id| r.state().get(id))
            {
                if let Some(gateway) = text(transaction, "gateway")
                    && !gateways
                        .iter()
                        .any(|known| *known == SearchValue::Keyword(gateway.to_string()))
                {
                    gateways.push(SearchValue::Keyword(gateway.to_string()));
                }
            }
            Some(gateways)
        })
        .search_field("Order", "current_total_price", |_r, _item, order| {
            Some(vec![SearchValue::Number(
                model::field_cents(order, "currentTotalPriceSet") as f64 / 100.0,
            )])
        })
        .search_field("Order", "return_status", |_r, _item, order| {
            Some(keywords(
                text(order, "returnStatus").map(str::to_ascii_lowercase),
            ))
        })
        .search_field("Order", "delivery_method", |r, _item, order| {
            Some(
                fulfillment_orders(r, order)
                    .iter()
                    .filter_map(|fulfillment_order| {
                        fulfillment_order
                            .get("deliveryMethod")?
                            .get("methodType")?
                            .as_str()
                    })
                    .map(|method| {
                        SearchValue::Keyword(match method {
                            "PICK_UP" => "pick-up".to_string(),
                            "PICKUP_POINT" => "pickup-point".to_string(),
                            other => other.to_ascii_lowercase(),
                        })
                    })
                    .collect(),
            )
        })
        .search_field("Order", "fulfillment_location_id", |r, _item, order| {
            Some(ids(gids(order.get("fulfillments")).iter().filter_map(
                |fulfillment| r.state().get(fulfillment).and_then(|f| text(f, "location")),
            )))
        })
        .search_field("Order", "location_id", |r, _item, order| {
            Some(ids(fulfillment_orders(r, order).iter().filter_map(
                |fulfillment_order| {
                    fulfillment_order
                        .get("assignedLocation")?
                        .get("location")?
                        .as_str()
                },
            )))
        })
        .default_text("Order", |r, order| {
            let mut texts: Vec<String> = ["name", "email", "phone"]
                .iter()
                .filter_map(|field| text(order, field).map(str::to_string))
                .collect();
            if let Some(name) = text(order, "name") {
                texts.push(name.trim_start_matches('#').to_string());
            }
            if let Some(customer) = text(order, "customer").and_then(|id| r.state().get(id)) {
                texts.extend(text(customer, "displayName").map(str::to_string));
            }
            texts.extend(
                order
                    .get("tags")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string),
            );
            texts
        })
        .default_text("DraftOrder", |r, draft| {
            let mut texts: Vec<String> = ["name", "email"]
                .iter()
                .filter_map(|field| text(draft, field).map(str::to_string))
                .collect();
            if let Some(customer) = text(draft, "customer").and_then(|id| r.state().get(id)) {
                texts.extend(text(customer, "displayName").map(str::to_string));
            }
            texts
        })
        .sort_key("Order", "ORDER_NUMBER", |r, item| {
            SortValue::of(r.item_map(item).and_then(|order| order.get("number")))
        })
        .sort_key("Order", "TOTAL_PRICE", |r, item| {
            amount(r, item, "totalPriceSet")
        })
        .sort_key("Order", "CURRENT_TOTAL_PRICE", |r, item| {
            amount(r, item, "currentTotalPriceSet")
        })
        .sort_key("Order", "CUSTOMER_NAME", customer_name)
        .sort_key("Order", "FINANCIAL_STATUS", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|order| order.get("displayFinancialStatus")),
            )
        })
        .sort_key("Order", "FULFILLMENT_STATUS", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|order| order.get("displayFulfillmentStatus")),
            )
        })
        .sort_key("Order", "TOTAL_ITEMS_QUANTITY", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|order| order.get("currentSubtotalLineItemsQuantity")),
            )
        })
        .sort_key("Order", "DESTINATION", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|order| order.get("shippingAddress"))
                    .and_then(|address| address.get("city")),
            )
        })
        .sort_key("DraftOrder", "NUMBER", |r, item| {
            let number = r
                .item_map(item)
                .and_then(|draft| text(draft, "name"))
                .and_then(|name| name.trim_start_matches("#D").parse::<f64>().ok());
            number.map_or(SortValue::Missing, SortValue::Number)
        })
        .sort_key("DraftOrder", "STATUS", |r, item| {
            SortValue::of(r.item_map(item).and_then(|draft| draft.get("status")))
        })
        .sort_key("DraftOrder", "TOTAL_PRICE", |r, item| {
            amount(r, item, "totalPriceSet")
        })
        .sort_key("DraftOrder", "CUSTOMER_NAME", customer_name)
        .query("fulfillmentOrders", |r, ctx, _object| {
            let include_closed = ctx
                .arg("includeClosed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let items = fulfillment_orders_where(r, |record| {
                include_closed || !matches!(text(record, "status"), Some("CLOSED" | "CANCELLED"))
            });
            r.connection(ctx, items)
        })
        .query("manualHoldsFulfillmentOrders", |r, ctx, _object| {
            let items =
                fulfillment_orders_where(r, |record| text(record, "status") == Some("ON_HOLD"));
            r.connection(ctx, items)
        })
        .query("assignedFulfillmentOrders", assigned_fulfillment_orders)
        .field("FulfillmentOrder", "supportedActions", supported_actions)
        .field("FulfillmentOrder", "locationsForMove", locations_for_move)
        .query("returnableFulfillments", |r, ctx, _object| {
            let order = ctx.arg_str("orderId").unwrap_or_default();
            let items = returnable_fulfillments(r, order);
            r.connection(ctx, items)
        })
        .query("returnableFulfillment", |r, ctx, _object| {
            let id = ctx.arg_str("id").unwrap_or_default();
            let fulfillment = parse_gid(id)
                .filter(|(type_name, _)| *type_name == "ReturnableFulfillment")
                .map(|(_, number)| gid("Fulfillment", number));
            let found = fulfillment
                .as_deref()
                .and_then(|fulfillment| model::order_of(r.state(), fulfillment))
                .and_then(|order| {
                    returnable_fulfillments(r, &order)
                        .into_iter()
                        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
                });
            match found {
                Some(Value::Object(map)) => {
                    Ok(Output::Object(Obj::built("ReturnableFulfillment", map)))
                }
                _ => Ok(Output::Null),
            }
        });
}

fn keywords(value: Option<String>) -> Vec<SearchValue> {
    value.map(SearchValue::Keyword).into_iter().collect()
}

fn ids<'a>(gids: impl Iterator<Item = &'a str>) -> Vec<SearchValue> {
    gids.filter_map(parse_gid)
        .map(|(_, id)| SearchValue::Id(id))
        .collect()
}

/// What `fulfillment_status:` matches an order on, as shopify.dev's valid values name them.
fn fulfillment_status_values(order: &Record) -> Vec<SearchValue> {
    let values: &[&str] = match text(order, "displayFulfillmentStatus") {
        Some("FULFILLED") => &["fulfilled", "shipped"],
        Some("PARTIALLY_FULFILLED") => &["partial", "unfulfilled"],
        Some("ON_HOLD") => &["on_hold", "unfulfilled"],
        Some("SCHEDULED") => &["scheduled", "unfulfilled"],
        Some("REQUEST_DECLINED") => &["request_declined", "unfulfilled"],
        Some("RESTOCKED") => &["restocked"],
        _ => &["unshipped", "unfulfilled"],
    };
    values
        .iter()
        .map(|value| SearchValue::Keyword(value.to_string()))
        .collect()
}

/// What `status:` matches an order on: `cancelled`, `closed`, else `open`; `not_closed` unless
/// closed.
fn status_values(order: &Record) -> Vec<SearchValue> {
    let closed = order.get("closed").and_then(Value::as_bool) == Some(true);
    let mut values = Vec::new();
    if model::is_cancelled(order) {
        values.push("cancelled");
    } else if !closed {
        values.push("open");
    }
    values.push(if closed { "closed" } else { "not_closed" });
    values
        .into_iter()
        .map(|value| SearchValue::Keyword(value.to_string()))
        .collect()
}

fn fulfillment_orders<'s>(r: &'s ShopResolver<'_>, order: &Record) -> Vec<&'s Record> {
    gids(order.get("fulfillmentOrders"))
        .iter()
        .filter_map(|id| r.state().get(id))
        .collect()
}

fn amount(r: &ShopResolver<'_>, item: &Value, field: &str) -> SortValue {
    match r.item_map(item) {
        Some(record) => SortValue::Number(model::field_cents(record, field) as f64 / 100.0),
        None => SortValue::Missing,
    }
}

fn customer_name(r: &ShopResolver<'_>, item: &Value) -> SortValue {
    SortValue::of(
        r.item_map(item)
            .and_then(|record| text(record, "customer"))
            .and_then(|customer| r.state().get(customer))
            .and_then(|customer| customer.get("displayName")),
    )
}

fn fulfillment_orders_where(r: &ShopResolver<'_>, keep: impl Fn(&Record) -> bool) -> Vec<Value> {
    r.state()
        .keys("FulfillmentOrder")
        .filter(|key| r.state().get(key).is_some_and(&keep))
        .map(|key| json!(key))
        .collect()
}

/// Whether a location is a fulfillment service's.
fn is_service_location(r: &ShopResolver<'_>, location: Option<&str>) -> bool {
    location
        .and_then(|location| r.state().get(location))
        .and_then(|location| location.get("isFulfillmentService"))
        .and_then(Value::as_bool)
        == Some(true)
}

fn assigned_fulfillment_orders(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let status = ctx.arg_str("assignmentStatus").map(|status| match status {
        "FULFILLMENT_REQUESTED" => "SUBMITTED",
        "FULFILLMENT_ACCEPTED" => "ACCEPTED",
        "CANCELLATION_REQUESTED" => "CANCELLATION_REQUESTED",
        _ => "UNSUBMITTED",
    });
    let locations: Option<Vec<String>> = ctx.arg("locationIds").map(|ids| gids(Some(ids)));
    let items = fulfillment_orders_where(r, |record| {
        let location = record
            .get("assignedLocation")
            .and_then(|assigned| assigned.get("location"))
            .and_then(Value::as_str);
        is_service_location(r, location)
            && locations.as_ref().is_none_or(|ids| {
                location.is_some_and(|location| ids.iter().any(|id| id == location))
            })
            && status.is_none_or(|status| text(record, "requestStatus") == Some(status))
            && !matches!(text(record, "status"), Some("CLOSED" | "CANCELLED"))
    });
    r.connection(ctx, items)
}

/// The active locations other than the one a fulfillment order is assigned to.
fn other_locations(r: &ShopResolver<'_>, fulfillment_order: &Record) -> Vec<String> {
    let assigned = fulfillment_order
        .get("assignedLocation")
        .and_then(|assigned| assigned.get("location"))
        .and_then(Value::as_str);
    r.state()
        .keys("Location")
        .filter(|location| Some(*location) != assigned)
        .filter(|location| {
            r.state().get(location).is_some_and(|record| {
                record.get("isActive").and_then(Value::as_bool) != Some(false)
            })
        })
        .map(str::to_string)
        .collect()
}

fn supported_actions(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(record) = r.map_of(object) else {
        return r.generic(ctx, object);
    };
    let remaining: i64 = gids(record.get("lineItems"))
        .iter()
        .filter_map(|id| r.state().get(id))
        .map(|item| int(item, "remainingQuantity"))
        .sum();
    let location = record
        .get("assignedLocation")
        .and_then(|assigned| assigned.get("location"))
        .and_then(Value::as_str);
    let movable = !other_locations(r, record).is_empty();
    let mut actions = Vec::new();
    match text(record, "status") {
        Some("OPEN" | "IN_PROGRESS") if is_service_location(r, location) => {
            match text(record, "requestStatus") {
                Some("UNSUBMITTED" | "REJECTED" | "CANCELLATION_ACCEPTED") | None => {
                    actions.extend(["REQUEST_FULFILLMENT", "HOLD"]);
                    if movable {
                        actions.push("MOVE");
                    }
                }
                Some("SUBMITTED" | "ACCEPTED") => actions.push("REQUEST_CANCELLATION"),
                _ => {}
            }
        }
        Some("OPEN" | "IN_PROGRESS") => {
            actions.extend(["CREATE_FULFILLMENT", "HOLD"]);
            if movable {
                actions.push("MOVE");
            }
            if remaining > 1 {
                actions.push("SPLIT");
            }
        }
        Some("ON_HOLD") => {
            actions.extend(["RELEASE_HOLD", "HOLD"]);
            if movable {
                actions.push("MOVE");
            }
        }
        Some("SCHEDULED") => {
            actions.push("MARK_AS_OPEN");
            if movable {
                actions.push("MOVE");
            }
        }
        Some("INCOMPLETE") => actions.push("MARK_AS_OPEN"),
        _ => {}
    }
    let value: Vec<Value> = actions
        .into_iter()
        .map(|action| json!({"action": action, "externalUrl": null}))
        .collect();
    r.convert(ctx, &Value::Array(value), false)
}

fn locations_for_move(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(record) = r.map_of(object) else {
        return r.generic(ctx, object);
    };
    let lines = record.get("lineItems").cloned().unwrap_or(json!([]));
    let count = lines.as_array().map_or(0, Vec::len);
    let items: Vec<Value> = other_locations(r, record)
        .into_iter()
        .map(|location| {
            json!({
                "location": location,
                "movable": true,
                "message": null,
                "availableLineItems": lines,
                "availableLineItemsCount": {"count": count, "precision": "EXACT"},
                "unavailableLineItems": [],
                "unavailableLineItemsCount": {"count": 0, "precision": "EXACT"},
            })
        })
        .collect();
    r.connection(ctx, items)
}

/// The returnable fulfillments of an order, as inline `ReturnableFulfillment`s.
fn returnable_fulfillments(r: &ShopResolver<'_>, order: &str) -> Vec<Value> {
    let state = r.state();
    let Some(record) = state
        .get(order)
        .filter(|_| state.type_of(order) == Some("Order"))
    else {
        return Vec::new();
    };
    gids(record.get("fulfillments"))
        .iter()
        .filter(|fulfillment| {
            state
                .get(fulfillment)
                .is_some_and(|fulfillment| text(fulfillment, "status") == Some("SUCCESS"))
        })
        .filter_map(|fulfillment| {
            let number = parse_gid(fulfillment)?.1;
            let lines: Vec<Value> = gids(state.get(fulfillment)?.get("fulfillmentLineItems"))
                .iter()
                .filter_map(|line| {
                    let left = super::returns::returnable_quantity(state, line);
                    (left > 0).then(|| json!({"fulfillmentLineItem": line, "quantity": left}))
                })
                .collect();
            (!lines.is_empty()).then(|| {
                json!({
                    "id": gid("ReturnableFulfillment", number),
                    "fulfillment": fulfillment,
                    "returnableFulfillmentLineItems": lines,
                })
            })
        })
        .collect()
}
