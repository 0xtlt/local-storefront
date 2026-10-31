//! One module per family of resources: its seed, its custom resolvers, its mutations. The
//! webhook payloads of each family are in `webhooks::payload`. See
//! `crates/admin/docs/design.md`, and "Writing a resolver or a mutation" there.

pub mod app;
pub mod catalog;
pub mod content;
pub mod customers;
pub mod logistics;
pub mod markets;
pub mod orders;
pub mod subscriptions;
pub mod tags;
pub mod webhook_subscriptions;

use crate::resolve::Registry;

/// Registers the custom resolvers and the mutations of every family: what
/// [`Registry::standard`] holds.
pub fn register(registry: &mut Registry) {
    app::register(registry);
    catalog::register(registry);
    content::register(registry);
    customers::register(registry);
    markets::register(registry);
    logistics::register(registry);
    orders::register(registry);
    subscriptions::register(registry);
    tags::register(registry);
    webhook_subscriptions::register(registry);
}
