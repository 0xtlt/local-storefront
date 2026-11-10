//! A local Shopify Admin API: the GraphQL Admin API of a shop seeded from the store data, in the
//! versions of [`version`] (the default one unless told others), the OAuth endpoints and the
//! webhooks of an app, for the backend of the app to be developed and tested offline. `lsf`
//! serves it next to the storefront and the checkout.
//!
//! The design, and what each module owns: `crates/app/docs/admin/design.md`.

pub mod api;
pub mod assets;
pub mod auth;
pub mod domains;
pub mod graphql;
pub mod outbox;
pub mod partner;
pub mod resolve;
pub mod schema;
pub mod search;
pub mod sources;
pub mod state;
pub mod sync;
pub mod throttle;
pub mod version;
pub mod webhooks;
