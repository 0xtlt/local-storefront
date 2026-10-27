# Design of the local checkout

## Goal

`lsf` gets a fake Shopify checkout (standard one-page checkout, and a "Shop" skin that looks
like the Shop app / Shop Pay checkout) that runs the **Shopify Functions** and renders the
**checkout UI extensions** of a Shopify **app** directory (not the theme). One or several
extensions at the same time. No Shopify store, no network.

## User-facing surface (CLI, owned by the integration step)

```bash
lsf checkout --app ../my-app [--extension HANDLE]... [--variant standard|shop] [--port 9292]
#   serves the checkout alone (no theme needed), with the session's cart from the store data
lsf serve --app ../my-app [--extension HANDLE]...
#   the theme's /checkout leads to the same checkout
lsf function run [HANDLE]... --app ../my-app [--session FILE|JSON] [--checkout FILE|JSON] [--format text|json]
#   one-shot, no server: builds the input of each selected Function from the store data,
#   runs them in checkout order, prints each input/output/logs and the resulting checkout
```

No `--extension`: every extension of the app is enabled. `--build` runs each Function's
`[extensions.build] command` before starting.

Store data gets a `checkout` section (model in `lsf-core` `store/model.rs`, validated and in
the JSON Schema) mapped to `lsf_checkout::CheckoutConfig`:

```json
"checkout": {
  "shipping_rates": [{"handle": "standard", "title": "Standard", "price": "4.90"}],
  "payment_methods": [{"handle": "card", "name": "Credit card"}, {"handle": "cod", "name": "Cash on Delivery"}],
  "discounts": [{"function": "volume-discount", "title": "Volume", "code": null,
                  "classes": ["PRODUCT"], "metafields": {"$app:config.settings": {"type": "json", "value": {...}}}}],
  "functions": {"delivery-rules": {"metafields": {...}}},
  "tax_rate": 20
}
```

Defaults when absent: one shipping rate "Standard" 5.00, payment methods "Credit card",
"Cash on Delivery", "Bank Deposit", no tax.

## App layout read by `lsf_checkout::app`

- `shopify.app.toml` (or `shopify.app.*.toml`) at the root: `name`, `client_id`, `handle`.
- `extensions/*/shopify.extension.toml` (also the legacy `[[extensions]]` array form):
  - Function: `type = "function"`, `api_version`, `[[extensions.targeting]] target, input_query, export`,
    `[extensions.build] command, path` (default wasm path `dist/function.wasm`),
    `[extensions.input.variables] namespace, key`.
  - Checkout UI extension: `type = "ui_extension"`, `[[extensions.targeting]] target, module`,
    `[extensions.capabilities]`, `[extensions.settings]`. Its built bundle is
    `dist/<handle>.js` (what `shopify app build` writes). Missing bundle → a diagnostic saying
    to run `shopify app build`.
  - Other types: listed, ignored.

## Function execution (`runner`)

Use Shopify's own `function-runner` as a library (git tag v9.2.2, Apache-2.0), which supports
the WASI stdin/stdout ABI, the Shopify Function Wasm API (`shopify_function_v1/v2` providers)
and Javy (JS Functions). If it cannot be used as a library, embed wasmtime and its providers
the same way. Cache compiled modules by path + mtime + size so a rebuild is picked up on the
next run. Report logs, instructions, errors.

Findings from the implementation:

- function-runner v9.2.2 builds and runs as a library (`function_runner::engine::run`, with
  wasmtime 39 and the providers embedded). It enables `serde_json/arbitrary_precision` for the
  whole build: the workspace tests pass with it.
- It does not stop a Function (fuel is `u64::MAX`, the epoch never moves): `runner` moves the
  engine's epoch after 10 seconds.
- A Rust Function built for `wasm32-unknown-unknown` (`shopify_function` 2.x, Wasm API) only runs
  once `shopify-function-trampoline` linked it to the provider; the Shopify CLI does it when it
  builds an app. An unlinked module fails with "unknown import", so `runner` says so instead.
  `mise run fixtures:functions` does it for the test Functions.
- Failures come back as one string: function-runner appends the trap or exit code to the logs.
  `runner` splits them again (a Rust panic message becomes the error).

## API versions

Two releases are supported, 2026-10 and 2027-01 (RC): see `api-versions.md`. A Function's
input query and output are checked against the schema of its `api_version`.

## Function input (`input`)

