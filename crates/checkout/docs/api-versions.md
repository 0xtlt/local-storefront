# API versions of the local checkout: Functions

`lsf-checkout` supports two Shopify API releases: **2026-10** (stable) and **2027-01** (release
candidate, stable on 2027-01-01). A Function declares its release with `api_version` in
`shopify.extension.toml` (`ApiVersion`, `crates/checkout/src/version.rs`); any other value, or none,
is an error diagnostic that names the supported ones, and the Function is not run.

## What differs between 2026-10 and 2027-01

**Nothing, as of the pinned data (2026-10-26): the Function API schemas of the two releases are identical.**
The 2027-01 release candidate is 2026-10 for Functions until Shopify changes it.

How this was established (all three agree):

1. **Schemas.** The npm package `@shopify/dev-mcp@1.17.0` (ISC, Shopify, last modified 2026-10-16) ships the
   introspection of each Function API per release. For the 8 Function APIs the pipeline runs, the
   `2026-10` and `2027-01` files are byte-identical.
2. **Documentation.** `https://shopify.dev/docs/api/functions/{2026-10,2027-01}/<api>.md` (the
   reference, with every type, field, target and example) are identical for the 8 APIs and the index,
   once the version string is replaced.
3. **shopify-dev MCP.** `learn_shopify_api(api: functions, version: …)` and `validate` per version:
   the validator is version aware (`Customer.createdAt` is rejected at 2026-07, accepted at 2026-10
   and 2027-01) and answers the same for 2026-10 and 2027-01.
4. **Changelog.** The release notes of 2027-01 (`/changelog/release-notes/2027-01`) list only
   GraphQL Admin API (automatic discount query, `metafieldInteger` collection condition, market
   currency mutation, metaobject access values, packed dimensions, access scope downgrade) and tax
   webhook changes; they have no Functions section. The Functions section of the 2026-10 notes has
   the two input fields below and app intents for discount apps (not a schema change).

The RC can still change before 2027-01-01. When it does: pin a newer `@shopify/dev-mcp` in
`tools/checkout/function_schemas.mjs`, run `mise run checkout:schemas`, `mise run
fixtures:functions`, and look at `git status crates/checkout/schemas`: a schema that changes in
2027-01 gets its own file in `crates/checkout/schemas/2027-01/`.

### What 2026-10 added to both (against 2026-07)

Both are additive, in all 8 APIs. Both are answered by the input builder.

| Field | Changelog | Local answer |
| --- | --- | --- |
| `Customer.createdAt: DateTime!` (`cart.buyerIdentity.customer`) | 2026-07-30 | the date of the first order of the customer, else the checkout's clock (the store data has no creation date) |
| `CartDeliveryOption.metafield(namespace, key!): Metafield` | 2026-09-15 | always `null` (shipping rates have no metafields) |

2026-07 is not supported, so these are not "version only" fields between the two releases.

## The Function APIs the pipeline runs

Same in 2026-10 and 2027-01. "Legacy" is the first generation of the APIs (`purchase.*` targets,
the `run` export, `FunctionRunResult`). Both generations are in every schema of both releases.

| Pipeline step | Current target → result type | Legacy target → result |
| --- | --- | --- |
| cart transform | `cart.transform.run` → `CartTransformRunResult` `{operations: [{lineExpand \| linesMerge \| lineUpdate}]}` | `purchase.cart-transform.run` → `FunctionRunResult` `{operations: [{expand \| merge \| update}]}` |
| line and order discounts | `cart.lines.discounts.generate.run` → `CartLinesDiscountsGenerateRunResult` `{operations: [{productDiscountsAdd \| orderDiscountsAdd \| enteredDiscountCodesAccept \| enteredDiscountCodesReject}]}` | `purchase.product-discount.run`, `purchase.order-discount.run` → `FunctionRunResult` `{discounts, discountApplicationStrategy}` (deprecated APIs) |
| shipping discounts | `cart.delivery-options.discounts.generate.run` → `CartDeliveryOptionsDiscountsGenerateRunResult` `{operations: [{deliveryDiscountsAdd \| enteredDiscountCodesAccept \| enteredDiscountCodesReject}]}` | `purchase.shipping-discount.run` → `FunctionRunResult` (deprecated API) |
| delivery customization | `cart.delivery-options.transform.run` → `CartDeliveryOptionsTransformRunResult` `{operations: [{deliveryOptionHide \| deliveryOptionMove \| deliveryOptionRename}]}` | `purchase.delivery-customization.run` → `FunctionRunResult` `{operations: [DeprecatedOperation]}`: **`DeprecatedOperation` has no field**, a legacy delivery customization cannot return any operation |
| payment customization | `cart.payment-methods.transform.run` → `CartPaymentMethodsTransformRunResult` `{operations: [{paymentMethodHide \| paymentMethodMove \| paymentMethodRename \| paymentTermsSet \| orderReviewAdd}]}` | `purchase.payment-customization.run` → `FunctionRunResult` `{operations: [{paymentTermsSet}]}`: **only payment terms** (no hide, move, rename) |
| validation | `cart.validations.generate.run` → `CartValidationsGenerateRunResult` `{operations: [{validationAdd: {errors: [{message, target}]}}]}` | `purchase.validation.run` → `FunctionRunResult` `{errors: [{localizedMessage, target}]}` (complete) |

