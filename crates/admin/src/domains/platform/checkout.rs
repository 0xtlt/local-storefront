//! The look of the checkout: `checkoutAndAccountsConfigurationUpdate` (and the deprecated
//! `checkoutBrandingUpsert`); `checkoutAndAccountsConfiguration(s)`, `checkoutProfile(s)`,
//! `checkoutBranding(checkoutProfileId:)`.
//!
//! Every shop has a published checkout profile and its configuration, "Default"
//! (`CheckoutProfile/1`, `CheckoutAndAccountsConfiguration/1`), as a new shop has, with no
//! branding. A branding input is merged into what the profile has: "Creates new branding
//! settings if none exist, or modifies existing settings" — a field given replaces its value, a
//! field given `null` resets it, a field left out stays. Overrides given with an `id` are
//! updated the same way; one without an `id` is created.
//!
//! lsf's checkout does not draw the branding (it is the local checkout's own look, not
//! Shopify's checkout): the branding is kept and read back, and changes `editedAt` and
//! `updatedAt`. Shopify has no webhook topic for it in 2026-10.

use serde_json::{Map, Value, json};

use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::seed::Seed;
use crate::state::{gid, value};

/// The field of a profile or a configuration that keeps its branding (`CheckoutProfile` has
/// no such field in GraphQL: `checkoutBranding` reads it).
const BRANDING: &str = "lsfBranding";

pub fn register(registry: &mut Registry) {
    registry
        .mutation(
            "checkoutAndAccountsConfigurationUpdate",
            configuration_update,
        )
        .mutation("checkoutBrandingUpsert", branding_upsert)
        .query("checkoutBranding", checkout_branding);
}

/// The default profile and configuration of a shop.
pub fn seed(seed: &mut Seed, created: &str) {
    seed.insert(json!({
        "__typename": "CheckoutProfile",
        "id": gid("CheckoutProfile", 1),
        "name": "Default",
        "isPublished": true,
        "typOspPagesActive": false,
        "createdAt": created,
        "editedAt": created,
        "updatedAt": created,
    }));
    seed.insert(json!({
        "__typename": "CheckoutAndAccountsConfiguration",
        "id": gid("CheckoutAndAccountsConfiguration", 1),
        "name": "Default",
        "isPublished": true,
        "branding": null,
        "overrides": [],
        "createdAt": created,
        "editedAt": created,
        "updatedAt": created,
    }));
}

/// `base` with `patch` on top: a key given replaces, `null` removes, objects merge.
fn merge(base: &Value, patch: &Value) -> Value {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            let mut merged = base.clone();
            for (key, given) in patch {
                if given.is_null() {
                    merged.remove(key);
                } else {
                    let next = merge(merged.get(key).unwrap_or(&Value::Null), given);
                    merged.insert(key.clone(), next);
                }
            }
            Value::Object(merged)
        }
        (_, patch) => patch.clone(),
    }
}

fn configuration_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let error = |field: &[&str], message: &str, code: &str| {
        let mut error = UserError::new(field, message).code(code).to_json();
        // `reason`, a `String!` of this error type only: why, in a word.
        error["reason"] = json!(code.to_ascii_lowercase());
        error
    };
    let Some(mut record) = m
        .state()
        .get_of(&id, "CheckoutAndAccountsConfiguration")
        .cloned()
    else {
        return Ok(json!({
            "configuration": null,
            "userErrors": [error(&["id"], "Configuration not found", "NOT_FOUND")],
        }));
    };
    let input = |name: &str| ctx.arg("configuration").and_then(|input| input.get(name));
    let now = m.timestamp();
    if let Some(branding) = input("branding") {
        let before = record.get("branding").cloned().unwrap_or(Value::Null);
        let merged = if branding.is_null() {
            Value::Null
        } else {
            merge(&before, branding)
        };
        record.insert("branding".into(), merged);
    }
    let mut overrides: Vec<Value> = record
        .get("overrides")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut created = Vec::new();
    for (index, given) in input("overrides")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let branding = given.get("branding").cloned().unwrap_or(Value::Null);
        match given.get("id").and_then(Value::as_str) {
            Some(override_id) => {
                let Some(existing) = overrides
                    .iter_mut()
                    .find(|item| item.get("id").and_then(Value::as_str) == Some(override_id))
                else {
                    let position = index.to_string();
                    return Ok(json!({
                        "configuration": null,
                        "userErrors": [error(&["configuration", "overrides", &position, "id"], "Override not found", "NOT_FOUND")],
                    }));
                };
                let before = existing.get("branding").cloned().unwrap_or(Value::Null);
                existing["branding"] = merge(&before, &branding);
                existing["editedAt"] = json!(now);
                existing["updatedAt"] = json!(now);
            }
            None => created.push(branding),
        }
    }
    let state = m.state_mut();
    for branding in created {
        let override_id = state.new_gid("CheckoutAndAccountsConfigurationOverride");
        overrides.push(json!({
            "__typename": "CheckoutAndAccountsConfigurationOverride",
            "id": override_id,
            "name": format!("Override {}", overrides.len() + 1),
            "branding": branding,
            "createdAt": now,
            "editedAt": now,
            "updatedAt": now,
        }));
    }
    record.insert("overrides".into(), Value::Array(overrides));
    record.insert("editedAt".into(), json!(now));
    record.insert("updatedAt".into(), json!(now));
    state.insert(record);
    state.emit("checkout_and_accounts_configurations/update", &id);
    Ok(json!({"configuration": id, "userErrors": []}))
}

fn branding_upsert(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx
        .arg_str("checkoutProfileId")
        .unwrap_or_default()
        .to_string();
    let Some(mut profile) = m.state().get_of(&id, "CheckoutProfile").cloned() else {
        let error = UserError::new(&["checkoutProfileId"], "Checkout profile not found")
            .code("INTERNAL_ERROR");
        return Ok(payload(json!({"checkoutBranding": null}), &[error]));
    };
    let given = ctx
        .arg("checkoutBrandingInput")
        .cloned()
        .unwrap_or(Value::Null);
    let before = profile.get(BRANDING).cloned().unwrap_or(Value::Null);
    let branding = if given.is_null() {
        Value::Null
    } else {
        merge(&before, &given)
    };
    let now = m.timestamp();
    profile.insert(BRANDING.into(), branding.clone());
    profile.insert("editedAt".into(), json!(now));
    profile.insert("updatedAt".into(), json!(now));
    let state = m.state_mut();
    state.insert(profile);
    state.emit("checkout_profiles/update", &id);
    let branding = match branding {
        Value::Object(map) => Value::Object(map),
        _ => json!({"customizations": null, "designSystem": null}),
    };
    Ok(payload(json!({"checkoutBranding": branding}), &[]))
}

/// `checkoutBranding(checkoutProfileId:)`: the branding of the profile, `null` without one.
fn checkout_branding(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("checkoutProfileId").unwrap_or_default();
    let state = r.state();
    let Some(profile) = state.get_of(id, "CheckoutProfile") else {
        return Ok(Output::Null);
    };
    let mut branding: Map<String, Value> = match profile.get(BRANDING) {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };
    for field in ["customizations", "designSystem"] {
        branding.entry(field).or_insert(Value::Null);
    }
    Ok(Output::Object(Obj::built(
        "CheckoutBranding",
        value::object(Value::Object(branding)),
    )))
}
