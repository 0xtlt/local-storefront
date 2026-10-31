//! One module per family of resources: its seed, its custom resolvers, its mutations and its
//! webhook payloads. See `crates/admin/docs/design.md`.

pub mod app;
pub mod catalog;
pub mod content;
pub mod customers;
pub mod orders;
