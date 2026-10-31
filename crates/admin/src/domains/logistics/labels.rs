//! `shippingLabelPurchase`: one label for one fulfillment order, "purchased" at once.
//!
//! As the reference of 2026-10 describes it, the mutation validates the fulfillment order ("is
//! fulfillable and requires shipping", "has a destination shipping address", "has an assigned
//! location"), the shipping date, the package and the weight, then answers a
//! `ShippingLabelPurchaseResult` that starts `PENDING_PURCHASE` and ends `PURCHASED`. lsf buys
//! nothing: the result is `PURCHASED` at once (`done`, readable with `node`), with a
//! `ShippingLabel` whose tracking number lsf makes from the label's id (the same every run), and
//! the purchase is recorded in the shop's outbox (`GET /__lsf/admin`), the carrier and service
//! asked included. The label's document has no URL: there is no PDF.
//!
//! The units are then fulfilled with the label's tracking, as a label bought in Shopify's admin
//! fulfills them: through `fulfillmentCreate` (its events, stock and statuses), within the same
//! mutation. `notifyCustomer` puts the shipping confirmation in the outbox; nothing is sent.
//!
//! Errors (the descriptions of `ShippingLabelPurchaseUserErrorCode`): `NOT_FOUND`,
//! `FULFILLMENT_ORDER_INVALID`, `PICKUP_POINT_NOT_SUPPORTED`, `AT_LEAST_1_ITEM_REQUIRED`,
//! `SHIPPING_DATE_IN_THE_PAST` (a date before the shop's day), `INVALID_PACKAGE_DIMENSIONS`,
//! `TOTAL_WEIGHT_ZERO` (the weight given, else the remaining items'), `CARRIER_NOT_SUPPORTED`
//! ("FedEx label purchases are not supported"), `PACKAGE_CARRIER_MISMATCH`. The rest (terms of
//! service, customs, rates) cannot fail locally. The carrier is the one asked
//! (`preferredRateSelection`), else USPS for a shipment from the United States, Canada Post from
//! Canada, DHL Express otherwise *(chosen)*.

use serde_json::{Map, Value, json};

use super::{coded, int, objects, send, text};
use crate::domains::catalog::gids;
use crate::domains::orders::model;
use crate::graphql::{self, FieldContext, FieldError, Request};
use crate::resolve::{Mutation, Registry, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry.mutation("shippingLabelPurchase", purchase);
}

/// A weight in grams, from a `Weight` or a `WeightInput` (`{unit, value}`).
fn grams(weight: Option<&Value>) -> f64 {
    let Some(weight) = weight else {
        return 0.0;
    };
    let amount = weight.get("value").and_then(Value::as_f64).unwrap_or(0.0);
    match weight.get("unit").and_then(Value::as_str) {
        Some("KILOGRAMS") => amount * 1000.0,
        Some("POUNDS") => amount * 453.592,
        Some("OUNCES") => amount * 28.3495,
        _ => amount,
    }
}

/// The remaining units of a fulfillment order and their weight in grams.
fn remaining(state: &ShopState, record: &Record) -> (i64, f64) {
    gids(record.get("lineItems"))
        .iter()
        .filter_map(|line| state.get(line))
        .fold((0, 0.0), |(units, weight), line| {
            let quantity = int(line, "remainingQuantity");
            (
                units + quantity,
                weight + grams(line.get("weight")) * quantity as f64,
            )
        })
}

/// The carrier a label is bought from: `(display name, code)`.
fn carrier(code: Option<&str>, origin_country: Option<&str>) -> (String, String) {
    let code = code.map(str::to_ascii_lowercase).unwrap_or_else(|| {
        match origin_country {
            Some("US") => "usps",
            Some("CA") => "canada_post",
            _ => "dhl_express",
        }
        .to_string()
    });
    let name = match code.as_str() {
        "usps" => "USPS".to_string(),
        "ups" => "UPS".to_string(),
        "canada_post" => "Canada Post".to_string(),
        "dhl_express" => "DHL Express".to_string(),
        other => value::humanize(other),
    };
    (name, code)
}

