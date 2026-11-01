//! What an order says of its risk and of where it came from: risk assessments, Shopify
//! Protect, order attribution definitions, and a buyer asking for a new order link.
//!
//! As shopify.dev's reference of 2026-10 describes them (`mutations/orderRiskAssessmentCreate`,
//! `objects/OrderRiskSummary`, `mutations/orderAttributionDefinitionUpsert`, the guide "Configure
//! order attribution for sales channel apps"):
//!
//! - **Risk assessments**: `orderRiskAssessmentCreate` adds the app's assessment (its level and
//!   at most 20 facts) to an order that is not fulfilled yet, and fires
//!   `orders/risk_assessment_changed`. `Order.risk` lists the assessments with a
//!   recommendation *(lsf's rule, Shopify does not publish its own)*: `CANCEL` when one is high,
//!   `INVESTIGATE` when one is medium, `ACCEPT` when they are low or none, `NONE` without one
//!   (or while they are pending). The deprecated `riskLevel` and `risks` follow them. lsf makes no
//!   assessment of its own: an order without an app's has none.
//! - **Shopify Protect** is off on a local shop: `Order.shopifyProtect` is `null` ("if Shopify
//!   Protect is disabled for the shop or the order isn't eligible"), until a test sets it with
//!   `POST /__lsf/admin/orders/protect`, which fires `orders/shopify_protect_eligibility_changed`.
//! - **Attribution definitions** (`orderAttributionDefinitionUpsert`, `…Delete`,
//!   `orderAttributionDefinitions`) are the app's, by handle (case insensitive); an order whose
//!   `sourceName` is one of their handles is attributed to it (`Order.attribution`), as the guide
//!   says `orderCreate` and carts pass it. An icon must be an SVG with a `0 0 20 20` viewbox.
//! - **A new order link**: `POST /__lsf/admin/orders/link-request` is the buyer asking for one on
//!   an expired order status page: lsf records the email of the new link in the shop's outbox and
//!   fires `orders/link_requested`.
//!
//! Messages: lsf's wording, with the codes of the payloads' error types; shopify.dev shows none.

use serde_json::{Value, json};

use super::model::{self, text};
use crate::domains::app::CHANGED;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::outbox::{self, Entry};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("orderAttributionDefinitionDelete", attribution_delete)
        .mutation("orderAttributionDefinitionUpsert", attribution_upsert)
        .mutation("orderRiskAssessmentCreate", assessment_create)
        .field("Order", "attribution", attribution)
        .field("Order", "risk", risk_summary)
        .field("Order", "riskLevel", |r, _ctx, object| {
            let level = r.map_of(object).map_or("LOW", |order| match highest(order) {
                Some("HIGH") => "HIGH",
                Some("MEDIUM") => "MEDIUM",
                _ => "LOW",
            });
            Ok(Output::value(level))
        })
        .field("Order", "risks", |r, ctx, object| {
            let risks: Vec<Value> = r
                .map_of(object)
                .map(assessments)
                .unwrap_or_default()
                .iter()
                .flat_map(|assessment| {
                    let level = match assessment.get("riskLevel").and_then(Value::as_str) {
                        Some("HIGH") => "HIGH",
                        Some("MEDIUM") => "MEDIUM",
                        _ => "LOW",
                    };
                    assessment
                        .get("facts")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(move |fact| {
                            json!({"display": true, "level": level, "message": fact.get("description")})
                        })
                })
                .collect();
            r.convert(ctx, &Value::Array(risks), false)
        });
}

/// The order field that keeps its risk assessments, newest last.
pub const ASSESSMENTS: &str = "lsfRiskAssessments";

/// The most facts an assessment takes (`TOO_MANY_FACTS`).
const MAX_FACTS: usize = 20;

fn assessments(order: &Record) -> Vec<Value> {
    model::array(order, ASSESSMENTS)
}

/// The highest risk level of an order's assessments that are not pending.
fn highest(order: &Record) -> Option<&'static str> {
    let levels: Vec<String> = assessments(order)
        .iter()
        .filter_map(|assessment| assessment.get("riskLevel")?.as_str().map(str::to_string))
        .collect();
    ["HIGH", "MEDIUM", "LOW", "NONE"]
        .into_iter()
        .find(|level| levels.iter().any(|known| known == level))
}

