# Checkout playground

A Shopify app made to try `lsf checkout` on: many Functions and checkout UI extensions, and a
scenario for each case. Nothing here is deployed.

```bash
cd examples/checkout-playground
lsf checkout --app .
```

Or, from the root of the repository, `mise run playground:checkout`. Open
<http://127.0.0.1:9292/>: the home page lists what is wrong first (the broken extensions, and the
Functions that fail when their scenario runs), then every scenario and every extension, and opens
a checkout for each. The data folder is found from the current
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
ones, so that none of them breaks, and the everyday checkout UI extensions (no `ui-broken-*`, and
none that holds Pay).

```bash
lsf function run --app . --scenario fn-discount-codes
lsf function run --app . --scenario fn-validation-street-number --step interaction
```

### Network access

`discount-network` and `validation-network` have a `.fetch` target: the checkout sends the HTTP
request they ask for, to `http://localhost:8787`. Start the mock server for real answers (no
dependency, Node only; it listens on the loopback addresses only):

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

To change a Function and see it at once, `mise run playground:dev` (`lsf checkout --app .
--watch`) builds every Function, then builds one again when you save it and reloads the open
pages ([watch mode](../../docs/checkout.md#watch-mode)). It rewrites the committed modules: a
build on another machine or toolchain does not give the same bytes, so
`git checkout -- examples/checkout-playground` puts them back.

### Tests

`crates/cli/src/commands/function/playground.rs` checks that the store data and every scenario
are valid, that the `lsf checkout init` scenarios run no broken Function, and what each `fn-*`
scenario comes to (the Functions that ran, a discount, a hidden option, an error message). It
needs neither the network nor the mock server, and does not care what listens on port 8787: it
runs on a copy of the store data whose network Functions call a closed port (502), or a small
server of the test (its answers):

```bash
cargo test -p lsf playground
```

## Functions and checkout UI extensions together

The `mix-*` scenarios run Functions and checkout UI extensions on the same checkout:

| Scenario | What runs |
|---|---|
| `mix-bundles-upsell` | the cart transform from line properties and a tag discount, with `ui-line-properties`, `ui-upsell`, `ui-line-quantity` and `ui-order-summary` |
| `mix-b2b` | a B2B contract price, the payment methods of a company and net-30 terms, a minimum order, with `ui-checkout-inspector` and `ui-payment-notes` |
| `mix-gated-order` | codes, a minimum order and a quantity cap, with `ui-reductions-codes` and the two extensions that hold Pay (`ui-terms-gate`, `ui-cart-limit-gate`) |
| `mix-france` | a French buyer: the Europe market discount, express hidden, French delivery titles, and the extensions in French |
| `mix-everything` | every Function of `fn-everything` and every extension of `ui-everything` |

`ui-discount-codes` runs `discount-codes` too, so that the codes the extension enters apply.

## Checkout UI extensions

The `extensions/ui-*` folders are checkout UI extensions. Each bundle, `dist/<handle>.js`, is
written by hand: a plain script that registers its targets with `shopify.extend` and draws
Polaris web components (`s-banner`, `s-stack`, ...) into `document.body`. There is no build
step and no source to rebuild from: edit the bundle and reload the page. Each bundle starts with
a comment that says what it shows. Every one has a scenario, `shopify-local/checkouts/ui-*.json`
(open `/checkout?scenario=<name>`; add `&targets=all` to see every target, the thank-you page
included).

Only Polaris web components are used: the local checkout runs API 2026-10 and 2027-01, and
React and remote-ui extensions stopped at 2025-07 (`ui-broken-old-api` shows the refusal).

| Extension | API | Targets | What it shows | Scenario |
|---|---|---|---|---|
| `ui-header-notice` | 2026-10 | `checkout.header.render-after` | A banner with the shop's name and the distance to free shipping, live | `ui-static-targets` |
| `ui-contact-newsletter` | 2027-01 | `checkout.contact.render-after` | Who gets the confirmation (`buyerIdentity`); a newsletter checkbox that writes a cart attribute | `ui-static-targets`, `ui-address` |
| `ui-address-autofill` | 2026-10 | `checkout.delivery-address.render-before`, `-after` | The country of the checkout; buttons that fill the address, change only the ZIP, or move it to France (`applyShippingAddressChange`) | `ui-address` |
| `ui-shipping-options` | 2027-01 | `checkout.shipping-option-list.render-before`, `-after`, `shipping-option-item.render-after`, `shipping-option-item.details.render` | The options, their costs and the selected one | `ui-static-targets` |
| `ui-payment-notes` | 2026-10 | `checkout.payment-method-list.render-before`, `-after` | The payment options and their types, the selected one | `ui-static-targets` |
| `ui-reductions-codes` | 2027-01 | `checkout.reductions.render-before`, `-after` | The codes entered, each removable; buttons that enter codes (`applyDiscountCodeChange`) | `ui-discount-codes` |
| `ui-line-properties` | 2026-10 | `checkout.cart-line-item.render-after` | Under each line: SKU, options, line item properties, selling plan, discounts | `ui-static-targets` |
| `ui-line-quantity` | 2027-01 | `checkout.cart-line-item.render-after` | −, + and Remove under each line (`applyCartLinesChange`) | `ui-cart-edits`, `ui-buyer-journey` |
| `ui-order-summary` | 2026-10 | `checkout.cart-line-list.render-after` | Lines, units and the four amounts of `shopify.cost` | `ui-static-targets` |
| `ui-footer-links` | 2026-10 | `checkout.footer.render-after` | Links (opened in a new tab) and `shopify.extension` | `ui-static-targets` |
| `ui-thank-you` | 2027-01 | the 7 `thank-you.*` targets | The order confirmation, the customer, the lines; a closable announcement | `ui-thank-you` |
| `ui-every-target` | 2026-10 | the 24 targets the page renders | A badge naming each target: a map of the page | `ui-every-target` |
| `ui-gift-wrap` | 2027-01 | `checkout.block.render` | A checkbox that adds the Gift Wrap line and a `gift_wrap` attribute, and takes them back | `ui-cart-edits` |
| `ui-upsell` | 2026-10 | `checkout.block.render` | Three offers: one added, one sold out, one unknown variant (both refused, with the message) | `ui-cart-edits` |
| `ui-note-attributes` | 2026-10 | `checkout.block.render` | Edits the cart note and two attributes; lists the attributes, each removable | `ui-note-attributes` |
| `ui-metafield-writer` | 2027-01 | `checkout.block.render` | Writes and removes a cart metafield declared with `owner_type = "CART"` | `ui-metafields` |
| `ui-metafield-legacy` | 2026-10 | `checkout.block.render` | Counts clicks with the older `updateMetafield`, declared without `owner_type` | `ui-metafields` |
| `ui-terms-gate` | 2027-01 | `checkout.actions.render-before` | A terms checkbox; `buyerJourney.intercept` blocks Pay until it is ticked (`block_progress`) | `ui-buyer-journey` |
| `ui-cart-limit-gate` | 2026-10 | `checkout.block.render` | Blocks Pay while the cart has more than five units (`block_progress`) | `ui-buyer-journey` |
| `ui-intercept-no-capability` | 2026-10 | `checkout.block.render` | An interceptor that answers "block" without `block_progress`: ignored | `ui-buyer-journey` |
| `ui-checkout-inspector` | 2026-10 | `checkout.block.render` | Everything the `shopify` global exposes, live: buyer, localization, cost, discounts, delivery, lines, attributes, metafields... | `ui-inspector`, `ui-inspector-b2b` |
| `ui-i18n-greeting` | 2027-01 | `checkout.block.render` | `i18n.translate` from `locales/en.default.json` and `locales/fr.json`, plurals, a fallback, a missing key; money, number and date formats | `ui-i18n`, `ui-i18n-fr` |
| `ui-settings-empty` | 2026-10 | `checkout.block.render` | Three declared settings; `shopify.settings.value` is `{}` locally, so the defaults show | `ui-settings` |
| `ui-network-fetch` | 2027-01 | `checkout.block.render` | `fetch` to `http://localhost:8787/` (`network_access`), or a warning when the fake API is down | `ui-capabilities` |
| `ui-api-access` | 2026-10 | `checkout.block.render` | `query` rejected (no Storefront API locally), the session token's claims, `storage`, `analytics` | `ui-capabilities` |
| `ui-change-refusals` | 2026-10 | `checkout.block.render` | One button per change the checkout refuses, and its answer | `ui-change-refusals` |

`ui-everything` runs all of the above at once, and `ui-shop-look` a set of them in the Shop
look. The upsell and gift wrap add the products `shopify-local/products/ui-*.json`, whose
variant ids are fixed (`9100000011`, `9100000021`, `9100000031`).

The `ui-broken-*` extensions fail on purpose. They are in no scenario but their own:

| Extension | API | What goes wrong | Scenario |
|---|---|---|---|
| `ui-broken-throws-on-load` | 2026-10 | The bundle throws before `shopify.extend`: nothing is registered | `ui-broken-runtime` |
| `ui-broken-throws-on-render` | 2026-10 | Throws as soon as its target runs | `ui-broken-runtime` |
| `ui-broken-throws-on-event` | 2027-01 | Throws in a click handler, rejects a promise, throws in a signal subscriber | `ui-broken-runtime` |
| `ui-broken-renders-nothing` | 2027-01 | Draws nothing: a frame 0px tall | `ui-broken-runtime` |
| `ui-broken-unregistered-target` | 2026-10 | Registers the footer target while its toml declares the block one | `ui-broken-runtime` |
| `ui-broken-slow` | 2026-10 | Renders after 4 s; its interceptor answers after 7 s, past the 5 s the checkout waits | `ui-broken-runtime` |
| `ui-broken-unknown-target` | 2026-10 | Declares `purchase.checkout.nowhere.render` and `chat.render` (no place) next to the block | `ui-broken-loader` |
| `ui-broken-old-api` | 2025-07 | A remote-ui bundle on an API version that is not supported | `ui-broken-loader` |
| `ui-broken-no-api-version` | none | No `api_version` | `ui-broken-loader` |
| `ui-broken-not-built` | 2026-10 | No `dist/` bundle | `ui-broken-loader` |
| `ui-broken-missing-owner-type` | 2027-01 | A metafield without the `owner_type` 2027-01 requires | `ui-broken-loader` |
| `ui-broken-bad-owner-type` | 2027-01 | `owner_type = "ORDER"`, not one of the seven | `ui-broken-loader` |
| `ui-broken-tall` | 2026-10 | 150 rows, far taller than the window | `ui-broken-layout` |
| `ui-broken-long-text` | 2026-10 | Long unbroken words, URLs and codes in a banner, badge, button, field, link and under each line | `ui-broken-layout` |

A scenario without an `extensions` list runs every extension of the app, the broken ones
included. `crates/checkout/assets/tests/playground.test.mjs` (`mise run test:js`) checks that
every bundle registers the targets of its toml and that the working ones draw without an error.
