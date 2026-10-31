//! `tagsAdd` and `tagsRemove`: the tags of any resource that has them (a product, a customer, an
//! order, a draft order, an article), by its GID.
//!
//! - Tags are trimmed, a comma-separated item is split ("Tags can be passed as an array of
//!   strings or a comma-separated string", shopify.dev), and a tag the resource has, whatever its
//!   case, is not added twice; `tagsRemove` removes them ignoring case.
//! - What they require depends on the resource ("The scope of the resource", which the access
//!   table cannot say): `write_products` for a product, `write_customers` for a customer,
//!   `write_orders` for an order, `write_draft_orders` for a draft order, `write_content` for an
//!   article. lsf checks it, as the `ACCESS_DENIED` of the field.
//! - Events: the update topic of the resource (`products/update`, `customers/update`,
//!   `orders/updated`, `draft_orders/update`, `articles/update`), and for a customer
//!   `customer.tags_added` or `customer.tags_removed`.

use serde_json::{Value, json};

use crate::auth::scopes::{RequiredAccess, Root};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::access::denied;
use crate::resolve::{Mutation, Registry, UserError, payload};

use super::catalog::tags_of;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("tagsAdd", tags_add)
        .mutation("tagsRemove", tags_remove);
}

/// What a type of tagged resource is: the scope that writes it, its update topic, and the
/// topics a customer's tag change has too (added, removed).
struct Taggable {
    scope: &'static str,
    topic: &'static str,
    customer_topics: Option<(&'static str, &'static str)>,
}

fn taggable(type_name: &str) -> Option<Taggable> {
    let (scope, topic, customer_topics) = match type_name {
        "Product" => ("write_products", "products/update", None),
        "Customer" => (
            "write_customers",
            "customers/update",
            Some(("customer.tags_added", "customer.tags_removed")),
        ),
        "Order" => ("write_orders", "orders/updated", None),
        "DraftOrder" => ("write_draft_orders", "draft_orders/update", None),
        "Article" => ("write_content", "articles/update", None),
        _ => return None,
    };
    Some(Taggable {
        scope,
        topic,
        customer_topics,
    })
}

fn tags_add(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    change(mutation, ctx, true)
}

fn tags_remove(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    change(mutation, ctx, false)
}

fn change(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    add: bool,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let tags = tags_of(ctx.arg("tags"));
    let state = mutation.state();
    let Some(Taggable {
        scope,
        topic,
        customer_topics,
    }) = state.type_of(&id).and_then(taggable)
    else {
        let error = UserError::new(&["id"], "Resource does not exist or cannot be tagged");
        return Ok(payload(json!({"node": null}), &[error]));
    };
    if let Some(grant) = mutation.grant() {
        let access = RequiredAccess::parse(&format!("`{scope}` access scope."), Root::Mutation);
        if !grant.allows(&access) {
            return Err(denied(&access, ctx.field_name()));
        }
    }
    if tags.is_empty() {
        let error = UserError::new(&["tags"], "Tags can't be blank");
        return Ok(payload(json!({"node": null}), &[error]));
    }
    let state = mutation.state_mut();
    // The customer's tag topics say which tags changed: their payload compares with the record
    // as it was.
    let previous = customer_topics.and_then(|_| state.get(&id).cloned());
    let Some(record) = state.get_mut(&id) else {
        return Ok(payload(json!({"node": null}), &[]));
    };
    let mut current = tags_of(record.get("tags"));
    let before = current.len();
    if add {
        for tag in tags {
            if !current.iter().any(|known| known.eq_ignore_ascii_case(&tag)) {
                current.push(tag);
            }
        }
    } else {
        current.retain(|known| !tags.iter().any(|tag| tag.eq_ignore_ascii_case(known)));
    }
    let changed = current.len() != before;
    record.insert("tags".into(), json!(current));
    if changed {
        state.touch(&id);
        state.emit(topic, &id);
        if let (Some((added, removed)), Some(previous)) = (customer_topics, previous) {
            state.emit_with(if add { added } else { removed }, &id, previous);
        }
    }
    Ok(payload(json!({"node": id}), &[]))
}
