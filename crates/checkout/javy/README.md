# Shopify's Javy plugin

`shopify_functions_javy_v4.wasm` is the plugin that JavaScript Functions built with
`@shopify/shopify_function` 2.x import: QuickJS, and the `ShopifyFunction` global the Functions
read their input from and write their output to. lsf compiles JavaScript to QuickJS bytecode
with it when it builds a Function (`src/javascript/javy.rs`).

It is the copy function-runner v9.2.2 links when it runs such a module (its `providers/`,
Apache-2.0), so that a module runs on the plugin that compiled it. The one Shopify's CLI
downloads (`https://cdn.shopify.com/shopifycloud/shopify-functions-javy-plugin/shopify_functions_javy_v4.wasm`)
has the same code; only its `git_hash` custom section differs.

To update it, take the file from the `providers/` of the function-runner version
`crates/checkout/Cargo.toml` pins.
