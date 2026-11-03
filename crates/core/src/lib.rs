//! The store data of a local Shopify store, and what the storefront and the app around it both
//! need of it: lsf-theme renders a theme with it, lsf-app runs an app against it. Nothing here
//! renders Liquid.
//!
//! - [`store`] loads and validates the store data (products, collections, customers, ...).
//! - [`session`] is a visitor's state, and [`cart`] resolves its cart lines against the store.
//! - [`theme`] is a theme as files on disk, which the storefront serves and the Admin API edits.
//! - [`color`], [`money`] and [`urls`] are the values both sides print.

pub mod cart;
pub mod color;
pub mod diagnostics;
pub mod error;
pub mod json;
pub mod money;
pub mod session;
pub mod store;
pub mod theme;
pub mod urls;
pub mod util;

pub use error::{Error, Result};
pub use session::Session;
pub use store::Store;
