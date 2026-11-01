//! Carrier services: `carrierServiceCreate`, `carrierServiceUpdate`, `carrierServiceDelete`,
//! `carrierService(s)` (sort keys `CREATED_AT`, `ID`, `UPDATED_AT`, search `active`
//! and `id`), `availableCarrierServices`, and `DeliveryCarrierService.availableServicesForCountries`.
//!
//! A carrier service is the app's: "Carrier services provide real-time shipping rates from
//! external providers … through a callback URL. When customers reach checkout, Shopify sends
//! order details to your callback URL and displays the returned shipping rates." lsf never calls
//! the callback: the local checkout offers the rates of the store data (`checkout.shipping_rates`,
//! and the general delivery profile's: see `profiles`), and `availableServicesForCountries`,
//! which Shopify reads from the service's discovery answer, is empty, with a note.
//!
//! Each payload's error type has one code (`CARRIER_SERVICE_CREATE_FAILED`, …), and shopify.dev
//! publishes no message: the messages are Rails' validations, as REST's carrier services answer
//! them (`can't be blank`, `has already been taken`) *(lsf's wording)*. `formattedName` is the
//! name *(chosen)*; `icon` (Shopify's image of the app) is not known, so it is a default,
//! reported in `extensions.lsf.defaulted`. Shopify has no webhook topic for carrier services:
//! their changes are `carrier_services/create`, `…/update`, `…/delete`, which no subscription can
//! name.

use lsf_core::urls::is_http_url;
use serde_json::{Map, Value, json};

use super::{profiles, text};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::sort::SortValue;
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("carrierServiceCreate", create)
        .mutation("carrierServiceUpdate", update)
        .mutation("carrierServiceDelete", delete)
        .query("availableCarrierServices", available)
        .sort_key("DeliveryCarrierService", "UPDATED_AT", |r, item| {
            SortValue::of(r.item_map(item).and_then(|service| service.get(UPDATED_AT)))
        })
        .field(
            "DeliveryCarrierService",
            "availableServicesForCountries",
            |r, _ctx, _object| {
                r.note(
                    "DeliveryCarrierService.availableServicesForCountries: lsf does not call the \
                     carrier service's callbackUrl, so it knows none of its services",
                );
                Ok(Output::List(Vec::new()))
            },
        );
}

/// The record field where a carrier service keeps when it last changed, which its type has no
/// field for: what `carrierServices(sortKey: UPDATED_AT)` sorts by.
const UPDATED_AT: &str = "lsfUpdatedAt";

/// The checks of a carrier service input (`creating`: the fields it must have), with `code`.
fn check(
    state: &ShopState,
    input: &Map<String, Value>,
    id: Option<&str>,
    creating: bool,
    code: &str,
) -> Vec<UserError> {
    let mut errors = Vec::new();
    let name = text(input, "name").map(str::trim);
    if (creating || input.contains_key("name")) && name.is_none_or(str::is_empty) {
        errors.push(UserError::new(&["input", "name"], "Name can't be blank").code(code));
    } else if let Some(name) = name
        && state.all("DeliveryCarrierService").any(|service| {
            service.get("id").and_then(Value::as_str) != id
                && text(service, "name").is_some_and(|known| known.eq_ignore_ascii_case(name))
        })
    {
        errors.push(UserError::new(&["input", "name"], "Name has already been taken").code(code));
    }
    if let Some(url) = input.get("callbackUrl")
        && url.as_str().is_none_or(|url| !is_http_url(url))
    {
        errors
            .push(UserError::new(&["input", "callbackUrl"], "Callback url is invalid").code(code));
    }
    errors
}

fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let errors = check(
        mutation.state(),
        &input,
        None,
        true,
        "CARRIER_SERVICE_CREATE_FAILED",
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"carrierService": null}), &errors));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let id = state.new_gid("DeliveryCarrierService");
    let name = text(&input, "name").unwrap_or_default().trim().to_string();
    state.insert(value::object(json!({
        "__typename": "DeliveryCarrierService",
        "id": id,
        "name": name,
        "formattedName": name,
        "callbackUrl": input.get("callbackUrl"),
        "active": input.get("active").and_then(Value::as_bool).unwrap_or(true),
        "supportsServiceDiscovery": input.get("supportsServiceDiscovery").and_then(Value::as_bool).unwrap_or(false),
        "createdAt": now,
        UPDATED_AT: now,
    })));
    state.emit("carrier_services/create", &id);
    Ok(payload(json!({"carrierService": id}), &[]))
}

/// The carrier service of an id.
fn service(state: &ShopState, id: &str) -> Option<String> {
    (state.type_of(id) == Some("DeliveryCarrierService")).then(|| id.to_string())
}

fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let code = "CARRIER_SERVICE_UPDATE_FAILED";
    let Some(id) = service(mutation.state(), text(&input, "id").unwrap_or_default()) else {
        // lsf's wording.
        return Ok(payload(
            json!({"carrierService": null}),
            &[UserError::new(&["input", "id"], "Carrier service not found").code(code)],
        ));
    };
    let errors = check(mutation.state(), &input, Some(&id), false, code);
    if !errors.is_empty() {
        return Ok(payload(json!({"carrierService": null}), &errors));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    if let Some(record) = state.get_mut(&id) {
        if let Some(name) = text(&input, "name") {
            let name = name.trim().to_string();
            record.insert("name".into(), json!(name));
            record.insert("formattedName".into(), json!(name));
        }
        for field in ["callbackUrl", "active", "supportsServiceDiscovery"] {
            if let Some(given) = input.get(field).filter(|given| !given.is_null()) {
                record.insert(field.into(), given.clone());
            }
        }
        record.insert(UPDATED_AT.into(), json!(now));
    }
    state.emit("carrier_services/update", &id);
    Ok(payload(json!({"carrierService": id}), &[]))
}

/// `carrierServiceDelete`: the rates of delivery profiles that it provided go with it.
fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let Some(id) = service(mutation.state(), ctx.arg_str("id").unwrap_or_default()) else {
        return Ok(payload(
            json!({"deletedId": null}),
            &[UserError::new(&["id"], "Carrier service not found")
                .code("CARRIER_SERVICE_DELETE_FAILED")],
        ));
    };
    let state = mutation.state_mut();
    profiles::drop_carrier_service(state, &id);
    state.delete(&id, "carrier_services/delete");
    Ok(payload(json!({"deletedId": id}), &[]))
}

/// `availableCarrierServices`: each active carrier service with the locations that ship (active,
/// and not a fulfillment service's).
fn available(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let locations: Vec<Value> = state
        .keys("Location")
        .filter(|location| {
            state.get(location).is_some_and(|record| {
                record.get("isActive").and_then(Value::as_bool) == Some(true)
                    && record.get("isFulfillmentService").and_then(Value::as_bool) != Some(true)
            })
        })
        .map(|location| json!(location))
        .collect();
    let services: Vec<Value> = state
        .all("DeliveryCarrierService")
        .filter(|service| service.get("active").and_then(Value::as_bool) == Some(true))
        .map(|service| json!({"carrierService": service.get("id"), "locations": locations}))
        .collect();
    r.convert(ctx, &Value::Array(services), true)
}
