//! Channels: `channelCreate`, `channelUpdate`, `channelDelete`, `channelFullSync`, the
//! deprecated `publishablePublishToCurrentChannel` and `publishableUnpublishToCurrentChannel`,
//! `bulkProductResourceFeedbackCreate`; the fields that read a channel's publication
//! (`Channel.products`, `publishedOnChannel`, `publishedOnCurrentChannel`,
//! `unpublishedChannels`, …), `productResourceFeedback`; and the product and collection
//! listings of a sales channel app.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `channelCreate` "creates a Channel representing a connection between the shop and an
//!   external selling platform account": its account (`accountId`, `accountName`), its
//!   specification (`specificationHandle`, whose country coverage it "intersects with the
//!   shop's available region markets"), and a handle "auto-generated from the specification
//!   handle and account ID" when none is given, unique on the shop. lsf has no channel
//!   specification to read (`shopify app deploy` deploys it to Shopify): every country of the
//!   shop's active region markets is covered (`activeRegions`).
//! - A channel is a publishing destination: lsf gives each its own `Publication` and
//!   `AppCatalog` (`catalogType: APP`), which supports future publishing.
//! - `channelDelete` removes it ("All associated product feeds are removed"), with
//!   `channels/delete`; `channelUpdate` changes its account and specification.
//! - `channelFullSync` "triggers a full product resync": one trace per country and language it
//!   covers (or those given), with the id of the sync operation, and the full sync of the
//!   channel's product feeds of that country and language (`catalog::feeds`:
//!   `product_feeds/full_sync` per product, then `product_feeds/full_sync_finish`).
//! - `publishablePublishToCurrentChannel` publishes on "the current Channel associated with the
//!   requesting app": every channel of the app here (its example answers `publishable`, `shop`
//!   and `userErrors`, "Resource does not exist" for an unknown id).
//! - Listings: a product published on a channel's publication is `product_listings/add`,
//!   unpublished `product_listings/remove` (also when it is deleted), changed while listed
//!   `product_listings/update`; the same for collections (`collection_listings/…`). A product
//!   published at a future `publishDate` is scheduled: `scheduled_product_listings/add`, then
//!   `/update` when it changes, `/remove` when it is unpublished before; it is listed by the
//!   first mutation that runs once the date has passed (lsf has no clock of its own to run it
//!   at the date).
//! - `bulkProductResourceFeedbackCreate` records an app's feedback on products ("Feedback for a
//!   later version of this resource was already accepted.", the reference's example error),
//!   which `productResourceFeedback` reads. `shopResourceFeedbackCreate` records its feedback on
//!   the shop (or one of its channels), which `App.feedback` and `Channel.resourceFeedback` read:
//!   each message starts with the name of the app or the channel ("Generic Channel is not
//!   connected. …", the example), and `ACCEPTED` "removes the active feedback entry".
//!
//! Errors without a published message are lsf's (each says so).

use chrono::{DateTime, Utc};
use lsf_core::util::handleize;
use serde_json::{Map, Value, json};

use super::{is, number, objects, regions_of, shop_locales, strings, text};
use crate::domains::catalog::{self, gids};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, coded, payload};
use crate::state::{Record, ShopState, value};

/// The record field of a channel's publication that names the channel.
const CHANNEL_OF: &str = "lsfChannel";

/// The field of a channel's publication that holds its scheduled listings:
/// `[{"publishable", "publishDate"}]`.
const SCHEDULED: &str = "lsfScheduled";

/// The field of the snapshot of a listing event with the listing: `{"publishedAt",
/// "publishAt"}`, which the payload reads.
pub const LISTING: &str = "lsfListing";

/// The most feedback inputs `bulkProductResourceFeedbackCreate` takes.
const FEEDBACK_LIMIT: usize = 50;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("channelCreate", create)
        .mutation("channelUpdate", update)
        .mutation("channelDelete", delete)
        .mutation("channelFullSync", full_sync)
        .mutation(
            "publishablePublishToCurrentChannel",
            publish_to_current_channel,
        )
        .mutation(
            "publishableUnpublishToCurrentChannel",
            unpublish_from_current_channel,
        )
        .mutation("bulkProductResourceFeedbackCreate", feedback_create)
        .mutation("shopResourceFeedbackCreate", shop_feedback_create)
        .field("App", "feedback", app_feedback)
        .field("Channel", "resourceFeedback", channel_feedback)
        .query("productResourceFeedback", resource_feedback)
        .field("Channel", "products", channel_products)
        .field("Channel", "productsCount", channel_products_count)
        .field("Channel", "collections", channel_collections)
        .field("Channel", "hasCollection", channel_has_collection)
        .field(
            "Channel",
            "productPublicationsV3",
            channel_product_publications,
        )
        .field(
            "Channel",
            "productPublications",
            channel_product_publications,
        )
        .field(
            "Channel",
            "collectionPublicationsV3",
            channel_collection_publications,
        )
        .field("Channel", "markets", channel_markets)
        .field("Channel", "marketsCount", channel_markets_count)
        .field("App", "channels", all_channels)
        .field("AppInstallation", "channel", |r, _ctx, _object| {
            Ok(Output::object_or_null(
                r.state().keys("Channel").next().map(Obj::record),
            ))
        })
        .field("Shop", "channels", all_channels)
        .field("Publication", "channels", publication_channels);
    for type_name in ["Product", "Collection", "ProductVariant"] {
        registry
            .field(type_name, "publishedOnChannel", published_on_channel)
            .field(type_name, "publishedOnCurrentChannel", published_on_current)
            .field(
                type_name,
                "publishedOnCurrentPublication",
                published_on_current,
            )
            .field(type_name, "unpublishedChannels", unpublished_channels);
    }
}

