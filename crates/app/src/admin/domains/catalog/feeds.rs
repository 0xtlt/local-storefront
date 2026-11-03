//! Product feeds: `productFeedCreate`, `productFeedDelete`, `productFullSync`, the feeds'
//! statuses, and the `product_feeds/*` webhooks a sales channel app syncs its catalog with.
//!
//! As shopify.dev describes them ("Contextual product feeds", read on 2026-10-31, and the
//! reference and the webhook reference of 2026-10):
//!
//! - A feed is a country and a language of a channel ("distinct feeds for the different
//!   language and country pairs that a merchant supports"). With `productFeedManagement =
//!   "manual"`, the app makes them with `productFeedCreate` (`country`, `language`, the
//!   `channelId`, else the app's channel); `product_feeds/create`. lsf reads no channel
//!   specification: feeds are those the app makes.
//! - "Product feeds might change as the merchant modifies a market configuration. For example,
//!   if a merchant unpublishes a language, then dependent feeds become inactive":
//!   `product_feeds/update` when a feed's `status` changes. A feed is `ACTIVE` while its country
//!   is in an active region market and its language is published, `INACTIVE` otherwise.
//! - `productFullSync(id:, updatedAtSince:, beforeUpdatedAt:)` "Runs the full product sync" of a
//!   feed: "Shopify fires individual `PRODUCT_FEEDS_FULL_SYNC` webhook payloads for each of the
//!   merchant's published products" (published on the channel), then "a confirmation event to the
//!   `PRODUCT_FEEDS_FULL_SYNC_FINISH` subscription"; "The returned id can be used to attribute
//!   the webhooks" (`fullSyncId`). `channelFullSync` syncs the channel's feeds the same way.
//! - Incremental sync: "Occurs whenever a product publication is created, updated or removed
//!   for a product feed": `product_feeds/incremental_sync` for each active feed of the channel
//!   when one of its products is listed (`CREATE`), changes while listed, its stock included
//!   (`UPDATE`), or is unlisted or deleted (`DELETE`). [`after_mutation`] emits them from the
//!   listing events of the mutation, which `markets::after_mutation` has emitted before.
//! - Errors are the codes' descriptions ("The input value is invalid.", "The input value is
//!   already taken."), as the reference documents them by code only.
//!
//! An event of a sync names no record: its key is the feed's GID with the product and the sync
//! (`gid://shopify/ProductFeed/1?product_id=2&sync=3`), its snapshot the feed, the product (a copy
//! of it, for a product deleted) and the sync, which the payload is built from. No price is
//! converted: a feed's payload is in the shop's currency (lsf has no exchange rates), and has no
//! delivery estimates (`deliveryOptionEstimates: null`). Feeds do not reach the storefront.

use serde_json::{Value, json};

use super::gids;
use super::gift_cards::normalize_time;
use super::records::{self, field, text};
use crate::admin::graphql::{FieldContext, FieldError};
use crate::admin::resolve::{Mutation, Registry, UserError, payload};
use crate::admin::state::{Record, ShopState, gid, gid_number, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("productFeedCreate", feed_create)
        .mutation("productFeedDelete", feed_delete)
        .mutation("productFullSync", product_full_sync);
}

/// The record type of a feed.
pub const FEED: &str = "ProductFeed";

/// The topic of a full sync's product.
pub const FULL_SYNC: &str = "product_feeds/full_sync";

/// The topic that ends a full sync.
pub const FULL_SYNC_FINISH: &str = "product_feeds/full_sync_finish";

/// The topic of a change of a listed product.
pub const INCREMENTAL_SYNC: &str = "product_feeds/incremental_sync";

/// The snapshot field of a sync event with the feed.
pub const SYNC_FEED: &str = "feed";

/// The snapshot field of a sync event with the product's GID.
pub const SYNC_PRODUCT: &str = "product";

/// The snapshot field of a sync event with a copy of the product (what a deleted product's
/// payload reads).
pub const SYNC_PRODUCT_RECORD: &str = "productRecord";

/// The snapshot field of a sync event with what happened: `{"action", "type", "fullSyncId",
/// "occurredAt"}`, and for the end of a full sync `{"count", "createdAt"}`.
pub const SYNC_METADATA: &str = "metadata";

/// The code descriptions of the feeds' errors.
const INVALID: &str = "The input value is invalid.";
const TAKEN: &str = "The input value is already taken.";

/// The `LanguageCode` of a locale: `pt-BR` is `PT_BR`.
fn language_code(locale: &str) -> String {
    locale.replace('-', "_").to_ascii_uppercase()
}

