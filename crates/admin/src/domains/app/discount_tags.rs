//! `discountBulkTagsAdd` and `discountBulkTagsRemove`: tags added to (or removed from) the
//! discounts that `ids`, `search` or `savedSearchId` names, as one job.
//!
//! As shopify.dev's reference of 2026-10 describes them: they add (remove) tags "to multiple
//! discounts asynchronously using one of the following: A search query, A saved search ID, A
//! list of discount IDs"; `search` takes the syntax and the fields of `discountNodes`. The
//! payload is a `Job` and `DiscountUserError`s. lsf does the work within the request: the job
//! answers `done: true`, as the other bulk discount mutations' (`discountCodeBulkActivate`, …).
//!
//! - Exactly one of `ids`, `search`, `savedSearchId`: the messages of `discountAutomaticBulkDelete`
//!   ("Only one of IDs, search argument or saved search ID is allowed.", shopify.dev's example).
//!   A saved search is read from its record (`SavedSearch.query`), when the shop has one.
//! - A discount takes 5 tags at most ("The number of tags exceeds the maximum of 5.", the error
//!   of the create and update mutations): a discount the new tags would take past it keeps its
//!   tags, as a background job leaves a discount it cannot save, and `extensions.lsf.notes`
//!   says which.
//! - Tags are trimmed, compared ignoring case, and kept in their order; a discount whose tags
//!   change gets `updatedAt` and `discounts/update`.

use serde_json::{Value, json};

use super::discounts::{MAX_TAGS, done_job, inner_mut, node_of, search_nodes, tags_of};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::ShopState;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("discountBulkTagsAdd", |m, ctx| change(m, ctx, true))
        .mutation("discountBulkTagsRemove", |m, ctx| change(m, ctx, false));
}

/// The node types a discount id can name.
const NODE_TYPES: &[&str] = &["DiscountAutomaticNode", "DiscountCodeNode"];

/// The longest tag lsf takes (Shopify's limit of a tag elsewhere: 255 characters).
const TAG_LENGTH: usize = 255;

/// The discount nodes the arguments name.
fn targets(state: &ShopState, ctx: &FieldContext<'_>) -> Result<Vec<String>, UserError> {
    let given = |name: &str| ctx.arg(name).is_some_and(|value| !value.is_null());
    let count = ["ids", "search", "savedSearchId"]
        .iter()
        .filter(|name| given(name))
        .count();
    if count > 1 {
        return Err(UserError::new(
            &[],
            "Only one of IDs, search argument or saved search ID is allowed.",
        )
        .code("TOO_MANY_ARGUMENTS"));
    }
    if count == 0 {
        return Err(UserError::new(
            &[],
            "One of IDs, search argument or saved search ID is required.",
        )
        .code("MISSING_ARGUMENT"));
    }
    if let Some(ids) = ctx.arg("ids").and_then(Value::as_array) {
        let mut nodes = Vec::new();
        for (index, id) in ids.iter().enumerate() {
            match id.as_str().and_then(|id| node_of(state, id, NODE_TYPES)) {
                Some(node) => nodes.push(node),
                None => {
                    // lsf's wording, as the other discount mutations'.
                    return Err(UserError::new(
                        &["ids", &index.to_string()],
                        "Discount does not exist",
                    )
                    .code("INVALID"));
                }
            }
        }
        return Ok(nodes);
    }
    let search = match ctx.arg_str("savedSearchId") {
        Some(id) => match state
            .get(id)
            .filter(|_| state.type_of(id) == Some("SavedSearch"))
        {
            Some(saved) => saved
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            None => {
                return Err(
                    UserError::new(&["savedSearchId"], "Saved search does not exist")
                        .code("INVALID"),
                );
            }
        },
        None => ctx.arg_str("search").unwrap_or_default().to_string(),
    };
    Ok(search_nodes(state, &search, NODE_TYPES))
}