// --- Channels and their publications ---

/// The publication of a channel.
fn publication_of(state: &ShopState, channel: &str) -> Option<String> {
    state
        .get(channel)?
        .get("lsfPublication")?
        .as_str()
        .filter(|publication| state.contains(publication))
        .map(str::to_string)
}

/// Whether a publication is a channel's.
fn is_channel_publication(state: &ShopState, publication: &str) -> bool {
    state
        .get(publication)
        .and_then(|record| record.get(CHANNEL_OF))
        .is_some_and(|channel| !channel.is_null())
}

/// The publications of the channels of the app (every channel is the app's: lsf serves one).
fn app_publications(state: &ShopState) -> Vec<String> {
    state
        .keys("Channel")
        .filter_map(|channel| publication_of(state, channel))
        .collect()
}

/// The list of a publication that holds publishables of a record's type.
fn list_of(state: &ShopState, publishable: &str) -> &'static str {
    if state.type_of(publishable) == Some("Collection") {
        "collections"
    } else {
        "products"
    }
}

/// Whether a publication lists a publishable (a variant: its product).
fn lists(state: &ShopState, publication: &str, publishable: &str) -> bool {
    let publishable = if state.type_of(publishable) == Some("ProductVariant") {
        state
            .get(publishable)
            .and_then(|record| record.get("product"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else {
        publishable
    };
    state.get(publication).is_some_and(|record| {
        gids(record.get(list_of(state, publishable)))
            .iter()
            .any(|known| known == publishable)
    })
}

/// The items of a channel's publication's list.
fn items_of_channel(r: &ShopResolver<'_>, object: &Obj, list: &str) -> Vec<Value> {
    let state = r.state();
    object
        .key()
        .and_then(|channel| publication_of(state, channel))
        .and_then(|publication| state.get(&publication))
        .and_then(|record| record.get(list))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn channel_products(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, items_of_channel(r, object, "products"))
}

fn channel_products_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = r.search(ctx, "Product", items_of_channel(r, object, "products"));
    Ok(r.count(ctx, items.len()))
}

fn channel_collections(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, items_of_channel(r, object, "collections"))
}

fn channel_has_collection(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    Ok(Output::value(
        items_of_channel(r, object, "collections")
            .iter()
            .any(|item| item.as_str() == Some(id)),
    ))
}

/// The `ResourcePublication`s of a channel's list, inline.
fn resource_publications(r: &ShopResolver<'_>, object: &Obj, list: &str) -> Vec<Value> {
    let state = r.state();
    let publication = object
        .key()
        .and_then(|channel| publication_of(state, channel));
    items_of_channel(r, object, list)
        .iter()
        .filter_map(Value::as_str)
        .map(|publishable| {
            let date = state
                .get(publishable)
                .and_then(|record| record.get("publishedAt").filter(|date| !date.is_null()))
                .or_else(|| {
                    state
                        .get(publishable)
                        .and_then(|record| record.get("createdAt"))
                })
                .cloned()
                .unwrap_or(Value::Null);
            json!({
                "publication": publication,
                "channel": object.key(),
                "publishable": publishable,
                "publishDate": date,
                "isPublished": true,
                "product": publishable,
            })
        })
        .collect()
}

fn channel_product_publications(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, resource_publications(r, object, "products"))
}

fn channel_collection_publications(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, resource_publications(r, object, "collections"))
}

/// The markets of a channel: the active region markets of its regions.
fn markets_of(state: &ShopState, channel: &str) -> Vec<Value> {
    let regions = strings(
        state
            .get(channel)
            .and_then(|record| record.get("activeRegions")),
    );
    state
        .keys("Market")
        .filter(|market| {
            state.get(market).is_some_and(|record| {
                super::is_active(record)
                    && regions_of(record).iter().any(|code| regions.contains(code))
            })
        })
        .map(|market| json!(market))
        .collect()
}

