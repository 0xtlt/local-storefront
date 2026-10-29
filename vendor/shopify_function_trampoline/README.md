# shopify-function-trampoline 2.0.1

The library of Shopify's `shopify-function-trampoline` (MIT, `LICENSE.md`), from
https://github.com/Shopify/shopify-function-wasm-api, tag `shopify_function_trampoline/v2.0.1`:
the version Shopify's CLI 5.0.0 runs. lsf runs it on a Rust Function's module after its build
command (`crates/checkout/src/trampoline.rs`).

`src/lib.rs` is the one of that tag, with one change, marked `lsf:`. `Cargo.toml` differs:

- walrus 0.24.5 instead of 0.23.3, and wasmparser 0.240 instead of 0.238: they read and write
  Wasm with the wasmparser and wasm-encoder wasmtime and lsf use. walrus 0.23 brought a second
  copy of each in lsf's binary, and the trampoline a third wasmparser (about 1 MB of code).
- No clap: the command line (`src/main.rs`) is not here.

walrus writes the module the same way; it only names itself, with its version, in the producers
section. The `lsf:` change names walrus 0.23.3 there, as the binary Shopify's CLI runs does, so
that the module lsf writes is that binary's, to the byte (`crates/checkout/src/trampoline.rs`
tests it).

To update it, take `trampoline/src/lib.rs` of the new tag, keep the `lsf:` change with the walrus
of that binary, and run the tests of `lsf-checkout`.