/// `Order.risk`: the assessments and lsf's recommendation from them.
fn risk_summary(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let order = r.map_of(object).cloned().unwrap_or_default();
    let recommendation = match highest(&order) {
        Some("HIGH") => "CANCEL",
        Some("MEDIUM") => "INVESTIGATE",
        Some(_) => "ACCEPT",
        None => "NONE",
    };
    let assessments: Vec<Value> = assessments(&order)
        .into_iter()
        .map(|assessment| {
            json!({
                "riskLevel": assessment.get("riskLevel"),
                "facts": assessment.get("facts"),
                "provider": assessment.get("provider"),
            })
        })
        .collect();
    Ok(Output::Object(Obj::built(
        "OrderRiskSummary",
        value::object(json!({"assessments": assessments, "recommendation": recommendation})),
    )))
}

/// `orderRiskAssessmentCreate`.
fn assessment_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"orderRiskAssessment": null}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    let input = model::input_of(ctx.arg("orderRiskAssessmentInput"));
    let id = input
        .get("orderId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(order) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("Order"))
    else {
        return failed(
            &["orderRiskAssessmentInput", "orderId"],
            "Order does not exist",
            "NOT_FOUND",
        );
    };
    if text(order, "displayFulfillmentStatus") == Some("FULFILLED") {
        return failed(
            &["orderRiskAssessmentInput", "orderId"],
            "The order is fulfilled and can no longer take risk assessments",
            "ORDER_ALREADY_FULFILLED",
        );
    }
    let facts = model::array(&input, "facts");
    if facts.len() > MAX_FACTS {
        return failed(
            &["orderRiskAssessmentInput", "facts"],
            "A risk assessment can have at most 20 facts",
            "TOO_MANY_FACTS",
        );
    }
    for (index, fact) in facts.iter().enumerate() {
        if fact
            .get("description")
            .and_then(Value::as_str)
            .is_none_or(|text| text.trim().is_empty())
        {
            let index = index.to_string();
            return failed(
                &["orderRiskAssessmentInput", "facts", &index, "description"],
                "Description can't be blank",
                "INVALID",
            );
        }
    }
    let facts: Vec<Value> = facts
        .iter()
        .map(|fact| json!({"description": fact.get("description"), "sentiment": fact.get("sentiment")}))
        .collect();
    let provider = state.keys("App").next().map(str::to_string);
    let assessment = json!({
        "riskLevel": input.get("riskLevel"),
        "facts": facts,
        "provider": provider,
        "createdAt": mutation.timestamp(),
    });
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        let mut all = assessments(record);
        all.push(assessment.clone());
        record.insert(ASSESSMENTS.into(), Value::Array(all));
    }
    state.emit("orders/risk_assessment_changed", &id);
    Ok(payload(
        json!({"orderRiskAssessment": {
            "riskLevel": assessment["riskLevel"],
            "facts": assessment["facts"],
            "provider": assessment["provider"],
        }}),
        &[],
    ))
}

// --- Attribution ---

/// The attribution definition of a handle, ignoring case.
fn definition_of(state: &ShopState, handle: &str) -> Option<String> {
    state
        .keys("OrderAttributionDefinition")
        .find(|key| {
            state
                .get(key)
                .and_then(|record| text(record, "handle"))
                .is_some_and(|known| known.eq_ignore_ascii_case(handle))
        })
        .map(str::to_string)
}

/// `Order.attribution`: the definition its `sourceName` names, `null` otherwise.
fn attribution(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let definition = r
        .map_of(object)
        .and_then(|order| text(order, "sourceName"))
        .and_then(|source| definition_of(state, source))
        .and_then(|definition| state.get(&definition));
    Ok(match definition {
        Some(definition) => Output::Object(Obj::built(
            "OrderAttribution",
            value::object(json!({
                "handle": definition.get("handle"),
                "displayName": definition.get("displayName"),
                "icon": definition.get("icon"),
            })),
        )),
        None => Output::Null,
    })
}

/// Whether an icon is an SVG with the `0 0 20 20` viewbox the guide requires.
fn valid_icon(icon: &str) -> bool {
    let icon = icon.trim();
    icon.starts_with("<svg")
        && icon.ends_with("</svg>")
        && (icon.contains("viewBox=\"0 0 20 20\"") || icon.contains("viewBox='0 0 20 20'"))
        && !icon.to_ascii_lowercase().contains("<script")
}

