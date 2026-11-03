//! A Shopify app, run locally against the store data of lsf-core: the checkout its Functions and
//! UI extensions make, and the Admin API and webhooks its backend talks to.
//!
//! - [`config`] reads the app: `shopify.app.toml`, its extensions and the API versions they use.
//! - [`checkout`] runs the Functions against the cart of a session and describes the checkout
//!   the buyer sees, with the checkout and customer account UI extensions to show.
//! - [`admin`] serves the GraphQL Admin API of a shop seeded from the store data, the OAuth
//!   endpoints and the webhooks of the app, and syncs what the backend changes into the store.

pub mod admin;
pub mod checkout;
pub mod config;
