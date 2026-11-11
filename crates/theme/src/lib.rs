//! Renders Shopify themes locally, without calling Shopify, from the store data of lsf-core.
//!
//! - [`theme`] loads a theme directory (Liquid files, JSON templates, settings, locales).
//! - [`drops`], [`filters`] and [`tags`] implement Shopify's Liquid objects, filters and tags.
//! - [`render`] ties it together: a request goes in, a page comes out.

pub mod app_extensions;
pub mod drops;
pub mod filters;
pub mod fonts;
pub mod images;
pub mod previews;
pub mod render;
pub mod site;
pub mod tags;
pub mod theme;
pub mod urls;

pub use previews::Preview;
pub use render::{Rendered, Renderer, Target, environment, environment_with};
pub use site::{Request, Site};
pub use theme::Theme;
