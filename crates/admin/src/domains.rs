//! One module per family of resources: its seed, its custom resolvers, its mutations. The
//! webhook payloads of each family are in `webhooks::payload`. See
//! `crates/admin/docs/design.md`, and "Writing a resolver or a mutation" there.

pub mod analytics;
pub mod app;
pub mod catalog;
pub mod checkouts;
pub mod content;
pub mod customer_merge;
pub mod customers;
pub mod logistics;
pub mod marketing;
pub mod markets;
pub mod orders;
pub mod outbox;
pub mod pos;
pub mod privacy;
pub mod segments;
pub mod staff;
pub mod subscriptions;
pub mod tags;
pub mod webhook_subscriptions;

use crate::resolve::Registry;

/// Registers the custom resolvers and the mutations of every family: what
/// [`Registry::standard`] holds.
pub fn register(registry: &mut Registry) {
    analytics::register(registry);
    app::register(registry);
    catalog::register(registry);
    checkouts::register(registry);
    content::register(registry);
    customer_merge::register(registry);
    customers::register(registry);
    logistics::register(registry);
    marketing::register(registry);
    markets::register(registry);
    orders::register(registry);
    pos::register(registry);
    privacy::register(registry);
    segments::register(registry);
    staff::register(registry);
    subscriptions::register(registry);
    tags::register(registry);
    webhook_subscriptions::register(registry);
}