/// The status a feed has: `ACTIVE` while its country is in an active region market and its
/// language is a published locale of the shop.
fn status_of(state: &ShopState, country: &str, language: &str) -> &'static str {
    let covered = state.all("Market").any(|market| {
        crate::admin::domains::markets::is_active(market)
            && field(market, "type").is_none_or(|kind| kind == "REGION")
            && crate::admin::domains::markets::regions_of(market)
                .iter()
                .any(|code| code == country)
    });
    let published = state.all("ShopLocale").any(|locale| {
        locale.get("published").and_then(Value::as_bool) == Some(true)
            && field(locale, "locale").map(language_code).as_deref() == Some(language)
    });
    if covered && published {
        "ACTIVE"
    } else {
        "INACTIVE"
    }
}

/// The publication of a channel.
fn publication_of(state: &ShopState, channel: &str) -> Option<String> {
    state
        .get(channel)
        .and_then(|record| field(record, "lsfPublication"))
        .filter(|publication| state.contains(publication))
        .map(str::to_string)
}

/// The feeds of a channel.
pub fn feeds_of(state: &ShopState, channel: &str) -> Vec<String> {
    state
        .keys(FEED)
        .filter(|feed| {
            state
                .get(feed)
                .and_then(|record| field(record, "channelId"))
                == Some(channel)
        })
        .map(str::to_string)
        .collect()
}

/// The snapshot of a sync event.
fn snapshot(state: &ShopState, feed: &str, product: &str, metadata: Value) -> Record {
    value::object(json!({
        "__typename": "ProductFeedSync",
        SYNC_FEED: state.get(feed),
        SYNC_PRODUCT: product,
        SYNC_PRODUCT_RECORD: state.get(product),
        SYNC_METADATA: metadata,
    }))
}

/// The key of a sync event: the feed's GID with the product and the sync.
fn event_key(feed: &str, product: &str, sync: &str) -> String {
    let number = |gid: &str| gid_number(gid);
    format!(
        "{feed}?product_id={}&sync={}",
        number(product),
        value::uuid(sync)
    )
}

/// Runs a full sync of a feed: `product_feeds/full_sync` for each product published on the
/// feed's channel (updated in the period given), then `product_feeds/full_sync_finish` with how
/// many there were. `sync` is the GID of the `ProductFullSync`.
pub fn full_sync(
    state: &mut ShopState,
    feed: &str,
    sync: &str,
    updated_since: Option<&str>,
    updated_before: Option<&str>,
) -> usize {
    let channel = state
        .get(feed)
        .and_then(|record| field(record, "channelId"))
        .unwrap_or_default()
        .to_string();
    let products: Vec<String> = publication_of(state, &channel)
        .and_then(|publication| state.get(&publication))
        .map(|publication| gids(publication.get("products")))
        .unwrap_or_default()
        .into_iter()
        .filter(|product| {
            let updated = state
                .get(product)
                .and_then(|record| field(record, "updatedAt"))
                .unwrap_or_default();
            updated_since.is_none_or(|since| updated >= since)
                && updated_before.is_none_or(|before| updated < before)
        })
        .collect();
    let now = state.timestamp();
    for product in &products {
        let metadata =
            json!({"action": "CREATE", "type": "FULL", "fullSyncId": sync, "occurredAt": now});
        let record = snapshot(state, feed, product, metadata);
        state.emit_with(FULL_SYNC, &event_key(feed, product, sync), record);
    }
    let metadata = json!({"action": "CREATE", "type": "FULL", "fullSyncId": sync, "occurredAt": now,
        "count": products.len(), "createdAt": now});
    let record = snapshot(state, feed, "", metadata);
    state.emit_with(
        FULL_SYNC_FINISH,
        &format!("{feed}?sync={}", value::uuid(sync)),
        record,
    );
    products.len()
}

/// The feed an id names.
fn feed_arg(state: &ShopState, id: &str) -> Option<String> {
    state.is_of(id, FEED).then(|| id.to_string())
}

