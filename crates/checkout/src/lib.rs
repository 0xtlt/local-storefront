//! A local Shopify checkout: loads a Shopify app, runs its Functions against the cart of a
//! session and describes the checkout the buyer sees, with the checkout UI extensions to show.
//!
//! The flow: [`app::load`] reads the app, [`pipeline::compute`] builds each Function's input
//! ([`input`]), runs it ([`runner`]) and applies its output to the checkout.

pub mod app;
pub mod assets;
pub mod input;
pub mod pipeline;
pub mod runner;
pub mod schema;
pub mod types;
pub mod version;

pub use types::*;
