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
pub mod platform;
pub mod pos;
pub mod privacy;
pub mod segments;
pub mod staff;
pub mod store_admin;
pub mod storefront;
pub mod subscriptions;
pub mod tags;
pub mod themes;
pub mod webhook_subscriptions;

use crate::resolve::Registry;
use crate::state::{Record, ShopState};

/// What the shop's clock passing did that the api layer finishes, beyond the events it emitted.
#[derive(Debug, Default)]
pub struct Ticked {
    /// The customers whose data was erased, as they were before: what `customers/redact`
    /// reports. A compliance topic, so the api layer triggers it rather than emits it.
    pub erased: Vec<Record>,
    /// How many abandoned checkouts were old enough to be deleted (`checkouts/delete`).
    pub deleted_checkouts: usize,
}

/// Does what the shop's clock passed since it last ran: every family's work that waits on time,
/// in one place. Runs before every request of the Admin API, and when the clock is moved
/// (`POST /__lsf/admin/clock`); what it changes is a change of the shop like a mutation's
/// (its events delivered, the store sync told).
///
/// - the payment schedules that came due fire `payment_schedules/due`, store credit that
///   expired leaves its account, Shopify Payments pays out the days left behind
///   ([`orders::tick`]);
/// - the app's charges pending for more than two days expire ([`app::billing::expire_stale`]);
/// - the scheduled product and collection listings whose date passed are listed
///   ([`markets::publish_due_listings`]);
/// - the data erasures ten days old are processed ([`privacy::process_due_erasures`]);
/// - the abandoned checkouts three months old are deleted ([`checkouts::delete_expired`]).
///
/// What is worked out when it is read (an abandoned checkout's ten minutes, a subscription's
/// next billing date, a discount's or a gift card's dates, a payment's `overdue`) needs no tick.
pub fn tick(state: &mut ShopState) -> Ticked {
    orders::tick(state);
    app::billing::expire_stale(state);
    markets::publish_due_listings(state);
    Ticked {
        erased: privacy::process_due_erasures(state),
        deleted_checkouts: checkouts::delete_expired(state),
    }
}

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
    platform::register(registry);
    pos::register(registry);
    privacy::register(registry);
    segments::register(registry);
    staff::register(registry);
    store_admin::register(registry);
    storefront::register(registry);
    subscriptions::register(registry);
    tags::register(registry);
    themes::register(registry);
    webhook_subscriptions::register(registry);
}
