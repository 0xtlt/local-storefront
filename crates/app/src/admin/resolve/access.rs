//! Access scopes, field by field: what [`crate::admin::auth::scopes`] says each field requires in the
//! request's version, checked against the scopes of the request's token.
//!
//! - A root field requires its own `requiredAccess` text, or that of the type it answers
//!   (`products` → `Product`: "`read_products` access scope.").
//! - A nested field requires its own text (`Shop.productTags`), and the text of the type it
//!   answers (`Order.customer` → `Customer`: "`read_customers` access scope."), as Shopify denies
//!   a field that answers a record the token cannot read, with the field's name. Nested fields
//!   are read as fields of a query, in a mutation's payload too: reading is reading.
//! - `node` and `nodes` require what the type of the record they find requires.
//!
//! The error is Shopify's: "Access denied for customer field. Required access:
//! `read_customers` access scope.", with `extensions` `code: ACCESS_DENIED`, `documentation` and
//! `requiredAccess`.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::{Map, Value};

use super::{Obj, ShopResolver};
use crate::admin::auth::scopes::{self, RequiredAccess, Root};
use crate::admin::graphql::{FieldContext, FieldError};
use crate::admin::version::ApiVersion;

/// What fields require, by version, parent type and field.
type Requirements = HashMap<(ApiVersion, String, String), Arc<Vec<RequiredAccess>>>;

/// What each field requires, read from the table the first time the field is asked.
static REQUIRED: LazyLock<Mutex<Requirements>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// What a field requires.
fn required(version: ApiVersion, ctx: &FieldContext<'_>) -> Arc<Vec<RequiredAccess>> {
    let parent = ctx.parent_type();
    let field = ctx.field_name();
    let key = (version, parent.to_string(), field.to_string());
    if let Some(found) = REQUIRED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
    {
        return found.clone();
    }
    let schema = ctx.schema();
    let table = scopes::for_version(version);
    let accesses: Vec<RequiredAccess> = if parent == schema.query_type() {
        table
            .required_access(Root::Query, field)
            .into_iter()
            .collect()
    } else if Some(parent) == schema.mutation_type() {
        table
            .required_access(Root::Mutation, field)
            .into_iter()
            .collect()
    } else {
        let named = ctx.ty().named();
        let answered = schema.connection_node(named).unwrap_or(named);
        table
            .field_access(parent, field, Root::Query)
            .into_iter()
            .chain(table.type_access(answered, Root::Query))
            .collect()
    };
    let accesses = Arc::new(accesses);
    REQUIRED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, accesses.clone());
    accesses
}

/// Refuses a field the token's scopes do not cover.
pub(super) fn check(
    resolver: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<(), FieldError> {
    let Some(grant) = resolver.grant else {
        return Ok(());
    };
    match required(resolver.version(), ctx)
        .iter()
        .find(|access| !grant.allows(access))
    {
        Some(access) => Err(denied(access, ctx.field_name())),
        None => Ok(()),
    }
}

/// Refuses a record found by `node` or `nodes` whose type the token cannot read.
pub(super) fn check_record(
    resolver: &ShopResolver<'_>,
    object: &Obj,
    field: &str,
) -> Result<(), FieldError> {
    let Some(grant) = resolver.grant else {
        return Ok(());
    };
    match scopes::for_version(resolver.version()).type_access(resolver.type_of(object), Root::Query)
    {
        Some(access) if !grant.allows(&access) => Err(denied(&access, field)),
        _ => Ok(()),
    }
}

/// The `ACCESS_DENIED` error for `field`.
pub fn denied(access: &RequiredAccess, field: &str) -> FieldError {
    let extensions = match access.extensions() {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    FieldError {
        message: access.message(field),
        extensions: Some(extensions),
    }
}