fn channel_markets(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, markets_of(r.state(), object.key().unwrap_or_default()))
}

fn channel_markets_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let count = markets_of(r.state(), object.key().unwrap_or_default()).len();
    Ok(r.count(ctx, count))
}

/// `App.channels` and `Shop.channels`: every channel (lsf serves one app).
fn all_channels(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, r.records_of("Channel"))
}

/// `Publication.channels`: the channel of a channel's publication.
fn publication_channels(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items: Vec<Value> = r
        .map_of(object)
        .and_then(|record| record.get(CHANNEL_OF))
        .filter(|channel| !channel.is_null())
        .cloned()
        .into_iter()
        .collect();
    r.connection(ctx, items)
}

fn published_on_channel(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let channel = ctx.arg_str("channelId").unwrap_or_default();
    let publishable = object.key().unwrap_or_default();
    let published = publication_of(state, channel)
        .is_some_and(|publication| lists(state, &publication, publishable));
    Ok(Output::value(published))
}

/// `publishedOnCurrentChannel` and `publishedOnCurrentPublication`: on a channel of the app.
fn published_on_current(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let publishable = object.key().unwrap_or_default();
    Ok(Output::value(
        app_publications(state)
            .iter()
            .any(|publication| lists(state, publication, publishable)),
    ))
}

fn unpublished_channels(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let publishable = object.key().unwrap_or_default();
    let items = state
        .keys("Channel")
        .filter(|channel| {
            !publication_of(state, channel)
                .is_some_and(|publication| lists(state, &publication, publishable))
        })
        .map(|channel| json!(channel))
        .collect();
    r.connection(ctx, items)
}

// --- Listings ---

