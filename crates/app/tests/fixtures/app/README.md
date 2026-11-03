# Fixture app

A Shopify app for the tests of `lsf-checkout`: five Functions written with the `shopify_function`
crate, each configured by the JSON metafield `$app:config.settings` of the object that owns it.

| Extension          | API version | Target                                                                | What it does                                                              |
| ------------------ | ----------- | --------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `volume-discount`  | 2026-10     | `cart.lines.discounts.generate.run`, `cart.delivery-options.discounts.generate.run` | percentage off lines with enough items, off the order, free shipping |
| `delivery-rules`   | 2026-10     | `cart.delivery-options.transform.run`                                 | hides, renames and moves delivery options                                 |
| `payment-rules`    | 2027-01     | `cart.payment-methods.transform.run`                                  | hides and renames payment methods                                         |
| `quantity-limit`   | 2027-01     | `purchase.validation.run` (legacy: still complete in both versions)   | errors on too many items per line and on a blocked email domain           |
| `bundle-transform` | 2026-10     | `cart.transform.run`                                                  | runs the `update`, `expand` and `merge` operations its configuration lists |

The two versions have the same Function schemas (`crates/checkout/docs/api-versions.md`), so there is one module per
Function: the app mixes the versions, and `tests/versions.rs` moves the whole app to each version.

The built modules (`extensions/*/dist/function.wasm`) are committed. After changing a Function:

```bash
mise run fixtures:functions
```

It copies the schema of each Function (its `api_version` and Function API, from
`crates/checkout/schemas`, not committed here), builds for `wasm32-unknown-unknown` and links each
module with `shopify-function-trampoline`, as `shopify app build` does.
