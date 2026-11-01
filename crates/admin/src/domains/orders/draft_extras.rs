//! The rest of draft orders: tags and deletions in bulk (`draftOrderBulkAddTags`,
//! `draftOrderBulkRemoveTags`, `draftOrderBulkDelete`), a draft made from an order
//! (`draftOrderCreateFromOrder`), and the preview of a draft's invoice
//! (`draftOrderInvoicePreview`).
//!
//! As shopify.dev's reference of 2026-10 describes them, with its examples:
//!
//! - The bulk mutations take the drafts of `ids`, or those `search` matches (the filters of
//!   `draftOrders`); one of the two (lsf has no saved searches). They answer a `Job` already
//!   `done`, as lsf's other bulk mutations do, the work done. Events: `draft_orders/update` for
//!   each draft whose tags changed, `draft_orders/delete` for each one deleted.
//! - `draftOrderCreateFromOrder` makes an open draft of an order's line items as they are now
//!   (their current quantities: variants at their price now, custom items at theirs), its
//!   customer (or company location), contact, addresses, shipping line, note, tags and custom
//!   attributes: `draft_orders/create`.
//! - `draftOrderAvailableDeliveryOptions` and `draftOrderCalculate`'s `availableShippingRates` answer the
//!   checkout's delivery rates of the store data (`checkout.shipping_rates`, every address
//!   charged the same) and the locations that offer local pickup.
//! - `draftOrderInvoicePreview` answers the invoice email as `draftOrderInvoiceSend` would send
//!   it, in the HTML of the reference's examples ("Invoice #D1 from <shop>", the custom message
//!   with its line breaks), the subject given or "Draft Order #D1"; a completed draft answers the
//!   example's "Draft order Invoice can't be sent. This draft order is already paid.". Nothing is
//!   sent or changed.
//!
//! Messages: those of the examples; the others are lsf's wording.

use serde_json::{Map, Value, json};

use super::drafts::{create_from, find_draft, invoice_text, remove_draft};
use super::model::{self, int, text};
use crate::domains::catalog::{gids, tags_of};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::selection::{Selection, Wording, selection};
use crate::resolve::{Mutation, Registry, UserError, done_job, payload};
use crate::state::{ShopState, gid_number, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("draftOrderBulkAddTags", |m, ctx| tags(m, ctx, true))
        .mutation("draftOrderBulkDelete", bulk_delete)
        .mutation("draftOrderBulkRemoveTags", |m, ctx| tags(m, ctx, false))
        .mutation("draftOrderCreateFromOrder", create_from_order)
        .mutation("draftOrderInvoicePreview", invoice_preview)
        .query("draftOrderAvailableDeliveryOptions", delivery_options);
}

/// The checkout's delivery rates of a kind (`SHIPPING`, `LOCAL`, `PICK_UP`), as
/// `DraftOrderShippingRate`s (and `ShippingRate`s, which read the same fields).
pub(super) fn shipping_rates(state: &ShopState, kind: &str) -> Vec<Value> {
    let currency = state.currency();
    state
        .setting(super::DELIVERY_RATES_SETTING)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|rate| rate.get("kind").and_then(Value::as_str) == Some(kind))
        .map(|rate| {
            json!({
                "handle": rate.get("handle"),
                "code": rate.get("handle"),
                "title": rate.get("title"),
                "source": "shopify",
                "price": value::money_v2(rate.get("price").and_then(Value::as_i64).unwrap_or(0), &currency),
            })
        })
        .collect()
}

/// `draftOrderAvailableDeliveryOptions`: the checkout's shipping and local delivery rates (lsf
/// charges every address the same), and the locations that offer pickup, `localPickupCount` of
/// them from `localPickupFrom`, those whose name has the `search` term.
fn delivery_options(
    r: &crate::resolve::ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &crate::resolve::Obj,
) -> Result<crate::graphql::Output<crate::resolve::Obj>, FieldError> {
    let state = r.state();
    let search = ctx.arg_str("search").map(str::to_ascii_lowercase);
    let pickup: Vec<Value> = state
        .keys("Location")
        .filter_map(|key| {
            let location = state.get(key)?;
            let settings = location.get("localPickupSettingsV2").filter(|settings| !settings.is_null())?;
            if location.get("isActive").and_then(Value::as_bool) == Some(false) {
                return None;
            }
            let name = text(location, "name").unwrap_or_default();
            if search.as_ref().is_some_and(|search| !name.to_ascii_lowercase().contains(search)) {
                return None;
            }
            let handle = format!("pickup-{}", gid_number(key));
            Some(json!({
                "code": handle,
                "handle": handle,
                "title": name,
                "source": "shopify",
                "locationId": key,
                "instructions": settings.get("instructions").and_then(Value::as_str).unwrap_or_default(),
                "distanceFromBuyer": null,
            }))
        })
        .collect();
    let from = usize::try_from(ctx.arg_i64("localPickupFrom").unwrap_or(0).max(0)).unwrap_or(0);
    let count = ctx
        .arg_i64("localPickupCount")
        .map_or(pickup.len(), |count| {
            usize::try_from(count.max(0)).unwrap_or(0)
        });
    let page: Vec<Value> = pickup.iter().skip(from).take(count).cloned().collect();
    let has_next = from + page.len() < pickup.len();
    let answer = value::object(json!({
        "availableShippingRates": shipping_rates(state, "SHIPPING"),
        "availableLocalDeliveryRates": shipping_rates(state, "LOCAL"),
        "availableLocalPickupOptions": page,
        "pageInfo": {
            "hasNextPage": has_next,
            "hasPreviousPage": from > 0,
            "startCursor": null,
            "endCursor": null,
        },
    }));
    Ok(crate::graphql::Output::Object(crate::resolve::Obj::built(
        "DraftOrderAvailableDeliveryOptions",
        answer,
    )))
}