`input_query` (GraphQL) is executed against the local checkout: a GraphQL executor over a
resolver of the Function input graph (`cart { lines { id quantity cost {...} merchandise
{ __typename ... on ProductVariant { id sku title inAnyCollection(ids:) inCollections(ids:) product { id handle title productType vendor
hasAnyTag(tags:) hasTags(tags:) inAnyCollection(ids:) inCollections(ids:) metafield(namespace:,key:) } metafield(...) } } attribute(key:) }
cost buyerIdentity { email phone customer {...} } deliveryGroups { ... deliveryOptions { handle title cost ... } }
attribute(key:) }`, `discount { discountClasses metafield }`, `deliveryCustomization { metafield }`,
`paymentCustomization`, `validation`, `cartTransform`, `paymentMethods { id name }`,
`localization { country { isoCode } language { isoCode } market {...} }`, `shop { localTime ... metafield }`,
`presentmentCurrencyRate`, `enteredDiscountCodes { code rejectable }`, `triggeringDiscountCode`,
aliases, fragments, variables from `[extensions.input.variables]`). Money in input is a
decimal string (`"19.99"`), ids are GIDs, as Shopify sends them.
An argument the query leaves out (or gives a variable without a value) is worth the default of
the schema of its API version: `hasAnyTag` without `tags` tests the empty list.

## Pipeline (`pipeline::compute`)

```rust
pub fn compute(app: &App, enabled: &Selection, store: &lsf_core::Store,
               session: &lsf_core::Session, state: &CheckoutState, config: &CheckoutConfig) -> CheckoutResult
```

Order (as Shopify): cart transform (`cart.transform.run`) → line/order discounts
(`cart.lines.discounts.generate.run`, legacy `purchase.product-discount.run`,
`purchase.order-discount.run`) → delivery groups from `config.shipping_rates` → delivery
customization (`cart.delivery-options.transform.run`, legacy `purchase.delivery-customization.run`)
→ shipping discounts (`cart.delivery-options.discounts.generate.run`, legacy
`purchase.shipping-discount.run`) → payment customization (`cart.payment-methods.transform.run`,
legacy `purchase.payment-customization.run`) → validation (`cart.validations.generate.run`,
legacy `purchase.validation.run`) → cost. Out of scope (listed, not run): fulfillment
constraints, order routing, pickup point/local pickup generators, the `.fetch` targets, discount
`conditions` and combinations (all discounts stack).

The lines start as the cart lines of the session, priced by `lsf_core::drops::cart::resolve_line`:
the same `ResolvedLine` the storefront cart (`/cart.js`, the `cart` drop) is computed from, so that
the selling plan, the quantity price breaks and the compare-at price of a line cannot differ
between the cart and the checkout. `lsf-checkout` reads them from it and keeps no copy of that logic.

The delivery group is `group-1`. Outputs name a delivery group or a payment method by its
handle or by a gid ending with it, so `input` must give them an `id` of that form. A Function
whose run fails adds a blocking `CheckoutError`; a discount whose Function is not in the app
too.

## Server routes (integration)