/// `discountBulkTagsAdd` (`add`) and `discountBulkTagsRemove`.
fn change(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    add: bool,
) -> Result<Value, FieldError> {
    let tags = tags_of(ctx.arg("tags"));
    let mut errors = Vec::new();
    if tags.is_empty() {
        errors.push(UserError::new(&["tags"], "Tags can't be blank").code("BLANK"));
    }
    if add && tags.len() > MAX_TAGS {
        errors.push(
            UserError::new(&["tags"], "The number of tags exceeds the maximum of 5.")
                .code("TOO_MANY_TAGS"),
        );
    }
    if tags.iter().any(|tag| tag.chars().count() > TAG_LENGTH) {
        errors.push(
            UserError::new(&["tags"], "Tag is too long (maximum is 255 characters)")
                .code("INVALID_TAG_LENGTH"),
        );
    }
    let nodes = match targets(mutation.state(), ctx) {
        Ok(nodes) => nodes,
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };
    if !errors.is_empty() {
        return Ok(payload(json!({"job": null}), &errors));
    }
    let now = mutation.timestamp();
    let mut skipped = Vec::new();
    let mut changes = Vec::new();
    for node in &nodes {
        let current = tags_of(
            mutation
                .state()
                .get(node)
                .and_then(super::discounts::inner)
                .and_then(|discount| discount.get("tags")),
        );
        let mut next = current.clone();
        if add {
            for tag in &tags {
                if !next.iter().any(|known| known.eq_ignore_ascii_case(tag)) {
                    next.push(tag.clone());
                }
            }
        } else {
            next.retain(|known| !tags.iter().any(|tag| tag.eq_ignore_ascii_case(known)));
        }
        if next.len() > MAX_TAGS {
            skipped.push(node.clone());
        } else if next != current {
            changes.push((node.clone(), next));
        }
    }
    if !skipped.is_empty() {
        mutation.note(format!(
            "{}: {} would take more than {MAX_TAGS} tags and kept theirs",
            ctx.field_name(),
            skipped.join(", ")
        ));
    }
    let state = mutation.state_mut();
    for (node, next) in &changes {
        if let Some(discount) = state.get_mut(node).and_then(inner_mut) {
            discount.insert("tags".into(), json!(next));
            discount.insert("updatedAt".into(), json!(now));
        }
        state.emit("discounts/update", node);
    }
    let job = done_job(state, ctx.field_name());
    Ok(payload(json!({"job": job}), &[]))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domains::catalog::mutation_tests::{demo, ok, run, run_with, topics, unchanged};

    /// Creates a code discount with tags; its node's GID.
    fn discount(shop: &mut crate::state::ShopState, title: &str, tags: &[&str]) -> String {
        let answer = run_with(
            shop,
            r#"mutation($input: DiscountCodeBasicInput!) { discountCodeBasicCreate(basicCodeDiscount: $input) {
                codeDiscountNode { id } userErrors { field message } } }"#,
            json!({"input": {
                "title": title, "code": title.to_uppercase(), "startsAt": "2026-01-01T00:00:00Z", "tags": tags,
                "customerGets": {"value": {"percentage": 0.1}, "items": {"all": true}},
                "context": {"all": "ALL"},
            }}),
        );
        ok(&answer, "discountCodeBasicCreate")["codeDiscountNode"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn tags(shop: &crate::state::ShopState, node: &str) -> serde_json::Value {
        shop.get(node).unwrap()["codeDiscount"]["tags"].clone()
    }

    #[test]
    fn tags_are_added_and_removed_by_ids_and_by_search() {
        let mut shop = demo();
        let spring = discount(&mut shop, "spring", &["season"]);
        let summer = discount(&mut shop, "summer", &[]);
        shop.take_events();
        let added = run_with(
            &mut shop,
            r#"mutation($ids: [ID!]) { discountBulkTagsAdd(ids: $ids, tags: [" promo ", "Season"]) {
                job { done } userErrors { field message code } } }"#,
            json!({"ids": [spring, summer]}),
        );
        assert_eq!(
            ok(&added, "discountBulkTagsAdd")["job"]["done"],
            json!(true)
        );
        assert_eq!(tags(&shop, &spring), json!(["season", "promo"]));
        assert_eq!(tags(&shop, &summer), json!(["promo", "Season"]));
        assert_eq!(topics(&mut shop), ["discounts/update", "discounts/update"]);

        // `discountNodes` filters by tag; a search removes the tag from what it matches.
        let tagged = run(
            &mut shop,
            r#"{ discountNodes(first: 5, query: "tag:promo") { nodes { discount { ... on DiscountCodeBasic { title } } } } }"#,
        );
        assert_eq!(
            tagged["data"]["discountNodes"]["nodes"],
            json!([{"discount": {"title": "spring"}}, {"discount": {"title": "summer"}}])
        );
        let removed = run(
            &mut shop,
            r#"mutation { discountBulkTagsRemove(search: "title:summer", tags: ["promo"]) { job { done } userErrors { message } } }"#,
        );
        ok(&removed, "discountBulkTagsRemove");
        assert_eq!(tags(&shop, &summer), json!(["Season"]));
        assert_eq!(tags(&shop, &spring), json!(["season", "promo"]));
        assert_eq!(topics(&mut shop), ["discounts/update"]);
    }

    #[test]
    fn a_bulk_tag_change_without_one_target_or_tags_is_refused_and_changes_nothing() {
        let mut shop = demo();
        let spring = discount(&mut shop, "spring", &[]);
        shop.take_events();
        let before = shop.clone();
        let both = run_with(
            &mut shop,
            r#"mutation($ids: [ID!]) { discountBulkTagsAdd(ids: $ids, search: "x", tags: ["a"]) { job { done } userErrors { field message code } } }"#,
            json!({"ids": [spring]}),
        );
        assert_eq!(
            both["data"]["discountBulkTagsAdd"],
            json!({"job": null, "userErrors": [{"field": null,
                "message": "Only one of IDs, search argument or saved search ID is allowed.", "code": "TOO_MANY_ARGUMENTS"}]})
        );
        let none = run(
            &mut shop,
            r#"mutation { discountBulkTagsRemove(tags: []) { userErrors { field code } } }"#,
        );
        assert_eq!(
            none["data"]["discountBulkTagsRemove"]["userErrors"],
            json!([{"field": ["tags"], "code": "BLANK"}, {"field": null, "code": "MISSING_ARGUMENT"}])
        );
        let too_many = run_with(
            &mut shop,
            r#"mutation($ids: [ID!]) { discountBulkTagsAdd(ids: $ids, tags: ["a", "b", "c", "d", "e", "f"]) { userErrors { field message code } } }"#,
            json!({"ids": [spring]}),
        );
        assert_eq!(
            too_many["data"]["discountBulkTagsAdd"]["userErrors"],
            json!([{"field": ["tags"], "message": "The number of tags exceeds the maximum of 5.", "code": "TOO_MANY_TAGS"}])
        );
        unchanged(&mut shop, &before, &[&spring]);
    }
}
