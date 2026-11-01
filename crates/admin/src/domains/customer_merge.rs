//! Merging customers: `customerMerge`, `customerMergePreview`, `customerMergeJobStatus`,
//! `Customer.mergeable`; and `customers/purchasing_summary`, which follows a customer's sales.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - **Which customer is kept**, in order: the one of `overrideFields.customerIdOfEmailToKeep`
//!   when it is one of the two and has an email; the only one with an email; when both have one,
//!   an `ENABLED` account, else an `INVITED` one unless the other's consent is `SUBSCRIBED` or
//!   `PENDING`, else the one whose consent is; otherwise `customerTwoId`.
//! - **What blocks a merge** (`CustomerMergeErrorFieldType`, each with the customer that has it):
//!   a company contact, gift cards, a subscription contract, store credit, payment methods, a
//!   multipass identifier, a pending data erasure (`PENDING_DATA_REQUEST`), merged tags over 250
//!   or a merged note over 5000 characters (the preview's `blockingFields`). The two ids must be
//!   two customers of the shop (`INVALID_CUSTOMER_ID`), an override must name one of them and a
//!   field it has (`OVERRIDE_ATTRIBUTE_INVALID`, `MISSING_OVERRIDE_ATTRIBUTE`). The messages are
//!   lsf's, in the form of the `customers/merge` example ("John Doe is currently being merged.").
//! - **What the merge does** (the Help Center's "Merging customer profiles"): the kept customer
//!   gets the other's orders, draft orders, addresses and metafields it lacks, the tags of both
//!   (or `overrideFields.tags`), both notes (or `overrideFields.note`), and the name, email,
//!   phone and default address of the customer each override names; the other customer is
//!   deleted. Shopify runs it as a job: lsf merges at once, and the `Job` is `done`;
//!   `customerMergeJobStatus` answers `COMPLETED`.
//! - **Webhooks**: `customers/update` of the kept customer, `customers/delete` of the other,
//!   then `customers/merge` (`admin_graphql_api_customer_kept_id`, `…_deleted_id`,
//!   `…_job_id`, `status: completed`, `errors: []`).
//! - **`customers/purchasing_summary`** ("Occurs when a customer sales history change"): after a
//!   change of a customer's orders (`orders/*`, `refunds/create`), when their number of orders,
//!   amount spent or last order is not what it was, its payload `{customerId, numberOfOrders,
//!   amountSpent, lastOrderId, occurredAt}`.

use serde_json::{Map, Value, json};

use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::value::non_empty_str_field as text;
use crate::state::{Record, ShopState};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("customerMerge", customer_merge)
        .query("customerMergePreview", customer_merge_preview)
        .query("customerMergeJobStatus", customer_merge_job_status)
        .field("Customer", "mergeable", |r, ctx, object| {
            match object.key().filter(|key| r.state().contains(key)) {
                Some(customer) => r.convert(ctx, &mergeable(r.state(), customer), false),
                None => r.generic(ctx, object),
            }
        });
}

/// What blocks a customer from being merged: the fields, and why.
fn blockers(state: &ShopState, customer: &str) -> Vec<(&'static str, String)> {
    let Some(record) = state.get(customer) else {
        return vec![("DELETED_AT", "The customer does not exist.".into())];
    };
    let name = record
        .get("displayName")
        .and_then(Value::as_str)
        .unwrap_or("The customer")
        .to_string();
    let owns = |type_name: &str, field: &str| {
        state
            .all(type_name)
            .any(|record| record.get(field).and_then(Value::as_str) == Some(customer))
    };
    let mut found = Vec::new();
    let contact = record
        .get("companyContactProfiles")
        .and_then(Value::as_array)
        .is_some_and(|profiles| !profiles.is_empty());
    if contact {
        found.push(("COMPANY_CONTACT", format!("{name} is a company contact.")));
    }
    if owns("GiftCard", "customer") {
        found.push(("GIFT_CARDS", format!("{name} has gift cards.")));
    }
    if owns("SubscriptionContract", "customer") {
        found.push((
            "SUBSCRIPTIONS",
            format!("{name} has a subscription history."),
        ));
    }
    if owns("StoreCreditAccount", "owner") {
        found.push(("STORE_CREDIT", format!("{name} has store credit.")));
    }
    if owns("CustomerPaymentMethod", "customer") {
        found.push((
            "CUSTOMER_PAYMENT_METHODS",
            format!("{name} has payment methods."),
        ));
    }
    if record
        .get("multipassIdentifier")
        .is_some_and(|id| !id.is_null())
    {
        found.push((
            "MULTIPASS_IDENTIFIER",
            format!("{name} has a multipass identifier."),
        ));
    }
    if crate::domains::privacy::erasure_pending(state, customer) {
        found.push((
            "PENDING_DATA_REQUEST",
            format!("{name} has a pending data request."),
        ));
    }
    found
}