/// `shippingLabelPurchase`.
fn purchase(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"shippingLabelPurchaseResult": null});
    let input = super::input(ctx, "shippingLabelPurchase");
    let state = mutation.state();
    let at = |field: &'static str| ["shippingLabelPurchase", field];
    let fail = |field: &[&str], code: &str| Ok(payload(empty.clone(), &[coded(ctx, field, code)]));
    let id = text(&input, "fulfillmentOrderId")
        .unwrap_or_default()
        .to_string();
    let Some(record) = (state.type_of(&id) == Some("FulfillmentOrder"))
        .then(|| state.get(&id).cloned())
        .flatten()
    else {
        return fail(&at("fulfillmentOrderId"), "NOT_FOUND");
    };
    let method = record
        .get("deliveryMethod")
        .and_then(|method| method.get("methodType"))
        .and_then(Value::as_str);
    if method == Some("PICKUP_POINT") {
        return fail(&at("fulfillmentOrderId"), "PICKUP_POINT_NOT_SUPPORTED");
    }
    let requires_shipping = gids(record.get("lineItems"))
        .iter()
        .filter_map(|line| state.get(line))
        .any(|line| line.get("requiresShipping").and_then(Value::as_bool) != Some(false));
    let destination = record
        .get("destination")
        .and_then(Value::as_object)
        .filter(|address| {
            text(address, "address1").is_some() && text(address, "countryCode").is_some()
        });
    let origin = model::assigned_location(state, &id);
    if !matches!(text(&record, "status"), Some("OPEN" | "IN_PROGRESS"))
        || !matches!(method, Some("SHIPPING") | None)
        || !requires_shipping
        || destination.is_none()
        || origin.is_none()
    {
        return fail(&at("fulfillmentOrderId"), "FULFILLMENT_ORDER_INVALID");
    }
    let (units, weight) = remaining(state, &record);
    if units == 0 {
        return fail(&at("fulfillmentOrderId"), "AT_LEAST_1_ITEM_REQUIRED");
    }
    let today = state.now().date_naive();
    let shipping_date = text(&input, "shippingDatetime")
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.with_timezone(&chrono::Utc));
    match shipping_date {
        None => return fail(&at("shippingDatetime"), "MISSING_SHIPPING_DATE"),
        Some(date) if date.date_naive() < today => {
            return fail(&at("shippingDatetime"), "SHIPPING_DATE_IN_THE_PAST");
        }
        Some(_) => {}
    }
    let package = input.get("packageInfo").and_then(Value::as_object);
    let custom = package.and_then(|package| package.get("customPackage"));
    if let Some(dimensions) = custom.and_then(|custom| custom.get("dimensions"))
        && ["length", "width", "height"]
            .iter()
            .any(|side| dimensions.get(side).and_then(Value::as_f64).unwrap_or(0.0) <= 0.0)
    {
        return fail(
            &["shippingLabelPurchase", "packageInfo"],
            "INVALID_PACKAGE_DIMENSIONS",
        );
    }
    // Without a package, the shop's default one (`shippingPackageMakeDefault`).
    let default_package = super::delivery::default_package(state);
    let package_weight = match package {
        Some(_) => grams(custom.and_then(|custom| custom.get("weight"))),
        None => grams(
            default_package
                .as_ref()
                .and_then(|package| package.get("weight")),
        ),
    };
    let weight = match input.get("totalWeight") {
        Some(given) if !given.is_null() => grams(Some(given)),
        _ => weight + package_weight,
    };
    if weight <= 0.0 {
        return fail(&at("totalWeight"), "TOTAL_WEIGHT_ZERO");
    }
    let preferred = input
        .get("preferredRateSelection")
        .and_then(Value::as_object);
    let carrier_code = preferred.and_then(|preferred| text(preferred, "carrierCode"));
    if carrier_code.is_some_and(|code| code.eq_ignore_ascii_case("fedex")) {
        return fail(&at("preferredRateSelection"), "CARRIER_NOT_SUPPORTED");
    }
    if let (Some(asked), Some(package_carrier)) = (
        carrier_code,
        package
            .and_then(|package| package.get("carrierPackage"))
            .and_then(|carrier| carrier.get("carrierCode"))
            .and_then(Value::as_str),
    ) && !asked.eq_ignore_ascii_case(package_carrier)
    {
        return fail(
            &["shippingLabelPurchase", "packageInfo"],
            "PACKAGE_CARRIER_MISMATCH",
        );
    }
    let origin = origin.unwrap_or_default();
    let origin_country = state
        .get(&origin)
        .and_then(|location| location.get("address"))
        .and_then(|address| address.get("countryCode"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let (company, code) = carrier(carrier_code, origin_country.as_deref());
    let service = preferred
        .and_then(|preferred| text(preferred, "serviceCode"))
        .map(str::to_string);
    let notify = input.get("notifyCustomer").and_then(Value::as_bool) == Some(true);

    let state = mutation.state_mut();
    let label = state.new_gid("ShippingLabel");
    let number = format!(
        "LSF{:012}",
        lsf_core::util::stable_id("shipping_label", &label) % 1_000_000_000_000
    );
    let tracking = json!({"company": company, "number": number, "url": null});
    state.insert(value::object(json!({
        "__typename": "ShippingLabel",
        "id": label,
        "cancellable": true,
        "printed": false,
        "location": origin,
        "trackingInfo": tracking,
        "shippingDocuments": [{
            "documentType": "LABEL",
            "format": "PDF",
            "printedAt": null,
            "shippingLabelId": label,
            "url": null,
        }],
    })));
    let result = state.new_gid("ShippingLabelPurchaseResult");
    state.insert(value::object(json!({
        "__typename": "ShippingLabelPurchaseResult",
        "id": result,
        "done": true,
        "status": "PURCHASED",
        "errors": [],
        "shippingLabels": [label],
    })));
    send(
        state,
        "shipping_label",
        &label,
        json!({
            "fulfillmentOrder": id,
            "carrier": company,
            "carrierCode": code,
            "serviceCode": service,
            "trackingNumber": number,
            "weightGrams": weight.round(),
            "shippingDatetime": shipping_date.map(value::date_time),
            "package": package.cloned().map(Value::Object).or(default_package),
        }),
    );
    fulfill(mutation, ctx.schema(), &id, &tracking, notify)?;
    mutation.note(
        "shippingLabelPurchase: lsf buys nothing: the label is recorded in the shop's outbox \
         (GET /__lsf/admin)",
    );
    Ok(payload(json!({"shippingLabelPurchaseResult": result}), &[]))
}

/// Fulfills what remains of a fulfillment order with a label's tracking, through
/// `fulfillmentCreate` run within this mutation.
fn fulfill(
    mutation: &mut Mutation<'_>,
    schema: &crate::schema::Schema,
    fulfillment_order: &str,
    tracking: &Value,
    notify: bool,
) -> Result<(), FieldError> {
    let query = "mutation($fulfillment: FulfillmentInput!) { fulfillmentCreate(fulfillment: $fulfillment) { fulfillment { id } userErrors { message } } }";
    let variables = json!({"fulfillment": {
        "lineItemsByFulfillmentOrder": [{"fulfillmentOrderId": fulfillment_order}],
        "trackingInfo": {"company": tracking["company"], "number": tracking["number"]},
        "notifyCustomer": notify,
    }});
    let request = Request::new(query).with_variables(variables);
    let answer = {
        let mut resolver = mutation.resolver();
        graphql::run(schema, &mut resolver, &request, None).to_json()
    };
    let errors: Vec<Map<String, Value>> =
        objects(answer.pointer("/data/fulfillmentCreate/userErrors"));
    if let Some(error) = errors.first().or(answer
        .get("errors")
        .and_then(Value::as_array)
        .and_then(|errors| errors.first())
        .and_then(Value::as_object))
    {
        return Err(FieldError::new(format!(
            "shippingLabelPurchase: the fulfillment of the label failed: {}",
            text(error, "message").unwrap_or_default()
        )));
    }
    if notify {
        let state = mutation.state_mut();
        let order = model::order_of(state, fulfillment_order);
        let email = order
            .as_deref()
            .and_then(|order| state.get(order))
            .and_then(|order| order.get("email").cloned())
            .unwrap_or(Value::Null);
        send(
            state,
            "notification",
            fulfillment_order,
            json!({"template": "Shipping confirmation", "to": email, "order": order, "tracking": tracking}),
        );
    }
    Ok(())
}