/// The drafts a bulk mutation names: `ids`, or those `search` (or the saved search of draft
/// orders `savedSearchId` names) matches. The payload's error type has no `code`.
fn targets(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Vec<String>, UserError> {
    let state = mutation.state();
    let search = match selection(state, ctx, "DRAFT_ORDER") {
        Err(error) => return Err(error.user_error(Wording::Arguments, false)),
        Ok(Selection::Ids(ids)) => {
            let mut drafts = Vec::new();
            for (index, id) in ids.iter().enumerate() {
                let id = id.as_str().unwrap_or_default();
                if state.type_of(id) != Some("DraftOrder") {
                    let index = index.to_string();
                    return Err(UserError::new(&["ids", &index], "Draft order not found"));
                }
                drafts.push(id.to_string());
            }
            return Ok(drafts);
        }
        Ok(Selection::Search(search)) => search,
    };
    let all: Vec<Value> = state.keys("DraftOrder").map(|key| json!(key)).collect();
    let found =
        mutation
            .resolver()
            .search_query("DraftOrder", ("QueryRoot", "draftOrders"), &search, all);
    Ok(found
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect())
}

/// `draftOrderBulkAddTags` and `draftOrderBulkRemoveTags`.
fn tags(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    add: bool,
) -> Result<Value, FieldError> {
    let failed = |error: UserError| Ok(payload(json!({"job": null}), &[error]));
    let given = tags_of(ctx.arg("tags"));
    if given.is_empty() {
        return failed(UserError::new(&["tags"], "Tags can't be blank"));
    }
    let drafts = match targets(mutation, ctx) {
        Ok(drafts) => drafts,
        Err(error) => return failed(error),
    };
    let state = mutation.state_mut();
    for draft in &drafts {
        let Some(record) = state.get(draft) else {
            continue;
        };
        let mut tags = tags_of(record.get("tags"));
        let before = tags.clone();
        if add {
            for tag in &given {
                if !tags.iter().any(|known| known.eq_ignore_ascii_case(tag)) {
                    tags.push(tag.clone());
                }
            }
        } else {
            tags.retain(|known| !given.iter().any(|tag| tag.eq_ignore_ascii_case(known)));
        }
        if tags == before {
            continue;
        }
        if let Some(record) = state.get_mut(draft) {
            record.insert("tags".into(), json!(tags));
        }
        state.touch(draft);
        state.emit("draft_orders/update", draft);
    }
    let job = done_job(state, ctx.field_name());
    Ok(payload(json!({"job": job}), &[]))
}

/// `draftOrderBulkDelete`.
fn bulk_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let drafts = match targets(mutation, ctx) {
        Ok(drafts) => drafts,
        Err(error) => return Ok(payload(json!({"job": null}), &[error])),
    };
    let state = mutation.state_mut();
    for draft in &drafts {
        if let Some(record) = state.get(draft).cloned() {
            remove_draft(state, draft, &record);
        }
    }
    let job = done_job(state, ctx.field_name());
    Ok(payload(json!({"job": job}), &[]))
}