/// The time of a `DateTime` input.
fn time_of(date: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(date)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

/// The snapshot of a listing event: the product or collection, with the listing.
fn listing_snapshot(
    state: &ShopState,
    publishable: &str,
    published_at: Option<&str>,
    publish_at: Option<&str>,
) -> Record {
    let mut record = state.get(publishable).cloned().unwrap_or_default();
    record.insert(
        LISTING.into(),
        json!({"publishedAt": published_at, "publishAt": publish_at}),
    );
    record
}

/// The key of a listing event: the publishable's GID, with the publication's id (it names no
/// record, so the payload is built from the snapshot).
fn listing_key(publishable: &str, publication: &str) -> String {
    format!("{publishable}?publication_id={}", number(publication))
}

/// Emits a listing event of a channel's publication.
fn emit_listing(
    state: &mut ShopState,
    topic: &str,
    publishable: &str,
    publication: &str,
    publish_at: Option<&str>,
) {
    let now = state.timestamp();
    let published_at = (!topic.starts_with("scheduled")).then_some(now.as_str());
    let snapshot = listing_snapshot(state, publishable, published_at, publish_at);
    state.emit_with(topic, &listing_key(publishable, publication), snapshot);
}

/// The scheduled listings of a channel's publication.
fn scheduled_of(state: &ShopState, publication: &str) -> Vec<Map<String, Value>> {
    objects(
        state
            .get(publication)
            .and_then(|record| record.get(SCHEDULED)),
    )
}

/// Takes a publishable off a publication's schedule. Whether it was on it.
fn unschedule(state: &mut ShopState, publication: &str, publishable: &str) -> bool {
    let scheduled = scheduled_of(state, publication)
        .iter()
        .any(|entry| text(entry, "publishable") == Some(publishable));
    if scheduled
        && let Some(Value::Array(entries)) = state
            .get_mut(publication)
            .and_then(|record| record.get_mut(SCHEDULED))
    {
        entries.retain(|entry| entry.get("publishable") != Some(&json!(publishable)));
    }
    scheduled
}

/// What the catalog family's publish path asks before publishing on a publication: whether a
/// channel schedules the product rather than lists it (a future `publishDate` on a channel's
/// publication), or takes it off its schedule (unpublished while scheduled). `true` when it did
/// either: the publication's list is then not to change.
pub fn scheduled(
    state: &mut ShopState,
    publishable: &str,
    publication: &str,
    date: Option<&str>,
    published: bool,
) -> bool {
    if !is_channel_publication(state, publication) || state.type_of(publishable) != Some("Product")
    {
        return false;
    }
    let future = date
        .and_then(time_of)
        .is_some_and(|date| date > state.now());
    if !published {
        if unschedule(state, publication, publishable) {
            emit_listing(
                state,
                "scheduled_product_listings/remove",
                publishable,
                publication,
                None,
            );
            return true;
        }
        return false;
    }
    if !future {
        // Listed now: off the schedule, if it was on it.
        if unschedule(state, publication, publishable) {
            emit_listing(
                state,
                "scheduled_product_listings/remove",
                publishable,
                publication,
                None,
            );
        }
        return false;
    }
    if lists(state, publication, publishable) {
        // Listed already: a later date changes nothing.
        return false;
    }
    let date = date.unwrap_or_default().to_string();
    let was = unschedule(state, publication, publishable);
    if let Some(record) = state.get_mut(publication) {
        let entry = record.entry(SCHEDULED).or_insert_with(|| json!([]));
        if let Value::Array(entries) = entry {
            entries.push(json!({"publishable": publishable, "publishDate": date}));
        }
    }
    let topic = if was {
        "scheduled_product_listings/update"
    } else {
        "scheduled_product_listings/add"
    };
    emit_listing(state, topic, publishable, publication, Some(&date));
    true
}

/// What the catalog family's publish path reports once it published (or unpublished) a
/// product or a collection on a publication: on a channel's, its listing event.
pub fn listed(state: &mut ShopState, publishable: &str, publication: &str, published: bool) {
    if !is_channel_publication(state, publication) {
        return;
    }
    let kind = match state.type_of(publishable) {
        Some("Product") => "product_listings",
        Some("Collection") => "collection_listings",
        _ => return,
    };
    let action = if published { "add" } else { "remove" };
    emit_listing(
        state,
        &format!("{kind}/{action}"),
        publishable,
        publication,
        None,
    );
}

/// What the catalog family runs before it deletes a product or a collection: the listings of
/// the channels that have it are removed (`product_listings/remove`,
/// `scheduled_product_listings/remove`, `collection_listings/remove`).
pub fn removing(state: &mut ShopState, publishable: &str) {
    if state.count("Channel") == 0 {
        return;
    }
    let kind = match state.type_of(publishable) {
        Some("Product") => "product_listings",
        Some("Collection") => "collection_listings",
        _ => return,
    };
    for publication in app_publications(state) {
        if lists(state, &publication, publishable) {
            emit_listing(
                state,
                &format!("{kind}/remove"),
                publishable,
                &publication,
                None,
            );
        }
        if unschedule(state, &publication, publishable) {
            emit_listing(
                state,
                "scheduled_product_listings/remove",
                publishable,
                &publication,
                None,
            );
        }
    }
}

/// What runs after every mutation that emitted events (`resolve`): the products it created
/// join the publications with `autoPublish`; a product (or collection) it changed that a
/// channel lists is `product_listings/update` (`collection_listings/update`,
/// `scheduled_product_listings/update` when it is scheduled), unless the mutation listed or
/// unlisted it; and the scheduled listings whose date has passed are listed
/// ([`publish_due_listings`]).
pub fn after_mutation(state: &mut ShopState, start: usize) {
    if state.event_count() <= start {
        return;
    }
    let events: Vec<(String, String)> = state.events()[start..]
        .iter()
        .map(|event| (event.topic.clone(), event.gid.clone()))
        .collect();
    let auto = super::publications::auto_publishing(state);
    if !auto.is_empty() {
        for (topic, gid) in &events {
            if topic == "products/create" && state.contains(gid) {
                let targets: Vec<(String, Option<String>)> = auto
                    .iter()
                    .map(|publication| (publication.clone(), None))
                    .collect();
                catalog::publish_on(state, gid, &targets, true);
            }
        }
    }
    if state.count("Channel") == 0 {
        return;
    }
    let publications = app_publications(state);
    let listing_events: Vec<String> = state.events()[start..]
        .iter()
        .filter(|event| event.topic.contains("_listings/"))
        .map(|event| event.gid.clone())
        .collect();
    for (topic, gid) in &events {
        let kind = match topic.as_str() {
            "products/update" => "product_listings",
            "collections/update" => "collection_listings",
            _ => continue,
        };
        for publication in &publications {
            let key = listing_key(gid, publication);
            if listing_events.contains(&key) {
                continue;
            }
            if lists(state, publication, gid) {
                emit_listing(state, &format!("{kind}/update"), gid, publication, None);
            } else if let Some(entry) = scheduled_of(state, publication)
                .into_iter()
                .find(|entry| text(entry, "publishable") == Some(gid.as_str()))
            {
                let date = text(&entry, "publishDate").map(str::to_string);
                emit_listing(
                    state,
                    "scheduled_product_listings/update",
                    gid,
                    publication,
                    date.as_deref(),
                );
            }
        }
    }
    publish_due_listings(state);
}

/// Lists the scheduled listings whose date the shop's clock has passed. Run after every
/// mutation that emitted events, and by the shop's tick (`domains::tick`).
pub fn publish_due_listings(state: &mut ShopState) {
    if state.count("Channel") == 0 {
        return;
    }
    let now = state.now();
    for publication in &app_publications(state) {
        for entry in scheduled_of(state, publication) {
            let (Some(publishable), Some(date)) = (
                text(&entry, "publishable").map(str::to_string),
                text(&entry, "publishDate").map(str::to_string),
            ) else {
                continue;
            };
            if time_of(&date).is_some_and(|date| date <= now) {
                unschedule(state, publication, &publishable);
                catalog::publish_on(
                    state,
                    &publishable,
                    &[(publication.clone(), Some(date))],
                    true,
                );
            }
        }
    }
}

// --- Mutations ---

fn not_found(field: &str) -> UserError {
    // lsf's wording.
    coded(&[field], "Channel does not exist", "NOT_FOUND")
}

/// The channel of an argument.
fn channel_arg(state: &ShopState, ctx: &FieldContext<'_>, name: &str) -> Option<String> {
    let id = ctx.arg_str(name)?;
    is(state, id, "Channel").then(|| id.to_string())
}

/// The countries a channel covers: those of the shop's active region markets.
fn coverage(state: &ShopState) -> Vec<String> {
    let mut regions: Vec<String> = Vec::new();
    for market in state.all("Market") {
        if super::is_active(market) && market.get("type") == Some(&json!("REGION")) {
            for code in regions_of(market) {
                if !regions.contains(&code) {
                    regions.push(code);
                }
            }
        }
    }
    regions
}

/// `channelCreate`.
fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let state = mutation.state();
    let mut errors = Vec::new();
    let account_id = text(&input, "accountId")
        .unwrap_or_default()
        .trim()
        .to_string();
    let account_name = text(&input, "accountName")
        .unwrap_or_default()
        .trim()
        .to_string();
    let specification = text(&input, "specificationHandle")
        .unwrap_or_default()
        .trim()
        .to_string();
    for (field, value) in [
        ("accountId", &account_id),
        ("accountName", &account_name),
        ("specificationHandle", &specification),
    ] {
        if value.is_empty() {
            // Rails' wording.
            let name = match field {
                "accountId" => "Account id",
                "accountName" => "Account name",
                _ => "Specification handle",
            };
            errors.push(coded(
                &["input", field],
                format!("{name} can't be blank"),
                "INVALID",
            ));
        }
    }
    let handle = text(&input, "handle")
        .map(handleize)
        .unwrap_or_else(|| handleize(&format!("{specification}-{account_id}")));
    if state.handle_taken("Channel", &handle, None, None) {
        errors.push(coded(
            &["input", "handle"],
            "Handle has already been taken",
            "INVALID",
        ));
    }
    if state.all("Channel").any(|channel| {
        channel.get("accountId") == Some(&json!(account_id))
            && channel.get("specificationHandle") == Some(&json!(specification))
    }) {
        // lsf's wording: the account and the specification identify the connection.
        errors.push(coded(
            &["input", "accountId"],
            "A channel of this account and specification already exists",
            "INVALID",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"channel": null}), &errors));
    }
    mutation.note(
        "channelCreate: lsf has no channel specification to read: the channel covers every country of the shop's active region markets",
    );
    let state = mutation.state();
    let app = state.keys("App").next().map(str::to_string);
    let regions = coverage(state);
    let state = mutation.state_mut();
    let channel = state.new_gid("Channel");
    let publication = state.new_gid("Publication");
    let app_catalog = state.new_gid("AppCatalog");
    state.insert(value::object(json!({
        "__typename": "Publication",
        "id": publication,
        "name": account_name,
        "autoPublish": false,
        "catalog": app_catalog,
        "supportsFuturePublishing": true,
        "operation": null,
        "channels": [channel],
        "products": [],
        "collections": [],
        CHANNEL_OF: channel,
        SCHEDULED: [],
    })));
    state.insert(value::object(json!({
        "__typename": "AppCatalog",
        "id": app_catalog,
        "title": account_name,
        "status": "ACTIVE",
        "priceList": null,
        "publication": publication,
        "operations": [],
        "apps": app.iter().collect::<Vec<_>>(),
        "channels": [channel],
    })));
    state.insert(value::object(json!({
        "__typename": "Channel",
        "id": channel,
        "name": account_name,
        "handle": handle,
        "accountId": account_id,
        "accountName": account_name,
        "specificationHandle": specification,
        "app": app,
        "activeRegions": regions,
        "supportsFuturePublishing": true,
        "navigationItems": [],
        "overviewPath": null,
        "lsfPublication": publication,
        "lsfCatalog": app_catalog,
    })));
    // Shopify has no `channels/create` topic.
    state.emit("channels/create", &channel);
    Ok(payload(json!({"channel": channel}), &[]))
}

