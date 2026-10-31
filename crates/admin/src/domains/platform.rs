//! The app platform: what an app does to its own installation (`appUninstall`,
//! `appRevokeAccessScopes`, `appDowngradeAccessScopes`, delegate access tokens), what it hands
//! to Shopify's other services (Flow triggers, mobile platform applications, the checkout and
//! accounts configuration, resource feedback), and the queries of apps, installations and staff.
//!
//! **The outside world.** What Shopify would hand to a service lsf does not run (a Flow
//! workflow, an Apple or Google app link file, an EventBridge or Pub/Sub endpoint) is never
//! sent: the state Shopify would show is kept (records), and what would have been sent is
//! written to the shop's outbox ([`outbox`]), which `GET /__lsf/admin/outbox` lists. A reset
//! empties it.
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
pub mod feedback;
pub mod flow;
pub mod mobile;
pub mod queries;

use serde_json::{Value, json};

use crate::state::seed::Seed;
use crate::state::{ShopState, gid};

/// The memo of the shop that holds its outbox.
pub const OUTBOX: &str = "lsf/outbox";

/// How many entries the outbox keeps: the oldest go first.
pub const OUTBOX_LIMIT: usize = 1000;

/// Writes to the outbox what lsf would have sent to a service outside: `service` (`flow`,
/// `apple-app-site-association`, …), `target` (what it would have gone to), `payload`.
pub fn outbox(state: &mut ShopState, service: &str, target: &str, payload: Value) {
    let mut entries = state
        .memo(OUTBOX)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let at = state.timestamp();
    entries.push(json!({
        "service": service,
        "target": target,
        "payload": payload,
        "at": at,
    }));
    if entries.len() > OUTBOX_LIMIT {
        let over = entries.len() - OUTBOX_LIMIT;
        entries.drain(..over);
    }
    state.set_memo(OUTBOX, Value::Array(entries));
}

/// The outbox of a shop, oldest first.
pub fn outbox_entries(state: &ShopState) -> Vec<Value> {
    state
        .memo(OUTBOX)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The GID of lsf's staff member `id` of a shop: the account owner is 1, as the online tokens'
/// `associated_user` (`auth::tokens::AssociatedUser::local`).
pub fn staff_gid(id: u64) -> String {
    gid("StaffMember", id)
}

/// The records of the app platform: the shop owner as a staff member, the checkout profile and
/// its configuration, and the app's extensions lsf-checkout does not read (kept as settings).
pub fn seed(seed: &mut Seed) {
    let domain = seed.state.domain().to_string();
    let created = Seed::epoch_string();
    seed.insert(queries::staff_record(
        &crate::auth::tokens::AssociatedUser::local(&domain, 1),
        &domain,
        &created,
    ));
    checkout::seed(seed, &created);
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
    feedback::register(registry);
    flow::register(registry);
    mobile::register(registry);
    queries::register(registry);
}

#[cfg(test)]
mod tests;
