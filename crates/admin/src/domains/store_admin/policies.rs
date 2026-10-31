//! `shopPolicyUpdate`: "Updates a shop policy." The policy of the type is the shop's
//! (`Shop.shopPolicies`), its body replaced; a type the shop has no policy of yet gets one, with
//! the title and the URL Shopify gives it (`Refund policy`, `/policies/refund-policy`), and the
//! id the seed gives a policy of the store data (`content::policy_gid`), so that store sync
//! writes it to the data's `shop.policies` (the five types the store data has; the others stay
//! in the Admin API).
//!
//! The only error code is `TOO_BIG`: Shopify does not publish the limit; lsf's is 512 KB of
//! HTML, and the message is lsf's. Shopify has no webhook topic for policies in 2026-10.

use serde_json::{Value, json};

use crate::domains::content::policy_gid;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::value;

/// The longest policy body lsf keeps, in bytes.
pub const MAX_BODY_BYTES: usize = 512 * 1024;

pub fn register(registry: &mut Registry) {
    registry.mutation("shopPolicyUpdate", shop_policy_update);
}

/// The handle and the title Shopify gives a policy of a `ShopPolicyType`.
pub fn policy_name(kind: &str) -> Option<(&'static str, &'static str)> {
    Some(match kind {
        "CONTACT_INFORMATION" => ("contact-information", "Contact information"),
        "LEGAL_NOTICE" => ("legal-notice", "Legal notice"),
        "PRIVACY_POLICY" => ("privacy-policy", "Privacy policy"),
        "REFUND_POLICY" => ("refund-policy", "Refund policy"),
        "SHIPPING_POLICY" => ("shipping-policy", "Shipping policy"),
        "SUBSCRIPTION_POLICY" => ("subscription-policy", "Subscription policy"),
        "TERMS_OF_SALE" => ("terms-of-sale", "Terms of sale"),
        "TERMS_OF_SERVICE" => ("terms-of-service", "Terms of service"),
        _ => return None,
    })
}

fn shop_policy_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx
        .arg("shopPolicy")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let kind = input
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let body = input
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if body.len() > MAX_BODY_BYTES {
        let error = UserError::new(
            &["shopPolicy", "body"],
            format!("Body is too big (maximum is {} KB)", MAX_BODY_BYTES / 1024),
        )
        .code("TOO_BIG");
        return Ok(payload(json!({"shopPolicy": null}), &[error]));
    }
    let Some((handle, title)) = policy_name(&kind) else {
        let error = UserError::new(&["shopPolicy", "type"], "Type is invalid");
        return Ok(payload(json!({"shopPolicy": null}), &[error]));
    };
    let state = m.state();
    let existing = state
        .keys("ShopPolicy")
        .find(|key| {
            state
                .get(key)
                .and_then(|policy| policy.get("type"))
                .and_then(Value::as_str)
                == Some(kind.as_str())
        })
        .map(str::to_string);
    let shop = state.shop_gid().map(str::to_string);
    let origin = state
        .shop()
        .and_then(|shop| shop.get("url"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string();
    let state = m.state_mut();
    let now = state.timestamp();
    let id = match existing {
        Some(id) => {
            if let Some(policy) = state.get_mut(&id) {
                policy.insert("body".into(), json!(body));
            }
            state.touch(&id);
            id
        }
        None => {
            let id = policy_gid(handle);
            state.insert(value::object(json!({
                "__typename": "ShopPolicy",
                "id": id,
                "type": kind,
                "title": title,
                "body": body,
                "url": format!("{origin}/policies/{handle}"),
                "createdAt": now,
                "updatedAt": now,
            })));
            if let Some(shop) = &shop
                && let Some(record) = state.get_mut(shop)
            {
                let mut policies = record
                    .get("shopPolicies")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                policies.push(json!(id));
                record.insert("shopPolicies".into(), Value::Array(policies));
            }
            id
        }
    };
    state.emit("shop_policies/update", &id);
    Ok(payload(json!({"shopPolicy": id}), &[]))
}