/// `Customer.mergeable` (and `CustomerSegmentMember.mergeable`).
pub fn mergeable(state: &ShopState, customer: &str) -> Value {
    let blocked = blockers(state, customer);
    json!({
        "isMergeable": blocked.is_empty(),
        "errorFields": blocked.iter().map(|(field, _)| *field).collect::<Vec<_>>(),
        "reason": blocked.first().map(|(_, reason)| reason.clone()),
        "mergeInProgress": null,
    })
}

fn consent_rank(record: &Record) -> u8 {
    match record
        .get("emailMarketingConsent")
        .and_then(|consent| consent.get("marketingState"))
        .and_then(Value::as_str)
    {
        Some("SUBSCRIBED") => 2,
        Some("PENDING") => 1,
        _ => 0,
    }
}

/// The customer a merge keeps, by shopify.dev's rules.
fn kept<'a>(
    state: &ShopState,
    one: &'a str,
    two: &'a str,
    overrides: &Map<String, Value>,
) -> &'a str {
    if let Some(chosen) = overrides
        .get("customerIdOfEmailToKeep")
        .and_then(Value::as_str)
        && (chosen == one || chosen == two)
        && state
            .get(chosen)
            .is_some_and(|record| text(record, "email").is_some())
    {
        return if chosen == one { one } else { two };
    }
    let (Some(a), Some(b)) = (state.get(one), state.get(two)) else {
        return two;
    };
    match (text(a, "email").is_some(), text(b, "email").is_some()) {
        (true, false) => one,
        (false, true) | (false, false) => two,
        (true, true) => {
            let account = |record: &Record| text(record, "state").unwrap_or("DISABLED").to_string();
            let (sa, sb) = (account(a), account(b));
            if sa == "ENABLED" && sb != "ENABLED" {
                return one;
            }
            if sb == "ENABLED" && sa != "ENABLED" {
                return two;
            }
            let (ca, cb) = (consent_rank(a), consent_rank(b));
            if sa == "INVITED" && sb != "INVITED" && cb == 0 {
                return one;
            }
            if sb == "INVITED" && sa != "INVITED" && ca == 0 {
                return two;
            }
            if ca > cb { one } else { two }
        }
    }
}

/// The tags and the note the merged customer would have.
/// The default address and the addresses of the merged customer: the default one is the other
/// customer's when `from_other`, else the kept one's, else the other's; the addresses are the kept
/// customer's, then those of the other it lacks, the default first.
fn merged_addresses(a: &Record, b: &Record, from_other: bool) -> (Value, Vec<Value>) {
    let default_of = |record: &Record| record.get("defaultAddress").cloned().unwrap_or(Value::Null);
    let default_address = if from_other {
        default_of(b)
    } else {
        Some(default_of(a))
            .filter(|address| !address.is_null())
            .unwrap_or_else(|| default_of(b))
    };
    let mut addresses: Vec<Value> = Vec::new();
    for address in [a, b]
        .into_iter()
        .filter_map(|record| record.get("addressesV2").and_then(Value::as_array))
        .flatten()
    {
        if !addresses.contains(address) {
            addresses.push(address.clone());
        }
    }
    if !default_address.is_null() {
        addresses.retain(|address| *address != default_address);
        addresses.insert(0, default_address.clone());
    }
    (default_address, addresses)
}

