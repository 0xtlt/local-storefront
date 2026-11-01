//! `collectionDuplicate` and the operations a collection shows (`Collection.activeOperations`).
//!
//! As shopify.dev's reference of 2026-10 describes it: "Duplicates a collection. An existing
//! collection ID and new title are required." The copy has the new title, a handle made from
//! it, and the original's description, image, SEO, sort order, template suffix, rules or
//! sources, products ("products will be copied synchronously when the collection is created";
//! the `job` is "Only returned if async processing is required": never, here) and metafields
//! ("Metafield values are not duplicated if the unique values capability is enabled": a
//! metafield whose definition has `uniqueValues` enabled is left out). "Publications may be
//! excluded by passing `copyPublications: false`"; by default the copy "will be published to
//! the same channels as the original", through the publish path of `publishablePublish`.
//!
//! The errors are those of `CollectionDuplicateUserErrorCode`, whose descriptions are the
//! messages ("The collection was not found. Please check the collection ID and try again.").
//! Events: `collections/create`, and the publication events of the copy.

use serde_json::{Map, Value, json};

use super::records::{self, field, is, link_collection, text};
use super::{collection_sources, gids, publish_on};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, coded, payload};
use crate::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("collectionDuplicate", collection_duplicate)
        .field("Collection", "activeOperations", active_operations);
}

/// The longest title and handle Shopify takes.
const LENGTH: usize = 255;

/// `Collection.activeOperations`: lsf duplicates at once, so no operation is ever running.
fn active_operations(
    _r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(Output::Object(Obj::built(
        "CollectionOperations",
        value::object(json!({"duplicate": []})),
    )))
}

/// The metafield inputs that copy an owner's metafields, but those whose definition keeps its
/// values unique.
fn metafield_copies(state: &ShopState, owner: &str) -> Vec<Map<String, Value>> {
    let metafields = state
        .get(owner)
        .map(|record| gids(record.get("metafields")))
        .unwrap_or_default();
    metafields
        .iter()
        .filter_map(|metafield| state.get(metafield))
        .filter(|metafield| {
            let unique = field(metafield, "definition")
                .and_then(|definition| state.get(definition))
                .and_then(|definition| definition.get("capabilities"))
                .and_then(|capabilities| capabilities.get("uniqueValues"))
                .and_then(|unique| unique.get("enabled"))
                .and_then(Value::as_bool);
            unique != Some(true)
        })
        .map(|metafield| {
            value::object(json!({
                "namespace": metafield.get("namespace"),
                "key": metafield.get("key"),
                "type": metafield.get("type"),
                "value": metafield.get("value"),
            }))
        })
        .collect()
}

/// `collectionDuplicate`.
fn collection_duplicate(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"collection": null, "job": null});
    let input = ctx.arg_object_or_default("input");
    let state = mutation.state();
    let original = text(&input, "collectionId").unwrap_or_default().to_string();
    if !is(state, &original, "Collection") {
        let error = coded(
            &["input", "collectionId"],
            "The collection was not found. Please check the collection ID and try again.",
            "COLLECTION_NOT_FOUND",
        );
        return Ok(payload(empty, &[error]));
    }
    let title = text(&input, "newTitle")
        .unwrap_or_default()
        .trim()
        .to_string();
    let handle = lsf_core::util::handleize(&title);
    let error = if title.is_empty() {
        Some(("The collection title must not be blank.", "BLANK"))
    } else if title.chars().count() > LENGTH || handle.chars().count() > LENGTH {
        Some((
            "The collection title or URL handle is too long.",
            "TOO_LONG",
        ))
    } else if handle.is_empty() || state.handle_taken("Collection", &handle, None, None) {
        Some((
            "The URL handle generated from the new title is already in use. Please choose a different title.",
            "TAKEN",
        ))
    } else {
        None
    };
    if let Some((message, code)) = error {
        let error = coded(&["input", "newTitle"], message, code);
        return Ok(payload(empty, &[error]));
    }
    let copy_publications = input
        .get("copyPublications")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let publications: Vec<(String, Option<String>)> = if copy_publications {
        state
            .keys("Publication")
            .filter(|publication| {
                state
                    .get(publication)
                    .is_some_and(|record| gids(record.get("collections")).contains(&original))
            })
            .map(|publication| (publication.to_string(), None))
            .collect()
    } else {
        Vec::new()
    };
    let metafields = metafield_copies(state, &original);
    let Some(source) = state.get(&original).cloned() else {
        return Ok(payload(empty, &[]));
    };
    let state = mutation.state_mut();
    let now = state.timestamp();
    let id = state.new_gid("Collection");
    let mut record = source.clone();
    for (name, value) in [
        ("id", json!(id)),
        ("title", json!(title)),
        ("handle", json!(handle)),
        ("products", json!([])),
        ("metafields", json!([])),
        ("createdAt", json!(now)),
        ("updatedAt", json!(now)),
    ] {
        record.insert(name.into(), value);
    }
    if let Some(image) = record
        .get("image")
        .filter(|image| !image.is_null())
        .cloned()
    {
        let mut image = image;
        image["id"] = json!(state.new_gid("CollectionImage"));
        record.insert("image".into(), image);
    }
    record.remove("sources");
    state.insert(record);
    let sources = collection_sources::copy_sources(state, &original, &id);
    if !sources.is_empty()
        && let Some(record) = state.get_mut(&id)
    {
        record.insert("sources".into(), json!(sources));
    }
    if super::collections::is_smart(state.get(&id).unwrap_or(&source)) {
        super::collections::evaluate(state, &id);
    } else {
        for product in gids(source.get("products")) {
            link_collection(state, &id, &product, true);
        }
    }
    let inputs: Vec<&Map<String, Value>> = metafields.iter().collect();
    records::set_metafields(state, &id, "COLLECTION", &inputs);
    state.emit("collections/create", &id);
    publish_on(state, &id, &publications, true);
    Ok(payload(json!({"collection": id, "job": null}), &[]))
}
