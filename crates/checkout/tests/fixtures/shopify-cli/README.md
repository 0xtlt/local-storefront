# Modules built by Shopify's CLI

The playground's two Functions written in JavaScript and TypeScript, built by Shopify's CLI 5.0.0
(esbuild 0.28.1, Javy 9.1.0, Shopify's Javy plugin 4) from the root of an app, as
`shopify app deploy` builds them:

- `discount-typescript.wasm`: `examples/checkout-playground/extensions/discount-typescript`.
- `validation-javascript.wasm`: `examples/checkout-playground/extensions/validation-javascript`.
- `ui-preact.js` and `ui-preact-typescript.js`: the bundles `shopify app build` writes for the
  playground's checkout UI extensions built from their sources. `assets/tests/built-extensions.test.mjs`
  draws each one next to lsf's bundle and compares.

`crates/checkout/src/javascript/tests.rs` builds the same Functions with lsf and checks that its
modules do what these do: the same output, the same logs, as many instructions within 1%.
They are not the same bytes: lsf's JavaScript is not esbuild's to the character (the module
holds it, compiled and compressed), and the CLI's own bytes depend on the folder it runs in
(esbuild writes the paths of the modules in comments).

After a change to those Functions, or for another version of the CLI:

```bash
crates/checkout/tests/fixtures/shopify-cli/regenerate.sh 5.0.0
```