/// `channelUpdate`.
fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(channel) = channel_arg(state, ctx, "id") else {
        return Ok(payload(json!({"channel": null}), &[not_found("id")]));
    };
    let input = ctx.arg_object_or_default("input");
    let mut errors = Vec::new();
    for field in ["accountId", "accountName", "specificationHandle"] {
        if text(&input, field).is_some_and(|value| value.trim().is_empty()) {
            errors.push(coded(
                &["input", field],
                format!("{field} can't be blank"),
                "INVALID",
            ));
        }
    }
    let handle = text(&input, "handle").map(handleize);
    if let Some(handle) = &handle
        && (handle.is_empty() || state.handle_taken("Channel", handle, None, Some(&channel)))
    {
        errors.push(coded(
            &["input", "handle"],
            "Handle has already been taken",
            "INVALID",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"channel": null}), &errors));
    }
    let regions = coverage(state);
    let respecified = text(&input, "specificationHandle").is_some();
    let publication = publication_of(state, &channel);
    let catalog = state
        .get(&channel)
        .and_then(|record| record.get("lsfCatalog"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&channel) {
        for field in ["accountId", "accountName", "specificationHandle"] {
            if let Some(value) = text(&input, field) {
                record.insert(field.into(), json!(value.trim()));
            }
        }
        if let Some(name) = text(&input, "accountName") {
            record.insert("name".into(), json!(name.trim()));
        }
        if let Some(handle) = handle {
            record.insert("handle".into(), json!(handle));
        }
        if respecified {
            record.insert("activeRegions".into(), json!(regions));
        }
    }
    if let Some(name) = text(&input, "accountName") {
        if let Some(record) = publication.as_deref().and_then(|key| state.get_mut(key)) {
            record.insert("name".into(), json!(name.trim()));
        }
        if let Some(record) = catalog.as_deref().and_then(|key| state.get_mut(key)) {
            record.insert("title".into(), json!(name.trim()));
        }
    }
    // Shopify has no `channels/update` topic.
    state.emit("channels/update", &channel);
    Ok(payload(json!({"channel": channel}), &[]))
}

