# Javy fixtures

What Javy 9.1.0 writes (the Javy of Shopify's CLI 4.9.5) for some JavaScript, to check that
`lsf_checkout::javascript::javy` writes the same bytes.

- `discount.js`: the JavaScript Shopify's CLI 4.9.5 bundles for the playground's
  `discount-typescript` (`dist/function.js` of `shopify app function build`), and
  `discount.wasm`: its module, `javy build -C dynamic -C plugin=shopify_functions_javy_v4.wasm
  -C wit=javy-world.wit -C wit-world=shopify-function`, with the world the CLI writes:

  ```wit
  package function:impl;

  world shopify-function {
    export %cart-lines-discounts-generate-run: func();
  }
  ```

  It is the module the CLI writes: its `wasm-opt` pass changes nothing in it.
- `three-exports.js` and `three-exports.wasm`: three exports, in the world's order:
  `beta-fetch`, `alpha-run`, `gamma-run`.
- `no-export.wasm`: `three-exports.js` built without a world (`_start` only).

To write them again, take `javy-9.1.0` and `shopify_functions_javy_v4.wasm` from the `bin/` of
`@shopify/cli` 4.9.5 (after one `shopify app function build`, which downloads them).
