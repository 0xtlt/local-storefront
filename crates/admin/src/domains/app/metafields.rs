//! The `metafields: [MetafieldInput!]` of the app family's inputs: the configuration a Function
//! reads, set on the discount, cart transform, customization or validation that runs it.
//! (`metafieldsSet`, the content family's, sets the metafields of any owner.)
//!
//! They are checked and set as `metafieldsSet` checks and sets them
//! (`content::metafields::check_owned_inputs` and `set`): created with `namespace`, `key`, `type`
//! and `value`, or updated by `id` (or by `namespace` and `key`); every type, a definition's type
//! and validations; a namespace not given is the app's reserved one (`app--<app id>`), as `$app`
//! is, `$app:config` is `app--<app id>--config`, another app's is refused. A metafield is linked
//! to the definition of its owner type, namespace and key when there is one; none is created. The
//! records are in the shape the seed gives the metafields of `checkout.discounts` and
//! `checkout.functions`, so that the store sync writes them back as the settings of the Function.
//! The errors are the mutation's: at the path of its input, with the payload's code for
//! metafields.

use serde_json::{Value, json};

use crate::domains::content::metafields as content_metafields;
use crate::resolve::UserError;
use crate::state::ShopState;

/// Checks the metafield inputs of an input at `path` (`["automaticAppDiscount"]`) of an owner
/// of `owner_type` (`DISCOUNT`, `VALIDATION`, …; `owner` `None` for one being created): their
/// errors, with `code` (the payload's code for metafields, when its error type has one).
pub fn check(
    state: &ShopState,
    owner: Option<&str>,
    owner_type: &str,
    inputs: &[Value],
    path: &[&str],
    code: Option<&str>,
    errors: &mut Vec<UserError>,
) {
    let mut at = path.to_vec();
    at.push("metafields");
    let mut found = Vec::new();
    content_metafields::check_owned_inputs(
        state,
        owner,
        owner_type,
        Some(&Value::Array(inputs.to_vec())),
        &at,
        &mut found,
    );
    for mut error in found {
        error.code = code.map(str::to_string);
        errors.push(error);
    }
}

/// Creates or updates the metafields of `owner` from checked inputs, and keeps its `metafields`
/// list (the GIDs, in creation order). Answers whether anything changed: a metafield given its
/// own type and value again is not a change.
pub fn apply(state: &mut ShopState, owner: &str, owner_type: &str, inputs: &[Value]) -> bool {
    let mut ignored = Vec::new();
    let checked = content_metafields::check_owned_inputs(
        state,
        Some(owner),
        owner_type,
        Some(&Value::Array(inputs.to_vec())),
        &[],
        &mut ignored,
    );
    let mut changed = false;
    for metafield in &checked {
        let same = content_metafields::find(state, owner, &metafield.namespace, &metafield.key)
            .and_then(|id| state.get(&id))
            .is_some_and(|record| {
                record.get("value") == Some(&json!(metafield.checked.value))
                    && record.get("type") == Some(&json!(metafield.kind))
            });
        if same {
            continue;
        }
        content_metafields::set(
            state,
            owner,
            &metafield.namespace,
            &metafield.key,
            &metafield.kind,
            &metafield.checked,
        );
        changed = true;
    }
    changed
}

/// Removes the metafields of an owner that is being deleted: they go with it, as on Shopify.
pub fn remove_all(state: &mut ShopState, owner: &str) {
    let owned: Vec<String> = state
        .get(owner)
        .and_then(|record| record.get("metafields"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    for gid in owned {
        let definition = state
            .get(&gid)
            .and_then(|metafield| metafield.get("definition"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(definition) = definition.and_then(|id| state.get_mut(&id))
            && let Some(Value::Array(metafields)) = definition.get_mut("metafields")
        {
            metafields.retain(|known| known.as_str() != Some(gid.as_str()));
            let count = metafields.len();
            definition.insert("metafieldsCount".into(), json!(count));
        }
        state.remove(&gid);
    }
}

/// The metafield inputs of an input object (`metafields`), empty when there are none.
pub fn inputs_of(input: &serde_json::Map<String, Value>) -> Vec<Value> {
    input
        .get("metafields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
