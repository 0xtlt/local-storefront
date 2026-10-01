//! Renders Shopify themes locally, without calling Shopify.
//!
//! - [`theme`] loads a theme directory (Liquid files, JSON templates, settings, locales).
//! - [`store`] loads and validates the store data (products, collections, customers, ...).
//! - [`drops`], [`filters`] and [`tags`] implement Shopify's Liquid objects, filters and tags.
//! - [`render`] ties it together: a request goes in, a page comes out.

pub mod diagnostics;
pub mod drops;
pub mod error;
pub mod filters;
pub mod fonts;
pub mod images;
pub mod json;
pub mod render;
pub mod site;
pub mod store;
pub mod tags;
pub mod theme;
pub mod urls;
pub mod util;

pub use error::{Error, Result};
pub use render::{Rendered, Renderer, Target, environment};
pub use site::{FormResult, Request, Session, Site};
pub use store::Store;
pub use theme::Theme;