Operation fields that the pipeline reads: `deliveryOptionHandle`, `index`, `title` (delivery);
`paymentMethodId`, `index`, `name`, `placements` (payment: a hide for `ACCELERATED_CHECKOUT` only keeps
the method in the list); the discount `candidates` (targets, value, message, associated code,
selection strategy).

The legacy delivery and payment state is the same in 2026-01, 2026-04 and 2026-07 (the oldest
releases in the package), so "removed legacy operations" is not a 2026-10 → 2027-01 change either.
No `purchase.*` target is dropped by 2027-01: the three legacy discount APIs, and the legacy
`run` of every API, are still in its schemas. The documentation pages of the legacy discount APIs
still list `purchase.*-discount.run`, flagged "deprecated".

### Targets that are not run

The `.fetch` targets of the pickup point and local pickup generators, fulfillment constraints (`cart.fulfillment-constraints.generate.run`), order routing
(`cart.fulfillment-groups.location-rankings.generate.run`), pickup point and local pickup delivery option
generators (both only exist in `unstable`). Listed, not run, as before.

### Targets with network access

`cart.lines.discounts.generate.fetch`, `cart.delivery-options.discounts.generate.fetch` and
`cart.validations.generate.fetch` are in the 2026-10 and 2027-01 schemas (`MutationRoot` has their
`…GenerateFetch` field, `Input` has `fetchResult` in the discount and validation APIs only) and are run.
The two schemas are identical there. The delivery customization, payment customization, cart transform
and the three legacy discount APIs have no `fetch` target.

### Documentation quirks, for the next reader

- `…/functions/<version>/cart-checkout-validation` says "not supported in <version>", the real page is
  `…/cart-and-checkout-validation`. The schema name in the tooling is `cart_checkout_validation`.
- `…/delivery-customization` shows a comment `purchase.delivery-customization.run` above a
  `cart_delivery_options_transform_run` function: the example is current-generation.
- The docs add `input_variables` per `[[extensions.targeting]]` and deprecate
  `[extensions.input.variables]`. It is configuration, not an API version difference; the loader still
  reads only the deprecated form.

## What is version aware now

- **`ApiVersion`** (`version.rs`): the supported releases (the `releases` of
  `crates/checkout/assets/api.json`, which the page reads too), and the schema
  of each Function API (`FunctionApi`) per release, embedded with `include_str!` (`SCHEMA_FILES`).
  Releases that share a schema share its file and its parsed `Schema`. `target_spec` maps a target
  to its API and result type. Adding a release = one entry in `api.json`, plus a `SCHEMA_FILES` line for
  each schema file the generator wrote for it (a test checks the list against the folder).
- **Schemas** (`crates/checkout/schemas/<since>/<api>.graphql`, SDL, generated, committed): the
  schema of an API from release `<since>` on, until a newer file of the same API. A release gets a
  file only when its schema differs from the release before; the first line of a file lists the
  releases that use it. Today every file is in `2026-10/` and serves 2026-10 and 2027-01. From
  the `@shopify/dev-mcp@1.17.0` tarball pinned by SHA-256, by `tools/checkout/function_schemas.mjs`
  (`mise run checkout:schemas`), which compares the SDL of consecutive releases without the
  header. `tools/checkout/build_functions.sh` copies to a Function the file of the newest release
  at or before its `api_version`. Differences from Shopify's own `schema.graphql` files: no
  descriptions; `@oneOf` is rebuilt (the introspection has none: an input object with only optional
  fields, minus the `*FetchResult` types and 3 plain ones); an input object without field (`DeprecatedOperation` of the
  delivery API) gets a `_noField` placeholder.
- **Input** (`input::build`): the query is checked against the schema of the Function's version and
  API before it runs: unknown field, unknown argument, missing required argument, selection on a
  scalar, missing selection, unknown or impossible type condition, undefined fragment. The error says
  which version and API, the path of the field, and the fields the type has.
- **Output** (`pipeline::execute`): the output must fit the result type of its target in that schema
  (`Schema::validate_output`): no unknown field, no missing required field, scalars, enums, `@oneOf`
  with exactly one field. A mismatch fails the run (blocking checkout error, `FunctionRun.error`).
  Applying the operations then follows the current-generation names.
- **Loader**: a Warning for a legacy target says what to use instead and what the release still takes from it
  (`API version 2026-10 takes no operation from it`).

## Gaps

- Checkout UI extensions are gated on `api_version` (2026-10 and 2027-01 only, plus the `owner_type`
  rule of 2027-01: see `ui-api-versions.md`), but their API, the `shopify` global of the extension
  host (`crates/checkout/assets`), has one code path: the two versions have the same surface.
- The `Customer.createdAt` and delivery option metafield answers are placeholders.
- `orderReviewAdd` and `paymentTermsSet` (payment) are accepted and not applied.
- No real "version-only" field exists between the two releases, so the proof of "a field missing in
  one version is rejected" is a pair of small schemas in `schema/tests.rs`, and the rejection of fields
  no supported version has (`tests/versions.rs`).
- `@oneOf` is a heuristic of the generator (see above); a Shopify type that is all-optional and not
  `@oneOf` needs an entry in `NOT_ONE_OF` in `function_schemas.mjs` (the Rust fixtures compile against
  these files, which is how a wrong guess shows up).
- The per-target `input_variables` of the extension file is not read.
