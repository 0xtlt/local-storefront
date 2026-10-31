//! A local Shopify Admin API: the GraphQL Admin API (version 2026-10) of a shop seeded from the
//! store data, the OAuth endpoints and the webhooks of an app, for the backend of the app to be
//! developed and tested offline. `lsf` serves it next to the storefront and the checkout.
//!
//! The design, and what each module owns: `crates/admin/docs/design.md`.

pub mod api;
pub mod auth;
pub mod domains;
pub mod graphql;
pub mod resolve;
pub mod schema;
pub mod search;
pub mod state;
pub mod sync;
pub mod throttle;
pub mod version;
pub mod webhooks;
