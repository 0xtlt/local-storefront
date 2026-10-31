# Design of the local Admin API

## Goal

The backend of a Shopify app (a React Router or Remix app, Rails, Laravel, Express, Django, Go,
…) talks to Shopify through the **GraphQL Admin API** and receives **webhooks**. `lsf` serves
a local Admin API, **API version 2026-10 only**, backed by the store data, and delivers the
webhooks of the app to its backend, signed as Shopify signs them. A developer runs and tests
their backend offline: no development store, no tunnel, no network, the same data every run,
and the failures that are hard to provoke on a real store (throttling, a missing access scope, a
webhook delivered twice) on demand.

It is to the backend what `lsf app dev` is to the checkout: the same store data, the same app
directory (`shopify.app.toml`), the same server. A product the backend creates through the Admin
API is a product of the store; an order placed in the local checkout is an order the Admin API
lists and the `orders/create` webhook reports.

Not a goal: the REST Admin API (deprecated by Shopify; `/admin/api/<v>/*.json` other than
`graphql.json` answers that it is not served), the Storefront API, the Customer Account API,
other API versions (2027-01 comes later, the way the checkout added it), the admin web UI.

## User-facing surface

```bash
lsf app dev --app ../my-app --app-url http://localhost:3000
#   serves the checkout, the customer account, and now the Admin API of the store:
#   POST http://127.0.0.1:9292/admin/api/2026-10/graphql.json
#   and delivers the app's webhooks to http://localhost:3000/<uri of each subscription>
lsf serve --app ../my-app            # the same, with a theme
lsf app webhook trigger --topic orders/create --app ../my-app [--address URL] [--api-version 2026-10]
#   as `shopify app webhook trigger`: one webhook, built from the store data, signed, sent
```

The server prints, when it starts, the Admin API URL and an access token that works at once:

```text
Admin API  http://127.0.0.1:9292/admin/api/2026-10/graphql.json
           X-Shopify-Access-Token: shpat_…   (shop: my-shop.myshopify.com)
Webhooks   3 subscriptions of shopify.app.toml → http://localhost:3000
```

| Option | What it does |
|---|---|
| `--app-url <url>` (`LSF_APP_URL`) | Where the backend of the app listens. Relative webhook `uri`s are joined to it, and an absolute one on the origin of `application_url` is moved to it. Without it, `application_url` is used. |
| `--client-secret <secret>` (`LSF_CLIENT_SECRET`) | The secret webhooks, OAuth and session tokens are signed with. Default: `SHOPIFY_API_SECRET` of the environment, else of the app's `.env`, else a fixed local secret printed at start. |
| `--admin-throttle <plan>` (`LSF_ADMIN_THROTTLE`) | The cost bucket: `store` (the default: Plus for store data with companies, as the seed's `plan.shopifyPlus`, Standard otherwise), a plan (`standard`, `advanced`, `plus`, `enterprise`, `legacy`), or `off`. |

The client id is `client_id` of `shopify.app.toml` (a fixed local one without an app). The
Admin API is served whenever the server runs; the webhooks, the scopes and the client id come
from the app when there is one (`--app`).

## The HTTP protocol, as Shopify's

- `POST /admin/api/2026-10/graphql.json` with `{"query", "variables", "operationName"}`
  (`application/json`), or the query alone (`application/graphql`). `GET` with `?query=` too.
- Authentication: `X-Shopify-Access-Token`. Missing or unknown: `401`
  `{"errors":"[API] Invalid API key or access token (unrecognized login or wrong password)"}`.
- Another version (`2026-07`, `2027-01`, `unstable`): `404` with a JSON message saying lsf serves
  2026-10 only. Unlike Shopify, which serves the oldest supported version instead: a local tool
  says what it does not do.
- Response headers: `X-Shopify-API-Version: 2026-10`, `X-Request-Id`, JSON content type.
- Errors in the format of Shopify (graphql-ruby): parse errors, validation errors with
  `locations`, `path` and `extensions.code` (`undefinedField`, `argumentNotAccepted`,
  `missingRequiredArguments`, …), variable errors with `extensions.problems`, field errors with
  `path`, `ACCESS_DENIED`, `THROTTLED`, `MAX_COST_EXCEEDED`, `NOT_FOUND`. The messages are
  graphql-ruby's and Shopify's, word for word where we can know them.
- `extensions.cost` on every answer that ran, or that was refused for its cost or by the
  throttle: `requestedQueryCost`, `actualQueryCost`, `throttleStatus` (`maximumAvailable`,
  `currentlyAvailable`, `restoreRate`), computed as Shopify documents it (scalars and enums 0,
  objects 1, connections 2 + `first`/`last` × the cost of a node, a mutation 10), a leaky bucket
  per shop and app. A query over 1000 points is refused (`MAX_COST_EXCEEDED`), an empty bucket
  answers `THROTTLED`. `--admin-throttle off` (or the store data) turns it off; the plan sets the
  size of the bucket. A request that does not parse or validate, or whose variables are wrong,
  answers its errors alone (no `data`, no `extensions`): graphql-ruby does not cost a query it
  does not run. See "The GraphQL engine" below.
- `extensions.lsf` (only lsf adds it, and only when there is something to say): the fields that
  were answered with a default because lsf does not simulate them (`defaulted`: their paths), so
  that a developer never mistakes a default for data.

## The GraphQL engine (`schema`, `graphql`, `throttle`, `version`)

`graphql` is a GraphQL server over a `Resolver` trait; it knows the schema, not the data. The
resolver of the records (`resolve`) implements it; the HTTP side (`api`) calls
`graphql::run(schema, resolver, request, Some(bucket))` and sends `Response::to_json()`.

```rust
pub trait Resolver {
    type Object: Clone;                              // an opaque handle on an object
    fn query_root(&mut self) -> Self::Object;
    fn mutation_root(&mut self) -> Self::Object;     // its fields run one after the other
    fn type_name(&self, object: &Self::Object) -> String; // asked for interface and union fields
    fn field(&mut self, ctx: &FieldContext<'_>, object: &Self::Object)
        -> Result<Output<Self::Object>, FieldError>;
}
pub enum Output<O> { Null, Value(serde_json::Value), Object(O), List(Vec<Output<O>>) }
pub struct FieldError { pub message: String, pub extensions: Option<Map<String, Value>> }
```

`FieldContext` gives the parent type (always an object type), the field name, its definition
and type (`ty()`), the response key and path, the arguments (`arguments()`, `arg_str`, …:
coerced, the schema's defaults filled in, enum values as names, a single value given for a list
as a list of one, `Decimal`/`Money`/`BigInt`/`UnsignedInt64` as strings), the directives of the
field (`directive("idempotent")`), and a lookahead (`selection().field("edges").field("node")
.names()`). The executor completes what the resolver answers: a `null` in a non-null field goes
up to the nearest nullable one with graphql-ruby's error, an enum value must be one of the enum,
`__typename` and introspection are answered without the resolver. A `FieldError` is the field's
`null` and an error with its `locations` and `path`. Scalars are passed as the resolver gives
them.

Stages, for a caller that needs them apart: `graphql::prepare` (parse, select the operation,
validate, coerce the variables) then `Prepared::requested_cost` and `Prepared::execute`.

### Schema

`tools/admin/admin_schema.mjs` (`mise run admin:schema`) writes `schemas/2026-10/admin.graphql`
from the introspection of the pinned `@shopify/dev-mcp@1.17.0` (SHA-256 checked): every
description, deprecation reason, default value, `@oneOf`, and the six directives the schema
declares (`@idempotent`, `@oneOf`, `@include`, `@skip`, `@deprecated`, `@specifiedBy`), in the
order of Shopify's introspection. The introspection types (`__Schema`, …) are in
`introspection.graphql`: names starting with `__` are not valid SDL. A description is written as
a block string when graphql-parser reads it back the same, as a quoted string otherwise (4 of
them).

The SDL is 3,982,241 bytes. Embedded with `include_str!`, it would be 4 MB of the binary; the
generator also writes `admin.graphql.br` (Brotli, quality 11, 393,867 bytes), which the crate
embeds and decompresses on first use: 0.4 MB of the binary. A Brotli step in a `build.rs` was
rejected: build scripts run unoptimized, where quality 11 takes 46 s for this file (quality 9:
1.8 s for 440 KB), and the build would compile `brotli` a second time for the host. A test checks
that the `.br` is the `.graphql`. `schema::admin()` parses it once (`OnceLock`): 24 ms in
release on an Apple M4 Pro (decompression about 5 ms, graphql-parser about 18 ms, the
model the rest); `prepare` and the requested cost of a typical query take about 5 µs.

Shopify's introspection lists four possible types that the schema does not define (`ExchangeV2`
under `Node`, three user errors under `DisplayableError`). graphql-js's `buildClientSchema`
refuses a schema that names a type it lacks, so lsf's `possibleTypes` are the object types that
declare the interface. Everything else is Shopify's, which the tests check: graphql-js's
`getIntrospectionQuery()` (descriptions, `specifiedByURL`, `isOneOf`, `isRepeatable`, input value
deprecation) is compared with the dev-mcp introspection for a few types, and
`LSF_ADMIN_INTROSPECTION=<file> cargo test -p lsf-admin --test introspection` compares all 3699
(`node tools/admin/admin_schema.mjs --introspection <file>` writes the file).

### Parsing and validation

Queries have a parser of their own (`graphql::parse`), not graphql-parser's: graphql-parser 0.4
keeps the fields of an input object in a `BTreeMap` (sorted, duplicates lost, so
`inputFieldNotUnique` could not be reported) and has no position for arguments and values, which
graphql-ruby's errors point at. graphql-parser still reads the SDL. Parse errors are those of
graphql-ruby's C parser, which keeps the messages of its Racc grammar:
`Parse error on "}" (RCURLY) at [1, 9]` with the token's location, `Unexpected end of document`
with `"locations": []`. A document nested more than 100 deep (selections, values or the type of
a variable, `[[Int]]`; fragments included) answers graphql-ruby's `This query is too large to
execute.`, rather than running out of stack. Variables are JSON, which `serde_json` reads 128
deep at most (deeper, the body is not read: `400`), so their coercion recurses no further.

Validation is graphql-ruby 2.6's `StaticValidation::ALL_RULES` (read in its source, tag of
2026-10-30), as one walk of the document, each rule's errors in its order, with its messages,
`extensions` and `path` (`["query Q", "products", "first"]`); the rules that need the whole
document report at the end, in the reverse order of the list, as graphql-ruby's do. Where
graphql-ruby keeps a legacy behaviour behind a schema setting whose value on Shopify is unknown
(`allow_legacy_invalid_*`), lsf follows the spec: a union without selections, fields that cannot
merge and a variable used in a fragment where its type is not allowed are refused. graphql-ruby
adds "Did you mean" suggestions when the `did_you_mean` gem is loaded; Shopify's messages have
none, nor do lsf's. The operation is selected before validation, as graphql-ruby does:
`An operation name is required`, `No operation named "X"`.

### Variables and scalars

Variables are checked and coerced as graphql-ruby's `Query::Variables`:
`Variable $id of type ID! was provided invalid value` (followed by ` for path (explanation), …`
for the problems inside the value), `locations` at the variable, `extensions.value` and
`extensions.problems` (`path`, `explanation`, and `message` for a scalar's own error). What each
scalar accepts is in the doc comment of `graphql/input.rs`. An `ID` must be a GID; one that is
not fails as Shopify's does, ``Invalid global id `123` `` (the format of the Storefront API's
answers, from the same graphql-ruby stack: the Admin API's exact wording is not published).

### Answers

`{"errors": [...], "data": ..., "extensions": {"cost": ...}}`: `errors` first, as graphql-ruby
builds the hash, and only when there are some. A field error: `message`, `locations`, `path`,
`extensions`. Operation, parse, validation and variable errors: the errors alone.

### Cost

As shopify.dev's "GraphQL Admin API rate limits" (read in 2026-10, with its versions of 2023 to
2025 on the Web Archive, and Shopify's engineering post on query complexity): scalars and enums 0,
objects 1, a connection 2 + `first` (or `last`) × one node (edges, `cursor` and `pageInfo` are
free), an interface or a union the most of its possible types, a mutation 10 plus its payload.
`products(first: 1) { edges { node { title } } }` costs 3, shopify.dev's example. The actual cost
is what was returned: the nodes a connection really has, nothing for a `null` object, 10 for a
mutation that answered `null` (a forum answer of Shopify shows requested 72, actual 10 for a
`productCreate` that failed). lsf's choices where shopify.dev is silent: a list that is not a
connection costs one item; a connection given neither `first` nor `last` costs 2; introspection
and `__typename` are free; no field has a manual cost.

Over 1000 points (`cost::MAX_COST`), the query does not run: the error of the Admin API
reference's sample, word for word (`Query cost is 2003, which exceeds the single query max cost
limit (1000).` and the links that follow), with `extensions` `code: MAX_COST_EXCEEDED`, `cost`,
`maxCost`, `documentation`, and `actualQueryCost: null`.

### Throttle

A leaky bucket per shop and app (`throttle::key(shop, app)`): a query runs only if the bucket has
its requested cost, which it takes; afterwards the bucket gets back requested − actual. Refused:
`{"message": "Throttled", "extensions": {"code": "THROTTLED", "documentation":
"https://shopify.dev/api/usage/rate-limits"}}`, no `data`, `actualQueryCost: null`, as the Admin
API answers it. `throttleStatus` writes the size and the rate as floats and what is available as
an integer (`{"maximumAvailable": 1000.0, "currentlyAvailable": 997, "restoreRate": 50.0}`, as
shopify.dev's examples). Restore rates by plan are shopify.dev's: Standard 100, Advanced 200, Plus
1000, Enterprise 2000 points a second; bucket sizes are not published, and lsf gives each plan 20
seconds of restore (2000, 4000, 20000, 40000; the stores of the current plans report 2000.0 with
100.0). `Plan::LEGACY` is the 1000/50 of the examples. The clock is injected (`ManualClock` in
tests); `Throttle::set_enabled(false)` turns it off (a full bucket, nothing refused).

### Versions

`version::parse_path` reads `/admin/api/<version>/graphql.json`: `Graphql("2026-10")`,
`UnsupportedVersion(v)` for any other version (and for `/admin/api/graphql.json` without one), `Rest`
for the REST Admin API; `unsupported_version` and `rest_not_served` are the JSON of their 404s.

## Shops, apps and tokens

- **One shop by default**: the store data's (`shop.permanent_domain`, e.g.
  `my-shop.myshopify.com`), on which the app is installed when the server starts, with the
  scopes of `shopify.app.toml` (`[access_scopes] scopes`; every scope when there is no app).
- **More shops for isolation**: `POST /__lsf/admin/shops {"shop": "test-1"}` creates
  `test-1.myshopify.com`, seeded from the same store data, with the app installed, and answers its
  token. Parallel tests each get a shop of their own, as storefront tests each get a session.
  A token belongs to one shop: the token, not the host, picks the shop of a request (the backend
  sends every shop to the same `http://127.0.0.1:9292`).
