//! The app platform: what an app does to its own installation (`appUninstall`,
//! `appRevokeAccessScopes`, `appDowngradeAccessScopes`, delegate access tokens), what it hands
//! to Shopify's other services (Flow triggers, mobile platform applications, the checkout and
//! accounts configuration, resource feedback), and the queries of apps, installations and staff.
//!
//! **The outside world.** What Shopify would hand to a service lsf does not run (a Flow
//! workflow, an Apple or Google app link file, an EventBridge or Pub/Sub endpoint) is never
//! sent: the state Shopify would show is kept (records), and what would have been sent is
//! written to the shop's outbox ([`crate::outbox`]), which `GET /__lsf/admin/outbox` lists. A
//! reset empties it.
//!
//! **What the token store must know.** A mutation runs on the shop, under its lock; the tokens
//! are behind another lock, which comes first ("Locks are taken in one order" in
//! `api/admin.rs`). So a mutation that changes what tokens grant (an uninstall, a scope
//! revoked, a delegate token made or destroyed) changes the records, and emits an event of the
//! topic [`access::AUTH`] that says what the tokens must do; the api layer applies it once the
//! shop is unlocked, before it answers (`api::platform`). The event goes with the mutation:
//! one that fails emits nothing.

pub mod access;
pub mod checkout;
pub mod extensions;
pub mod flow;
pub mod mobile;
pub mod queries;
pub mod subscriptions;

use serde_json::Value;

use crate::state::gid;
use crate::state::seed::Seed;

/// The GID of lsf's staff member `id` of a shop: the account owner is 1, as the online tokens'
/// `associated_user` (`auth::tokens::AssociatedUser::local`).
pub fn staff_gid(id: u64) -> String {
    gid("StaffMember", id)
}

/// The records of the app platform: the shop owner as a staff member, the checkout profile and
/// its configuration, the standard definition templates, the local theme, and the app's
/// extensions lsf-checkout does not read (kept as settings).
pub fn seed(seed: &mut Seed) {
    let domain = seed.state.domain().to_string();
    let created = Seed::epoch_string();
    seed.insert(queries::staff_record(
        &crate::auth::tokens::AssociatedUser::local(&domain, 1),
        &domain,
        &created,
    ));
    checkout::seed(seed, &created);
    super::store_admin::standard::seed(seed);
    super::themes::seed(seed);
    if let Some(app) = seed.app {
        let read = extensions::read(&app.root);
        if let Some(pixel) = read.web_pixel {
            seed.state
                .set_setting(super::storefront::WEB_PIXEL_EXTENSION, pixel);
        }
        if !read.flow_triggers.is_empty() {
            seed.state
                .set_setting(flow::FLOW_TRIGGERS, Value::Array(read.flow_triggers));
        }
    }
}

/// The custom resolvers and the mutations of this family: see "Writing a resolver or a
/// mutation" in `crates/admin/docs/design.md`.
pub fn register(registry: &mut crate::resolve::Registry) {
    access::register(registry);
    checkout::register(registry);
    flow::register(registry);
    mobile::register(registry);
    queries::register(registry);
    subscriptions::register(registry);
}

#[cfg(test)]
mod tests;