fn merged_tags_and_note(
    a: &Record,
    b: &Record,
    overrides: &Map<String, Value>,
) -> (Vec<String>, Option<String>) {
    let tags = match overrides.get("tags").and_then(Value::as_array) {
        Some(tags) => tags
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        None => {
            let mut tags = crate::domains::catalog::tags_of(a.get("tags"));
            for tag in crate::domains::catalog::tags_of(b.get("tags")) {
                if !tags.iter().any(|known| known.eq_ignore_ascii_case(&tag)) {
                    tags.push(tag);
                }
            }
            tags
        }
    };
    let note = match overrides.get("note") {
        Some(Value::String(note)) => Some(note.clone()),
        Some(_) => None,
        None => {
            let notes: Vec<&str> = [text(a, "note"), text(b, "note")]
                .into_iter()
                .flatten()
                .collect();
            (!notes.is_empty()).then(|| notes.join("\n"))
        }
    };
    (tags, note)
}

/// What is wrong with a merge of `one` and `two`, as user errors.
fn merge_errors(
    state: &ShopState,
    one: &str,
    two: &str,
    overrides: &Map<String, Value>,
) -> Vec<UserError> {
    let mut errors = Vec::new();
    for (field, id) in [("customerOneId", one), ("customerTwoId", two)] {
        if state.type_of(id) != Some("Customer") {
            errors.push(
                UserError::new(&[field], "The customer ID is invalid.").code("INVALID_CUSTOMER_ID"),
            );
        }
    }
    if !errors.is_empty() {
        return errors;
    }
    if one == two {
        errors.push(
            UserError::new(
                &["customerTwoId"],
                "A customer cannot be merged with itself.",
            )
            .code("INVALID_CUSTOMER"),
        );
        return errors;
    }
    for (field, id) in [("customerOneId", one), ("customerTwoId", two)] {
        for (kind, reason) in blockers(state, id) {
            let code = if kind == "GIFT_CARDS" {
                "CUSTOMER_HAS_GIFT_CARDS"
            } else {
                "INVALID_CUSTOMER"
            };
            errors.push(UserError::new(&[field], reason).code(code));
        }
    }
    for (field, attribute) in [
        ("customerIdOfEmailToKeep", "email"),
        ("customerIdOfPhoneNumberToKeep", "phone"),
        ("customerIdOfFirstNameToKeep", "firstName"),
        ("customerIdOfLastNameToKeep", "lastName"),
        ("customerIdOfDefaultAddressToKeep", "defaultAddress"),
    ] {
        let Some(chosen) = overrides.get(field).and_then(Value::as_str) else {
            continue;
        };
        let path = ["overrideFields", field];
        if chosen != one && chosen != two {
            errors.push(
                UserError::new(&path, "The override attribute is invalid.")
                    .code("OVERRIDE_ATTRIBUTE_INVALID"),
            );
        } else if state
            .get(chosen)
            .and_then(|record| record.get(attribute))
            .is_none_or(|value| value.is_null() || value.as_str() == Some(""))
        {
            errors.push(
                UserError::new(
                    &path,
                    "The customer is missing the attribute requested for override.",
                )
                .code("MISSING_OVERRIDE_ATTRIBUTE"),
            );
        }
    }
    if let (Some(a), Some(b)) = (state.get(one), state.get(two)) {
        let (tags, note) = merged_tags_and_note(a, b, overrides);
        if tags.len() > 250 {
            errors.push(
                UserError::new(
                    &["overrideFields", "tags"],
                    "The merged tags are over the 250 limit.",
                )
                .code("INVALID_CUSTOMER"),
            );
        }
        if note.is_some_and(|note| note.chars().count() > 5000) {
            errors.push(
                UserError::new(
                    &["overrideFields", "note"],
                    "The merged note is over the 5000 character limit.",
                )
                .code("INVALID_CUSTOMER"),
            );
        }
    }
    errors
}

fn overrides_of(ctx: &FieldContext<'_>) -> Map<String, Value> {
    ctx.arg_object_or_default("overrideFields")
}

