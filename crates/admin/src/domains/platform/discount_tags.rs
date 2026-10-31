//! `discountBulkTagsAdd` and `discountBulkTagsRemove`: tags added to (or removed from) the
//! discounts named by `ids` (`DiscountNode`, `DiscountAutomaticNode` or `DiscountCodeNode`
//! GIDs), by a `search` of `discountNodes`, or by a saved search of discounts (`PRICE_RULE`).
//!
//! Exactly one of the three is given (`MISSING_ARGUMENT`, `TOO_MANY_ARGUMENTS`), and some tags
//! (`BLANK`); a tag is 255 characters at most (`INVALID_TAG_LENGTH`). Done within the request:
//! the `job` is done, each discount whose tags changed is `updatedAt` now and delivers
//! `discounts/update`. The wording is lsf's (Shopify publishes none), the codes Shopify's.

use serde_json::{Value, json};

use crate::domains::store_admin::done_job;
use crate::graphql::{self, FieldContext, FieldError, Request};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::ShopState;

/// The longest tag.
const MAX_TAG: usize = 255;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("discountBulkTagsAdd", |m, ctx| tags(m, ctx, true))
        .mutation("discountBulkTagsRemove", |m, ctx| tags(m, ctx, false));
}

/// The discount node a GID names: itself, or the node a `DiscountNode/<n>` is.
fn node_of(state: &ShopState, id: &str) -> Option<String> {
    let (kind, number) = crate::state::parse_gid(id)?;
    let candidates: &[&str] = match kind {
        "DiscountNode" => &["DiscountAutomaticNode", "DiscountCodeNode"],
        "DiscountAutomaticNode" | "DiscountCodeNode" => &[],
        _ => return None,
    };
    if candidates.is_empty() {
        return state.contains(id).then(|| id.to_string());
    }
    candidates
        .iter()
        .map(|kind| crate::state::gid(kind, number))
        .find(|gid| state.contains(gid))
}

/// The discount nodes a search of `discountNodes` keeps, every page.
fn searched(m: &mut Mutation<'_>, query: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut request = Request::new(
            "query($query: String!, $after: String) { discountNodes(first: 250, after: $after, query: $query) { nodes { id } pageInfo { hasNextPage endCursor } } }",
        );
        request.variables.insert("query".into(), json!(query));
        request.variables.insert("after".into(), json!(after));
        let schema = m.schema();
        let answer = {
            let mut resolver = m.resolver();
            graphql::run(schema, &mut resolver, &request, None).to_json()
        };
        let connection = &answer["data"]["discountNodes"];
        ids.extend(
            connection["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|node| node["id"].as_str().map(str::to_string)),
        );
        if connection["pageInfo"]["hasNextPage"] != json!(true) {
            return ids;
        }
        after = connection["pageInfo"]["endCursor"]
            .as_str()
            .map(str::to_string);
    }
}

fn tags(m: &mut Mutation<'_>, ctx: &FieldContext<'_>, add: bool) -> Result<Value, FieldError> {
    let refuse = |field: &str, message: &str, code: &str| {
        let path: Vec<&str> = if field.is_empty() {
            vec![]
        } else {
            vec![field]
        };
        Ok(payload(
            json!({"job": null}),
            &[UserError::new(&path, message).code(code)],
        ))
    };
    let tags: Vec<String> = ctx
        .arg("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect();
    if tags.is_empty() {
        return refuse("tags", "Tags can't be blank", "BLANK");
    }
    if tags.iter().any(|tag| tag.chars().count() > MAX_TAG) {
        return refuse(
            "tags",
            &format!("A tag is {MAX_TAG} characters at most"),
            "INVALID_TAG_LENGTH",
        );
    }
    let given = [
        ctx.arg("ids").is_some(),
        ctx.arg_str("search").is_some(),
        ctx.arg_str("savedSearchId").is_some(),
    ];
    match given.iter().filter(|given| **given).count() {
        0 => {
            return refuse(
                "",
                "One of ids, search or savedSearchId must be given",
                "MISSING_ARGUMENT",
            );
        }
        1 => {}
        _ => {
            return refuse(
                "",
                "Only one of ids, search or savedSearchId can be given",
                "TOO_MANY_ARGUMENTS",
            );
        }
    }
    let nodes: Vec<String> = if let Some(ids) = ctx.arg("ids").and_then(Value::as_array) {
        let mut nodes = Vec::new();
        for (index, id) in ids.iter().filter_map(Value::as_str).enumerate() {
            match node_of(m.state(), id) {
                Some(node) => nodes.push(node),
                None => {
                    let position = index.to_string();
                    let error = UserError::new(&["ids", &position], "Discount does not exist")
                        .code("INVALID");
                    return Ok(payload(json!({"job": null}), &[error]));
                }
            }
        }
        nodes
    } else if let Some(search) = ctx.arg_str("search") {
        searched(m, search)
    } else {
        let id = ctx.arg_str("savedSearchId").unwrap_or_default();
        let saved = crate::domains::store_admin::saved_searches::saved_search(m.state(), id)
            .filter(|search| {
                search.get("resourceType").and_then(Value::as_str) == Some("PRICE_RULE")
            });
        let Some(saved) = saved else {
            return refuse("savedSearchId", "Saved search not found", "INVALID");
        };
        let query = saved
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        searched(m, &query)
    };
    // A search answers `DiscountNode` ids: the records are the automatic or code nodes.
    let nodes: Vec<String> = nodes
        .iter()
        .filter_map(|id| node_of(m.state(), id))
        .collect();
    let state = m.state_mut();
    let now = state.timestamp();
    for node in nodes {
        let changed = state.get_mut(&node).is_some_and(|record| {
            let field = if record.contains_key("codeDiscount") {
                "codeDiscount"
            } else {
                "automaticDiscount"
            };
            let Some(discount) = record.get_mut(field).and_then(Value::as_object_mut) else {
                return false;
            };
            let mut current: Vec<String> = discount
                .get("tags")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            let before = current.clone();
            if add {
                for tag in &tags {
                    if !current.contains(tag) {
                        current.push(tag.clone());
                    }
                }
            } else {
                current.retain(|tag| !tags.contains(tag));
            }
            if current == before {
                return false;
            }
            discount.insert("tags".into(), json!(current));
            discount.insert("updatedAt".into(), json!(now));
            true
        });
        if changed {
            state.emit("discounts/update", &node);
        }
    }
    let job = done_job(
        state,
        if add {
            "discount_bulk_tags_add"
        } else {
            "discount_bulk_tags_remove"
        },
    );
    Ok(payload(json!({"job": job}), &[]))
}
