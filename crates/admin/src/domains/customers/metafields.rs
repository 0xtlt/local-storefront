//! The `metafields` input of `customerCreate`, `customerUpdate` and `customerSet`
//! (`[MetafieldInput!]`): the metafields of the customer, checked and set as `metafieldsSet`
//! checks and sets them (`content::metafields::check_owned_inputs` and `set`): every type, a
//! definition's type and validations, the app's `$app` namespaces (a namespace left out is the
//! app's, as `metafieldsSet` puts it), another app's refused. A metafield is linked to the
//! definition of its owner type, namespace and key when there is one; none is created. The errors
//! are the mutation's, at `["metafields", "<n>", "<field>"]`.

use serde_json::{Map, Value};

use crate::domains::content::{self, metafields as content_metafields};
use crate::resolve::UserError;
use crate::state::ShopState;

/// The code enum the errors are fitted to: `customerSet`'s (`customerCreate` and
/// `customerUpdate` answer `UserError`, which has no code).
const CODE_ENUM: &str = "CustomerSetUserErrorCode";

/// Checks the metafield inputs of a mutation on `owner` (`None`: being created).
pub fn check(
    state: &ShopState,
    owner: Option<&str>,
    inputs: Option<&Value>,
    errors: &mut Vec<UserError>,
) {
    let mut found = Vec::new();
    content_metafields::check_owned_inputs(
        state,
        owner,
        "CUSTOMER",
        inputs,
        &["metafields"],
        &mut found,
    );
    for mut error in found {
        error.code = content::fit_code(error.code.as_deref(), CODE_ENUM);
        errors.push(error);
    }
}

/// Sets the metafields of `owner` (of `owner_type`: `CUSTOMER`) the inputs give, which
/// [`check`] found right. Returns whether one was set.
pub fn apply(state: &mut ShopState, owner: &str, owner_type: &str, inputs: Option<&Value>) -> bool {
    let mut ignored = Vec::new();
    let checked = content_metafields::check_owned_inputs(
        state,
        Some(owner),
        owner_type,
        inputs,
        &[],
        &mut ignored,
    );
    content_metafields::set_owned(state, owner, &checked);
    !checked.is_empty()
}

/// Removes the metafields of a record being deleted.
pub fn remove_all(state: &mut ShopState, owner: &Map<String, Value>) {
    for id in owner
        .get("metafields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        state.remove(id);
    }
}
