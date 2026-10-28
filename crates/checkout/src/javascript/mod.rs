//! Builds a Function written in JavaScript or TypeScript into its Wasm module, as
//! `shopify app function build` does, without Node: the GraphQL typegen ([`typegen`]), one
//! JavaScript file for the Function and what it imports, then the module that runs it on
//! Shopify's Javy plugin.

pub mod bundle;
pub mod javy;
pub mod resolve;
pub mod typegen;