fn customer_merge(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let one = ctx.arg_str("customerOneId").unwrap_or_default().to_string();
    let two = ctx.arg_str("customerTwoId").unwrap_or_default().to_string();
    let overrides = overrides_of(ctx);
    let errors = merge_errors(m.state(), &one, &two, &overrides);
    if !errors.is_empty() {
        return Ok(payload(
            json!({"job": null, "resultingCustomerId": null}),
            &errors,
        ));
    }
    let keep = kept(m.state(), &one, &two, &overrides).to_string();
    let gone = if keep == one {
        two.clone()
    } else {
        one.clone()
    };
    let state = m.state_mut();
    merge_into(state, &keep, &gone, &overrides);
    let job = crate::resolve::done_job(state, "customer_merge");
    state.set_memo(
        format!("customer-merge/{job}"),
        json!({"resultingCustomerId": keep, "deletedCustomerId": gone}),
    );
    let snapshot = crate::state::value::object(json!({
        "__typename": "CustomerMergeRequest",
        "customerKept": keep,
        "customerDeleted": gone,
        "jobId": job,
        "status": "COMPLETED",
        "customerMergeErrors": [],
    }));
    state.emit_with(
        "customers/merge",
        &format!("customer-merge/{job}"),
        snapshot,
    );
    Ok(payload(
        json!({"job": job, "resultingCustomerId": keep}),
        &[],
    ))
}