- `GET /checkout` (and theme's cart form) → redirect `/checkouts/cn/<token>`.
- `GET /checkouts/cn/<token>` → HTML shell: `crates/checkout/assets/checkout.html` with
  `<!--LSF_STATE-->` replaced by the state JSON (escaped for a `<script type="application/json"
  id="lsf-checkout-state">`), `<!--LSF_ASSETS-->` by the asset base `/__lsf/checkout/assets`.
  `?variant=shop` picks the Shop skin.
- `GET /checkouts/cn/<token>.json` → state JSON.
- `POST /checkouts/cn/<token>/update` body `{"operations": [Op...]}` → 200 + new state JSON
  (400 + `{"error": "..."}` for a bad operation).
- `POST /checkouts/cn/<token>/complete` → 200 `{"redirect": ".../thank-you"}` or 422 + state with `errors`.
- `GET /checkouts/cn/<token>/thank-you` → same shell, `status: "completed"`.
- `GET /__lsf/checkout/assets/<file>` → files of `crates/checkout/assets/` (embedded).
- `GET /__lsf/extensions/<handle>/<file>` → the built files of a UI extension (`dist/`).
- `GET /__lsf/checkout?session=<id>` (control API) → state JSON, for e2e tests.

Operations (`type` field):

```json
{"type": "email", "value": "a@b.c"}
{"type": "phone", "value": "+33..."}
{"type": "shippingAddress", "address": {"firstName": "...", "countryCode": "FR", ...}}
{"type": "deliveryOption", "group": "<group handle>", "handle": "<option handle>"}
{"type": "paymentMethod", "handle": "card"}
{"type": "discountCode", "action": "add"|"remove", "code": "SAVE10"}
{"type": "cartLines", "change": <CartLineChange of the UI extension API: addCartLine/removeCartLine/updateCartLine>}
{"type": "attribute", "change": {"type": "updateAttribute"|"removeAttribute", "key": "...", "value": "..."}}
{"type": "note", "change": {"type": "updateNote"|"removeNote", "note": "..."}}
{"type": "metafield", "change": {"type": "updateMetafield"|"removeMetafield"|"updateCartMetafield"|"removeCartMetafield", "namespace": "...", "key": "...", "valueType": "string", "value": "..."}}
```

Cart lines, note and attributes live in the session cart (the same one the theme shows).

## State JSON (server → page)

`CheckoutResult` serialized (see `crates/checkout/src/types.rs`) plus:

```json
{
  "token": "…", "variant": "standard"|"shop", "status": "open"|"completed", "orderGeneration": 0,
  "shop": {"name": "…", "currencyCode": "EUR", "moneyFormat": "€{{amount}}", "locale": "en", "storefrontUrl": "http://127.0.0.1:9292"},
  "buyer": {"email": "…", "phone": "…", "customer": {"id": "gid://shopify/Customer/1", "email": "…", "firstName": "…", "lastName": "…"} | null},
  "shippingAddress": {…} | null,
  "note": "…" | null, "attributes": [{"key": "…", "value": "…"}],
  "metafields": [{"namespace","key","type","value"}],
  "lines": […], "discounts": […], "discountCodes": […], "deliveryGroups": […],
  "paymentMethods": […], "selectedPayment": "card" | null, "errors": […], "cost": {…}, "runs": […],
  "extensions": [{"handle": "…", "apiVersion": "2026-10", "script": "/__lsf/extensions/<handle>/<handle>.js",
                  "targets": ["purchase.checkout.block.render", …], "capabilities": {…},
                  "settings": {…}, "metafields": [{"namespace": "…", "key": "…", "ownerType": "CART"}]}],
  "diagnostics": ["…"]   // app problems: missing builds, unknown targets
}
```

## Page and extension host (`crates/checkout/assets/`)

- `checkout.html`, `checkout.css`, `checkout.js`: renders the page from the state JSON (no
  framework, ES modules, no build step), sends operations, re-renders from the reply.
  Standard skin = Shopify one-page checkout (contact, delivery address, shipping method,
  payment, order summary with discount field, "Pay now"). Shop skin (`data-variant="shop"`):
  Shop app / Shop Pay look. A collapsible "Functions" panel shows each run (input, output,
  logs, instructions, error). Thank-you page when `status == "completed"`.
- `extension-host.js` + `sandbox.html` + `sandbox.js`: each (extension, target) renders in a
  sandboxed iframe (`sandbox="allow-scripts"`), placed at its target in the page
  (`purchase.checkout.block.render` in the main column, `…delivery-address.render-before`,
  `…shipping-option-list.render-after`, `…payment-method-list.render-after`,
  `…cart-line-list.render-after`, `…cart-line-item.render-after` per line,
  `…reductions.render-before/after`, `…contact.render-after`, `…actions.render-before`,
  `purchase.thank-you.block.render` and the other `purchase.thank-you.*` targets).
  In the iframe: a global `shopify` implementing the checkout UI extension API from the state
  (signals with `.value`/`.subscribe`, `apply*Change` → postMessage → operation,
  `buyerJourney.intercept`, `i18n`, `settings`, `extension`, `storage`, `sessionToken`,
  `query` (rejects: no Storefront API)), `shopify.extend(target, fn)` registration, Polaris
  web components (`s-banner`, `s-stack`, `s-text`, `s-button`, `s-text-field`, …) defined
  with `customElements.define`. The iframe resizes to its content.
- `storage` is scoped as Shopify's Storage API is: to the checkout (`token` and `orderGeneration`,
  the number of orders the session placed before it, as the token is the one of the cart; the
  thank-you page of an order has a scope of its own), then to the extension, shared by its
  targets and not with the other extensions.
- `api-versions.js`: the API releases the host runs, `2026-10` and the `2027-01` release
  candidate; an extension on another version, or declaring its metafields against the rules of
  its version, is not loaded and the page shows why. See `ui-api-versions.md`.

## Contract between `pipeline` and `input`

```rust
// crates/checkout/src/input.rs (owned by the input agent; the engine agent stubs it)
pub struct InputContext<'a> {
    pub store: &'a lsf_core::Store,
    pub session: &'a lsf_core::Session,          // customer_id, country, cart note/attributes
    pub state: &'a crate::CheckoutState,          // email, phone, address, codes, metafields
    pub lines: &'a [crate::ResultLine],           // the lines as they are at this step
    pub delivery_groups: &'a [crate::DeliveryGroup],
    pub payment_methods: &'a [crate::PaymentMethod],
    pub owner: Owner<'a>,                         // the object that owns the Function
    pub entered_codes: &'a [String],
    pub triggering_code: Option<&'a str>,
    pub currency: &'a str,
    pub now: chrono::DateTime<chrono::FixedOffset>,
}
pub struct Owner<'a> {
    pub kind: OwnerKind,                          // which root field holds it
    pub id: String,                               // gid, e.g. gid://shopify/DiscountAutomaticNode/1
    pub metafields: &'a [crate::Metafield],
    pub discount_classes: &'a [crate::DiscountClass],
}
pub enum OwnerKind { Discount, DiscountNode, CartTransform, DeliveryCustomization, PaymentCustomization, Validation }
/// Runs `query` (the Function's input query) against the checkout, with `variables` from
/// `[extensions.input.variables]` (the JSON value of that owner metafield, or none).
pub fn build(query: &str, variables: Option<&serde_json::Value>, ctx: &InputContext) -> Result<serde_json::Value, String>;
```