/// `channelDelete`: the channel, its publication and its catalog go.
fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(channel) = channel_arg(state, ctx, "id") else {
        return Ok(payload(json!({"deletedId": null}), &[not_found("id")]));
    };
    let publication = publication_of(state, &channel);
    let catalog = state
        .get(&channel)
        .and_then(|record| record.get("lsfCatalog"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let state = mutation.state_mut();
    if let Some(publication) = publication {
        state.remove(&publication);
    }
    if let Some(catalog) = catalog {
        state.remove(&catalog);
    }
    state.delete(&channel, "channels/delete");
    Ok(payload(json!({"deletedId": channel}), &[]))
}

/// The `LanguageCode` of a locale: `pt-BR` is `PT_BR`.
fn language_code(locale: &str) -> String {
    locale.replace('-', "_").to_ascii_uppercase()
}

/// `channelFullSync`: a trace per country and language of the channel's coverage.
fn full_sync(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(channel) = channel_arg(state, ctx, "channelId") else {
        return Ok(payload(
            json!({"fullSyncTraceInfo": null}),
            &[not_found("channelId")],
        ));
    };
    let regions = strings(
        state
            .get(&channel)
            .and_then(|record| record.get("activeRegions")),
    );
    let languages: Vec<String> = shop_locales(state)
        .iter()
        .filter(|locale| {
            state
                .get(locale)
                .and_then(|record| record.get("published"))
                .and_then(Value::as_bool)
                == Some(true)
        })
        .map(|locale| language_code(locale))
        .collect();
    let mut errors = Vec::new();
    let countries: Vec<String> = match ctx.arg_str("country") {
        Some(country) if !regions.iter().any(|code| code == country) => {
            // lsf's wording.
            errors.push(coded(
                &["country"],
                format!("The channel does not cover {country}"),
                "INVALID",
            ));
            Vec::new()
        }
        Some(country) => vec![country.to_string()],
        None => regions,
    };
    let languages: Vec<String> = match ctx.arg_str("language") {
        Some(language) if !languages.iter().any(|code| code == language) => {
            errors.push(coded(
                &["language"],
                format!("The shop does not publish the language {language}"),
                "INVALID",
            ));
            Vec::new()
        }
        Some(language) => vec![language.to_string()],
        None => languages,
    };
    if !errors.is_empty() {
        return Ok(payload(json!({"fullSyncTraceInfo": null}), &errors));
    }
    let products = publication_of(state, &channel)
        .and_then(|publication| {
            state
                .get(&publication)
                .map(|record| gids(record.get("products")))
        })
        .unwrap_or_default()
        .len();
    let state = mutation.state_mut();
    let mut traces = Vec::new();
    for country in &countries {
        for language in &languages {
            let operation = state.new_gid("ProductFullSync");
            catalog::feeds::channel_full_sync(state, &channel, country, language, &operation);
            traces
                .push(json!({"country": country, "language": language, "operationId": operation}));
        }
    }
    state.emit("channels/full_sync", &channel);
    if catalog::feeds::feeds_of(state, &channel).is_empty() {
        mutation.note(format!(
            "channelFullSync: the channel has no product feed (`productFeedCreate`): no `product_feeds/full_sync` was sent for its {products} products"
        ));
    }
    Ok(payload(json!({"fullSyncTraceInfo": traces}), &[]))
}

/// `publishablePublishToCurrentChannel` and `publishableUnpublishToCurrentChannel`.
fn current_channel(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    published: bool,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let publishable = ctx
        .arg_str("id")
        .filter(|id| matches!(state.type_of(id), Some("Product" | "Collection")))
        .map(str::to_string);
    let Some(publishable) = publishable else {
        let error = UserError::new(&["id"], "Resource does not exist");
        return Ok(payload(json!({"publishable": null}), &[error]));
    };
    let publications = app_publications(state);
    if publications.is_empty() {
        // The message of the REST Admin API's product listings for an app without a channel.
        let error = UserError::new(
            &["id"],
            "Your app doesn't have a publication for this shop.",
        );
        return Ok(payload(json!({"publishable": null}), &[error]));
    }
    let targets: Vec<(String, Option<String>)> = publications
        .into_iter()
        .map(|publication| (publication, None))
        .collect();
    catalog::publish_on(mutation.state_mut(), &publishable, &targets, published);
    Ok(payload(json!({"publishable": publishable}), &[]))
}

fn publish_to_current_channel(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    current_channel(mutation, ctx, true)
}

fn unpublish_from_current_channel(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    current_channel(mutation, ctx, false)
}

// --- Resource feedback ---

/// The key of the feedback of a product (and a channel).
fn feedback_key(product: &str, channel: Option<&str>) -> String {
    format!("feedback/{product}/{}", channel.unwrap_or("app"))
}

/// `productResourceFeedback(id:, channelId:)`.
fn resource_feedback(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let product = ctx.arg_str("id").unwrap_or_default();
    let key = feedback_key(product, ctx.arg_str("channelId"));
    Ok(Output::object_or_null(
        r.state().contains(&key).then(|| Obj::record(&key)),
    ))
}

/// `bulkProductResourceFeedbackCreate`: all or nothing.
fn feedback_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let inputs = objects(ctx.arg("feedbackInput"));
    let mut errors = Vec::new();
    if inputs.len() > FEEDBACK_LIMIT {
        // lsf's wording of the code's description.
        errors.push(coded(
            &["feedbackInput"],
            format!("At most {FEEDBACK_LIMIT} feedback inputs can be given at once"),
            "MAXIMUM_FEEDBACK_LIMIT_EXCEEDED",
        ));
    }
    let mut records = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let index = index.to_string();
        let at = |field: &str| ["feedbackInput", index.as_str(), field].map(str::to_string);
        let error = |field: &str, message: &str, code: &str| UserError {
            field: Some(at(field).to_vec()),
            message: message.to_string(),
            code: Some(code.to_string()),
        };
        let before = errors.len();
        let product = text(input, "productId").unwrap_or_default().to_string();
        if !is(state, &product, "Product") {
            errors.push(error(
                "productId",
                "Product does not exist",
                "PRODUCT_NOT_FOUND",
            ));
        }
        let channel = text(input, "channelId").map(str::to_string);
        if channel
            .as_deref()
            .is_some_and(|channel| !is(state, channel, "Channel"))
        {
            errors.push(error("channelId", "Channel does not exist", "NOT_FOUND"));
        }
        let messages = strings(input.get("messages"));
        let feedback_state = text(input, "state").unwrap_or("ACCEPTED");
        if feedback_state == "REQUIRES_ACTION" && messages.is_empty() {
            errors.push(error(
                "messages",
                "Messages can't be blank when the state requires action",
                "BLANK",
            ));
        } else if feedback_state == "ACCEPTED" && !messages.is_empty() {
            errors.push(error(
                "messages",
                "Messages must be blank when the state is accepted",
                "PRESENT",
            ));
        }
        let generated = text(input, "feedbackGeneratedAt").unwrap_or_default();
        let updated = text(input, "productUpdatedAt").unwrap_or_default();
        let key = feedback_key(&product, channel.as_deref());
        if let Some(existing) = state.get(&key) {
            let later = |field: &str, given: &str| {
                existing
                    .get(field)
                    .and_then(Value::as_str)
                    .and_then(time_of)
                    .zip(time_of(given))
                    .is_some_and(|(known, given)| known > given)
            };
            if later("productUpdatedAt", updated) || later("feedbackGeneratedAt", generated) {
                // The reference's example.
                errors.push(error(
                    "feedbackGeneratedAt",
                    "Feedback for a later version of this resource was already accepted.",
                    "OUTDATED_FEEDBACK",
                ));
            }
        }
        if errors.len() == before {
            records.push((
                key,
                value::object(json!({
                    "__typename": "ProductResourceFeedback",
                    "productId": product,
                    "channel": channel,
                    "state": feedback_state,
                    "messages": messages,
                    "feedbackGeneratedAt": generated,
                    "productUpdatedAt": updated,
                })),
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"feedback": []}), &errors));
    }
    let state = mutation.state_mut();
    let mut answered = Vec::new();
    for (key, record) in records {
        answered.push(Value::Object(record.clone()));
        state.insert_as(key.clone(), record);
        // Shopify has no topic for resource feedback.
        state.emit("product_resource_feedback/create", &key);
    }
    Ok(payload(json!({"feedback": answered}), &[]))
}