- **Access tokens** (`auth::tokens`): the one printed at start (`default_token(shop, client_id)`,
  deterministic for a shop and an app, so a test can hard-code it; offline, never expiring), and
  those the OAuth endpoints issue. Every access token is `shpat_` and 32 hex digits: shopify.dev
  ("Access tokens") says offline and online tokens both begin with `shpat_`, "whichever grant
  produced them" (an earlier draft of this design said `shpua_` for online ones). Refresh tokens
  are `shprt_…`. Lifetimes are Shopify's: an expiring offline token an hour, its refresh token 90
  days, an online token and a client credentials token 24 hours (`expires_in` 86399). Refresh
  tokens rotate as Shopify documents: the one presented stays usable until the app uses its
  successor, a new grant is acquired, 30 days pass after its first use, or it expires.
  Uninstalling (`Tokens::uninstall`) revokes every token of the app on the shop.
- **OAuth**, as Shopify (`auth::oauth`): `GET /admin/oauth/authorize` approves at once and
  redirects to the `redirect_uri` with `code`, `hmac`, `host`, `shop`, `state`, `timestamp`;
  `POST /admin/oauth/access_token` (JSON, form, or query parameters) exchanges the code (once,
  within ten minutes), or a session token (token exchange,
  `urn:ietf:params:oauth:grant-type:token-exchange`, offline or online access token, expiring or
  not), or a non-expiring offline token to migrate it to an expiring one, or a refresh token, or
  the client credentials. Every answer is Shopify's JSON, and every error its
  `{"error", "error_description"}` with its status, `X-Lsf-Reason` saying why in plain words.
  - *The shop*: on Shopify it is the host; lsf serves every shop on one address, so the shop of
    an OAuth request is `?shop=`, else a `*.myshopify.com` `Host` or `X-Forwarded-Host`, else the
    default shop. A code, a session token and a refresh token carry their own shop.
  - *`redirect_uri`* must equal one of `[auth] redirect_urls` ("Must exactly match"); one on the
    origin of `application_url` is also accepted on the origin of `--app-url`, as webhooks move
    there. Without an app, any `http(s)` URL is accepted.
  - What shopify.dev documents word for word: a code used twice (`400` `invalid_request` "The
    authorization code was not found or was already used"), a dead refresh token (`401`
    `invalid_request` "This request requires an active refresh_token"), a bad session token
    (`400` `invalid_subject_token`), an unknown client id (`application_cannot_be_found` "Could
    not find Shopify API application with api_key …"), the authorization page's "Oauth error
    invalid_request: The redirect_uri is not whitelisted". Not documented, so lsf's best guess: a
    wrong client secret is `401` `invalid_client` "Client authentication failed" (the mocks of
    `@shopify/shopify-api`'s tests).
  - Client assertions (application-only tokens) are not simulated: lsf cannot verify the app's
    key pair, and says so.
- **Session tokens** (App Bridge's `id_token`, `auth::session_token`): `POST
  /__lsf/admin/session-token` (`{"shop"?, "user"?}`) mints one (HS256 with the client secret,
  the claims of shopify.dev's "ID tokens": `iss`, `dest`, `aud`, `sub`, `exp`, `nbf`, `iat`,
  `jti`, `sid`; no `sig`, which the page no longer lists), so that a test can call the
  authenticated routes of the backend; `GET /__lsf/admin/launch` redirects to the app URL with
  the query an embedded app is opened with (`embedded`, `hmac`, `host`, `id_token`, `locale`,
  `session`, `shop`, `timestamp`), the `hmac` signing every other parameter. Verification allows
  Shopify's libraries' 10-second leeway on `exp` and `nbf`.
- **HMACs** (`auth::signing`): a webhook's `X-Shopify-Hmac-Sha256` (base64 of the body), the
  `hmac` of an admin query (hex, the other parameters sorted and form-encoded as
  `@shopify/shopify-api` and the `shopify_api` gem sign them), and an app proxy's `signature`
  (hex, the sorted `name=value` strings concatenated, repeated names joined with commas),
  checked against shopify.dev's published examples.
- **Access scopes are enforced** (`auth::scopes`): a field or a mutation the token's scopes do not
  cover answers `ACCESS_DENIED` with Shopify's message ("Access denied for products field.
  Required access: `read_products` access scope.") and `extensions` (`code`, `documentation`,
  `requiredAccess`). `write_x` implies `read_x`, `unauthenticated_write_x` implies
  `unauthenticated_read_x`. The table is not hand-written: Shopify's introspection of 2026-10
  carries the `requiredAccess` text of every type and field, which `mise run admin:access-scopes`
  writes into `schemas/2026-10/access.json`; a root field without one requires that of the type
  it answers (`products` → `Product`). Staff permissions, plans and requirements that depend on
  the arguments (`metafieldsSet`, `tagsAdd`) are not checked.

## The state of a shop (`state`)

A shop's state is a set of **records**: JSON objects in the shape of their GraphQL type, keyed by
their GID (`gid://shopify/Product/1`). It is seeded from the `lsf_core::Store` the server
loaded, with the ids lsf-core gave each entity, so that the GIDs of the Admin API are those the
storefront, the checkout and the Function inputs already use.

```json
{
  "__typename": "Product",
  "id": "gid://shopify/Product/1",
  "title": "Organic cotton t-shirt",
  "handle": "organic-cotton-t-shirt",
  "status": "ACTIVE",
  "tags": ["cotton"],
  "createdAt": "2026-01-01T00:00:00Z",
  "variants": ["gid://shopify/ProductVariant/11", "gid://shopify/ProductVariant/12"],
  "priceRangeV2": { "minVariantPrice": { "amount": "19.0", "currencyCode": "EUR" }, "…": "…" }
}
```

- A field holds its GraphQL value: scalars as Shopify writes them (`Money` as `"19.00"`, which
  `ProductVariant.price` reads, `Decimal` as `"19.0"`, which `MoneyV2.amount` reads,
  `UnsignedInt64` as a string of digits, `DateTime` in RFC 3339 with `Z` and no fraction, `Date`
  as `2026-01-31`), enums as their name, embedded objects (`MoneyV2`, `MoneyBag`,
  `MailingAddress`, `SEO`, `Image`, …) as nested objects, references to other records as their
  key (their GID), a connection or a list of records as an array of keys.
- An array of a connection or of a list holds the keys of records, or the objects inline when
  their type is not one the state keeps as records (`Market.regions` of `MarketRegionCountry`,
  `Customer.addressesV2` of `MailingAddress`, `ProductVariant.barcodes`, `Product.images`,
  `DiscountCodeApp.codes`, `InventoryLevel.quantities`): the resolver pages over either, and an
  inline object's cursor is made of its position and its `id` when it has one.
- **Every record has `__typename`** (all of them implement `Node` or another interface).
  Nested objects take their type from the field, or from their own `__typename` when the field
  type is abstract (`SellingPlan.pricingPolicies`, `DiscountAutomaticNode.automaticDiscount`).
- **The key of a record is its `id`**, a GID. Two types have no GID and are kept under another
  key: `ShopifyFunction` (its `id` is a UUID string, the key; `PaymentCustomization
  .shopifyFunction` holds it), `ShopLocale` (no `id`; kept under its locale, `fr`). A field of an
  object type whose value is a string is always the key of a record.
- Records are kept in **creation order**: the order of the store data for the seeded ones, then
  the order the Admin API created the others in (`ShopState::all`, `all_of` for the members of an
  interface or a union). Shopify's root connections sort by `ID` by default (`locations` and
  `markets` by `NAME`, `orders` by `PROCESSED_AT`, `appInstallations` by `INSTALLED_AT`): the
  resolver applies the `sortKey` and its default on top. The arrays of a record are in the order
  Shopify shows them (variants and media by position, the products of a collection in its manual
  order).
- New records get ids from a counter per type that starts above every seeded id
  (`ShopState::new_gid`; the discount nodes share one counter, as they share ids on Shopify).
  `ShopState::touch` sets `updatedAt` (on the records whose type has it) from the clock of the
  shop: the store data's `now` when it has one, frozen there (a test reads the same dates every
  run), the system clock to the second otherwise. Dates the store data does not give are fixed
  (noon on 2024-01-01, the date lsf-core gives undated entities), never the clock, so that the
  same data gives the same records.
- Not stored, derived by the resolver from the record: `legacyResourceId` (the number of the
  GID, a string), `defaultCursor`, and a `Count` field `<x>Count` whose record has the array
  `<x>` (`{count, precision: EXACT}`). A `Count` the data gives without the list behind it is
  stored (`CompanyLocation.ordersCount`).
- Derived values that are stored are recomputed by one function of their domain, which the seed
  and the mutations share: `catalog::refresh_product` (price ranges, `totalInventory`,
  `tracksInventory`, `hasOutOfStockVariants`, `hasOnlyDefaultVariant`, `featuredMedia`, the
  variants' `inventoryQuantity`), `customers::refresh_customer` (`orders`, `numberOfOrders`,
  `amountSpent`, `lastOrder`, `canDelete`), `content::attach_definition` (the seed's: a
  metafield's `definition`, created when there is none, the definition's `metafields` and
  `metafieldsCount`; the mutations link a metafield to an existing definition only, with
  `content::metafields::link_definition`, as Shopify keeps metafields without one).
- URLs are on the shop's primary domain (`https://<shop.domain or permanent_domain>`):
  `onlineStoreUrl` is `https://my-shop.myshopify.com/products/<handle>`, files are
  `https://<domain>/cdn/shop/files/<path>?v=<version>`, the path the storefront serves them at.
- Settings: what the store data says of the shop that no record holds, by name
  (`ShopState::setting`, set by the seed and kept by a reset): the checkout's tax rate
  (`taxRateBps`), which draft orders and order edits compute their tax with.
- Events: `ShopState::emit(topic, gid)` records `{topic, shop, gid, at, id}` (a `…/delete` event
  keeps a copy of the record: emit before removing, or use `ShopState::delete`;
  `ShopState::emit_with` gives the copy, the record before the change for a topic that says
  what changed);
  `take_events` hands them over. Event ids are UUIDs made of the shop and a sequence that a reset
  does not restart, so a receiver never sees one twice.
- **Shops** (`Shops`): the default shop is the store data's (`shop.permanent_domain`); `create`
  makes another from the same seed (cloned, not seeded again) and moves it to its domain: its own
  `Shop` id (`stable_id("shop", domain)`), its `myshopifyDomain`, and every value naming the old
  domain or `Shop` GID rewritten (a custom primary domain stays). `reset` puts every shop back to
  the seed and keeps the shops; `reseed` seeds again from new store data, then resets.
- Store data edited while the server runs (`--watch`) reseeds every shop: what the Admin API
  changed is dropped, and the terminal says so. `DELETE /__lsf/admin` reseeds on demand.

### How a field is resolved (`resolve`)

`resolve::ShopResolver` implements the engine's `Resolver` over one shop, for one request (the
api layer holds the shop's lock meanwhile). Its objects (`resolve::Obj`) are the two roots, a
record by its key, and an inline object with its type (a value nested in a record, an item of an
array of inline objects, a connection and its edges, a `Count`, a mutation payload). For each
field, in order:

1. **Access**: a field the token's scopes do not cover answers `ACCESS_DENIED`
   (`resolve::access`): a root field what `auth::scopes::required_access` says; a nested field
   its own `requiredAccess` text and that of the type it answers (`Order.customer` needs
   `read_customers`), read as a query's, in a payload too; `node` and `nodes` what the type of the
   record they find requires. The requirements are read once per field and kept.
2. **A mutation** runs its registered function (see "Mutations" below); one without answers
   `LSF_NOT_SIMULATED`.
3. **A custom resolver** registered for `(type, field)` answers it (see "Writing a resolver or a
   mutation").
4. **Root conventions**: `node(id:)` and `nodes(ids:)` read any record (aliases followed:
   `DiscountNode/<n>` is the `DiscountAutomaticNode/<n>` or `DiscountCodeNode/<n>`); a connection
   (`products`) pages over every record of its node type, or of every type of an interface
   (`files`); `<x>Count` counts what the connection `<x>` lists, with the same `query`, `AT_LEAST`
   past `limit`; a field answering an object type reads `id:` (`product(id:)`), `handle:` for
   `…ByHandle`, `identifier:` for `…ByIdentifier` (`id`, `handle`, `emailAddress` as `email`,
   `phoneNumber` as `phone`, `customId` as a metafield the record owns, any other field by
   equality); a list of an object type without required arguments lists its records
   (`shopLocales`); a field without required arguments answers the only record of its type
   (`shop`, `app`, `currentAppInstallation`). Otherwise a default.
5. **The object's value**, converted by the field's type: a string for an object type is the key
   of a record (`null` when it is gone, or not of a possible type), an object is an inline object
   (its type the field's, or its `__typename` for an abstract one), an array of a connection is
   paged, an array of a list is mapped (cut to `first` when the list takes one, as
   `Product.options(first:)`). Before the value: `metafield(namespace:, key:)` and
   `metafields(namespace:, keys:)` of any owner read its `metafields`, `$app` and `$app:x` being
   the app's reserved namespaces (`app--<id>`, `app--<id>--x`; `metafield` without a namespace is
   in the app's). After it, when there is none: `legacyResourceId` (the number of the GID),
   `defaultCursor` (the cursor of the record in the `ID` order of its type), and the `Count` of an
   array (`variantsCount` of `variants`, with the field's `query`).
6. **A default**: `null`; for a non-null field an empty list, `""`, `0`, `false`, `"0.00"` for
   `Money` and `"0.0"` for `Decimal`, the seed's fixed date for `DateTime`, the first enum value,
   or an object of defaults (the first possible type of an abstract one). Its path goes into
   `extensions.lsf.defaulted` (the first 50, with `defaultedCount` past them) and its `Type.field`
   into `defaultedFields`, except for lists and connections (empty by nature) and inside what lsf
   builds whole (payloads, objects of defaults). An explicit `null` in a non-null field is a
   default too.

**Connections** (`resolve::connection`): the items (GIDs or inline objects) go through the
field's `query`, its `sortKey`, `reverse`, then `after`/`before`, then `first`/`last`. Shopify's
errors, as the Admin API answers them: neither `first` nor `last` is `you must provide one of
first or last` (quoted from an answer on zenn.dev, with `path` and `locations`), more than 250 is
`first cannot exceed 250. To query larger amounts of data with fewer limits, bulk operations
should be used instead.\nSee https://shopify.dev/api/usage/bulk-operations/queries for usage
details.\n` (an Admin API answer quoted on community.shopify.dev; `last` by analogy), a cursor
lsf did not make is `Malformed cursor` with empty `extensions` (also quoted there); each is a
field error, the connection `null`, even when the record has nothing to page. A cursor is the
base64 of `{"position", "id", "sortKey", "value"}` (`connection::Cursor`): the item's place, its
GID (or an inline object's `id`), and, when the list was sorted, the key and the item's value
under it, as Shopify's cursors hold the sort value and the id of their item. `after` finds its
item by id; when the item is gone (deleted, or filtered out by the `query`), it resumes at the
first item that sorts strictly after the value and the id, in the list's direction (`reverse`
included), and `before` ends at the last one strictly before them: deleting the page just read
does not make the next one skip ahead. A list in its own order (a record's array without
`sortKey`) or sorted by another key than the cursor's has no value to compare: the position,
where the item that took a removed one's place comes next. `pageInfo` is exact both ways.

**Sort keys** (`resolve::sort`): root connections sort by their `sortKey`, its default included
(`products` by `ID`, `orders` by `PROCESSED_AT`), and by `ID` when they take none; a record's
array keeps its order unless the query asks for another key than the default. `ID` and
`RELEVANCE` sort by the number of the GID; another key by the camelCase field of its name
(`TITLE` → `title`, `metaobjects(sortKey: "updated_at")` → `updatedAt`), numbers and decimal
strings by value, text ignoring case, ties by id. A key the type does not have keeps the order,
with a note in `extensions.lsf.notes`. `sort::sort` answers the order it applied
(`sort::Order`: the key, each item's value and id), which the cursors keep.

**Search** (`resolve::search`): the `query` argument is `search::Query`, matched through a
`Searchable` that reads the filters `search.json` documents for the connection, by their
documented type: `id`; `<x>_id` from the GID in `x` or the GIDs in `xs`; `tag` from `tags`
(`tag_not:x` is `NOT tag:x`, for any `<f>_not` whose `<f>` is documented); dates, booleans
(`gift_card` → `giftCard`, else `isGiftCard`) and numbers from the camelCase field; strings from it
or its plural, tokenized for `title`, `name`, `body`, `description`, `note`, `display_name`, whole
otherwise; `metafields.{namespace}.{key}` from the owner's metafield, `fields.{key}` from a
metaobject's field. A free term searches the
title, name, handle, email, vendor, product type, SKU and tags. A field the connection does not
document is ignored with Shopify's warning in `extensions.search` (`path`, `query`, `parsed`,
`warnings`, the keys and order of shopify.dev's example); a documented one lsf cannot read is
ignored with a note in `extensions.lsf.notes`.

### Writing a resolver or a mutation

The domains plug in what the generic rules do not answer through `resolve::Registry`. Each
module of `domains/` has a `register(&mut Registry)`, which `domains::register` calls and
`Registry::standard()` holds. Functions are plain `fn` pointers (a closure that captures nothing
is one): what they need is in what they are given.

```rust
pub fn register(registry: &mut Registry) {
    registry
        .query("productVariantsCount", variants_count)          // a root field (QueryRoot)
        .field("Product", "totalVariants", total_variants)     // any (type, field)
        .mutation("productVariantsBulkCreate", bulk_create)
        .sort_key("Product", "INVENTORY_TOTAL", |r, item| {
            SortValue::of(r.item_map(item).and_then(|product| product.get("totalInventory")))
        })
        .search_field("Product", "sku", |r, _item, product| {
            Some(catalog::gids(product.get("variants")).iter()
                .filter_map(|variant| r.state().get(variant)?.get("sku")?.as_str())
                .map(|sku| SearchValue::Keyword(sku.to_string()))
                .collect())
        })
        .default_text("Product", |_r, product| vec![/* what a free term searches */]);
}
```

A **field resolver** is `fn(&ShopResolver, &FieldContext, &Obj) -> Result<Output<Obj>,
FieldError>`. It reads the shop (`r.state()`, read-only), the arguments (`ctx.arg_str("id")`,
`ctx.arguments()`), the object (`r.map_of(object)`: the record or the inline value) and answers
with the generic tools rather than building outputs by hand:

| Tool | What it gives |
|---|---|
| `r.generic(ctx, object)` | The generic answer: fall back to it for the cases the resolver does not handle. |
| `r.convert(ctx, &value, true)` | A JSON value as the field's type wants it: GIDs as records, objects inline, arrays paged or mapped. |
| `r.connection(ctx, items)` | A page of items (GIDs or inline objects) with the field's `query`, `sortKey`, `reverse`, cursors and Shopify's errors. |
| `r.search(ctx, node_type, items)`, `r.count(ctx, n)` | The items a `query` keeps; a `Count` (`AT_LEAST` past `limit`). |
| `r.reference(gid, type)`, `r.lookup(gid)`, `r.find(type, predicate)`, `r.by_identifier(type, input)` | A record as an object (aliases followed), its key, the first record that matches, the record of an `…Identifier` input. |
| `r.records_of(type)`, `r.items_of(object, field)`, `r.item_map(item)` | The keys of the records of a type (or interface), the array of a field, the record of an item. |
| `r.app_namespace("$app:x")` | `app--<id>--x`. |
| `r.default(ctx, true)` | The default of the field, reported in `extensions.lsf.defaulted`. |
| `r.note("…")` | A line of `extensions.lsf.notes`: what lsf did not simulate. |
| `Obj::record(key)`, `Obj::inline(type, map)`, `Obj::built(type, map)` | An object: a record, an inline value whose missing fields are reported, one lsf builds whole. |

A **mutation** is `fn(&mut Mutation, &FieldContext) -> Result<Value, FieldError>`: it answers its
payload as JSON, record references as GIDs (`json!({"product": gid})`), which the resolver then
resolves like any object; `payload(fields, &errors)` adds `userErrors`, and the framework fills
`shop` when the payload type has it. The rules, which `domains/catalog/products.rs` (the model),
`domains/tags.rs` and `domains/webhook_subscriptions.rs` follow:

1. **Validate against `m.state()`**, collecting `UserError::new(&["title"], "Title can't be
   blank").code("BLANK")` (the `field` path is the input's, the message Shopify's where it is
   known, Rails' wording otherwise; `code` is ignored by error types that have none). Errors: answer
   the payload with its objects `null` and the errors, and change nothing.
2. **Change through `m.state_mut()`**: the first call keeps a copy of the shop, which a payload
   with user errors (`userErrors` or `…UserErrors` not empty) or an error puts back. Validating
   first costs no copy on failure; a mutation that finds a problem halfway can still answer it.
   A mutation that applies what it can and reports the rest (`productVariantsBulkUpdate` with
   `allowPartialUpdates`, `productCreateMedia`) calls `m.keep_changes()`: its user errors then
   leave the changes in place.
3. **Keep derived values right** with the domain's functions the seed uses (`refresh_product`,
   `refresh_customer`; for metafields `content::metafields::set`, which links the definition),
   `touch` what changed (`updatedAt`), and use `new_gid` for new records (or the id lsf-core
   derives, for what store sync writes back without an id: see "Store sync").
4. **Emit the events of what changed**: `state_mut().emit("products/update", &gid)`; a deletion
   with `state_mut().delete(&gid, "products/delete")`, which keeps the record for the payload. A
   change always emits an event: that is also how the api layer knows a shop changed. An event
   about something that is no record (a product on a publication) is
   `emit_with(topic, key, snapshot)`: its key names no record, and the payload is built
   from the snapshot. What a mutation keeps between requests that is not a record (an
   idempotency key and the answer it gave) is `set_memo`/`memo`: copied and put back with the
   shop, not seen by the store sync.
5. **Say what is not simulated**: `m.note("productCreate: lsf does not simulate `media` yet")` for
   an input it ignores; an argument-dependent scope the access table cannot express is checked in
   the mutation (`tagsAdd` needs the write scope of the resource: `access::denied`).

`m.webhooks()` gives the app's webhooks (their registry of subscriptions), `m.grant()` the token's
scopes, `m.api_version()`, `m.now()` and `m.timestamp()` the shop's clock.

**Checking a registry**: `Registry::check(schema)` lists what is registered for a field, a
mutation or a type the schema does not have; a test runs it on `Registry::standard()`.

### Search syntax (`search`)

The `query` argument of connections, and the `filter` of webhook subscriptions, use Shopify's
search syntax (shopify.dev, "Search query syntax"): terms, `field:value`, comparators (`:<`,
`:<=`, `:>`, `:>=`), `AND`, `OR` (which binds tighter: `bob OR norman Shopify` is
`(bob OR norman) AND Shopify`), `NOT`, `-`, parentheses, quoted phrases, `*` prefix and existence
(`field:*`), backslash escapes. Dates are what the reference documents: `now`, a year, a month,
a day, a time with or without a zone, and ranges (`2021-01-01..2021-01-02`), each a period
(`created_at:<=2024` is up to the end of 2024), in the shop's time zone. Shopify documents no
relative date (`-7d`) for search; lsf has none.

`search` parses a query into a tree (`Query::parse`, never an error: a malformed query is read
as far as it goes, with warnings, each given once) and matches a record through the `Searchable` trait, which
each domain implements from the filters its connections document (`documented_filters`, from
the reference of 2026-10: `schemas/2026-10/search.json`). A value is tokenized text, a keyword,
a number, a date, a boolean or an id, which says how `:` compares it. **A field the connection
does not know is ignored**, as shopify.dev says ("If you specify an invalid field, then the query
is ignored and all results are returned"; an earlier draft of this design said it matched
nothing), with the warning Shopify returns in `extensions.search` ("Invalid search field for
this query.", `Query::invalid_fields`). A free term matches the type's default text fields.

Parentheses nest 100 deep at most (`syntax::MAX_DEPTH`: the parser recurses once per `(`, on
the stack of the request's thread, which 20,000 of them overflowed): a deeper group is ignored
with the warning "Parentheses nest more than 100 deep: what they hold is ignored.", and refused
in a filter. A chain of `NOT` and `-` is counted, not nested (`NOT NOT x` is `x`).

A webhook `filter` (`search::Filter`) runs on the payload, as shopify.dev's "Filter webhook
deliveries" says: dotted paths through arrays (`variants.price:>=10.00`), case-sensitive
equality, a missing field or a type mismatch makes its term false (no delivery, unless negated),
tags matched one by one in their string; a filter that does not follow the grammar is refused.

### Mutations (`resolve::mutation`)

A mutation is a function from the shop and its arguments to its payload, run while the shop is
locked. It validates as Shopify does and answers `userErrors` (`field`, `message`, `code`) when
the input is wrong, changes nothing when it fails (the shop is copied on its first change and
put back on a failure), and emits the **events** of what it changed. A mutation lsf does not
simulate answers a GraphQL error with `extensions.code = "LSF_NOT_SIMULATED"`, never a silent
success; `GET /__lsf/admin` lists those it simulates. Simulated so far, as references:
`productCreate`, `productUpdate`, `productDelete`, `tagsAdd`, `tagsRemove`,
`webhookSubscriptionCreate`, `webhookSubscriptionUpdate`, `webhookSubscriptionDelete` (and the
`webhookSubscription`, `webhookSubscriptions`, `webhookSubscriptionsCount` queries). The
families' mutations are listed in the user guide (`docs/admin.md`, "What lsf simulates").

## The HTTP side (`api`)

`api::AdminApi` holds the server's shops, each behind a lock of its own (the seed in
`state::Shops`, which makes new ones with `new_shop` and resets them with `reset_shop`), the
tokens (`auth::Tokens`, the app installed on every shop with its scopes and its default token),
the webhooks (`Webhooks::for_app`, with `search::filter` plugged in as the `PayloadFilter`), the
throttle, and the app (`auth::AppAuth`: the client id, the secret of `auth::client_secret`, the
app URL). `AdminApi::handle(&HttpRequest) -> Option<HttpResponse>` answers:

| Path | What answers |
|---|---|
| `POST\|GET /admin/api/2026-10/graphql.json` | The token (`401` as Shopify, `X-Lsf-Reason` saying why), the GraphQL request (JSON, `application/graphql`, `?query=`; `400` `{"errors": {"query": "Required parameter missing or invalid"}}` without one), then `graphql::run` with the bucket of the shop and app, `X-Shopify-API-Version` and `X-Request-Id`. The events the request emitted are handed to `Webhooks::emit` with a lookup over the shop: the payloads are built then, the deliveries queued for the worker, nothing waits on the backend. |
| other `/admin/api/<v>/…` | `404`: another version, or the REST Admin API, which lsf does not serve. |
| `/admin/oauth/*`, `/__lsf/admin/session-token`, `/__lsf/admin/launch` | `auth::oauth::handle`, with `oauth::Shops` over the shops and the system clock. |
| `/__lsf/admin` (`GET` status, `DELETE` reset), `/__lsf/admin/shops`, `/__lsf/admin/uninstall`, `/__lsf/admin/install` | The control endpoints of `docs/control-api.md`. |
| `/__lsf/admin/charges/<n>` (`GET` the page, `POST …/approve`, `…/decline`), `/__lsf/admin/uploads/<shop>[/<key>]`, `/__lsf/admin/bulk/<shop>/<n>.jsonl` | `api::app`: the merchant's approval of a charge, staged uploads, bulk operations' results; no token, as Shopify's admin page and signed URLs need none from the app. The URLs the mutations answer for them are on the origin the request reached lsf at (`http://<Host>`, `ShopResolver::with_origin`). |
| `/__lsf/webhooks*` | `Webhooks::handle`, the payload of a trigger built from the record named, else the first record of the topic's type (`api::samples`), else lsf's sample of the topic. |

`lsf` builds one in `ServerState` (`crates/cli/src/server/admin.rs`), routes its paths before the
storefront, the checkout and the control API, records the orders `place_order` places
(`AdminApi::record_order`), and reseeds it when the store data reloads (`AdminApi::reseed`:
what the Admin API changed is dropped and the terminal says so; the subscriptions the backend made
stay, as the app's setup of the shop). It turns store sync on (`AdminApi::sync_store`): see
"Server wiring" under "Store sync". `lsf app webhook trigger` builds its payloads with
`api::samples::ShopSamples` over the shop seeded from the store data.

## Records

What the seed (`state::seed`, one module of `domains/` per family) makes of the store data and
the app. `stable_id(kind, key)` is lsf-core's (`lsf_core::util::stable_id`, the hash that gives
the store data its ids); "lsf-core's id" is the id lsf-core gave the entity, the one the
storefront and the checkout use. A field not listed is not set: the resolver gives it a default
and reports it in `extensions.lsf.defaulted`. The values chosen where Shopify has an obvious one
are marked *(chosen)*.

Fidelity: field names, types and enum values were checked against the 2026-10 introspection of
`@shopify/dev-mcp@1.17.0` (SHA-256 f651c221…), and every seeded record of the demo store and of
`tests/fixtures/rich` is checked against the SDL by the shape checker (`tests/support/shape.rs`).
Checked on shopify.dev: `Money` reads `"20.00"` and `MoneyV2.amount` `"10.0"`
(`productVariant` examples), the categories of the metafield types
(`metafieldDefinitionTypes` example), `"discounts"` as the `apiType` of a Function of the
Discount API (`shopifyFunctions` example). Not verified, chosen by analogy: the other `apiType`s
(the Function API names in the plural for the legacy discount APIs: `product_discounts`, …),
`Fulfillment.name` (`#1001-F1`), `Return.name` (`#1001-R1`), the opt-in level of consents.

### Ids

| Record | GID |
|---|---|
| `Shop`, `Product`, `ProductVariant`, `Collection`, `Page`, `Blog`, `Article`, `Comment`, `Customer`, `Company`, `CompanyLocation`, `Location`, `Order`, `GiftCard`, `SellingPlan` | lsf-core's id |
| `MediaImage`, `Video`, `ExternalVideo`, `Model3d` of a product | lsf-core's media id, the type by kind |
| `MediaImage`, `Video`, `GenericFile` of `files/` | `stable_id("image" \| "video" \| "file", path)`, by extension (the ids Liquid gives a `file_reference`) |
| `CompanyContact` | the customer's id (the checkout's `purchasingCompany.contact.id`) |
| `Market` | `stable_id("market", handle)` of a declared market, `stable_id("market", "FR")` (upper-case code) of a country in none (the checkout's) |
| `Metaobject` | `stable_id("metaobject", "<type>/<handle>")` (the checkout's, lsf-core's `metaobject_by_reference`) |
| `DiscountAutomaticNode`, `DiscountCodeNode` | `id` of the discount in `checkout.discounts`, its position from 1 by default (the checkout's `discount.id`; `DiscountNode/<n>` of the legacy APIs is the same discount) |
| `CartTransform`, `DeliveryCustomization`, `PaymentCustomization`, `Validation` | position of the Function in the app from 1 (the checkout's owner ids) |
| `ShopifyFunction` | no GID: a UUID made of the client id and the handle (`app::function_id`), also its key |
| `App` | `stable_id("app", client_id)`; `AppInstallation`: `stable_id("app_installation", client_id)` |
| `ProductOption` | `stable_id("product_option", "<product id>/<option name>")` |
| `ProductOptionValue` | `stable_id("product_option_value", "<product id>/<option>/<value>")` |
| `InventoryItem` | `stable_id("inventory_item", "<variant id>")` |
| `InventoryLevel` | `stable_id("inventory_level", "<variant id>/<location id>")`, with `?inventory_item_id=<item id>` as Shopify writes it |
| `LineItem` | `stable_id("line_item", "<order id>/<position>")` |
| `FulfillmentOrder`, `Fulfillment`, `OrderTransaction`, `Return`, their line items | `stable_id(<kind>, "<order GID>/<n>")`: `fulfillment_order`, `fulfillment`, `order_transaction` (n from 1), `fulfillment_order_line_item` and `fulfillment_line_item` (n the line's position), `return` (n from 0), `return_line_item` (`"<return GID>/<n>"`) |
| `Metafield` | `stable_id("metafield", "<owner GID>/<namespace>.<key>")`, the namespace as Admin writes it |
| `MetafieldDefinition` | `stable_id("metafield_definition", "<OWNER_TYPE>/<namespace>.<key>")` |
| `MetaobjectDefinition` | `stable_id("metaobject_definition", type)` |
| `Publication` (Online Store) | `stable_id("publication", "online-store")` |
| `SellingPlanGroup` | `stable_id("selling_plan_group", lower-case name)` (lsf-core's id is a hexadecimal string, Liquid's) |
| `Domain` | `stable_id("domain", host)`; `ShopPolicy`: `stable_id("shop_policy", handle)`; `Menu`: `stable_id("menu", handle)` |
| `CompanyContactRole` | `stable_id("company_contact_role", "<company id>/location-admin" \| "ordering-only")`; assignment: `stable_id("company_contact_role_assignment", "<customer id>/<location id>")` |
| inline objects with an id | `MailingAddress/<lsf-core address id>?model_name=CustomerAddress` (address book) or `Address` (order); `ImageSource`/`CollectionImage`/`ArticleImage` + lsf-core's image id; `MenuItem`: `stable_id("menu_item", "<menu>/<i>/<j>…")`; `MarketRegionCountry`: `stable_id("market_region", code)`; others `stable_id(<snake type>, …)` |

Every id is a function of the data: the same data gives the same records every run.

### Catalog (`domains/catalog.rs`)

- **`Location`**: those of the data; when there are none, one named "Shop location" at the shop's
  address *(chosen: every Shopify shop has one)*, with the id lsf-core would give it. `name`,
  `address` (`LocationAddress` with `formatted`, `latitude`, `longitude`), `isActive`,
  `fulfillsOnlineOrders`, `shipsInventory`, `hasActiveInventory` *(chosen: true)*,
  `isFulfillmentService: false`, `localPickupSettingsV2` when pickup is on and its time is one of
  Shopify's (`Usually ready in 2 hours` → `TWO_HOURS`), `inventoryLevels`, `metafields`.
- **`Product`**, those on the storefront and the others (`Store::unlisted_products`): `title`,
  `handle`, `descriptionHtml` and `bodyHtml`, `description` (the text of the HTML), `vendor`,
  `productType`, `tags`, `status` (the data's `status`, `ACTIVE` by default), `createdAt`,
  `updatedAt`, `publishedAt` (when it was published on the Online Store: `null` when the data
  says `published: false`), `templateSuffix`, `giftCardTemplateSuffix: null`, `isGiftCard`,
  `requiresSellingPlan`, `options`, `variants`, `media`, `images` (inline), `seo` (`title` and
  `description` null: no override), `onlineStoreUrl` (`null` for a product not on the
  storefront), `onlineStorePreviewUrl`, `collections` (without `all`), `sellingPlanGroups`,
  `metafields`, and what `refresh_product` derives.
- **`ProductOption`** (`name`, `position`, `values`, `optionValues`, `createdAt`) and
  **`ProductOptionValue`** (`name`, `hasVariants`, `swatch` with its `color` and the file of its
  image).
- **`ProductVariant`**: `title`, `displayName` (`<product> - <variant>`), `sku`, `barcode` and
  `barcodes`, `price`, `compareAtPrice`, `position`, `selectedOptions` (with `optionValue`),
  `inventoryItem`, `inventoryPolicy`, `inventoryQuantity` (the `available` of its levels),
  `availableForSale` (lsf-core's), `taxable`, `requiresComponents: false`, `product`, `media`,
  `image`, `unitPrice`, `unitPriceMeasurement`, `showUnitPrice`, `sellingPlanGroups`,
  `createdAt`, `updatedAt`, `metafields`.
- **`InventoryItem`**: `sku`, `tracked`, `requiresShipping`, `measurement.weight` (in the
  variant's unit), `inventoryLevels`, `variant`, `variants`, dates.
- **`InventoryLevel`** per location that stocks the variant (`store_availabilities`): `item`,
  `location`, `isActive`, `canDeactivate`, and `quantities`, the eight names of Shopify
  (`available`, `committed`, `damaged`, `incoming`, `on_hand`, `quality_control`, `reserved`,
  `safety_stock`; `quantities(names:)` filters them). The variant's stock is spread evenly over
  the locations where it is in stock, the first ones taking the remainder; a stock below zero,
  or that no location has in stock, is at the first location *(chosen: the data gives one
  number)*. `available` and `on_hand` hold it, the others are 0.
- **`Collection`**: `title`, `handle`, `descriptionHtml`, `description`, `image`, `products` (in
  the collection's order), `sortOrder` (`price-descending` → `PRICE_DESC`, …), `ruleSet: null`
  (lsf-core resolves automated collections into their products: they are manual here),
  `templateSuffix`, `seo`, `createdAt` (= `publishedAt` of the data), `updatedAt`, `metafields`.
  The `all` collection lsf-core adds is left out: `/collections/all` is not a collection on
  Shopify.
- **`Publication`**: the Online Store's (`name`), with every published product and every
  collection.
- **`SellingPlanGroup`**: `name`, `appId`, `options`, `products`, `sellingPlans`, `createdAt`.
  **`SellingPlan`**: `name`, `description`, `options`, `position`, `pricingPolicies` (the first
  adjustment a `SellingPlanFixedPricingPolicy`, the second a `SellingPlanRecurringPricingPolicy`
  with `afterCycle` = the first one's `order_count`), `billingPolicy`, `deliveryPolicy`,
  `category`, `metafields: []`. A recurring plan bills and delivers by the interval its name or
  options say (`Every 2 weeks` → `WEEK`, 2; `monthly`), a month otherwise *(chosen: the data has
  no interval)*, and is a `SUBSCRIPTION`; a plan that does not recur has a fixed billing policy
  with its checkout charge and is a `PRE_ORDER` when it takes a deposit, `OTHER` otherwise.
- **`GiftCard`**: `balance`, `initialValue` (in the card's currency), `enabled`, `expiresOn` (the
  day of the data, in the shop's time zone),
  `isRedeemable` (enabled, not expired by the shop's clock, a balance left), `lastCharacters`,
  `maskedCode`, `customer`, `recipientAttributes` when the recipient is a customer (`recipient`
  is a `Customer!`: the message and nickname of a card for someone else are not kept),
  `templateSuffix`, dates.

#### The catalog's mutations (`domains/catalog/`)

One submodule per area, each with its mutations and its custom fields: `products`,
`product_set`, `variants`, `options`, `media`, `collections`, `inventory`, `locations`,
`publications`, `selling_plans`, `gift_cards`, and `queries` (sort keys, search filters, the
fields that take an argument). `records` is what they share: reading inputs, and making
variants, inventory items and levels, option values and metafields in the seed's shapes. Each
module's header says what shopify.dev documents of its mutations (validations, side effects,
events) and its sources; `mutation_tests` tests them all over the demo store.

- **Derived values**: a mutation that changed a product runs `product_changed`, which runs
  `refresh_product` (price ranges, `totalInventory`, `featuredMedia`, …), renumbers and renames
  the variants, sets each option value's `hasVariants`, and puts the product in or out of the
  smart collections (`collections::apply_rules`).
- **Smart collections** keep their `ruleSet` and their `products`, which `apply_rules` keeps
  right for every product a mutation changes: the store sync reads `Collection.products` only.
- **`@idempotent`**: `inventory::with_idempotency` keeps each key with the arguments' fingerprint
  and the answer (`ShopState::set_memo`), so a retry answers the first answer again.
- **Partial updates** (`allowPartialUpdates`, `productCreateMedia`) call `m.keep_changes()`.
- **Publication events** name no record: `emit_with` with the product (or collection) and the
  publication as snapshot (`publications.rs`).

### Customers (`domains/customers.rs` and `domains/customers/`)

- **`Customer`**: `firstName`, `lastName`, `displayName` (the name, else the email, else the
  phone, as the schema describes it), `email`, `phone`, `defaultEmailAddress`,
  `defaultPhoneNumber` (both derived from the consents by `customers::contact_fields`, which the
  seed and the mutations share), `emailMarketingConsent`,
  `smsMarketingConsent` (subscribed when `accepts_marketing`; opt-in level `SINGLE_OPT_IN`
  *(chosen: Shopify's default setting)*), `addresses` and `addressesV2` (inline
  `MailingAddress`es), `defaultAddress` (the first), `state` (`ENABLED` with an account,
  `DISABLED` otherwise), `verifiedEmail`, `taxExempt`, `taxExemptions: []`, `dataSaleOptOut:
  false`, `tags`, `locale` (the primary language of the store *(chosen)*),
  `companyContactProfiles`, `createdAt` (the first order's, else the fixed date: as the
  checkout's `Customer.createdAt` when the data freezes the clock), `metafields`, and what
  `refresh_customer` derives: `amountSpent` is what the orders that are not cancelled cost, as
  the checkout counts it.
- **`Company`**: `name`, `externalId`, `note: null`, `customerSince`, dates, `locations`,
  `contacts` (every buyer of the company, as in the checkout), `mainContact` (the first),
  `contactRoles` (Shopify's two: "Location admin", "Ordering only"), `defaultRole` (Ordering
  only), `totalSpent` (the sum of its locations'), `metafields`.
- **`CompanyLocation`**: `name`, `externalId`, `company`, `locale`, `currency`,
  `shippingAddress` (an inline `CompanyAddress` with its id), `billingAddress: null` (the data
  has none), `taxSettings` (`taxRegistrationId`), `ordersCount`, `totalSpent`,
  `roleAssignments`, dates, `metafields`.
- **`CompanyContact`**: `company`, `customer`, `title`, `locale`, `isMainContact`,
  `roleAssignments`, dates. **`CompanyContactRoleAssignment`**: one per location the buyer can
  order for, with the default role *(chosen)*.

**Mutations** (`domains/customers/`: `customer`, `addresses`, `consent`, `companies`,
`contacts`; the user guide's "Customers and companies" lists them with their webhooks). What
they create has the seed's shape, so that store sync writes it back:

- A customer gets a `Customer` id of the counter, `state: DISABLED`, `verifiedEmail: true`, the
  primary locale, the default consents (`NOT_SUBSCRIBED`, `SINGLE_OPT_IN`), and what
  `refresh_customer` derives. An address is inline, `MailingAddress/<n>?model_name=
  CustomerAddress` with `n` from the shop's `MailingAddress` counter, built by
  `value::mailing_address` as the seed's; the default first in `addresses` and `addressesV2`,
  copied in `defaultAddress`.
- A company gets its two roles with the seed's ids (`contact_role_gid_of(<company id>, …)`), a
  location, and its contact. A contact's id is its customer's number (`CompanyContact/<customer
  id>`, the seed's and the checkout's), a new one of the counter when a customer is a contact of
  a second company. Addresses of a location are inline `CompanyAddress`es with an id of the
  counter; `billingSameAsShipping` gives both the same one, as the webhook examples show.
- Validation follows Shopify where it is published (the examples of the reference, the REST
  reference of the same resource, the Help Center), Rails' wording otherwise; each message's
  source is in a comment where it is written.
- Events whose payload says what changed keep the record before it
  (`ShopState::emit_with`): `customer.tags_added` and `customer.tags_removed` name the tags that
  changed, not all of them. A deletion that goes with its company's (`companyDelete`) embeds the
  company in its snapshot: the payloads of `company_contacts/delete` and
  `company_locations/delete` name it though it is gone.

### Orders (`domains/orders.rs`)

An order of the data or of the checkout is first an `OrderDraft`; `insert_order` makes its
records, so both are the same shape.

- **`Order`**: `name`, `number` (1001 for `#1001`), `createdAt`, `processedAt`, `updatedAt`,
  `cancelledAt` (the creation date of a cancelled order, the only date the data has) with
  `cancelReason: OTHER` *(chosen)*, `confirmed`, `test: false`, `email`, `phone`, `customer`,
  `purchasingEntity`, `customerAcceptsMarketing`, `currencyCode`, `presentmentCurrencyCode`,
  `taxesIncluded`, `taxExempt`, `note`, `tags: []`, `customAttributes`, `discountCode(s)`,
  `sourceName` (`web` for the checkout's), `lineItems`, `shippingAddress`, `billingAddress`,
  `displayAddress`, `billingAddressMatchesShippingAddress`, `shippingLine`/`shippingLines` (the
  title of the checkout rate that costs what the order paid for shipping, else "Shipping"),
  `taxLines` (one, "Tax", when there is tax *(chosen)*), the `MoneyBag`s: subtotal, shipping,
  tax, discounts, total (subtotal + shipping + tax, as the storefront's `order.total_price`),
  received, refunded, outstanding, capturable; `totalWeight`, `displayFinancialStatus`,
  `displayFulfillmentStatus`, `fullyPaid`, `unpaid`, `capturable`, `canMarkAsPaid`,
  `refundable`, `restockable`, `fulfillable`, `requiresShipping`, `returnStatus`, `returns`,
  `fulfillments`, `fulfillmentOrders`, `transactions`, `refunds: []`, `metafields: []`.
- **`LineItem`**: `name`, `title`, `variantTitle` (null for the default variant), `sku`,
  `vendor`, `quantity`, `currentQuantity`, `unfulfilledQuantity`, the unit and total
  `MoneyBag`s before and after discounts, `requiresShipping`, `taxable`, `isGiftCard`,
  `product`, `variant`, `image`, `weight`, `customAttributes`. The price is the variant's in the
  data (it has no price of its own for an order).
- **`FulfillmentOrder`** (every order has one, at the first location): `status` (`OPEN`,
  `IN_PROGRESS` when partly fulfilled, `CLOSED`, `CANCELLED`), `requestStatus: UNSUBMITTED`,
  `assignedLocation`, `destination` (the shipping address), `deliveryMethod`, `lineItems`
  (`FulfillmentOrderLineItem`: `totalQuantity`, `remainingQuantity`, …), `fulfillments`.
- **`Fulfillment`** of a fulfilled or partly fulfilled order: `status: SUCCESS`, `displayStatus:
  FULFILLED`, `fulfillmentLineItems`, `location`, `totalQuantity`. Which lines a `partial` order
  shipped the data does not say: the first line, or half the units of an order of one line
  *(chosen)*.
- **`OrderTransaction`**: a `SALE` of what was paid for a paid order (and a `REFUND` of it for a
  refunded one), an `AUTHORIZATION` for an authorized one, none for a pending one.
- **`Return`** per return of the data: `name`, `status`, `totalQuantity`, `returnLineItems`: a
  `ReturnLineItem` of the line's fulfillment when it was fulfilled, an
  `UnverifiedReturnLineItem` (with `unitPrice`) otherwise.
- **Orders of the checkout**: `orders::record_placed_order(state, &PlacedOrder)` takes what the
  server's `place_order` has (the `CheckoutResult`, the order number and id it gave, the time,
  the customer, the contact, the address, the note, the attributes, the payment method's name as
  the gateway), keeps the checkout's id when it is free (sessions number their orders on their
  own: a taken id gets a new one), refreshes the customer, and emits `orders/create`,
  `order_transactions/create` for the payment and, when paid in full, `orders/paid`. It commits
  no stock: the local checkout keeps the stock of the data *(chosen: test purchases never sell a
  product out)*. `lsf`'s `place_order` calls it through `AdminApi::record_order`.

What the mutations of the family (`domains/orders/`, see "Orders, fulfillments and draft
orders" in `docs/admin.md`) make and keep:

- **Derived values**: `orders::model::refresh_order` recomputes what an order derives from its
  records the way `insert_order` computes it (a test runs it on every seeded order and finds
  nothing to change, but the outstanding amount of a `refunded` order of the data, which has a
  refund transaction and no refund): a line item's `currentQuantity` (its `quantity` less what
  refunds took; an edit's removals are refunds of restock type `CANCEL` or `NO_RESTOCK`, as
  Shopify records them), `unfulfilledQuantity` (what its fulfillment order line items still
  have to fulfill), `refundableQuantity`; the order's `current…` amounts (less what refunds took
  back, and less what a refund gave back beyond the value of its lines, Shopify's
  `REFUND_DISCREPANCY` adjustment), its total (subtotal + shipping + tax), the money received,
  refunded, net and capturable (from its successful transactions), `totalOutstandingSet`
  (current total less net payment, negative when the customer is owed money),
  `displayFulfillmentStatus`, `returnStatus`, the flags (`fullyPaid`, `capturable`,
  `canMarkAsPaid`, `refundable`, `fulfillable`, …), and the customer's counters.
  `displayFinancialStatus` follows a change of money only (`settle_financial_status`): the data
  gives statuses the money alone cannot tell.
- **Records**: `OrderTransaction`, `Fulfillment` (`#1001-F2`), `FulfillmentLineItem`,
  `FulfillmentOrder` (a split, a partial move or a reopened cancellation makes another),
  `FulfillmentHold`, `FulfillmentEvent`, `FulfillmentOrderMerchantRequest`, `Refund` (its
  `RefundLineItem`s and `RefundShippingLine`s inline), `Return` (`#1001-R2`) and
  `ReturnLineItem`, `CalculatedOrder` (its `CalculatedLineItem`s inline) and `OrderEditSession`
  of the same number, `OrderCancelJobResult`, `DraftOrder` (`#D1`) and `DraftOrderLineItem`,
  `FulfillmentService` and its `Location`. Their ids come from `new_gid`.
- **Stock** (the levels of the catalog, changed in place): an order that claims stock commits it
  (`available` to `committed`: `orderCreate` with a `DECREMENT_…` behaviour, a completed draft
  order, units an edit adds); shipping takes units out of `committed` and `on_hand` as far as
  they were committed; a restock puts them back (`CANCEL` from `committed`, `RETURN` to
  `on_hand`). `inventory_levels/update` follows each change of a tracked item.
- **What a webhook needs beyond the record**, kept on it under `lsf…` keys, which GraphQL never
  reads: the fulfillment order of a `FulfillmentHold` (`fulfillmentOrder`) and the fulfillment
  of a `FulfillmentEvent` (`fulfillment`); the context of a `fulfillment_orders/…` topic on its
  fulfillment order (`lsfEvent`: the hold made and the fulfillment order split off, the
  original of a move, the replacement of a cancellation, the merchant request, the message);
  what `orders/edited` reports on the committed `CalculatedOrder` (`lsfCommittedAt`,
  `lsfAdditions`, `lsfRemovals`, …), and what its commit needs on each calculated line item
  (`lsfLineItem`, `lsfShipped`, `lsfDiscount`, …).
- **The tax rate** of draft orders and edits is the store data's `checkout.tax_rate`, which the
  seed keeps in the shop's settings (`ShopState::setting("taxRateBps")`), records having no
  field for it.

### Content (`domains/content.rs`)

- **`Shop`**: `name`, `email`, `contactEmail`, `description`, `myshopifyDomain`, `primaryDomain`
  and `domains` (`Domain`: the custom domain of the data, else the myshopify one, which is in
  `domains` either way), `url`, `currencyCode`, `enabledPresentmentCurrencies` (the shop's and
  its countries'), `currencyFormats`, `ianaTimezone`, `timezoneAbbreviation`, `timezoneOffset`,
  `timezoneOffsetMinutes` (at the shop's clock), `shopAddress` and `billingAddress`,
  `unitSystem` and `weightUnit` (the unit system of the shop's country), `taxesIncluded`,
  `customerAccounts`, `plan` (`partnerDevelopment: true`, "Development", `shopifyPlus` when the
  data has companies, B2B being a Plus feature *(chosen)*), `shopPolicies` (`ShopPolicy`:
  `type`, `title`, `body`, `url`), `shipsToCountries`, `orderNumberFormatPrefix: "#"`,
  `metafields`.
- **`Metafield`** for every owner (`Shop`, `Product`, `ProductVariant`, `Collection`,
  `Customer`, `Company`, `CompanyLocation`, `Location`, `Market`, `Page`, `Blog`, `Article`, the
  discounts and the owners of Functions): `namespace` (`$app:config` of the data is
  `app--<app id>--config`), `key`, `type`, `value` (the string Admin returns: references are
  GIDs, lists and objects compact JSON, `money` with a string amount and its currency),
  `jsonValue`, `reference` or `references` for the reference types, `owner`, `ownerType`,
  `definition`, `sizeInBytes`, dates (the owner's creation date). References of the data
  (handles, paths of `files/`, `type/handle`, SKUs, emails) become the GIDs they name; a file
  something names gets a file record.
- **`MetafieldDefinition`**, inferred: one per owner type, namespace and key that has
  metafields, with the type of the first (`validationStatus: SOME_INVALID` when another differs),
  `name` (the key, humanized), `type` (`content::types::definition_type`: `name`, `category`,
  `supportedValidations`, …), `validations: []`, `access` (public in the admin and on the
  storefront *(chosen: the theme reads them)*), `capabilities` (none enabled), `metafields`,
  `metafieldsCount`. None for the app's reserved namespaces: their definitions come from the
  app's configuration, which lsf does not read. Being definitions, they check what
  `metafieldsSet` writes to the same owner type, namespace and key (the type must be theirs).
- **`Metaobject`**: `type`, `handle`, `displayName` (its `name` or `title` field, else the
  handle humanized), `definition`, `fields` (inline `MetaobjectField`s: `key`, `type`, `value`,
  `jsonValue`, `reference(s)`, `definition`), `capabilities` (published, its template suffix),
  dates. **`MetaobjectDefinition`**, inferred from the entries of a type: `type`, `name`,
  `displayNameKey`, `fieldDefinitions` (every key the entries have, in order, not required),
  `metaobjects`, `metaobjectsCount`, `capabilities` (publishable, online store).
- **`Page`** (`body`, `isPublished`, `publishedAt`, `templateSuffix`, …), **`Blog`**
  (`commentPolicy` from `comments_enabled` and `moderated`, `articles`, `tags` of its articles),
  **`Article`** (`author.name`, `body`, `summary` = the excerpt, `image`, `tags`, `comments`, …),
  **`Comment`** (`author`, `body`, `bodyHtml`, `status: PUBLISHED`, …).
- **`Menu`**: `handle`, `title`, `isDefault` (`main-menu`, `footer`), `items`: inline
  `MenuItem`s (`type`, `url`, `resourceId`, nested `items`); a link to `/collections/all` is a
  `CATALOG` item.
- **`Market`**: the declared ones, then a market of its own per country in none, as the checkout
  sees them: `name`, `handle`, `type: REGION`, `status: ACTIVE`, `enabled`, `primary` (the
  shop's country), `conditions.regionsCondition.regions` and `regions` (inline
  `MarketRegionCountry`), `currencySettings`, `metafields`.
- **`ShopLocale`** per language: `locale`, `name`, `primary`, `published: true`.
- **Files**: `MediaImage`, `Video` or `GenericFile` for each path of `files` and each path
  something refers to, by extension: `alt`, `image` or `sources` or `url`, `mimeType`, statuses
  `READY`, dates.

**Mutations and queries of the family** (`domains/content/`; each module's documentation has the
rules and the sources):

| Module | What |
|---|---|
| `types` | The metafield types of 2026-10: categories, supported validations (`metafieldDefinitionTypes`, Shopify's example order), and `check`: a value checked against its type and a definition's validations, kept as the seed keeps it (`value`, `jsonValue`, `reference(s)`), which store sync projects back. |
| `metafields` | `metafieldsSet` (25 at most, atomic, `compareDigest`, the app's namespaces, the owner's write access), `metafieldsDelete`, `Metafield.compareDigest` (SHA-256 of the value, as Shopify's digests of translatable content: Shopify does not say how this one is made) and `translatable`. A metafield created gets the seed's GID (`metafield_gid`). |
| `metafield_definitions` | `metafieldDefinitionCreate`, `Update`, `Delete`, `Pin`, `Unpin`; `metafieldDefinitions`, `metafieldDefinition`, `metafieldDefinitionTypes`. |
| `metaobjects` | `metaobjectDefinitionCreate`, `Update`, `Delete`; `metaobjectCreate`, `Update`, `Upsert`, `Delete`, `BulkDelete`; `metaobjects(type:)`, `metaobjectByHandle`, `metaobjectDefinitionByType`, `Metaobject.field`, `Metaobject.values`. An entry created gets `metaobject_gid(type, handle)` (lsf-core's id), else a new one when a renamed entry holds it. |
| `online_store` | `page*`, `blog*`, `article*` (create, update, delete), `commentApprove`, `commentSpam`, `commentNotSpam`, `commentDelete`. |
| `navigation` | `menuCreate`, `Update`, `Delete` (a menu created gets `menu_gid(handle)`), `urlRedirectCreate`, `Update`, `Delete` (and `add_redirect`, for `redirectNewHandle`). |
| `translations` | `translationsRegister`, `translationsRemove`; `translatableResource(s)`, `translatableResourcesByIds`, the resources' `translations(locale:)`. A resource's translations are a `TranslatableResource` record (`resourceId`, `translations`) kept under `translations/<resource GID>`: not store data. |
| `files` | `fileCreate`, `fileUpdate`, `fileDelete`; `files`' sort keys and search text. The staged uploads are the app family's: its upload endpoint records an upload with `files::record_staged_upload(resource_url, StagedUpload {filename, mime_type, size})`, which `fileCreate` reads (`files::staged_upload`). |

Error codes the checks of metafield inputs share (`TOO_SHORT`, `APP_NOT_AUTHORIZED`, …) are fitted
to the code enum of each payload (`content::fit_code`: the code, else `INVALID`), since the
executor refuses an enum value the type does not have. Events without a webhook topic in 2026-10
(pages, blogs, articles, comments, menus, redirects, files, translations, metaobject definitions, the
metafields of owners other than products and variants) are emitted under a topic no
subscription can name (`pages/create`, `metafields/update`, …): the server still knows the shop
changed.

### App (`domains/app.rs`)

- **`App`**: `title` (`name` of `shopify.app.toml`, "lsf" without an app), `handle`, `apiKey`
  (the client id, `lsf-local-client-id` without one), `embedded`, `developerType: PARTNER`,
  `requestedAccessScopes`, `availableAccessScopes`, `optionalAccessScopes`, `installation`,
  `webhookApiVersion`. **`AppInstallation`**: `app`, `accessScopes` (`[access_scopes] scopes`;
  without an app every scope of the 2026-10 schema, `auth::scopes::ALL_SCOPES`, the list the access
  checks share), `launchUrl`.
- **`ShopifyFunction`** per Function of the app: `id`, `title`, `handle`, `apiType`,
  `apiVersion`, `app`, `appKey`, `inputQuery`.
- **Discounts** of `checkout.discounts`: a `DiscountAutomaticNode` (`automaticDiscount`: a
  `DiscountAutomaticApp`) or, with a code, a `DiscountCodeNode` (`codeDiscount`: a
  `DiscountCodeApp` with its `codes`): `title`, `discountId`, `appDiscountType` (the Function),
  `discountClasses` (all three when the data names none), `combinesWith` all false and
  `appliesOnOneTimePurchase: true`, `appliesOnSubscription: false` *(chosen: Shopify's defaults
  on creation)*, `status: ACTIVE`, `startsAt`, `context` (all buyers), and the node's
  `metafields`.
- **`CartTransform`**, **`DeliveryCustomization`**, **`PaymentCustomization`**,
  **`Validation`** for each Function of the app with such a target (whether or not
  `checkout.functions` names it): `title`, `enabled`, `functionId`, `shopifyFunction`,
  `blockOnFailure: false`, and the metafields `checkout.functions` gives it. Without an app,
  none: there is no Function to own.

What the app family's mutations make (`domains/app/`: `billing`, `discounts`, `functions`,
`bulk`, `uploads`, `metafields`), in the same shapes, so that the store sync reads them as it
reads the seed's:

- **`AppSubscription`** (`name`, `status`, `test`, `trialDays`, `returnUrl`,
  `currentPeriodEnd`, `lineItems`, `createdAt`, and lsf's `updatedAt`, `replacementBehavior`,
  `activatedAt`, `trialEndsAt`), whose **`AppSubscriptionLineItem`s** are records of their own
  (not `Node`s: `node(id:)` does not find them) under Shopify's id,
  `gid://shopify/AppSubscriptionLineItem/<subscription n>?v=1&index=<i>`, with
  `plan.pricingDetails` inline (`AppRecurringPricing` with its `discount`, or
  `AppUsagePricing` with `balanceUsed`) and their `usageRecords` (**`AppUsageRecord`**s); a new
  capped amount waiting for the merchant is the line's `pendingCappedAmount`.
  **`AppPurchaseOneTime`** (`name`, `price`, `status`, `test`, `createdAt`, lsf's `returnUrl`).
  Subscriptions and purchases share their numbers, which are the `charge_id` of the
  confirmation page. `AppInstallation.activeSubscriptions`, `allSubscriptions` and
  `oneTimePurchases` are read from them.
- **Discounts** made by the mutations: the node and its discount as above, for each of the eight
  types, the dates in UTC (`startsAt` given with an offset is written with `Z`, as Shopify answers
  it, and as the sync compares it with the shop's clock); `status` is computed from the dates when
  it is read. Shopify's own types keep their `customerGets` (`AllDiscountItems`,
  `DiscountProducts`, `DiscountCollections`; `DiscountPercentage`, `DiscountAmount`,
  `DiscountOnQuantity`), `customerBuys`, `minimumRequirement`, `destinationSelection`, and a
  `summary` of lsf's wording. The metafields of a node have its `owner` and `ownerType:
  DISCOUNT`; they are deleted with it. **`DiscountRedeemCodeBulkCreation`** records what
  `discountRedeemCodeBulkAdd` did.
- **`CartTransform`**, **`DeliveryCustomization`**, **`PaymentCustomization`**, **`Validation`**
  made by the mutations: new ids of each type's counter, `functionId` and `shopifyFunction` the
  Function's id (a validation has only `shopifyFunction`), `enabled`, their metafields.
- **`BulkOperation`** (`status`, `type`, `query`, `objectCount`, `rootObjectCount`,
  `fileSize`, `url` or `partialDataUrl`, `errorCode`, dates); its JSONL is a file of the shop
  (`ShopState::put_blob`, key `bulk/<n>.jsonl`), as the bytes of a staged upload are (their key,
  `tmp/<shop id>/<folder>/<uuid>/<file>`).
- A change Shopify has no webhook topic for (a customization, a validation, a cart transform, a
  usage record, a charge waiting for the merchant) emits `domains::app::CHANGED`
  (`lsf/changed`): no subscription can receive it, but it says the shop changed.

## Events and webhooks (`webhooks`)

A change emits events: `{topic, shop, gid, at, id, snapshot}` (`products/update`, `orders/create`,
…), after the mutation succeeded. `Webhooks::emit(event, lookup)` turns an event into a delivery
for each subscription of the shop whose topic matches and whose `filter` matches the payload:

- **Topics**: the 227 values of the 2026-10 `WebhookSubscriptionTopic` enum with their written
  form, read from the description of each value in the pinned introspection
  (`tools/admin/webhook_topics.mjs` writes `webhooks/topics_2026_10.rs`): the split between
  resource and action is not mechanical (`CUSTOMERS_EMAIL_MARKETING_CONSENT_UPDATE` is
  `customers_email_marketing_consent/update`, `CUSTOMER_TAGS_ADDED` is `customer.tags_added`).
- **Subscriptions**: those of `shopify.app.toml` (`[[webhooks.subscriptions]]`, app-wide, one per
  topic, `api_version` of `[webhooks]`), and those the backend creates through the Admin API
  (`webhookSubscriptionCreate`, per shop, in `webhooks::Registry`, which gives the
  `WebhookSubscription` record of each). Compliance topics (`customers/data_request`,
  `customers/redact`, `shop/redact`) are only triggered on demand.
- **Where**: a `uri` starting with `/` joins the app URL (`--app-url`, else `application_url`),
  trailing slashes removed, as Shopify's CLI prepends `application_url`; an absolute one under
  `application_url` (or on its origin) moves to `--app-url` when it is given; another HTTP(S) URL
  is kept. `pubsub://`, `arn:aws:events:` and `kafka://` are logged as not sent, with why.
- **Payload**: the JSON of Shopify's webhooks, which is the REST representation of the resource
  (`id` as a number, `admin_graphql_api_id`, snake_case, prices as strings with two decimals,
  enums in lowercase, tags as one string), in the key order of Shopify's example payload of the
  topic. `webhooks::payload` builds it from the record through a `Lookup` (the records a record
  names by GID, and the `Shop`), so that it needs nothing of the state but its records: the
  builders are there, by family, rather than in `domains/`. A test compares the payload of every
  topic lsf builds with Shopify's example (`tests/fixtures/webhooks/examples-2026-10.json`,
  written by `tools/admin/webhook_examples.mjs`), key for key and value for value, with the
  differences listed and explained. `include_fields` keeps only those fields (`variants.price`
  for a nested one). Topics whose payload lsf does not build are logged as not sent, never sent
  with a made-up body.
- **When the payload is built**: when the event is emitted, from the record as the change left it
  (Shopify builds it when it delivers, usually within a second; the two differ only when another
  change comes first). The worker then needs nothing of the state, and holds no lock on it. A
  `…/delete` topic (and `inventory_levels/disconnect`) is built from the event's `snapshot`;
  `app/scopes_update` reads the scopes before the change from the snapshot of the installation;
  another topic whose event has a snapshot gets it as `payload::PREVIOUS` (`customer.tags_added`
  names the tags the record did not have).
- **Filter**: evaluated on the payload after `include_fields` (Shopify requires the fields of the
  filter to be included), through `webhooks::PayloadFilter`, where `search` plugs in. Filters
  differ from searches: `:` is equality, case counts, a field the payload does not have sends
  nothing. The api layer plugs in `search::filter::webhook_filter_matches`; without an evaluator
  (the webhooks used alone), a filter matches, and the delivery notes that it was not evaluated.
- **Headers**: `Content-Type: application/json`, `X-Shopify-Topic`, `X-Shopify-Hmac-Sha256`
  (base64 HMAC-SHA256 of the body with the client secret), `X-Shopify-Shop-Domain`,
  `X-Shopify-API-Version` (`2026-10`, whatever the subscription asks, with a note when it asks
  for another), `X-Shopify-Webhook-Id` (one per delivery, the same across its retries),
  `X-Shopify-Triggered-At` (the event's time, `2025-07-10T16:15:47.000000Z`),
  `X-Shopify-Event-Id` (shared by the deliveries of one event), `X-Shopify-Name` for a named
  subscription, `User-Agent: Shopify-Captain-Hook`. Ids are random UUIDs: a backend that
  remembers the deliveries it processed does not take those of a new run for duplicates.
- **Delivery**: from a background worker, never from the thread of a request (the backend's
  handler may call the Admin API back). One worker by default, so that deliveries go out in the
  order they were made; a delivery waiting for a retry does not hold the others. A `2xx` within
  5 seconds (1 to connect) is a success; anything else, a redirect included, is retried after 1,
  5 and 15 seconds (Shopify retries 8 times over 4 hours), then marked failed. Each attempt is
  logged: the start, the duration, the status, the start of the answer, the error.
- **Control**: `GET /__lsf/webhooks` lists the deliveries (`?topic=`, `?shop=`, `?state=`;
  `?wait=<n>` waits until n of them are done, `?timeout=<ms>` at most, 10 s by default, and
  answers `408` when they were not), `DELETE /__lsf/webhooks` forgets them,
  `POST /__lsf/webhooks/trigger` sends one (`{"topic", "id"?, "shop"?, "address"?, "payload"?,
  "wait"?}`: the api layer builds the payload from the state, `payload` gives it as is), as
  `lsf app webhook trigger` does from the command line.

## Store sync (`sync`)

What the backend changes through the Admin API shows in the storefront and the local checkout,
as far as the store data can say it: a product it creates is on the theme's pages, a customer it
creates can log in at the checkout, a metafield it sets is read by Liquid and by the Function
inputs, an app discount it creates with `discountAutomaticAppCreate` runs its Function at the
checkout with the metafields it was given. The state is the truth: the store the storefront
sees is the files with the Admin API's changes on top, never seeded back from.

**Only the default shop syncs.** The other shops (`POST /__lsf/admin/shops`) exist to isolate
tests from one another; what they change stays in the Admin API.

### How

```rust
pub fn sync::overlay(state: &ShopState, store: &Store) -> Option<lsf_core::store::load::Overlay>;
pub const sync::OVERLAY_NAME: &str = "(admin api)";
```

`store` is the store the shop was seeded from: the files, loaded without any overlay. The
overlay is `{name: "(admin api)", removed, data}`: the entities to remove (by the key the data
names them by: handle, email, name, code), then store data (the format of a data file) to add
and replace. `load_with_overlays(source, options, &[overlay])` gives the store with the
changes. `None` when nothing the store data expresses changed.

- **Change tracking** (`ShopState`): once the seed is made it calls `track_changes`; from then
  on the first change of a record (`insert`, `insert_as`, `remove`, `get_mut`, `touch`) keeps a
  copy of the record as the seed made it. `changes()` lists the records created, changed or
  removed (a change undone, or a record read through `get_mut` and left as it was, is not one),
  `original(key)` gives the seed's record. One copy per record changed; nothing at all for a shop
  nothing changed (`touched()` is false, and `overlay` returns at once). Moving a shop to its
  domain is not a change; a reset or a reseed forgets them. `revision()` grows with every write
  and is never reset: the server tells a request that wrote from one that only read by it.
- **What is projected**: the entity of the store data each changed record belongs to (a variant,
  an option, a medium, an inventory item or level belongs to its product; a metafield to its
  owner; an article and its comments to their blog; a company location to its company; a
  company contact and its role assignments to their customer; a policy to the shop).
- **Projected twice**, from the records as they are and as the seed made them, the same way.
  The same store data both times: the change does not show in the store (a derived field such
  as `amountSpent` after an order placed at the checkout, a change undone), and nothing is
  written. Different: the entity is written whole. None now: it is removed by the key the seed's
  projection gives (the key the files name it by). A diff of the whole state against the seed
  would find the same changes, but would compare every record; the tracker gives the few that
  changed, and the seed's copies of them are what the projection compares and removes by.
- **Renames**: an entity whose key changed (a product's handle, a customer's email, a company's
  or a location's name) is written with its `id`, which replaces it where it is (an overlay
  entity that gives its id replaces the one with that id). An entity without an `id` in the data
  (menu, metaobject, selling plan group) is removed under its old key and added under the new
  one.
- **What names it**: the data names entities by their keys (a collection lists products by
  handle, an order names variants by SKU, a metafield names a product by handle, a customer
  names their company). When an entity is removed or its key changes (or a variant's SKU), every
  entity whose records name it by GID is written again, with the keys as they are now, even if
  it projects the same: a renamed product is renamed in the collections, selling plan groups,
  metafields, menus and orders of the data. A product that joined or left a collection or a
  selling plan group is written again too: its own data may name them.
- **Fidelity**: every entity of the rich fixture and of the demo store, projected from the
  seed's records, loads back to the same store (`sync::tests`): the projection writes what
  lsf-core reads, the fields the records do not hold come from `store` (a variant's quantity
  rule and price breaks, an image's focal point, a page's author, a gift card's code, the
  orders of a customer), and the ids are those lsf-core gave.

### What syncs

| Store data | From |
|---|---|
| `products` | `Product` (title, handle, `descriptionHtml`, vendor, type, tags, template suffix, dates, gift card, `requiresSellingPlan`, `status`, `published` = `publishedAt` set), its `options` and their values (order, swatch color and image), its `variants` (title, options, price, compare-at price, SKU, barcode, taxable, inventory policy, unit price and measurement; tracked, weight and shipping of the `InventoryItem`; the stock is the `available` quantity of its `InventoryLevel`s, `available` is written when `availableForSale` differs from what lsf-core derives; the locations that stock it are the data's while its inventory is the seed's, its levels once the Admin API changed them, in stock where the quantity is above 0 or the variant is not tracked), its `media` (images of `files/` with alt, size, id and focal point; videos and 3D models with their sources and preview; external videos), the `collections` and `selling_plan_groups` whose records list it, its metafields. A `DRAFT` or `ARCHIVED` product, or one whose `publishedAt` is `null`, is in the data with its status and leaves the storefront. |
| `collections` | `Collection` (title, handle, `descriptionHtml`, image, sort order, template suffix, dates, `products` in their order), its metafields |
| `customers` | `Customer` (email, names, phone, tags, `accepts_marketing` from the email consent, `has_account` from `state: ENABLED`, tax exempt, `addressesV2` with their ids and the default first), the company of its `CompanyContact` with the locations its role assignments give and its title, language and dates, its metafields; the orders are those of the data, their lines naming variants by id, without the lines of variants no longer on the storefront |
| `companies` | `Company` (name, external id, dates), its locations (name, external id, shipping address, tax registration, language, dates, orders count, total spent, metafields), its metafields |
| `pages`, `blogs` | `Page` (title, handle, body, template suffix, dates); `Blog` (title, handle, comment policy, template suffix) with its published `Article`s (title, handle, author, body, summary, image, tags, template suffix, dates, published comments); their metafields. An unpublished page or article leaves the storefront. |
| `menus` | `Menu` (title, items): an item names its resource by its key when the resource is on the storefront, its URL otherwise |
| `locations` | `Location` (name, address, coordinates, pickup and its time), its metafields. A deactivated location is removed. |
| `gift_cards` | the cards of the data: balance, initial value, currency, expiry, enabled, customer, recipient, message |
| `metaobjects` | `Metaobject` (type, handle, fields, template suffix). A draft entry is removed. |
| `selling_plan_groups` | `SellingPlanGroup` (name, app id, options, products) with its `SellingPlan`s (name, description, options, pricing policies, recurring or not, checkout charge) |
| metafields | of every owner above, of variants and company locations, of the shop and of the declared markets: the namespace of the app (`app--<id>--config`) as `$app:config`, references as the keys the data names them by (handle, `blog/article`, `type/handle`, path of `files/`, variant id, email, company name), JSON types as JSON |
| `shop` | name, email, description, the five policies of the data, metafields (merged field by field; a metafield removed is in `removed.shop_metafields`) |
| `localization` | rewritten with the data's countries and languages when a market changed: the markets declared in the data or created by the Admin API (id `market_gid(handle)`), enabled, with their countries (a country they add is added) and metafields |
| `checkout.discounts` | rewritten when an app discount changed: every `DiscountAutomaticApp` and `DiscountCodeApp` active by the shop's clock (`startsAt` ≤ now < `endsAt`), in id order, with its `id`, the handle of its Function (`appDiscountType.functionId`), title, first code, classes and metafields |
| `checkout.functions` | the settings of a Function whose owner changed: the metafields of its enabled `CartTransform`, `DeliveryCustomization`, `PaymentCustomization` and `Validation` objects, in creation order, a later one's taking the place of an earlier one's |

### What does not sync

Kept here so that nobody invents a field for it; each would need the store data to say more.

- **Orders**, and what belongs to them: an order the Admin API creates (`orderCreate`, draft
  orders) or changes (tags, note, cancellation, fulfillments, refunds, returns) is not in the
  store data. The account pages show the orders of the data and the checkout's.
- **Gift cards the Admin API creates**: the data keys a card by its code, which Shopify returns
  only once and the record does not keep. Changes to the cards of the data sync.
- **Discounts that are not app discounts** (`DiscountCodeBasic`, `…Bxgy`, `…FreeShipping`, the
  automatic ones): the local checkout runs Functions only. Of app discounts: `combinesWith`,
  usage limits, `appliesOncePerCustomer`, the buyers they apply to (`context`), every code but
  the first.
- **Turning a Function off**: a disabled customization or validation is left out of the
  settings, but its Function still runs (only a scenario's `extensions` chooses which run). Two
  owners of one Function share one set of settings.
- **Products**: SEO, the category, bundles and combined listings, the inventory item's cost and
  customs data, media taken from a URL outside `files/`, the recommendations a product of the
  data sets (written again, it gets the default ones), whether an untracked variant is in stock
  at one location and not another (it is wherever it is stocked once its inventory changed).
- **Collections**: rules (lsf-core resolves them; a changed automated collection is written as
  a manual one with the products it has), publication (an unpublished collection stays), SEO.
- **Customers**: the note, the language, tax exemptions, SMS consent, a customer without an
  email (removed from the storefront), the roles of a contact beyond the locations it orders
  for, a second contact.
- **Companies**: the note, the billing address, catalogs and price lists.
- **Content**: a blog's feed, comments that are not published, menu items of a type the data
  has no field for (a metaobject, a customer account page: written by URL); `main-menu` and
  `footer` always exist (lsf-core adds them back).
- **Markets**: a country's market of its own (its metafields, its currency), currency settings,
  web presences, catalogs, price lists.
- **Locations**: fulfillment services; the "Shop location" lsf adds to a store without locations
  becomes a location of the data once it changes.
- **The shop**: anything else (domains, currency, time zone, plan: the Admin API does not
  change them).
- Metafield and metaobject definitions, files (`files/` is on disk), translations, webhooks,
  publications other than the Online Store, inventory transfers and shipments, fulfillment
  orders.
- The `session` of the data and the scenarios of `checkouts/` are not written: a product,
  variant or customer they name by a key the Admin API changed or removed is reported as unknown
  there.

### What the projection reads, for the mutations

- A product is on the storefront when its `status` is `ACTIVE` and its `publishedAt` is set:
  publishing it on the Online Store sets `publishedAt` (and adds it to the publication's
  `products`), unpublishing clears it. Its collections are the `Collection.products` that list
  it, its selling plan groups the `SellingPlanGroup.products` that list it.
- Stock is the `available` quantity of the `InventoryLevel`s of the variant's `InventoryItem`;
  `availableForSale` follows Shopify's rule (not tracked, or stock above 0, or `CONTINUE`).
- A metafield belongs to its `owner`; a company contact to its `customer` and `company`, the
  locations its `roleAssignments` name; an article to its `blog`.
- A discount runs the Function of `appDiscountType.functionId` (the key of a `ShopifyFunction`
  record); an owner of a Function names it in `functionId` (or `shopifyFunction`) and is
  disabled by `enabled: false`.
- Pages and articles: `isPublished`; metaobjects: `capabilities.publishable.status`; locations:
  `isActive`.
- **Ids**: a record created with an id of the counter keeps it in the store, for the entities
  whose data has an `id` (products, variants, collections, pages, blogs, articles, comments,
  customers and their addresses, companies and their locations, locations, selling plans,
  images, media other than images, discounts). The others take the id lsf-core derives from
  their key, which the mutation that creates one must give it: `metaobject_gid(type, handle)`,
  `menu_gid(handle)`, `market_gid(handle)`, a gift card `stable_id("gift_card", code)`, an image
  of a product `stable_id("media", "<handle>/<position>")`. Their id changes in the store when
  their key does (a metaobject's handle, a menu's handle), and the images of a product follow
  their positions.

### Ids

The ids of what the Admin API did not touch do not change: the overlay writes only what
changed, every entity it writes carries its id, a renamed entity stays where it was, and a
removal keeps the ids and the dates of the others (an undated entity after a removed one keeps
the date its position gave it). Sessions and carts that name variants by id keep working.

### Server wiring (done)

The plan was to project after each change, under the shop's lock, and to build the store lazily.
The projection is lazy too: on the store of 3,000 products of the timing below it takes 70 ms,
which every mutation would have paid on the Admin request's path, ten times for a burst of ten.

1. **At start** (`ServerState::new`), the store of the files is loaded once and kept as the
   *base* (`Loaded::base`): it seeds the shops (`AdminApi::new`) and turns sync on
   (`AdminApi::sync_store(base)`). The store the storefront sees starts as the base.
2. **A change of the default shop is counted, not projected.** `ShopState::revision` grows with
   every write (`insert`, `insert_as`, `remove`, `get_mut`, `touch`, a reset), so `graphql` and
   `change_shop` (an order the checkout records goes through it) tell a request that wrote from
   one that only read by comparing it before and after, and count the first kind
   (`AdminApi::store_changes`, an atomic counter) while the shop is still locked. A reset
   (`DELETE /__lsf/admin`) counts too. Only the default shop's `LiveShop` (`default: true`, kept
   through a reseed that moves it) counts: the other shops never sync. A change that does not go
   through `graphql` or `change_shop` must count itself the same way (`AdminApi::install` writes
   only the installation's scopes, which do not sync, and does not).
3. **The next request that reads the store** (`dispatch`, after the Admin API's own paths, and
   `observe_changes`, which live reload calls) compares the count with the one its store was
   built at (`ServerState::admin_changes`): equal, it goes on (one atomic read). Different, it
   takes the build lock (`ServerState::rebuilding`, which `reload` takes too) and asks
   `AdminApi::synced`, which projects the shop under its lock when the count moved since the
   last projection (`api::sync::StoreSync::update`), keeps the overlay when it is the same
   (`Overlay: PartialEq`), and moves the generation on only when it is not. A new generation is
   built (`load_with_overlays(&source, &options, &[overlay])`, or the base itself for no
   overlay) outside the shop's lock, and swapped into `Loaded` whole: a page rendered meanwhile
   has the `Arc<Loaded>` it started with. A burst of mutations is projected and built once, by
   the first request that follows it; an order recorded at the checkout makes the same overlay
   and builds nothing (`an_order_placed_at_the_checkout_does_not_build_the_store_again`).
4. **Sessions with their own data** keep it (`SessionEntry::data`, with the generation their
   store was built at). Their store is `[sync_overlay, Overlay::new(OVERLAY_FILE, data)]`
   (`ServerState::session_store`, which `PUT /__lsf/session` uses too), built again on the
   session's next request when the generation moved. A session named only by
   `GET /__lsf/checkout?session=` sees its store as of its own last request.
5. **Live reload**: the token has the generation of the store built (`Loaded::synced`), and
   `observe_changes` builds before it computes the token: the pages reload once the store is
   built, and find it so.
6. **The files change** (`--watch`, `POST /__lsf/reload`): `reload` loads the new base, reseeds
   (`AdminApi::reseed`; the terminal says what was dropped), moves sync to the new base
   (`sync_store`, which counts a change), and builds from `synced()`: the store is the new base,
   unless the backend changed the shop in between.
7. **What the store data refuses** of the overlay: the problems of the store built with it that
   the files alone do not have (`Applied::diagnostics`) are printed when they differ from the
   last build's, and given to `AdminApi::sync_built`, which `GET /__lsf/admin` shows in
   `storeSync.built`. The store is built from what is valid, as from files with errors.
8. The checkout's configuration comes from the store (`config_from_store`): with the synced
   store, it has the discounts and the settings of the Functions the Admin API changed, and a
   scenario's `checkout` still goes on top of them.

`GET /__lsf/admin` gives `storeSync`: `enabled`, `generation`, `pending` (counted, not projected
yet), `overlay` (`written`: entities by section; `removed`: their keys), `built` (`generation`,
`ms`, `diagnostics`).

**What a mutation costs** (release build, through the handlers, the fixture theme):

| Store | Records | Mutation (Admin request) | Next page: projection and build | Page without a change |
|---|---|---|---|---|
| Demo (8 products) | 251 | 0.5 ms | 0.5 to 1.3 ms | 0.1 ms |
| 3,000 products of 12 variants | 198,000 | 400 ms | 290 to 370 ms (projection 70 ms, build 250 to 300 ms) | 0.1 ms |

- The build was cubic before: a product's default recommendations checked every product against
  the list so far before keeping ten. 2,000 products took 1.2 to 2 s to load, 3,000 products 5 to
  6 s; now 180 ms and 300 to 370 ms (`lsf-core`, `build_recommendations`).
- The 400 ms of a mutation on the large store are not sync's: the mutation framework clones the
  whole shop before the first write of a mutation, to undo it on `userErrors`
  (`resolve::mutation`, `snapshot`). An undo log of the records written (what change tracking
  keeps already) would make it as cheap as on the demo store.
- Seeding 3,000 products takes 1.7 to 3.4 s, at start and on each reseed, not on a mutation.

## Crate layout

`lsf-admin` (`crates/admin`) depends on `lsf-core` (the store) and `lsf-checkout` (the app:
`app::App`, whose `config` is the backend's part of `shopify.app.toml`, and the Functions). It
serves nothing itself: `lsf` routes the requests to [`api`], as it does for the checkout.

| Module | Role |
|---|---|
| `version` | The API versions lsf serves (2026-10), from the path of a request. |
| `schema` | The schema of the Admin API, parsed once from `schemas/2026-10/admin.graphql` (generated by `tools/admin/admin_schema.mjs` from the pinned `@shopify/dev-mcp`, with its descriptions, for introspection; embedded Brotli-compressed). |
| `graphql` | Parsing (a parser of its own), validation, variables, execution, introspection, cost: a GraphQL server over a `Resolver`. Knows nothing of Shopify's data. |
| `state` | Shops, records, ids, the clock, events, the changes since the seed; the seed from `lsf_core::Store` (`state::seed`, which runs the seed of each domain). |
| `sync` | Store sync: the records changed since the seed, projected to an overlay of the store data. |
| `resolve` | The generic resolution of records, connections and root fields, and the registry of custom resolvers. |
| `domains/` | One module per family of resources: its seed, its custom resolvers, its mutations (`catalog`, `orders`, `customers`, `content`, `app`). Its webhook payloads are in `webhooks::payload`, built from records alone. |
| `search` | Shopify's search syntax. |
| `auth` | Tokens, OAuth, token exchange, session tokens, access scopes, HMAC. |
| `webhooks` | Topics, subscriptions and their registry, payloads (by family), signing, the delivery worker and its log, the `/__lsf/webhooks` endpoints, the sample records of `lsf app webhook trigger`. |
| `throttle` | The cost bucket of each shop and app. |
| `api` | One HTTP request in, one answer out: the Admin API endpoints, OAuth, and the `/__lsf/admin` and `/__lsf/webhooks` control endpoints. `api::sync`: store sync as a server runs it (the count of changes, the overlay made when asked). |