/// `productFeedCreate`.
fn feed_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let state = mutation.state();
    let country = text(&input, "country").unwrap_or_default().to_string();
    let language = text(&input, "language").unwrap_or_default().to_string();
    let channel = match text(&input, "channelId") {
        Some(channel) if state.is_of(channel, "Channel") => Some(channel.to_string()),
        Some(_) => None,
        None => state.keys("Channel").next().map(str::to_string),
    };
    let Some(channel) = channel else {
        // The code's description; the feed of an app without a channel names none.
        let error = UserError::new(&["input", "channelId"], INVALID).code("INVALID");
        if text(&input, "channelId").is_none() {
            mutation
                .note("productFeedCreate: the app has no channel: create one with channelCreate");
        }
        return Ok(payload(json!({"productFeed": null}), &[error]));
    };
    let taken = state.all(FEED).any(|feed| {
        field(feed, "channelId") == Some(channel.as_str())
            && field(feed, "country") == Some(country.as_str())
            && field(feed, "language") == Some(language.as_str())
    });
    if taken {
        let error = UserError::new(&["input"], TAKEN).code("TAKEN");
        return Ok(payload(json!({"productFeed": null}), &[error]));
    }
    let status = status_of(state, &country, &language);
    let state = mutation.state_mut();
    let feed = state.new_gid(FEED);
    state.insert(value::object(json!({
        "__typename": FEED,
        "id": feed,
        "channelId": channel,
        "country": country,
        "language": language,
        "status": status,
    })));
    state.emit("product_feeds/create", &feed);
    Ok(payload(json!({"productFeed": feed}), &[]))
}

/// `productFeedDelete`.
fn feed_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let Some(feed) = feed_arg(mutation.state(), id) else {
        let error = UserError::new(&["id"], INVALID).code("INVALID");
        return Ok(payload(json!({"deletedId": null}), &[error]));
    };
    // Shopify has no `product_feeds/delete` topic.
    mutation.state_mut().delete(&feed, "product_feeds/delete");
    Ok(payload(json!({"deletedId": feed}), &[]))
}

/// `productFullSync`.
fn product_full_sync(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let Some(feed) = feed_arg(mutation.state(), id) else {
        let error = UserError::new(&["id"], INVALID).code("INVALID");
        return Ok(payload(json!({"id": null}), &[error]));
    };
    // In UTC with `Z`, as the records write them, to compare with them.
    let since = ctx.arg_str("updatedAtSince").map(normalize_time);
    let before = ctx.arg_str("beforeUpdatedAt").map(normalize_time);
    let state = mutation.state_mut();
    let sync = state.new_gid("ProductFullSync");
    full_sync(state, &feed, &sync, since.as_deref(), before.as_deref());
    Ok(payload(json!({"id": sync}), &[]))
}

// --- What follows a mutation ---

/// The product and the publication of a listing event's key
/// (`gid://shopify/Product/1?publication_id=2`).
fn listing_of(key: &str) -> Option<(String, String)> {
    let (product, publication) = key.split_once("?publication_id=")?;
    let number: u64 = publication.parse().ok()?;
    Some((product.to_string(), gid("Publication", number)))
}

/// The feeds whose channel has a publication: `(channel publication, feed)`, the active ones.
fn active_feeds(state: &ShopState) -> Vec<(String, String)> {
    state
        .all(FEED)
        .filter(|feed| field(feed, "status") == Some("ACTIVE"))
        .filter_map(|feed| {
            let channel = field(feed, "channelId")?;
            Some((
                publication_of(state, channel)?,
                field(feed, "id")?.to_string(),
            ))
        })
        .collect()
}

