# Fixture app

A Shopify app for the tests of `lsf-checkout`: five Functions written with the `shopify_function`
crate, each configured by the JSON metafield `$app:config.settings` of the object that owns it.

| Extension          | Target                                                                | What it does                                                              |
| ------------------ | --------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `volume-discount`  | `cart.lines.discounts.generate.run`, `cart.delivery-options.discounts.generate.run` | percentage off lines with enough items, off the order, free shipping |
| `delivery-rules`   | `purchase.delivery-customization.run`                                 | hides, renames and moves delivery options                                 |
| `payment-rules`    | `purchase.payment-customization.run`                                  | hides and renames payment methods                                         |
| `quantity-limit`   | `purchase.validation.run`                                             | errors on too many items per line and on a blocked email domain           |
| `bundle-transform` | `cart.transform.run`                                                  | runs the `update`, `expand` and `merge` operations its configuration lists |

The built modules (`extensions/*/dist/function.wasm`) are committed. After changing a Function:

```bash
mise run fixtures:functions
```

It downloads Shopify's Function API schemas (not committed), builds for `wasm32-unknown-unknown`
and links each module with `shopify-function-trampoline`, as `shopify app build` does.
