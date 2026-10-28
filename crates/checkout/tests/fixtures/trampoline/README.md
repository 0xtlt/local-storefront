# A Rust Function before shopify-function-trampoline

`quantity-limit.raw.wasm` is what `cargo build --release --target wasm32-unknown-unknown`
writes for the test app's `quantity-limit` (`crates/checkout/tests/fixtures/app`): a module that
calls the Function Wasm API by name (`shopify_function_v2::shopify_function_input_get`, …).

Shopify's `shopify-function-trampoline` 2.0.1 (the binary Shopify's CLI 5.0.0 downloads) turns
it into `crates/checkout/tests/fixtures/app/extensions/quantity-limit/dist/function.wasm`, to
the byte. `lsf_checkout::trampoline` must write the same (`src/trampoline.rs`).

To write it again: `cargo build --manifest-path crates/checkout/tests/fixtures/app/Cargo.toml
--release --target wasm32-unknown-unknown -p quantity-limit`, then copy
`target/wasm32-unknown-unknown/release/quantity_limit.wasm` here.