/// The key of the app's feedback on the shop (or on one of its channels).
fn shop_feedback_key(channel: Option<&str>) -> String {
    format!("feedback/shop/{}", channel.unwrap_or("app"))
}

/// `App.feedback`: the app's active feedback on the shop.
fn app_feedback(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let key = shop_feedback_key(None);
    Ok(Output::object_or_null(
        r.state().contains(&key).then(|| Obj::record(&key)),
    ))
}

/// `Channel.resourceFeedback`: the app's active feedback on the channel.
fn channel_feedback(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let key = shop_feedback_key(object.key());
    Ok(Output::object_or_null(
        r.state().contains(&key).then(|| Obj::record(&key)),
    ))
}

/// `shopResourceFeedbackCreate`.
fn shop_feedback_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let input = ctx.arg_object_or_default("input");
    let mut errors = Vec::new();
    let channel = text(&input, "channelId").map(str::to_string);
    if channel
        .as_deref()
        .is_some_and(|channel| !is(state, channel, "Channel"))
    {
        errors.push(coded(
            &["input", "channelId"],
            "Channel does not exist",
            "NOT_FOUND",
        ));
    }
    let messages = strings(input.get("messages"));
    let feedback_state = text(&input, "state").unwrap_or("ACCEPTED").to_string();
    // lsf's wording, for the codes Shopify documents without a message.
    if feedback_state == "REQUIRES_ACTION" && messages.is_empty() {
        errors.push(coded(
            &["input", "messages"],
            "Messages can't be blank when the state requires action",
            "BLANK",
        ));
    } else if feedback_state == "ACCEPTED" && !messages.is_empty() {
        errors.push(coded(
            &["input", "messages"],
            "Messages must be blank when the state is accepted",
            "PRESENT",
        ));
    }
    let generated = text(&input, "feedbackGeneratedAt").and_then(time_of);
    if generated.is_some_and(|generated| generated > state.now()) {
        errors.push(coded(
            &["input", "feedbackGeneratedAt"],
            "Feedback generated at must not be in the future",
            "FEEDBACK_DATE_IN_FUTURE",
        ));
    }
    let key = shop_feedback_key(channel.as_deref());
    // The latest feedback the app sent, accepted or not.
    let memo = format!("{key}/generatedAt");
    if let (Some(generated), Some(last)) = (
        generated,
        state.memo(&memo).and_then(Value::as_str).and_then(time_of),
    ) && last > generated
    {
        // The reference's example.
        errors.push(coded(
            &["input"],
            "Feedback for a later version of this resource was already accepted.",
            "OUTDATED_FEEDBACK",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"feedback": null}), &errors));
    }
    let name = match &channel {
        Some(channel) => state.get(channel).and_then(|record| record.get("name")),
        None => state.all("App").next().and_then(|app| app.get("title")),
    }
    .and_then(Value::as_str)
    .unwrap_or_default()
    .to_string();
    let app = state.keys("App").next().map(str::to_string);
    let at = generated.map(value::date_time);
    let state = mutation.state_mut();
    state.set_memo(memo, json!(at));
    if feedback_state == "ACCEPTED" {
        // "Sending `state: ACCEPTED` removes the active feedback entry."
        if state.contains(&key) {
            state.remove(&key);
        }
        state.emit("app_feedback/update", &key);
        return Ok(payload(json!({"feedback": null}), &[]));
    }
    let messages: Vec<Value> = messages
        .iter()
        .map(|message| json!({"field": null, "message": format!("{name} {message}").trim()}))
        .collect();
    state.insert_as(
        key.clone(),
        value::object(json!({
            "__typename": "AppFeedback",
            "app": app,
            "channel": channel,
            "feedbackGeneratedAt": at,
            "link": null,
            "messages": messages,
            "state": feedback_state,
        })),
    );
    // Shopify has no topic for resource feedback.
    state.emit("app_feedback/update", &key);
    Ok(payload(json!({"feedback": key}), &[]))
}
