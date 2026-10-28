# Checkout playground

A Shopify app made to try `lsf checkout` on: many Functions and checkout UI extensions, and a
scenario for each case. Nothing here is deployed.

```bash
cd examples/checkout-playground
lsf checkout --app .
```

Or, from the root of the repository, `mise run playground:checkout`. Open
<http://127.0.0.1:9292/>: the home page lists every extension (with what is wrong with it) and
every scenario, and opens a checkout for each. The data folder is found from the current
directory, so run lsf from this folder.

The store data (`shopify-local/`) is the demo store that `lsf checkout init` writes, with what
the Functions need on top:

- products: a Limited Edition Print (tag `limited`), a Gift Box (tag `gift-box`), a Weekend
  Bundle (its components in the metafield `custom.bundle_components`) and a Cold-Weather Set (the
  parent of a merge); the metafields `custom.member_price` (tee, overshirt) and
  `custom.weekend_sale` (overshirt, beanie);
- the shop metafield `custom.weekend_sale_settings`, Germany as a country, and a market `europe`
  (France and Germany);
- a `checkout` section in `store.json`: four shipping rates (standard, express, local delivery,
  store pickup), seven payment methods (two of them express buttons), every discount (automatic
  and with a code) and the settings of every other Function.

The customers are the demo store's: `jane.doe@example.com` (tag `vip`), `alex.morgan@example.com`
(B2B, Northwind Hotels), `camille.martin@example.com` (France), `new.customer@example.com`.

## Functions

50 Functions, one Rust crate each (`extensions/<handle>`), written with the `shopify_function`
crate. The comment at the top of each `src/lib.rs` says what it does and what it shows. They are
configured the way Shopify's are, by the JSON metafield `$app:config.settings` of the discount or
the customization that owns them: the `checkout` section of `shopify-local/store.json` holds all of
them.

| Handle | API | Version | What it shows | Scenario |
|---|---|---|---|---|
| `discount-tag-percentage` | discount | 2026-10 | % off lines by product tag or collection; input query variables | `fn-discount-tag-percentage` |
| `discount-order-fixed` | discount | 2027-01 | fixed amount off the order above a subtotal | `fn-discount-order-fixed` |
| `discount-buy-two-get-one` | discount | 2026-10 | buy 2 get 1 free: 100% off part of a line | `fn-discount-buy-two-get-one` |
| `discount-free-shipping` | discount | 2027-01 | free shipping above a subtotal | `fn-discount-free-shipping` |
| `discount-vip-customer` | discount | 2026-10 | % off the order for a customer tag (`vip`) | `fn-discount-vip-customer` |
| `discount-b2b-location` | discount | 2027-01 | % off for a B2B company location | `fn-discount-b2b-location` |
| `discount-by-market` | discount | 2026-10 | % off by country, else by market | `fn-discount-by-market` |
| `discount-codes` | discount | 2027-01 | code discounts; a code that no discount has | `fn-discount-codes` |
| `discount-from-metafields` | discount | 2026-10 | settings read from shop and product metafields | `fn-discount-from-metafields` |
| `discount-lines-and-shipping` | discount | 2027-01 | one Function, lines and shipping targets | `fn-discount-lines-and-shipping` |
| `discount-legacy-product` | product discount | 2026-10 | legacy product discount API | `fn-discount-legacy-product` |
| `discount-legacy-order` | order discount | 2027-01 | legacy order discount API | `fn-discount-legacy-order` |
| `discount-legacy-shipping` | shipping discount | 2026-10 | legacy shipping discount API | `fn-discount-legacy-shipping` |
| `discount-shipping-amount` | discount | 2027-01 | two shipping discounts: one is not applied | `fn-discount-not-applied` |
| `discount-network` | discount | 2026-10 | network access: `.fetch` then `.run` | `fn-discount-network` |
| `delivery-hide-express` | delivery customization | 2026-10 | hide express for countries and PO boxes | `fn-delivery-hide-express` |
| `delivery-rename-by-language` | delivery customization | 2027-01 | rename options in the buyer's language | `fn-delivery-rename-by-language` |
| `delivery-reorder` | delivery customization | 2026-10 | move store pickup first in Canada | `fn-delivery-reorder` |
| `delivery-legacy` | delivery customization | 2027-01 | legacy target, which takes no operation | `fn-delivery-legacy` |
| `payment-hide-cod` | payment customization | 2026-10 | hide cash on delivery above an amount | `fn-payment-hide-cod` |
| `payment-hide-for-b2b` | payment customization | 2027-01 | hide methods for B2B, the invoice for others | `fn-payment-hide-for-b2b` |
| `payment-rename` | payment customization | 2026-10 | rename payment methods | `fn-payment-rename` |
| `payment-reorder` | payment customization | 2027-01 | move payment methods to the top | `fn-payment-reorder` |
| `payment-legacy` | payment customization | 2026-10 | legacy target: payment terms for B2B | `fn-payment-legacy` |
| `validation-minimum-order` | validation | 2026-10 | minimum order amount | `fn-validation-minimum-order` |
| `validation-max-quantity` | validation | 2027-01 | max units of a tagged product, error on the line | `fn-validation-max-quantity` |
| `validation-blocked-country` | validation | 2026-10 | blocked shipping country | `fn-validation-blocked-country` |
| `validation-gift-note` | validation | 2027-01 | a cart attribute required by a product | `fn-validation-gift-note` |
| `validation-street-number` | validation | 2026-10 | an error at `CHECKOUT_COMPLETION` only | `fn-validation-street-number` |
| `validation-legacy` | validation | 2027-01 | legacy validation target | `fn-validation-legacy` |
| `validation-network` | validation | 2027-01 | network access: a stock check | `fn-validation-network` |
| `cart-transform-expand` | cart transform | 2026-10 | `lineExpand` of a bundle, from a product metafield | `fn-cart-transform-expand` |
| `cart-transform-merge` | cart transform | 2027-01 | `linesMerge` into a set, 10% off | `fn-cart-transform-merge` |
| `cart-transform-update` | cart transform | 2026-10 | `lineUpdate`: member price and title | `fn-cart-transform-update` |
| `cart-transform-properties` | cart transform | 2027-01 | update, expand and merge from line properties | `fn-cart-transform-properties` |
| `cart-transform-legacy` | cart transform | 2026-10 | legacy target: `update` to a free gift box | `fn-cart-transform-legacy` |
| `broken-panic` | validation | 2026-10 | a panic: the Wasm module traps | `fn-broken-panic` |
| `broken-instruction-limit` | payment customization | 2027-01 | past the 11 million instructions | `fn-broken-instruction-limit` |
| `broken-bad-output` | delivery customization | 2026-10 | an output out of the schema | `fn-broken-bad-output` |
| `broken-logs` | validation | 2027-01 | logs on a success | `fn-broken-logs` |
| `broken-huge-output` | delivery customization | 2026-10 | 2,000 operations | `fn-broken-huge-output` |
| `broken-missing-build` | validation | 2027-01 | no `dist/function.wasm` | `fn-broken-missing-build` |
| `broken-unknown-target` | validation | 2026-10 | a target no API has | `fn-broken-unknown-target` |
| `broken-api-version` | payment customization | 2025-07 | `api_version` 2025-07, not supported | `fn-broken-api-version` |
| `broken-big-input` | validation | 2027-01 | an input query that asks for a lot | `fn-broken-big-input` |
| `broken-fetch-timeout` | validation | 2026-10 | a request out of the limits: the `.fetch` fails | `fn-broken-fetch-timeout` |
| `listed-fulfillment-constraints` | fulfillment constraints | 2026-10 | fulfillment constraints: listed, not run | `fn-listed` |
| `listed-order-routing` | order routing | 2027-01 | order routing: listed, not run | `fn-listed` |
| `listed-pickup-points` | pickup point generator | 2027-01 | pickup points, with `.fetch`: listed, not run | `fn-listed` |
| `listed-local-pickup` | local pickup generator | 2026-10 | local pickup: listed, not run | `fn-listed` |