/// Moves what `gone` has to `keep`, then deletes `gone`.
fn merge_into(state: &mut ShopState, keep: &str, gone: &str, overrides: &Map<String, Value>) {
    let (Some(a), Some(b)) = (state.get(keep).cloned(), state.get(gone).cloned()) else {
        return;
    };
    let (tags, note) = merged_tags_and_note(&a, &b, overrides);
    for type_name in ["Order", "DraftOrder"] {
        let owned: Vec<String> = state
            .all(type_name)
            .filter(|record| record.get("customer").and_then(Value::as_str) == Some(gone))
            .filter_map(|record| record.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        for key in owned {
            if let Some(record) = state.get_mut(&key) {
                record.insert("customer".into(), json!(keep));
                if record.get("purchasingEntity").and_then(Value::as_str) == Some(gone) {
                    record.insert("purchasingEntity".into(), json!(keep));
                }
            }
        }
    }
    let chosen = |field: &str| overrides.get(field).and_then(Value::as_str) == Some(gone);
    let from_gone = |field: &str| b.get(field).cloned().unwrap_or(Value::Null);
    let (default_address, addresses) =
        merged_addresses(&a, &b, chosen("customerIdOfDefaultAddressToKeep"));
    let mut metafields: Vec<Value> = a
        .get("metafields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has = |metafields: &[Value], state: &ShopState, candidate: &Value| {
        let key_of = |gid: &Value| {
            gid.as_str()
                .and_then(|gid| state.get(gid))
                .map(|metafield| {
                    (
                        metafield.get("namespace").cloned(),
                        metafield.get("key").cloned(),
                    )
                })
        };
        let wanted = key_of(candidate);
        metafields.iter().any(|known| key_of(known) == wanted)
    };
    let moved: Vec<Value> = b
        .get("metafields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|metafield| !has(&metafields, state, metafield))
        .cloned()
        .collect();
    for metafield in &moved {
        if let Some(gid) = metafield.as_str()
            && let Some(record) = state.get_mut(gid)
        {
            record.insert("owner".into(), json!(keep));
        }
        metafields.push(metafield.clone());
    }
    if let Some(record) = state.get_mut(keep) {
        for (field, attributes) in [
            ("customerIdOfFirstNameToKeep", &["firstName"][..]),
            ("customerIdOfLastNameToKeep", &["lastName"][..]),
            (
                "customerIdOfEmailToKeep",
                &["email", "emailMarketingConsent"][..],
            ),
            (
                "customerIdOfPhoneNumberToKeep",
                &["phone", "smsMarketingConsent"][..],
            ),
        ] {
            for attribute in attributes {
                let missing = record
                    .get(*attribute)
                    .is_none_or(|value| value.is_null() || value.as_str() == Some(""));
                if chosen(field) || (missing && !from_gone(attribute).is_null()) {
                    record.insert((*attribute).into(), from_gone(attribute));
                }
            }
        }
        record.insert("tags".into(), json!(tags));
        record.insert("note".into(), json!(note));
        record.insert("addresses".into(), json!(addresses));
        record.insert("addressesV2".into(), json!(addresses));
        record.insert(
            "defaultAddress".into(),
            if default_address.is_null() {
                Value::Null
            } else {
                default_address
            },
        );
        record.insert("metafields".into(), json!(metafields));
        crate::domains::customers::contact_fields(record);
    }
    // The other customer's metafields that the kept one has already go with it.
    let leftover: Vec<String> = b
        .get("metafields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|metafield| !moved.contains(metafield))
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    for metafield in leftover {
        state.remove(&metafield);
    }
    crate::domains::customers::refresh_customer(state, keep);
    state.touch(keep);
    state.emit("customers/update", keep);
    state.delete(gone, "customers/delete");
}

fn customer_merge_preview(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let one = ctx.arg_str("customerOneId").unwrap_or_default();
    let two = ctx.arg_str("customerTwoId").unwrap_or_default();
    let overrides = overrides_of(ctx);
    let errors = merge_errors(state, one, two, &overrides);
    let mut value = Record::new();
    if !errors.is_empty() {
        let merge_errors: Vec<Value> = errors
            .iter()
            .map(|error| {
                let field = match error.code.as_deref() {
                    Some("CUSTOMER_HAS_GIFT_CARDS") => "GIFT_CARDS",
                    Some("OVERRIDE_ATTRIBUTE_INVALID" | "MISSING_OVERRIDE_ATTRIBUTE") => {
                        "OVERRIDE_FIELDS"
                    }
                    Some("INVALID_CUSTOMER_ID") => "DELETED_AT",
                    _ => blocker_field(state, one, two, &error.message),
                };
                json!({"errorFields": [field], "message": error.message})
            })
            .collect();
        value.insert("customerMergeErrors".into(), json!(merge_errors));
        return Ok(Output::Object(Obj::built("CustomerMergePreview", value)));
    }
    let keep = kept(state, one, two, &overrides);
    let other = if keep == one { two } else { one };
    let (Some(a), Some(b)) = (state.get(keep), state.get(other)) else {
        return Ok(Output::Object(Obj::built("CustomerMergePreview", value)));
    };
    let (tags, note) = merged_tags_and_note(a, b, &overrides);
    let pick = |field: &str, attribute: &str| -> Value {
        let source = match overrides.get(field).and_then(Value::as_str) {
            Some(chosen) if chosen == other => b,
            _ => a,
        };
        source.get(attribute).cloned().unwrap_or(Value::Null)
    };
    let orders: Vec<Value> = [a, b]
        .iter()
        .flat_map(|record| {
            record
                .get("orders")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    let owned = |type_name: &str, field: &str| -> Vec<Value> {
        state
            .all(type_name)
            .filter(|record| {
                let owner = record.get(field).and_then(Value::as_str);
                owner == Some(keep) || owner == Some(other)
            })
            .filter_map(|record| record.get("id").cloned())
            .collect()
    };
    let draft_orders = owned("DraftOrder", "customer");
    let gift_cards = owned("GiftCard", "customer");
    // What the merge makes of them: no address twice, the default one first.
    let (default_address, addresses) = merged_addresses(
        a,
        b,
        overrides
            .get("customerIdOfDefaultAddressToKeep")
            .and_then(Value::as_str)
            == Some(other),
    );
    let metafields = [a, b]
        .iter()
        .map(|record| {
            record
                .get("metafields")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        })
        .sum::<usize>();
    let name = |first: &Value, last: &Value| {
        let first = first.as_str().unwrap_or_default();
        let last = last.as_str().unwrap_or_default();
        format!("{first} {last}").trim().to_string()
    };
    let first = pick("customerIdOfFirstNameToKeep", "firstName");
    let last = pick("customerIdOfLastNameToKeep", "lastName");
    let display = match name(&first, &last) {
        name if !name.is_empty() => name,
        _ => a
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    };
    value.insert("resultingCustomerId".into(), json!(keep));
    value.insert("customerMergeErrors".into(), json!([]));
    value.insert(
        "defaultFields".into(),
        json!({
            "firstName": first,
            "lastName": last,
            "displayName": display,
            "email": pick("customerIdOfEmailToKeep", "defaultEmailAddress"),
            "phoneNumber": pick("customerIdOfPhoneNumberToKeep", "defaultPhoneNumber"),
            "defaultAddress": default_address,
            "addresses": addresses,
            "tags": tags,
            "note": note,
            "orders": orders,
            "orderCount": orders.len().to_string(),
            "draftOrders": draft_orders,
            "draftOrderCount": draft_orders.len().to_string(),
            "giftCards": gift_cards,
            "giftCardCount": gift_cards.len().to_string(),
            "discountNodes": [],
            "discountNodeCount": "0",
            "metafieldCount": metafields.to_string(),
        }),
    );
    value.insert(
        "alternateFields".into(),
        json!({
            "firstName": b.get("firstName"),
            "lastName": b.get("lastName"),
            "email": b.get("defaultEmailAddress"),
            "phoneNumber": b.get("defaultPhoneNumber"),
            "defaultAddress": b.get("defaultAddress"),
        }),
    );
    value.insert("blockingFields".into(), json!({"note": null, "tags": []}));
    Ok(Output::Object(Obj::built("CustomerMergePreview", value)))
}

/// The field type of a blocker's message.
fn blocker_field(state: &ShopState, one: &str, two: &str, message: &str) -> &'static str {
    [one, two]
        .iter()
        .flat_map(|customer| blockers(state, customer))
        .find(|(_, reason)| reason == message)
        .map_or("OVERRIDE_FIELDS", |(field, _)| field)
}

/// `customerMergeJobStatus(jobId:)`: the merge a job did.
fn customer_merge_job_status(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let job = ctx.arg_str("jobId").unwrap_or_default();
    let Some(merge) = r.state().memo(&format!("customer-merge/{job}")) else {
        return Ok(Output::Null);
    };
    let mut value = Record::new();
    value.insert("jobId".into(), json!(job));
    value.insert("status".into(), json!("COMPLETED"));
    value.insert(
        "resultingCustomerId".into(),
        merge
            .get("resultingCustomerId")
            .cloned()
            .unwrap_or(Value::Null),
    );
    value.insert("customerMergeErrors".into(), json!([]));
    Ok(Output::Object(Obj::built("CustomerMergeRequest", value)))
}

/// A customer's sales, as `customers/purchasing_summary` reports them.
fn summary(record: &Record) -> Value {
    json!({
        "numberOfOrders": record.get("numberOfOrders"),
        "amountSpent": record.get("amountSpent"),
        "lastOrder": record.get("lastOrder"),
    })
}

/// Emits `customers/purchasing_summary` for the customers whose sales the events since `start`
/// changed: an order of theirs changed, and their summary is not the one last reported (or, the
/// first time, the store data's). Without `report` (no subscription to the topic), the summary
/// is only remembered.
pub fn follow_purchases(state: &mut ShopState, start: usize, customers: &[String], report: bool) {
    let order_changed = state
        .events()
        .iter()
        .skip(start)
        .any(|event| event.topic.starts_with("orders/") || event.topic.starts_with("refunds/"));
    if !order_changed {
        return;
    }
    for customer in customers {
        let Some(now) = state.get(customer).map(summary) else {
            continue;
        };
        let memo = format!("purchasing-summary/{customer}");
        let before = match state.memo(&memo) {
            Some(before) => Some(before.clone()),
            None => state.original(customer).map(summary),
        };
        if before.as_ref() == Some(&now) {
            continue;
        }
        state.set_memo(memo, now.clone());
        if !report {
            continue;
        }
        let at = state.timestamp();
        let snapshot = crate::state::value::object(json!({
            "__typename": "CustomerPurchasingSummary",
            "customerId": customer,
            "numberOfOrders": now["numberOfOrders"],
            "amountSpent": now["amountSpent"],
            "lastOrderId": now["lastOrder"],
            "occurredAt": at,
        }));
        state.emit_with(
            "customers/purchasing_summary",
            &format!("purchasing-summary/{customer}"),
            snapshot,
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::domains::segments::tests::{customer, data, demo, run, topics};

    fn with_addresses(addresses: Value, default: Value) -> super::Record {
        json!({"addressesV2": addresses, "defaultAddress": default})
            .as_object()
            .cloned()
            .unwrap()
    }

    /// The preview listed both customers' addresses end to end, a shared one twice, while the
    /// merge keeps each once with the default first: both use `merged_addresses`.
    #[test]
    fn merged_addresses_are_listed_once_the_default_first() {
        let (home, work, shared) = (
            json!({"a": "home"}),
            json!({"a": "work"}),
            json!({"a": "both"}),
        );
        let kept = with_addresses(json!([home, shared]), home.clone());
        let other = with_addresses(json!([shared, work]), work.clone());
        assert_eq!(
            super::merged_addresses(&kept, &other, false),
            (
                home.clone(),
                vec![home.clone(), shared.clone(), work.clone()]
            )
        );
        assert_eq!(
            super::merged_addresses(&kept, &other, true),
            (
                work.clone(),
                vec![work.clone(), home.clone(), shared.clone()]
            )
        );
        // Without a default of its own, the kept customer takes the other's.
        let bare = with_addresses(json!([shared]), Value::Null);
        assert_eq!(
            super::merged_addresses(&bare, &other, false),
            (work.clone(), vec![work, shared])
        );
    }

    #[test]
    fn two_customers_merge_into_the_one_with_an_account_or_an_email() {
        let mut shop = demo();
        let camille = customer(&shop, "camille.martin@example.com");
        let new = customer(&shop, "new.customer@example.com");
        let preview = run(
            &mut shop,
            "query($one: ID!, $two: ID!) { customerMergePreview(customerOneId: $one, customerTwoId: $two) {
                resultingCustomerId customerMergeErrors { message }
                defaultFields { tags orderCount email { emailAddress } }
                alternateFields { firstName } blockingFields { tags } } }",
            json!({"one": new, "two": camille}),
        );
        let previewed = data(&preview, "customerMergePreview");
        assert_eq!(previewed["customerMergeErrors"], json!([]), "{preview}");
        // Both have an email, no account and the same consent: `customerTwoId` is kept.
        assert_eq!(previewed["resultingCustomerId"], json!(camille));
        assert_eq!(previewed["defaultFields"]["tags"], json!(["newsletter"]));
        assert_eq!(previewed["defaultFields"]["orderCount"], "1");
        assert_eq!(
            previewed["defaultFields"]["email"]["emailAddress"],
            "camille.martin@example.com"
        );

        let merged = run(
            &mut shop,
            "mutation($one: ID!, $two: ID!) { customerMerge(customerOneId: $one, customerTwoId: $two,
                overrideFields: {tags: [\"merged\"], note: \"Two profiles\"}) {
                resultingCustomerId job { id done } userErrors { field message code } } }",
            json!({"one": new, "two": camille}),
        );
        let payload = data(&merged, "customerMerge");
        assert_eq!(payload["userErrors"], json!([]), "{merged}");
        assert_eq!(payload["resultingCustomerId"], json!(camille));
        assert_eq!(payload["job"]["done"], true);
        let kept = shop.get(&camille).unwrap();
        assert_eq!(kept["tags"], json!(["merged"]));
        assert_eq!(kept["note"], "Two profiles");
        assert_eq!(kept["numberOfOrders"], "1");
        assert!(shop.get(&new).is_none());
        let fired = topics(&mut shop);
        assert_eq!(
            &fired[..3],
            ["customers/update", "customers/delete", "customers/merge"]
        );
        let status = run(
            &mut shop,
            "query($job: ID!) { customerMergeJobStatus(jobId: $job) { status resultingCustomerId customerMergeErrors { message } } }",
            json!({"job": payload["job"]["id"]}),
        );
        assert_eq!(
            data(&status, "customerMergeJobStatus"),
            &json!({"status": "COMPLETED", "resultingCustomerId": camille, "customerMergeErrors": []})
        );
    }

    #[test]
    fn a_merge_that_cannot_happen_changes_nothing() {
        let mut shop = demo();
        let jane = customer(&shop, "jane.doe@example.com");
        let camille = customer(&shop, "camille.martin@example.com");
        let before = shop.picture();
        let merge = "mutation($one: ID!, $two: ID!) { customerMerge(customerOneId: $one, customerTwoId: $two) {
            resultingCustomerId userErrors { field message code } } }";
        // Jane has gift cards.
        let blocked = run(&mut shop, merge, json!({"one": jane, "two": camille}));
        assert_eq!(
            data(&blocked, "customerMerge")["userErrors"],
            json!([{"field": ["customerOneId"], "message": "Jane Doe has gift cards.", "code": "CUSTOMER_HAS_GIFT_CARDS"}])
        );
        let missing = run(
            &mut shop,
            merge,
            json!({"one": camille, "two": "gid://shopify/Customer/1"}),
        );
        assert_eq!(
            data(&missing, "customerMerge")["userErrors"],
            json!([{"field": ["customerTwoId"], "message": "The customer ID is invalid.", "code": "INVALID_CUSTOMER_ID"}])
        );
        let itself = run(&mut shop, merge, json!({"one": camille, "two": camille}));
        assert_eq!(
            data(&itself, "customerMerge")["userErrors"][0]["code"],
            "INVALID_CUSTOMER"
        );
        assert_eq!(
            data(&itself, "customerMerge")["resultingCustomerId"],
            Value::Null
        );
        assert_eq!(shop.picture(), before);
        let mergeable = run(
            &mut shop,
            "query($jane: ID!, $camille: ID!) { jane: customer(id: $jane) { mergeable { isMergeable errorFields reason } }
                camille: customer(id: $camille) { mergeable { isMergeable errorFields reason } } }",
            json!({"jane": jane, "camille": camille}),
        );
        assert_eq!(
            mergeable["data"]["jane"]["mergeable"],
            json!({"isMergeable": false, "errorFields": ["GIFT_CARDS"], "reason": "Jane Doe has gift cards."})
        );
        assert_eq!(
            mergeable["data"]["camille"]["mergeable"],
            json!({"isMergeable": true, "errorFields": [], "reason": null})
        );
    }

    #[test]
    fn a_change_of_sales_is_a_purchasing_summary_when_asked_for() {
        let mut shop = demo();
        let jane = customer(&shop, "jane.doe@example.com");
        let order = shop
            .all("Order")
            .find(|order| order["customer"] == json!(jane))
            .and_then(|order| order["id"].as_str())
            .unwrap()
            .to_string();
        let start = shop.event_count();
        let snapshot = shop.get(&order).cloned().unwrap();
        shop.emit_with("orders/cancelled", &order, snapshot);
        // The summary is unchanged: no webhook.
        crate::domains::segments::follow(&mut shop, start, &|_| true);
        assert!(
            topics(&mut shop)
                .iter()
                .all(|topic| topic != "customers/purchasing_summary")
        );
        let start = shop.event_count();
        if let Some(record) = shop.get_mut(&jane) {
            record.insert("numberOfOrders".into(), json!("1"));
        }
        shop.emit("orders/cancelled", &order);
        crate::domains::segments::follow(&mut shop, start, &|_| true);
        let events = shop.take_events();
        let summary = events
            .iter()
            .find(|event| event.topic == "customers/purchasing_summary")
            .unwrap();
        let payload = crate::webhooks::payload::payload(
            "customers/purchasing_summary",
            summary.snapshot.as_ref().unwrap(),
            &crate::api::samples::ShopLookup(&shop),
        )
        .unwrap();
        assert_eq!(payload["customerId"], json!(jane));
        assert_eq!(payload["numberOfOrders"], 1);
    }
}