/// The `DraftOrderInput` of an order: its lines as they are now, its buyer, its addresses and
/// shipping line, its note, tags and custom attributes.
fn input_of_order(state: &ShopState, order: &Map<String, Value>) -> Map<String, Value> {
    let lines: Vec<Value> = gids(order.get("lineItems"))
        .iter()
        .filter_map(|line| state.get(line))
        .filter(|line| int(line, "currentQuantity") > 0)
        .map(|line| {
            let quantity = int(line, "currentQuantity");
            let attributes = line.get("customAttributes").cloned().unwrap_or(json!([]));
            match text(line, "variant").filter(|variant| state.contains(variant)) {
                Some(variant) => json!({"variantId": variant, "quantity": quantity, "customAttributes": attributes}),
                None => json!({
                    "title": line.get("title"),
                    "quantity": quantity,
                    "originalUnitPrice": value::decimal(model::field_cents(line, "originalUnitPriceSet")),
                    "sku": line.get("sku"),
                    "requiresShipping": line.get("requiresShipping"),
                    "taxable": line.get("taxable"),
                    "customAttributes": attributes,
                }),
            }
        })
        .collect();
    let address = |field: &str| {
        order
            .get(field)
            .filter(|address| !address.is_null())
            .map(|address| {
                json!({
                    "firstName": address.get("firstName"),
                    "lastName": address.get("lastName"),
                    "company": address.get("company"),
                    "address1": address.get("address1"),
                    "address2": address.get("address2"),
                    "city": address.get("city"),
                    "province": address.get("province"),
                    "provinceCode": address.get("provinceCode"),
                    "countryCode": address.get("countryCodeV2"),
                    "zip": address.get("zip"),
                    "phone": address.get("phone"),
                })
            })
    };
    let mut input = value::object(json!({
        "lineItems": lines,
        "email": order.get("email"),
        "phone": order.get("phone"),
        "note": order.get("note"),
        "tags": order.get("tags").cloned().unwrap_or(json!([])),
        "customAttributes": order.get("customAttributes").cloned().unwrap_or(json!([])),
        "taxExempt": order.get("taxExempt"),
    }));
    for field in ["shippingAddress", "billingAddress"] {
        if let Some(address) = address(field) {
            input.insert(field.into(), address);
        }
    }
    match order.get("purchasingEntity") {
        Some(Value::Object(company)) => {
            input.insert(
                "purchasingEntity".into(),
                json!({"purchasingCompany": {
                    "companyId": company.get("company"),
                    "companyLocationId": company.get("location"),
                    "companyContactId": company.get("contact"),
                }}),
            );
        }
        _ => {
            if let Some(customer) = text(order, "customer") {
                input.insert("purchasingEntity".into(), json!({"customerId": customer}));
            }
        }
    }
    if let Some(line) = model::array(order, "shippingLines")
        .into_iter()
        .find(|line| line.get("isRemoved").and_then(Value::as_bool) != Some(true))
    {
        input.insert(
            "shippingLine".into(),
            json!({"title": line.get("title"), "price": value::decimal(model::cents(line.get("originalPriceSet")))}),
        );
    }
    input.retain(|_, value| !value.is_null());
    input
}

/// `draftOrderCreateFromOrder`.
fn create_from_order(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |message: &str| {
        Ok(payload(
            json!({"draftOrder": null}),
            &[UserError::new(&["orderId"], message)],
        ))
    };
    let id = ctx.arg_str("orderId").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(order) = state.get_of(&id, "Order").cloned() else {
        return failed("Order does not exist");
    };
    let input = input_of_order(state, &order);
    if model::array(&input, "lineItems").is_empty() {
        return failed("The order has no line items left to put in a draft order");
    }
    Ok(match create_from(mutation, ctx.field_name(), &input) {
        Ok(draft) => payload(json!({"draftOrder": draft}), &[]),
        Err(errors) => payload(json!({"draftOrder": null}), &errors),
    })
}

/// HTML of a text: escaped, its line breaks `<br>`.
fn html(text: &str) -> String {
    crate::api::escape_html(text)
        .replace("\r\n", "<br>")
        .replace('\n', "<br>")
}

/// `draftOrderInvoicePreview`.
fn invoice_preview(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"previewHtml": null, "previewSubject": null});
    let state = mutation.state();
    let record = match find_draft(state, ctx.arg_str("id").unwrap_or_default()) {
        Ok(record) => record,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    if text(&record, "status") == Some("COMPLETED") {
        return Ok(payload(
            empty,
            &[UserError::new(
                &[],
                "Draft order Invoice can't be sent. This draft order is already paid.",
            )],
        ));
    }
    let email = model::input_of(ctx.arg("email"));
    let (subject, _) = invoice_text(state, &record, &email);
    let name = text(&record, "name").unwrap_or_default();
    let shop = state
        .shop()
        .and_then(|shop| text(shop, "name"))
        .unwrap_or_default();
    let message = email
        .get("customMessage")
        .and_then(Value::as_str)
        .map(html)
        .unwrap_or_default();
    let preview = format!(
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.0 Transitional//EN\" \"http://www.w3.org/TR/REC-html40/loose.dtd\">\n<html><body>\n<p>Invoice {} from {}</p>\n<p>{message}</p>\n</body></html>\n",
        html(name),
        html(shop)
    );
    Ok(payload(
        json!({"previewHtml": preview, "previewSubject": subject}),
        &[],
    ))
}