- `broken-*` are edge cases: failures (a trap, too many instructions, a bad output, no build, an
  unsupported version, a request out of the limits) and cases that work but stress the checkout
  (logs, a huge output, a big input, an unknown target next to a known one).
- `listed-*` are the Function APIs the local checkout lists and does not run. lsf has no schema for
  them, so they are written without the typegen.
- The legacy `purchase.*` targets are there for every API that has one: they still run, and the
  home page says which target replaces them.

### Scenarios

Each case has a scenario, `shopify-local/checkouts/fn-<case>.json`, whose `extensions` runs only
the Functions of that case. `fn-everything` runs every Function that is not `broken-*` or
`listed-*`. The scenarios of `lsf checkout init` (`guest`, `customer`, `b2b`, `abroad`,
`subscription`, `pre-order`, `shop-look`, `large-cart`) run the same Functions without the network
ones, so that none of them breaks.

```bash
lsf function run --app . --scenario fn-discount-codes
lsf function run --app . --scenario fn-validation-street-number --step interaction
```

### Network access

`discount-network` and `validation-network` have a `.fetch` target: the checkout sends the HTTP
request they ask for, to `http://localhost:8787`. Start the mock server for real answers (no
dependency, Node only):

```bash
node tools/fake-api.mjs        # or, from the root: mise run playground:api
```

It answers a 12% partner price (15% from 100.00) and says that the slate mug is out of stock.
Without it the requests answer `502`, as on Shopify when the connection fails: the discount does
not apply and the stock check lets the order through, and both say so in their logs.
`broken-fetch-timeout` never sends its request: its timeout is out of Shopify's limits.

### Rebuild

The built modules (`extensions/*/dist/function.wasm`) are committed. After changing a Function:

```bash
mise run playground:functions                                         # all of them
tools/checkout/build_functions.sh examples/checkout-playground discount-codes  # one, from the root
```

It copies the schema of each Function (its `api_version` and Function API, from
`crates/checkout/schemas`, not committed here), builds the Cargo workspace of this folder for
`wasm32-unknown-unknown` and links each module with `shopify-function-trampoline`, as `shopify
app build` does. `lsf checkout --app . --build` runs the same command for each Function.
`broken-missing-build` has no build command and stays unbuilt.

### Tests

`crates/cli/src/commands/function/playground.rs` checks that the store data and every scenario
are valid, that the `lsf checkout init` scenarios run no broken Function, and what each `fn-*`
scenario comes to (the Functions that ran, a discount, a hidden option, an error message). It
needs neither the network nor the mock server:

```bash
cargo test -p lsf playground
```
