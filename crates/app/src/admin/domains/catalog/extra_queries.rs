//! The catalog's queries that read no record of their own: `productDuplicateJob` and
//! `productSavedSearches`.
//!
//! - `productDuplicateJob(id:)` "Returns a ProductDuplicateJob resource by ID": lsf duplicates a
//!   product within the request (`productDuplicate`, synchronous or not), so every duplicate job
//!   is `done`. An id of a duplication lsf did not run is said in `extensions.lsf.notes`.
//! - `productSavedSearches`: the saved searches of products (`resourceType: PRODUCT`), of
//!   whichever family keeps `SavedSearch` records.

use serde_json::{Value, json};

use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::{Obj, Registry, ShopResolver};
use crate::admin::state::{gid, gid_number, value};

pub fn register(registry: &mut Registry) {
    registry
        .query("productDuplicateJob", product_duplicate_job)
        .query("productSavedSearches", product_saved_searches);
}

/// `productDuplicateJob(id:)`.
fn product_duplicate_job(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let number = gid_number(id);
    let state = r.state();
    let known = state.contains(id) || state.contains(&gid("ProductDuplicateOperation", number));
    if !known {
        r.note(format!(
            "productDuplicateJob: lsf ran no duplication of {id}; its jobs are all done"
        ));
    }
    Ok(Output::Object(Obj::built(
        "ProductDuplicateJob",
        value::object(json!({"id": id, "done": true})),
    )))
}

/// `productSavedSearches`.
fn product_saved_searches(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let items: Vec<Value> = state
        .keys("SavedSearch")
        .filter(|search| {
            state
                .get(search)
                .and_then(|record| record.get("resourceType"))
                .and_then(Value::as_str)
                == Some("PRODUCT")
        })
        .map(|search| json!(search))
        .collect();
    r.connection(ctx, items)
}