/// What follows a mutation for the feeds (the resolver runs it after `markets::after_mutation`):
/// the incremental syncs of the products listed, changed or unlisted on a channel with feeds, and
/// `product_feeds/update` for each feed whose status the mutation changed. `start` is the number
/// of events before the mutation.
pub fn after_mutation(state: &mut ShopState, start: usize) {
    if state.count(FEED) == 0 || state.event_count() <= start {
        return;
    }
    let events: Vec<(String, String, Option<Record>)> = state.events()[start..]
        .iter()
        .map(|event| {
            (
                event.topic.clone(),
                event.gid.clone(),
                event.snapshot.clone(),
            )
        })
        .collect();
    // The statuses follow the markets and the locales.
    if events.iter().any(|(topic, _, _)| {
        topic.starts_with("markets/")
            || topic.starts_with("locales/")
            || topic.starts_with("channels/")
    }) {
        let feeds: Vec<String> = state.keys(FEED).map(str::to_string).collect();
        for feed in feeds {
            let Some(record) = state.get(&feed) else {
                continue;
            };
            let status = status_of(
                state,
                field(record, "country").unwrap_or_default(),
                field(record, "language").unwrap_or_default(),
            );
            if field(record, "status") != Some(status) {
                if let Some(record) = state.get_mut(&feed) {
                    record.insert("status".into(), json!(status));
                }
                state.emit("product_feeds/update", &feed);
            }
        }
    }
    let feeds = active_feeds(state);
    if feeds.is_empty() {
        return;
    }
    // Each product once per feed, with the strongest action: a listing over a change.
    let mut changes: Vec<(String, String, &'static str, Option<Record>)> = Vec::new();
    let mut note =
        |product: &str, publication: &str, action: &'static str, copy: Option<Record>| {
            for (channel_publication, feed) in &feeds {
                if channel_publication != publication {
                    continue;
                }
                if let Some(known) = changes
                    .iter_mut()
                    .find(|(known_feed, known, _, _)| known_feed == feed && known == product)
                {
                    if known.2 == "UPDATE" {
                        known.2 = action;
                        known.3 = copy.clone();
                    }
                    continue;
                }
                changes.push((feed.clone(), product.to_string(), action, copy.clone()));
            }
        };
    for (topic, key, snapshot) in &events {
        let action = match topic.as_str() {
            "product_listings/add" => "CREATE",
            "product_listings/update" => "UPDATE",
            "product_listings/remove" => "DELETE",
            _ => continue,
        };
        if let Some((product, publication)) = listing_of(key) {
            note(&product, &publication, action, snapshot.clone());
        }
    }
    // A change of stock is a change of the variants of a listed product.
    for (topic, key, _) in &events {
        if topic != "inventory_levels/update" {
            continue;
        }
        let product = state
            .get(key)
            .and_then(|level| field(level, "item"))
            .and_then(|item| records::product_of_item(state, item))
            .map(str::to_string);
        let Some(product) = product else {
            continue;
        };
        let listed: Vec<String> = feeds
            .iter()
            .filter(|(publication, _)| {
                state
                    .get(publication)
                    .is_some_and(|record| gids(record.get("products")).contains(&product))
            })
            .map(|(publication, _)| publication.clone())
            .collect();
        for publication in listed {
            note(&product, &publication, "UPDATE", None);
        }
    }
    let now = state.timestamp();
    for (feed, product, action, copy) in changes {
        let metadata = json!({"action": action, "type": "INCREMENTAL", "occurredAt": now});
        let mut record = snapshot(state, &feed, &product, metadata);
        if let Some(copy) = copy.filter(|_| !state.contains(&product)) {
            record.insert(SYNC_PRODUCT_RECORD.into(), Value::Object(copy));
        }
        let key = event_key(&feed, &product, &format!("{now}/{action}"));
        state.emit_with(INCREMENTAL_SYNC, &key, record);
    }
}

/// The feeds of a channel that `channelFullSync` syncs for a country and a language, with the
/// sync each gets: the trace's operation.
pub fn channel_full_sync(
    state: &mut ShopState,
    channel: &str,
    country: &str,
    language: &str,
    operation: &str,
) -> usize {
    let feeds: Vec<String> = feeds_of(state, channel)
        .into_iter()
        .filter(|feed| {
            state.get(feed).is_some_and(|record| {
                field(record, "country") == Some(country)
                    && field(record, "language") == Some(language)
            })
        })
        .collect();
    feeds
        .iter()
        .map(|feed| full_sync(state, feed, operation, None, None))
        .sum()
}

/// The record a triggered sync webhook of a feed is built from (`POST /__lsf/webhooks/trigger`):
/// the snapshot an event of the topic would have, of the first product of the feed's channel.
/// `None` for a record that is not a feed, or a topic that is not a sync.
pub fn trigger_snapshot(state: &ShopState, record: &Record, topic: &str) -> Option<Record> {
    if field(record, "__typename") != Some(FEED)
        || ![FULL_SYNC, FULL_SYNC_FINISH, INCREMENTAL_SYNC].contains(&topic)
    {
        return None;
    }
    let feed = field(record, "id")?;
    let products = field(record, "channelId")
        .and_then(|channel| publication_of(state, channel))
        .and_then(|publication| state.get(&publication))
        .map(|publication| gids(publication.get("products")))
        .unwrap_or_default();
    let now = state.timestamp();
    let sync = gid("ProductFullSync", 0);
    let metadata = match topic {
        INCREMENTAL_SYNC => json!({"action": "UPDATE", "type": "INCREMENTAL", "occurredAt": now}),
        _ => json!({"action": "CREATE", "type": "FULL", "fullSyncId": sync, "occurredAt": now,
            "count": products.len(), "createdAt": now}),
    };
    let product = if topic == FULL_SYNC_FINISH {
        ""
    } else {
        products.first().map(String::as_str).unwrap_or_default()
    };
    Some(snapshot(state, feed, product, metadata))
}