/// `orderAttributionDefinitionUpsert`.
fn attribution_upsert(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = |field: &str, message: &str| {
        Ok(payload(
            json!({"orderAttributionDefinition": null}),
            &[UserError::new(&["input", field], message).code("INVALID")],
        ))
    };
    let input = model::input_of(ctx.arg("input"));
    let handle = input
        .get("handle")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let name = input
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    if handle.is_empty() {
        return failed("handle", "Handle can't be blank");
    }
    if handle.chars().count() > 255 {
        return failed("handle", "Handle is too long (maximum is 255 characters)");
    }
    if name.is_empty() {
        return failed("displayName", "Display name can't be blank");
    }
    if let Some(icon) = input.get("icon").and_then(Value::as_str)
        && !valid_icon(icon)
    {
        return failed(
            "icon",
            "Icon must be sanitized SVG content with a 0 0 20 20 viewBox",
        );
    }
    let state = mutation.state_mut();
    let id = match definition_of(state, &handle) {
        Some(id) => id,
        None => {
            let id = state.new_gid("OrderAttributionDefinition");
            state.insert(value::object(json!({
                "__typename": "OrderAttributionDefinition",
                "id": id,
                "handle": handle,
                "displayName": name,
                "icon": null,
            })));
            id
        }
    };
    if let Some(record) = state.get_mut(&id) {
        record.insert("displayName".into(), json!(name));
        // Omitted, the icon stays; `null` clears it.
        if let Some(icon) = input.get("icon") {
            record.insert("icon".into(), icon.clone());
        }
    }
    state.emit(CHANGED, &id);
    Ok(payload(json!({"orderAttributionDefinition": id}), &[]))
}

/// `orderAttributionDefinitionDelete`.
fn attribution_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if mutation.state().type_of(&id) != Some("OrderAttributionDefinition") {
        return Ok(payload(
            json!({"deletedId": null}),
            &[
                UserError::new(&["id"], "Order attribution definition does not exist")
                    .code("NOT_FOUND"),
            ],
        ));
    }
    let state = mutation.state_mut();
    state.emit(CHANGED, &id);
    state.remove(&id);
    Ok(payload(json!({"deletedId": id}), &[]))
}

// --- What a test makes happen ---

/// Sets an order's Shopify Protect summary (`status`, `eligibility`, as their enums name them)
/// and fires `orders/shopify_protect_eligibility_changed`. `Err` with why when it cannot.
pub fn set_protect(
    state: &mut ShopState,
    order: &str,
    status: &str,
    eligibility: &str,
) -> Result<(), String> {
    if state.type_of(order) != Some("Order") {
        return Err(format!("{order} is not an order of {}", state.domain()));
    }
    const STATUSES: [&str; 5] = [
        "ACTIVE",
        "INACTIVE",
        "NOT_PROTECTED",
        "PENDING",
        "PROTECTED",
    ];
    const ELIGIBILITIES: [&str; 3] = ["ELIGIBLE", "NOT_ELIGIBLE", "PENDING"];
    let (status, eligibility) = (
        status.to_ascii_uppercase(),
        eligibility.to_ascii_uppercase(),
    );
    if !STATUSES.contains(&status.as_str()) || !ELIGIBILITIES.contains(&eligibility.as_str()) {
        return Err(format!(
            "`status` is one of {} and `eligibility` one of {}",
            STATUSES.join(", "),
            ELIGIBILITIES.join(", ")
        ));
    }
    if let Some(record) = state.get_mut(order) {
        record.insert(
            "shopifyProtect".into(),
            json!({"status": status, "eligibility": {"status": eligibility}}),
        );
    }
    state.emit("orders/shopify_protect_eligibility_changed", order);
    Ok(())
}

/// A buyer asks for a new link to an order's status page: the email of the link is recorded in
/// the outbox, and `orders/link_requested` fires. `Err` with why when it cannot.
pub fn request_link(state: &mut ShopState, order: &str) -> Result<(), String> {
    let Some(record) = state
        .get(order)
        .filter(|_| state.type_of(order) == Some("Order"))
        .cloned()
    else {
        return Err(format!("{order} is not an order of {}", state.domain()));
    };
    let Some(email) = text(&record, "email").map(str::to_string) else {
        return Err(format!("{order} has no email to send the link to"));
    };
    let name = text(&record, "name").unwrap_or_default().to_string();
    let origin = state
        .shop()
        .and_then(|shop| text(shop, "url"))
        .unwrap_or_default()
        .to_string();
    let number = model::number(order);
    let key = value::uuid(&format!("{}/{order}/{}", state.domain(), state.timestamp()));
    outbox::record(
        state,
        Entry::email("order_link_requested", &email)
            .subject(format!("Your order {name}"))
            .body(format!(
                "View your order: {origin}/account/orders/{number}?key={key}"
            ))
            .about(order),
    );
    state.emit("orders/link_requested", order);
    Ok(())
}
