# Design of the local Admin API

## Goal

The backend of a Shopify app (a React Router or Remix app, Rails, Laravel, Express, Django, Go,
…) talks to Shopify through the **GraphQL Admin API** and receives **webhooks**. `lsf` serves
a local Admin API, **API version 2026-10 by default** and the other versions it knows on demand
(2027-01, the release candidate: see "API versions"), backed by the store data, and delivers the
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
the versions lsf has no data for (adding one is a data drop: "Add a version"), the admin web UI
itself. lsf draws the admin around an embedded app only, with a local App Bridge: "The admin of
an embedded app".

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

With `--app`, the server prints, before `ready:`, the Admin API URL and an access token that works at
once (without it, nothing: `GET /__lsf/admin` has the token):

```text
admin:  http://127.0.0.1:9292/admin/api/2026-10/graphql.json
        X-Shopify-Access-Token: shpat_…   (shop: local-supply-co.myshopify.com)
        client secret: from SHOPIFY_API_SECRET of ../my-app/.env
webhooks: 3 subscriptions of shopify.app.toml → http://localhost:3000
```

| Option | What it does |
|---|---|
| `--app-url <url>` (`LSF_APP_URL`) | Where the backend of the app listens. Relative webhook `uri`s are joined to it, and an absolute one on the origin of `application_url` is moved to it. Without it, `application_url` is used, and a webhook is sent there only when it is on this machine. |
| `--client-secret <secret>` (`LSF_CLIENT_SECRET`) | The secret webhooks, OAuth and session tokens are signed with. Default: `SHOPIFY_API_SECRET` of the environment, else of the app's `.env`, else a fixed local secret printed at start. |
| `--admin-versions <list>` (`LSF_ADMIN_VERSIONS`) | The API versions served, among those lsf knows: `2026-10` (the default) alone without it, `2026-10,2027-01` with the release candidate. See "API versions". |
| `--admin-throttle <plan>` (`LSF_ADMIN_THROTTLE`) | The cost bucket: `store` (the default: Plus for store data with companies, as the seed's `plan.shopifyPlus`, Standard otherwise), a plan (`standard`, `advanced`, `plus`, `enterprise`, `legacy`), or `off`. |

The client id is `client_id` of `shopify.app.toml` (a fixed local one without an app). The
Admin API is served whenever the server runs; the webhooks, the scopes and the client id come
from the app when there is one (`--app`).

## The HTTP protocol, as Shopify's

- `POST /admin/api/<version>/graphql.json` with `{"query", "variables", "operationName"}`
  (`application/json`), or the query alone (`application/graphql`). `GET` with `?query=` too.
- Authentication: `X-Shopify-Access-Token`. Missing or unknown: `401`
  `{"errors":"[API] Invalid API key or access token (unrecognized login or wrong password)"}`.
- The version of the path is served when the server serves it; one Shopify no longer supports
  (`2025-07`) falls forward to the oldest stable version served, as Shopify does; another
  (`2026-07`, `unstable`, `2027-01` when not served) answers `404` with a JSON message saying which
  versions lsf serves (see "API versions").
- Response headers: `X-Shopify-API-Version` (the version that answered), `X-Request-Id`, JSON
  content type.
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

`tools/admin/admin_schema.mjs` (`mise run admin:schema -- --version <v>`) writes
`schemas/admin/<v>/admin.graphql` from the introspection of the pinned `@shopify/dev-mcp@1.17.0`
(SHA-256 checked; shopify.dev's public proxy for a version it lacks): every
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
that the `.br` is the `.graphql`, for each version. `schema::for_version(v)` parses a version's
once (`OnceLock`), at its first request; `schema::admin()` is the default version's: 24 ms in
release on an Apple M4 Pro (decompression about 5 ms, graphql-parser about 18 ms, the
model the rest); `prepare` and the requested cost of a typical query take about 5 µs.

Shopify's introspection lists four possible types that the schema does not define (`ExchangeV2`
under `Node`, three user errors under `DisplayableError`). graphql-js's `buildClientSchema`
refuses a schema that names a type it lacks, so lsf's `possibleTypes` are the object types that
declare the interface. Everything else is Shopify's, which the tests check: graphql-js's
`getIntrospectionQuery()` (descriptions, `specifiedByURL`, `isOneOf`, `isRepeatable`, input value
deprecation) is compared with the dev-mcp introspection for a few types, and
`LSF_ADMIN_INTROSPECTION=<file> cargo test -p lsf-app --test introspection` compares all 3699
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

`version::parse_path` reads `/admin/api/<version>/graphql.json`: `Graphql(v)` for any version
name, `UnsupportedVersion(x)` for a path segment that is not one (and for `/admin/api/graphql.json`
without one), `Rest` for the REST Admin API. Which version answers is the server's
`Served::resolve`: see "API versions" below.

## API versions (`version`)

A version of the Admin API is data, and a request runs in its own. lsf serves **2026-10 by
default** (`version::DEFAULT`, the one place the code names it), and the other versions it knows
on demand: `--admin-versions 2026-10,2027-01` (`LSF_ADMIN_VERSIONS`, `AdminApi::with_versions`).

### The registry

`version::KNOWN` lists every version lsf knows, oldest first, each a line:

```rust
static KNOWN: [Entry; COUNT] = [
    version!("2026-10", Stable, &[]),
    version!("2027-01", ReleaseCandidate, adapt::V2027_01),
];
```

An entry has its name, its status (`Stable`, `ReleaseCandidate`, `Unstable`, `Unsupported`:
shopify.dev's labels), its adapters, and its files, embedded by the `version!` macro from
`schemas/admin/<version>/`:

| File | What reads it |
|---|---|
| `admin.graphql.br`, `introspection.graphql` | `schema::for_version(v)`: validation, execution, introspection, cost |
| `access.json` | `auth::scopes::for_version(v)`: the `requiredAccess` of its fields (`ACCESS_DENIED`) |
| `search.json` | `search::for_version(v)`: the filters each connection's `query` documents |
| `topics.json` | `webhooks::topics::for_version(v)`: its `WebhookSubscriptionTopic` values, written forms and scopes |
| `tests/fixtures/webhooks/examples-<v>.json` (tests only) | the payload test: Shopify's example payload of each topic lsf builds |
| `tests/fixtures/<v>/introspection.json` (tests only) | the introspection test |

`ApiVersion` is a `Copy` handle on an entry (`ApiVersion::get("2027-01")`, `::all()`,
`::default_version()`, `.name()`, `.status()`, `.since("2027-01")`, `.schema()`). Each module
parses its part the first time a request of the version needs it, in a `OnceLock` per version
(`static PARSED: [OnceLock<T>; version::COUNT]`, as `lsf_app::config::version` keeps the Function schemas):
a server that serves 2026-10 only never decompresses 2027-01. The free functions that existed
before versions (`schema::admin()`, `scopes::required_access`, `search::documented_filters`,
`topics::all`, `topics::written_form`, …) stay, as shims on the default version: code that does
not know the version of its request keeps working, and reads 2026-10.

### Which version answers

`Served` is the set a server serves. `Served::resolve(asked)`:

1. A version it serves (and that is not `Unsupported`) answers.
2. A version Shopify no longer serves (a dated one older than `version::OLDEST_ACCESSIBLE`,
   2026-01 today, or one lsf knows as `Unsupported`) **falls forward** to the oldest stable
   version served, as Shopify does: "If your app targets an inaccessible version, Shopify falls
   forward and responds using the oldest accessible stable version", and "API responses include
   the `X-Shopify-API-Version` header reflecting the version used to fulfill the request"
   (shopify.dev, "API versioning", <https://shopify.dev/docs/api/usage/versioning>, read on
   2026-10-31; its release schedule has 2025-10 accessible until October 16, 2026).
   `POST /admin/api/2025-07/graphql.json` is answered by 2026-10, with
   `X-Shopify-API-Version: 2026-10` and an `X-Lsf-Reason` saying why. A release candidate is
   never the fallback.
3. Any other version answers `404`, saying which versions the server serves: one Shopify
   supports but lsf has no data for (`2026-07`), `unstable`, and one lsf knows but the server was
   not asked to serve, whose message says how (`lsf knows API version 2027-01 (release
   candidate) but this server serves 2026-10 only: start it with --admin-versions
   2026-10,2027-01`). lsf could answer them with another version, as Shopify answers an
   inaccessible one; it does not, since Shopify would answer them in their own: a local tool
   says what it does not do.

`X-Shopify-API-Version` is the version that answered, on every answer of the GraphQL endpoint.
`Served::primary` (the default when served, else the oldest) is the version the server names:
the banner, `GET /__lsf/admin` (`apiVersion`, and `apiVersions`: each known version, its status,
whether it is served), the answer of `POST /__lsf/admin/shops`.

### One store of records, a view per version

The shops keep **one set of records, in one shape**: that of the default version, which the
seed, the mutations, store sync and the payload builders write and read. No resolver, mutation or
builder is copied per version. A version is a view of the records:

- **What exists** is its schema's business. A field, an argument, an input field, an enum value
  or a mutation the version does not have is refused by validation, with graphql-ruby's error,
  before any resolver runs (`Field 'automaticDiscounts' doesn't exist on type 'QueryRoot'`,
  `undefinedField`). A resolver or a mutation registered for something only some versions have
  is reached only in those: there is nothing to gate by hand. `Registry::check_versions` (a test)
  accepts a registration that one known version has, and lists one that none has.
- **What reads differently** is an `Adapter` of the version (`version::adapt`), declared in its
  entry of `KNOWN`. Four kinds:
  - `Field { type_name, field, read }`: the version's `type_name.field` is `read(record)` rather
    than the record's value under `field`; the value is then converted by the field's type as a
    stored one is (a GID becomes its record, an object an inline one). A renamed field reads the
    records' name for it, a reshaped one computes its value, a removed enum value maps to the one
    that replaced it. Applied at rule 5 of "How a field is resolved", for records and inline
    objects alike; a custom resolver of the field, if any, reads `r.version()` itself.
  - `Arguments { type_name, field, write }`: the arguments of `type_name.field` (a mutation's
    input) as the version gives them, rewritten into the shape the resolvers read before access,
    the mutation or the resolver runs (`FieldContext::with_arguments`).
  - `Payload { topic, write }`: the payload of a webhook topic in the version, from the one the
    builders make; applied when a delivery is built in the version.
  - `Typename { from, to }`: a type the records hold that the version removed reads as the one
    that replaced it, for a record (`ShopResolver::type_of`, `reference`) and an inline object
    (`inline_of`, which otherwise falls back to the first possible type of an interface: a
    removed type silently became another). `version::adapt`'s
    `every_type_the_records_hold_exists_in_every_version` checks every type the code writes
    and the demo and rich stores hold against each version, through these adapters.

  An adapter maps any record to the version's view, whatever version wrote it: it is total (a
  value the version has passes through), so that it still holds when the default version, and
  with it the records' shape, moves on.
- **What behaves differently** (a mutation whose rules changed, not its shape) asks the version
  of its request: `r.version()` in a resolver, `m.version()` in a mutation
  (`m.version().since("2027-01")`), `m.api_version()` its name, `r.schema()`/`m.schema()` its
  schema (`ctx.schema()` is the same). Rare: a difference of shape is an adapter.

A domain helper that has no request (`content::fit_code`) reads the default version's schema: the
code enums it reads are the same in both versions lsf knows.

### A worked example: `MetaobjectAdminAccess` in 2027-01

2026-10 deprecates two values of `MetaobjectAdminAccess`, the admin access of a metaobject
definition (`MetaobjectDefinition.access.admin`): `PRIVATE` and `PUBLIC_READ`, "Use
MERCHANT_READ instead. Removed in API version 2027-01." 2027-01 removes them; its
`MetaobjectAdminAccessInput` never had them.

- A request of 2027-01 that names them (`__type`, an enum literal) is refused by its schema; a
  request of 2026-10 is not. Nothing to write.
- A definition kept with `PUBLIC_READ` (on Shopify, one made before 2027-01; in lsf, a record of
  the shape of 2026-10) has a value the enum of 2027-01 does not have: read as it is, the executor
  would answer an error for the field. 2027-01's adapter reads it as `MERCHANT_READ`:

  ```rust
  pub const V2027_01: &[Adapter] = &[Adapter::Field {
      type_name: "MetaobjectAccess",
      field: "admin",
      read: metaobject_admin_access_2027_01, // PRIVATE | PUBLIC_READ → MERCHANT_READ, the rest as is
  }];
  ```

  The record does not change: 2026-10 reads `PUBLIC_READ`, 2027-01 `MERCHANT_READ`
  (`tests/admin_versions.rs`, `a_record_written_in_2026_10_reads_through_the_adapter_of_2027_01`).
  Shopify does not say what it answers for such a definition in 2027-01: lsf reads the
  deprecation reason as the mapping.

The rest of what 2027-01 changes needs no code but two adapters: `QueryRoot.automaticDiscounts`,
`Mutation.marketCurrencySettingsUpdate`, `StandardMetaobjectDefinitionFieldTemplate
.visibleToStorefrontApi` and the collection sources' `metafieldInteger` are removed (refused by
the schema; a condition made with it in 2026-10 reads in 2027-01 as the
`CollectionSourceInclusionConditionMetafieldInt` that "replaces this one entirely", through a
`Typename` adapter and a `Field` one for its `value`, a `String` there); `InventoryItemMeasurement.packedDimensions: BoxDimensions!` and
`Mutation.appDowngradeAccessScopes` are added (the first answered with a default, reported in
`extensions.lsf.defaulted`; the second simulated, registered like any mutation: only 2027-01
reaches it); new `MarketUserErrorCode` values. Its
webhook topics and the keys of its example payloads are 2026-10's. `mise run admin:version`
prints this list for a new version.

A **rename** would be declared the same way. Were 2027-01 to rename `Product.bodyHtml` to
`Product.descriptionMarkup` and the `ProductInput.bodyHtml` field with it:

```rust
Adapter::Field { type_name: "Product", field: "descriptionMarkup",
    read: |product| product.get("bodyHtml").cloned() },
Adapter::Arguments { type_name: "Mutation", field: "productUpdate",
    write: |arguments| rename_input(arguments, "product", "descriptionMarkup", "bodyHtml") },
Adapter::Payload { topic: "products/update", write: |payload| rename_key(payload, "body_html", "description_markup") },
```

and the records, the mutation and the builder would stay those of 2026-10.
(`resolve::tests::a_version_adapts_the_arguments_it_is_given_and_the_fields_it_reads` runs an
`Arguments` and a `Field` adapter through the resolver; `version::adapt` tests a `Payload` one.)

### Webhooks per version

- A subscription has an `api_version`: `[webhooks] api_version` of `shopify.app.toml` for its
  subscriptions (the default version when there is none), the version of the request for one
  made with `webhookSubscriptionCreate`. `WebhookSubscription.apiVersion` answers it
  (`supported`: whether Shopify still serves it).
- A delivery is built in the subscription's version as the server serves it
  (`Served::for_webhook`): the version itself when served; an inaccessible one falls forward, as
  shopify.dev says webhooks do ("Shopify falls forward when your selected version becomes
  inaccessible, and webhooks include the `X-Shopify-Api-Version` header to confirm which version
  was used"); one the server does not serve is built in the primary version, the delivery's
  notes saying so and, for a version lsf knows, how to serve it. Its payload is the builders'
  with the version's `Payload` adapters, and `X-Shopify-API-Version` is that version.
- Topics are per version: a topic the version does not have is refused there,
  `webhookSubscriptionCreate` by validation (the `WebhookSubscriptionTopic` enum of its schema),
  a subscription of `shopify.app.toml` by its deliveries ("`x` is not a webhook topic of API
  version 2026-10"), `lsf app webhook trigger --api-version` before anything is sent. The scopes
  a subscription needs are those its version's topic names.
- `lsf app webhook trigger --api-version <v>` builds its payload in `v`, any version lsf knows.

### Size

What a version adds to the binary: its compressed schema (2027-01: 393,820 bytes) and its tables,
embedded as JSON (access 202,763, search 53,448, topics 53,481, introspection types 5,806):
709 KB. The release binary (macOS, arm64) went from 54,969,280 bytes before versions to
55,927,712 bytes with 2027-01 and the code of this section (+958 KB, 1.7 %): not enough to keep
the version out of the binary as a test fixture. Parsing waits for the first request of the
version (24 ms for the schema in release).

2027-01 is embedded and registered, not served by default: an app developer tests the next
release against lsf with `--admin-versions 2026-10,2027-01`, and a backend that hard-codes
2026-10 sees nothing change. A release candidate "may include backwards-incompatible changes":
it is not what an app talks to unless it asks.

### Add a version

1. `mise run admin:version -- <version>` (`tools/admin/add_version.mjs`). From one introspection
   (the pinned `@shopify/dev-mcp`, which ships `admin_<version>.json.gz` for the versions shopify.dev
   lists: 2026-01 to 2027-01 and `unstable` in 1.17.0; else shopify.dev's public proxy of the
   Admin API, `https://shopify.dev/admin-graphql-direct-proxy/<version>`, which answers
   introspection without a token, `requiredAccess` included; `LSF_ADMIN_SOURCE=shopify.dev`
   forces it) it writes `schemas/admin/<version>/` (`admin.graphql`, `.br`, `introspection.graphql`,
   `access.json`, `search.json`, `topics.json`), `tests/fixtures/<version>/introspection.json`
   and, from shopify.dev's webhook reference of the version,
   `tests/fixtures/webhooks/examples-<version>.json`. It then prints Shopify's status of the
   version, a newer `OLDEST_ACCESSIBLE` if there is one, and how the schema differs from the
   default version's. Each part has its own task too (`admin:schema`, `admin:access-scopes`,
   `admin:topics`, `admin:webhook-examples`, each with `-- --version <v>`).
2. `crates/app/src/admin/version.rs`: a `version!("<version>", <Status>, &[])` line in `KNOWN`, in
   date order, and `COUNT` one higher (the array's length checks it).
3. For each difference the tool listed that reads differently from the records (a removed or
   renamed value, field or input field, a reshaped payload), an `Adapter` in
   `version/adapt.rs`, with a test. What a version only removes or only adds needs none.
4. `cargo test -p lsf-app`: the payload test compares every builder with the examples of the
   version (keys and their order; values with the default version's examples, which the samples
   are made from), the introspection test with its fixture, the topics with its enum, and
   `ALL_SCOPES` with what its tables and topics name (add a scope it reports).
5. A test of what the version changes, in `tests/admin_versions.rs`.
6. To make it the default: `DEFAULT`, then the adapters move to the version that now differs from
   the records (the records' shape is the default's); the payload builders follow the new
   version's examples.
7. Docs: `docs/admin.md` ("API versions"), `CHANGELOG.md`.

When Shopify retires a version: its status becomes `Unsupported` (requests and webhooks for it
fall forward), and `OLDEST_ACCESSIBLE` moves; its files can go once nothing serves it.

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
  Uninstalling (`Tokens::uninstall`) revokes every token of the app on the shop; the webhooks
  of the shop stop (see "Events and webhooks").
- **Delegate tokens** (`delegateAccessTokenCreate`): `shppa_` and 32 hex digits (the guide's
  `shppa_…`), a subset of the parent's scopes, expiring with the parent or sooner; their grant's
  source is `GrantSource::Delegate`, which may not make delegates, nor be migrated to an
  expiring offline token (`Tokens::migrate`, `MigrationError::Delegate`): the token it would
  become is the installation's, with all its scopes. The other grants start from a code, a
  session token, a refresh token or the client secret, never from an access token. When the installation's scopes
  change (`Tokens::set_scopes`: an install with other scopes, `appRevokeAccessScopes`), every
  token of it carries the new ones, but a delegate keeps those of its own still granted.
- **What a mutation changes of the tokens.** A mutation runs under the shop's lock, and the tokens
  come before the shops in the order of the locks: a mutation cannot take them. So `appUninstall`,
  `appRevokeAccessScopes`, `appDowngradeAccessScopes` and the delegate tokens change the records
  (the installation's `accessScopes`, a `DelegateAccessToken` record kept under a key made of the
  token) and emit an event of `domains::platform::access::AUTH` (`lsf/auth`, which no
  subscription can name) whose snapshot says what the tokens must do
  (`AuthEffect::{Uninstall, Scopes, Delegate, Destroy}`). The api layer reads them from the
  request's events and applies them once the shop is unlocked, before it answers
  (`AdminApi::apply_auth_effects`): the backend's next request finds the token made, revoked or
  narrowed. A failed mutation emits nothing, so changes nothing of the tokens either. The
  uninstall is the control endpoint's path (`AdminApi::finish_uninstall`), its `app/uninstalled`
  delivered with the mutation's events.
- **A request that waited on the shop** runs with the tokens as they are when it gets the shop,
  not as they were when it looked its token up. Each `LiveShop` has an `auth_generation`, moved
  under its lock by every change of what its tokens grant: a request with auth effects (which
  also counts them `auth_pending` until it has applied them, the shop unlocked), a reset, an
  install, an uninstall and an OAuth grant (after they changed the tokens). A GraphQL request
  reads the generation once nothing is pending, looks its token up, locks the shop, and runs
  only if the generation did not move; otherwise it looks the token up again (`401` for one
  revoked, the narrowed scopes for one revoked from). Without it, a `productCreate` queued
  behind an `appUninstall` ran after it with the token it had revoked
  (`api::tests::a_request_that_waited_on_the_shop_runs_with_the_tokens_as_they_are_now`).
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
  cover answers `ACCESS_DENIED` with Shopify's message ("Access denied for products field. Required
  access: `read_products` access scope.") and `extensions` (`code`, `documentation`,
  `requiredAccess`). `write_x` implies `read_x`, `unauthenticated_write_x` implies
  `unauthenticated_read_x`. The table is not hand-written, and is the request's version's: Shopify's
  introspection of each version carries the `requiredAccess` text of every type and field, which
  `mise run admin:access-scopes -- --version <v>` writes into `schemas/admin/<v>/access.json`
  (`auth::scopes::for_version`); a root field without one requires that of the type it answers
  (`products` → `Product`). Staff permissions, plans and requirements that depend on the arguments
  (`metafieldsSet`, `tagsAdd`) are not checked.

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
- New records get ids from a counter per type that starts above every seeded id, and at a
  13-digit number of the type's name at least (`state::first_id`: `stable_id("new_gid",
  type)`), so that a new draft order, refund or webhook subscription has an id like Shopify's
  and the seeded products', the same every run (`ShopState::new_gid`; the discount nodes share
  one counter, as they share ids on Shopify).
- **An index** finds the records of a type by their `handle` (products, collections, metaobjects,
  pages, blogs, articles, markets, channels) and the orders by their `number`, without reading
  every record (`state::INDEXED`; `ShopState::with_handle`, `handle_taken`, `max_number`, and
  `state::free_handle` for the `-1`, `-2` suffixes). Every write keeps it in step, a rollback
  too; the record `get_mut` handed out last is read as it is until the next write puts it back.
  The next order number is one above the highest order's (`orders::model::next_order_number`):
  deleting the last order makes its number the next one again, as it did when every order was
  read.
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
  `amountSpent`, `lastOrder`, `canDelete`), `customers::refresh_company_location` (a company
  location's `ordersCount` and `totalSpent`, its company's `totalSpent`),
  `content::attach_definition` (the seed's: a
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
  `take_events` hands them over. One mutation emits one event per topic and record: the resolver
  merges the repeats of a mutation into the first (`ShopState::merge_events_from`, one pass
  through a map of the topics and records), whose
  payload is built from the record as the mutation left it (an order created fulfilled that
  commits then ships a level's units reports the level once). Event ids are UUIDs made of the shop and a sequence that a reset
  does not restart, so a receiver never sees one twice.
- **Shops** (`Shops`): the default shop is the store data's (`shop.permanent_domain`); `create`
  makes another from the same seed (cloned, not seeded again) and moves it to its domain: its own
  `Shop` id (`stable_id("shop", domain)`), its `myshopifyDomain`, and every value naming the old
  domain or `Shop` GID rewritten (a custom primary domain stays). `reset` puts every shop back to
  the seed and keeps the shops; `reseed` seeds again from new store data, then resets.
- Store data edited while the server runs (without `--static`) reseeds every shop: what the
  Admin API changed is dropped, and the terminal says so. A change of checkout scenarios alone
  (`checkouts/`) does not: the seed reads no scenario ("Server wiring", 6). A shop the app is not
  installed on gets no owner of its Functions from a reseed (`app::remove_function_owners` after
  it). `DELETE /__lsf/admin` reseeds on demand.

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
   `LSF_NOT_SIMULATED`. Before it runs, a money input of 10^13 or more in the currency's unit
   (`Money`, or a `Decimal` amount: `MoneyInput.amount`, a discount's amount, …) answers a user
   error in every user error list of the payload, the rest `null` (`resolve::money`). Shopify
   publishes no limit; `state::value::MAX_MONEY_CENTS` is lsf's, so that sums and products of
   amounts hold in 64 bits. The mutations check what they multiply (a line's total, an order's)
   against the same bound.
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
   of a record (`null` when it is gone, or not of a possible type; left out of a list or a
   connection, as Shopify never lists a deleted resource, and not counted by the `Count` or the
   `Int` count of the array: `MetafieldDefinition.metafieldsCount`), an object is an inline object
   (its type the field's, or its `__typename` for an abstract one), an array of a connection is
   paged, an array of a list is mapped (cut to `first` when the list takes one, as
   `Product.options(first:)`). Before the value: `metafield(namespace:, key:)` and
   `metafields(namespace:, keys:)` of any owner read its `metafields`, `$app` and `$app:x` being
   the app's reserved namespaces (`app--<id>`, `app--<id>--x`; `metafield` without a namespace is
   in the app's). After it, when there is none: `legacyResourceId` (the number of the GID),
   `defaultCursor` (the cursor of the record in the `ID` order of its type), the `Count` of an
   array (`variantsCount` of `variants`, with the field's `query`), and a deprecated field whose
   reason is "Use `x` instead." (`x` a field, or a path of fields, of the same object without
   required arguments): `x`'s value, `Money` from the amount of a `MoneyV2` or the shop money of
   a `MoneyBag`, `MoneyV2` from a `MoneyBag`, `Int` from a `Count`, a scalar from a scalar of its
   kind (`resolve::successor_path`; `OTHER_VALUES` lists the two whose replacement holds another
   value).
6. **A default**: `null`; for a non-null field an empty list, `""`, `0`, `false`, `"0.00"` for
   `Money` and `"0.0"` for `Decimal`, the seed's fixed date for `DateTime`, the first enum value,
   or an object of defaults (the first possible type of an abstract one). Its path goes into
   `extensions.lsf.defaulted` (the first 50, with `defaultedCount` past them) and its `Type.field`
   into `defaultedFields`, except inside what lsf builds whole (payloads, objects of defaults). A
   list or a connection a record lacks is reported too (`Order.agreements`, `Product.events`: lsf
   does not compute them), unless lsf fills it as things happen (`resolve::SIMULATED_LISTS`:
   `returns`, `refunds`, `fulfillmentOrders`, …), where missing means none. An explicit `null`
   in a non-null field is a default too.

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
2. **Change through `m.state_mut()`**: the first call opens a savepoint of the shop
   (`ShopState::savepoint`), from which each write keeps what it overwrote; a payload with user
   errors (`userErrors` or `…UserErrors` not empty) or an error rolls back to it. Validating
   first costs nothing on failure; a mutation that finds a problem halfway can still answer it.
   A mutation that applies what it can and reports the rest (`productVariantsBulkUpdate` with
   `allowPartialUpdates`, `productCreateMedia`) calls `m.keep_changes()`: its user errors then
   leave the changes in place.
3. **Keep derived values right** with the domain's functions the seed uses (`refresh_product`,
   `refresh_customer`; for metafields `content::metafields::set`, which links the definition,
   after `content::metafields::check_owned_inputs` or `check_input`, which check them against it:
   every family's `metafields` input goes through them, and none creates a definition),
   `touch` what changed (`updatedAt`), and use `new_gid` for new records (or the id lsf-core
   derives, for what store sync writes back without an id: see "Store sync").
4. **Emit the events of what changed**: `state_mut().emit("products/update", &gid)`; a deletion
   with `state_mut().delete(&gid, "products/delete")`, which keeps the record for the payload. A
   change always emits an event: that is also how the api layer knows a shop changed. An event
   about something that is no record (a product on a publication) is
   `emit_with(topic, key, snapshot)`: its key names no record, and the payload is built
   from the snapshot. What a mutation keeps between requests that is not a record (an
   idempotency key and the answer it gave) is `set_memo`/`memo`: put back with the records
   when the mutation fails, not seen by the store sync.
5. **Say what is not simulated**: `m.note("productCreate: lsf ignores `category`")` for an
   input it ignores; an argument-dependent scope the access table cannot express is checked in
   the mutation (`tagsAdd` needs the write scope of the resource: `access::denied`).
6. **Use the shared pieces**: what Shopify would send outside goes to the outbox
   (`outbox::record`, see "The outbox"), what waits on time joins `domains::tick` (see "The
   shop's clock"), background work answers `resolve::done_job`, and a bulk mutation's `ids` |
   `search` | `savedSearchId` is `resolve::selection`.

`m.webhooks()` gives the app's webhooks (their registry of subscriptions), `m.grant()` the token's
scopes, `m.version()` the request's version (`m.api_version()` its name, `m.schema()` its
schema; `r.version()` and `r.schema()` in a resolver), `m.now()` and `m.timestamp()` the shop's
clock. A field or a mutation that only some versions have needs no check, and one that reads
differently in a version is an adapter of the version, not a branch: see "API versions".

**Checking a registry**: `Registry::check(schema)` lists what is registered for a field, a
mutation or a type the schema does not have; `Registry::check_versions()` what no version lsf
knows has, which a test runs on `Registry::standard()`.

### Search syntax (`search`)

The `query` argument of connections, and the `filter` of webhook subscriptions, use Shopify's
search syntax (shopify.dev, "Search query syntax"): terms, `field:value`, comparators (`:<`,
`:<=`, `:>`, `:>=`), `AND`, `OR` (which binds tighter: `bob OR norman Shopify` is
`(bob OR norman) AND Shopify`), `NOT`, `-`, parentheses, quoted phrases, `*` prefix and existence
(`field:*`), backslash escapes. Dates are what the reference documents: `now`, a year, a month,
a day, a time with or without a zone, and ranges (`2021-01-01..2021-01-02`), each a period
(`created_at:<=2024` is up to the end of 2024), in the shop's time zone. Shopify documents no
relative date (`-7d`) for search; lsf has none. That zone (`search::Options::of`: the shop's
clock and `timezoneOffsetMinutes`) is also how every input or record that gives a day without a
time is read (`day_start`, `day_end`, `day_of`, `instant`): a gift card's `expiresOn` and an
analytics target's dates as search values, a store credit's `expiresAt` given as a date, a
shipping label's date (`SHIPPING_DATE_IN_THE_PAST` compares days of the shop), the point of
sale's `startDate`/`endDate`.

`search` parses a query into a tree (`Query::parse`, never an error: a malformed query is read
as far as it goes, with warnings, each given once) and matches a record through the `Searchable` trait, which
each domain implements from the filters its connections document (`documented_filters`, from
the reference of the request's version: `schemas/admin/<v>/search.json`, `search::for_version(v)`;
`documented_filters` is the default version's). A value is tokenized text, a keyword,
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
the input is wrong, changes nothing when it fails, and emits the **events** of what it changed.

**Rollback** (`ShopState::savepoint`, `rollback`, `release`): from the first write of a mutation
on, the shop keeps an undo log, each write with what it overwrote: a record's entry before its
first in-place change (one copy per record and savepoint), a record added (removed again), a
record removed or retyped (put back at its positions among the records and among those of its
type), a counter's value, a blob's, a memo's, a setting's. A failure replays the log backwards,
then truncates the events and the changes tracked for the store sync to their length at the
savepoint, and puts back the event sequence, the revision and the clock: the shop is as it was,
the order of every map included. **A panic** (a bug of lsf) is a failure too: `mutation::run`
catches it, rolls back (`ShopState::rollback_unwound`, which also closes the savepoints the
panic left open) and answers Shopify's internal error (`mutation::internal_error`: its message,
`extensions.code = "INTERNAL_SERVER_ERROR"`, a `requestId`, and the panic's message in
`extensions.lsf.panic`). The resolver runs the mutation and what follows it
(`markets::after_mutation`, `segments::follow`, `catalog::after_mutation`) within one more
savepoint, so that a panic in a follow-up undoes the mutation as well; `AdminApi::change_shop`
does the same for its change (`ShopState::unwinding`), then lets the panic go on. Without this,
a mutation that panicked after its first write kept it, and a follow-up that panicked on what a
mutation stored (a segment whose date overflowed) failed every later request. The shop lock's
poisoning is still recovered (`api::admin::lock`). Savepoints nest: a bulk operation's mutation per line opens its
own, which an outer failure still undoes once released. A success closes the log. The cost is
what the mutation wrote: a copy of the shop, as it was done first, took 180 to 400 ms on 198,000
records, before every mutation's first write.

**Jobs** (`resolve::done_job`): the work Shopify does in the background (a bulk delete, a
collection's products added, an order cancelled, a bulk billing) is done within the mutation,
which answers a `Job` already `done`: a record of the shop, its id a UUID per job, so that
`node(id:)` and `job(id:)` find it. A family that keeps more on its job (a bulk billing's cycles)
writes it on that record.

**Coverage**: every mutation of every version lsf knows is registered by a family
(`tests/coverage.rs` fails on one that is not, so a new version's mutations are written before
it is served). A mutation no family registered would answer a GraphQL error with
`extensions.code = "LSF_NOT_SIMULATED"` (`resolve::not_simulated`), never a silent success;
`GET /__lsf/admin` lists the registered ones. As references to read first: `productCreate`,
`productUpdate`, `productDelete`, `tagsAdd`, `tagsRemove`, `webhookSubscriptionCreate`,
`webhookSubscriptionUpdate`, `webhookSubscriptionDelete`. The families' mutations are listed in
the user guide (`docs/admin.md`, "What lsf simulates").

## The HTTP side (`api`)

`api::AdminApi` holds the server's shops, each behind a lock of its own (the seed in
`state::Shops`, which makes new ones with `new_shop` and resets them with `reset_shop`), the
tokens (`auth::Tokens`, the app installed on every shop with its scopes and its default token),
the webhooks (`Webhooks::for_app`, with `search::filter` plugged in as the `PayloadFilter`), the
throttle, and the app (`auth::AppAuth`: the client id, the secret of `auth::client_secret`, the
app URL). `AdminApi::handle(&HttpRequest) -> Option<HttpResponse>` answers:

| Path | What answers |
|---|---|
| `POST\|GET /admin/api/<version>/graphql.json` | The version (`Served::resolve`: served, fallen forward, or `404`), the token (`401` as Shopify, `X-Lsf-Reason` saying why), what the shop's clock passed since the last request (`domains::tick`, see "The shop's clock"), the GraphQL request (JSON, `application/graphql`, `?query=`; `400` `{"errors": {"query": "Required parameter missing or invalid"}}` without one), then `graphql::run` with the bucket of the shop and app, `X-Shopify-API-Version` and `X-Request-Id`. The events the request emitted are handed to `Webhooks::emit` with a lookup over the shop: the payloads are built then, the deliveries queued for the worker, nothing waits on the backend. |
| other `/admin/api/<v>/…`, `/admin/*.json`, another `/admin/oauth/*` | `404` in JSON: another version, or the REST Admin API, which lsf does not simulate (`version::rest_not_served_by`, also the `X-Lsf-Reason`). |
| `GET /admin/oauth/access_scopes.json` | The REST endpoint of the token's scopes (`AdminApi::access_scopes`), each write scope followed by the read scope it implies; `401` as the GraphQL endpoint. |
| `GET /__lsf/graphiql` | `graphiql`: lsf's GraphiQL, one page embedded in the binary (`assets/graphiql/graphiql.html`), which reads the shops, tokens and versions of `GET /__lsf/admin` and posts to the GraphQL endpoint. |
| `/admin/oauth/*`, `/__lsf/admin/session-token`, `/__lsf/admin/launch` | `auth::oauth::handle`, with `oauth::Shops` over the shops and the system clock. |
| `/__lsf/admin` (`GET` status, `DELETE` reset), `/__lsf/admin/shops`, `/__lsf/admin/uninstall`, `/__lsf/admin/install` | The control endpoints of `docs/control-api.md`. |
| `/__lsf/admin/charges/<n>` (`GET` the page, `POST …/approve`, `…/decline`), `/__lsf/admin/uploads/<shop>[/<key>]`, `/__lsf/admin/bulk/<shop>/<n>.jsonl` | `api::app`: the merchant's approval of a charge, staged uploads, bulk operations' results; no token, as Shopify's admin page and signed URLs need none from the app. The URLs the mutations answer for them are on the origin the request reached lsf at (`http://<Host>`, `ShopResolver::with_origin`). |
| `/__lsf/admin/pricing-plans` (`GET`, `PUT`, `DELETE`), `…/<plan>/select` | `api::pricing`: the plans of managed pricing and the merchant's choice of one (below, "Managed pricing"); no token. |
| `POST\|GET /<partner id>/api/<version>/graphql.json`, `GET /__lsf/admin/partner` | `api::partner`: the Partner API (`admin::partner`, below, "The Partner API"), its token checked; where a test finds its URL, token and GIDs. |
| `/__lsf/admin/card-sessions`, `/__lsf/admin/subscriptions`, `/__lsf/admin/billing-attempts/<n>/challenge[/complete\|/fail]`, `/__lsf/admin/payment-methods/<token>/update` | `api::subscriptions`: lsf's card vault, the log of the emails and charges, the customer's 3D Secure and card update pages; no token. |
| `/__lsf/admin/clock`, `/__lsf/admin/outbox`, `/__lsf/admin/emails`, `/__lsf/admin/disputes[/<n>/won\|lost\|accepted]`, `/__lsf/admin/orders/protect`, `/__lsf/admin/orders/link-request`, `/__lsf/admin/tax-app/activate` | `api::simulate`: the world outside the shop, which a test drives (see "The outbox", and "Payments, store credit, disputes and the clock" under "Orders"). No token. |
| `/__lsf/webhooks*` | `Webhooks::handle`, the payload of a trigger built from the record named, else the first record of the topic's type (`api::samples`), else lsf's sample of the topic. |
| `GET /admin`, `/admin/apps/<app>/*`, other `GET /admin/*` pages, `/__lsf/admin/app-bridge.js`, `/__lsf/admin/assets/*`, `/__lsf/admin/resources` | `api::embedded`: the admin of an embedded app (below). |

`lsf` builds one in `ServerState` (`crates/cli/src/server/admin.rs`), routes its paths before the
storefront, the checkout and the control API, records the orders `place_order` places
(`AdminApi::record_order`, while the cart is taken, so that the order's number is the Admin
API's; without the Admin API a session numbers its orders on its own), and reseeds it when the store data reloads (`AdminApi::reseed`:
what the Admin API changed is dropped and the terminal says so; the subscriptions the backend made
stay, as the app's setup of the shop). It turns store sync on (`AdminApi::sync_store`): see
"Server wiring" under "Store sync". `lsf app webhook trigger` builds its payloads with
`api::samples::ShopSamples` over the shop seeded from the store data.

## The admin of an embedded app (`api::embedded`, `assets/admin`)

An embedded app's front end runs App Bridge, Shopify's script (`cdn.shopify.com/shopifycloud/app-bridge.js`),
which talks to Shopify's admin around the app's frame: it gets the session tokens from it, and
the admin draws the title bar, the navigation, the save bar, toasts and the resource picker. lsf
serves both halves, so that a developer previews the app offline:

- **The admin**: `GET /admin/apps/<app>/<path>` (`<app>` the handle, or the client id). A page of
  lsf's origin (`assets/admin/admin.html`, `admin.css`, `admin.js`): a top bar, the navigation
  (the shop's name, "Preview": the storefront and the checkout the server runs, which the CLI
  gives with `AdminApi::with_previews`, the app and its items; no link to a section of the
  admin lsf does not simulate), the
  app's title bar, and the app in an `<iframe>` at `oauth::launch_url` of `<path>`, minted when
  the page is rendered: a new `hmac` and `id_token` on every load. `shop`, `user` and `locale` of
  the query are the admin's; the others go to the app, the launch's own parameters dropped.
- **App Bridge**: `GET /__lsf/admin/app-bridge.js` (`assets/admin/app-bridge.js`), a classic
  script the app's page loads first, in place of Shopify's.

Why these paths: `AppInstallation.launchUrl` and `App.launchUrl` already answer
`https://<shop>/admin/apps/<handle>`, Shopify's `<shop>/admin/apps/<handle>` (`admin.shopify.com/store/<name>/apps/<handle>`
today) has the app's path after it, and `/admin/*` is the Admin API's prefix that the server
already routes to `AdminApi::handle` before the storefront: the links lsf writes now open a page,
and an app path is a path of the admin's URL. `/__lsf/admin` itself stays the JSON control API.
`GET /admin` and `/admin/apps` redirect to the app; `GET /admin/<other>` (where
`shopify:admin/products` goes) is a page that says lsf has no such page, `404`. `/admin/api/*`
and `/admin/oauth/*` are the Admin API's and OAuth's, as before.

### The protocol

The app's frame is on the app's origin, the admin on lsf's. They talk with `postMessage`, each
side checking the other: the admin takes the messages of its frame's window
(`event.source === iframe.contentWindow`) from the origin of the launch URL; App Bridge takes those
of `window.parent` from the origin of its own `src`, the admin's (an admin on another host name,
`localhost` for `127.0.0.1`, is told so in the console and in a banner). Each message is tagged
(`lsf: "app-bridge"` from the frame, `lsf: "admin"` from the admin):

| From the frame | What the admin does |
|---|---|
| `hello` (`apiKey`, `scriptOrigin`), posted to `*` every 200 ms for 10 s until welcomed: it says nothing secret | `welcome` to the app's origin: the shop, the locale, the client id, the user, the scopes. What the frame posts before it waits for it. |
| `request` `{id, method, params}`: `idToken`, `resourcePicker`, `picker`, `leaveConfirmation` | `response` `{id, ok, value \| error}`. `idToken` is `POST /__lsf/admin/session-token` for the admin's shop and user. |
| `title`, `nav`, `saveBar` (sent when they change: a `MutationObserver` of the page, the form events), `loading`, `toast`, `toast-hide`, `modal`, `history` (the app's path), `navigate` (`shopify:` links, `_top`, a reauthorization), `unsupported`, `unload` | Drawn, or followed: the admin's URL follows `history` with `replaceState` (the frame's own history is the browser's); `shopify:/admin/…` goes to lsf's page of it. |

| From the admin | What App Bridge does |
|---|---|
| `event` `title.action` `{index}`, `navigate` `{path \| home}`, `saveBar.save`, `saveBar.discard`, `toast.action`, `toast.dismiss` | Clicks the page's element (a title bar button, a nav link, the save bar's buttons), submits or resets the `data-save-bar` forms, calls the toast's callbacks. |

### App Bridge

`app-bridge.js` defines `window.shopify` with every name of `@shopify/app-bridge-types` 0.7.4
(whose build downloads `cdn.shopify.com/shopifycloud/app-bridge.d.ts`), and the behaviour
of the CDN script 0.41.4 where shopify.dev leaves it open:

- **Before Shopify's**: the CDN script's first statement is `if ("then" in (self.shopify?.ready || {})) return;`.
  lsf's defines `shopify.ready` first, so a page that keeps AppProvider's tag runs on lsf's.
  Loaded after Shopify's, lsf's leaves it and says so.
- **Configuration** as the CDN script reads it, later wins: `sessionStorage["app-bridge-config"]`,
  a `shopify.config` set before, the `data-*` and query of App Bridge's `<script>` tags (Shopify's
  and lsf's, `/app-?bridge[/.-]/`) and `<script type="shopify/config">`, `<meta name="shopify-*">`,
  the page's `shop`, `host`, `locale`. Read again at `DOMContentLoaded`, since lsf's script runs
  before AppProvider's tag (which has `data-api-key`) is parsed.
- **`fetch`**: the rule of the CDN script (`Ft`): the page's protocol and host or a subdomain
  (the port is not compared), or `appOrigins`; the ID token unless `Authorization` is set,
  `X-Requested-With`, `Accept-Language`; one retry on `X-Shopify-Retry-Invalid-Session-Request`;
  `X-Shopify-API-Request-Failure-Reauthorize-Url` sends the admin there and never answers. One
  departure: never lsf's own origin (the `src` of the script) while the page is elsewhere. On
  Shopify the admin and the CDN are never the app's host; locally both are `localhost`, and the
  headers made each `fetch()` of an icon of lsf's Polaris a preflighted request. lsf answers
  those preflights too (`OPTIONS` on `/__lsf/cdn/`, `/__lsf/polaris`, `/__lsf/admin/*` files,
  `/cdn/`: `204`, the asked headers echoed), for a page that sets the headers itself.
- **Navigation** (the CDN script's click handler, `history` and `open` patches): `shopify:` links
  (`/admin/*` only, else the same error), `app:` paths, `target="_top"`; while the save bar
  shows, a link asks the admin first. A plain `<a>` the app's router does not take reloads the
  frame without the session token: Shopify's breaks the same way, so lsf does not mend it, it
  says so in the console. Without Polaris (it did not load), `s-link[href]` and its kin dispatch
  `shopify:navigate` as Polaris does (`AppProvider` routes on it), and navigate when nothing read
  the event's `href`.
- **Polaris** is Shopify's, served rather than simulated: lsf-cli's `server::polaris` answers
  `/__lsf/cdn/<path>` (a path of `https://cdn.shopify.com/`) with a redirect to the CDN
  (`online`, the default) or with the copy in the binary (`offline`: `crates/cli/assets/
  shopify-cdn`, Polaris 2.0.0 and 1.1.0, their 844 icons and their fonts), whose scripts and
  stylesheets get `https://cdn.shopify.com/` rewritten to lsf's `/__lsf/cdn/` (the icon fetcher's
  own check of the prefix is rewritten with it). `--polaris`, `GET`/`PUT`/`DELETE /__lsf/polaris`.
  App Bridge's part: a capturing `error` listener on the window sees the page's
  `cdn.shopify.com/shopifycloud/polaris*.js` fail to load, asks `GET /__lsf/polaris`, and when it
  is `offline` appends `<script data-lsf-polaris src="<lsf>/__lsf/cdn/<the same path>">`. Once,
  and never when `s-page` is defined: two Polaris would define the same elements twice.
- **Outside a frame**, the page goes to lsf's admin (`/admin/apps/<apiKey>/<path>`), as Shopify's
  goes to Shopify's, unless `auto-redirect` is disabled. **`?shopify-reload=`** (the bounce page
  of Shopify's libraries) comes back with a new `id_token` in the URL; the CDN script fetches it
  with the token in a header and writes the document instead.
- **Elements**: `ui-modal`, `s-app-window`, `ui-save-bar` are custom elements with the methods
  and events of the types; `ui-title-bar`, `ui-nav-menu`, `s-app-nav` are read, not drawn (a
  style hides them and `s-page`'s title bar slots, as the CDN script's does).

The parts the admin draws, in the admin page: toasts, the loading bar, the title bar, the nav
items, the save bar (in place of the top bar, as Shopify's), the resource picker and the picker
(`<dialog>`s), the confirmations (leave, discard), and the dialog of an `edit:` intent.
**Modals are drawn in the app's frame**, from a shadow root on the element (its children stay
where the app put them, React keeps them, their handlers work), and Polaris's `s-modal` draws
itself there (App Bridge hears its `show` and `hide`). While one shows, the frame is **lifted
over the whole window**: the admin sends `modal.lift` with the frame's box, the page sets
`position: fixed` on its body in that box (the frame's background and radius, the scroll kept)
and a transparent image on its root (with none, the body's background would paint the whole
canvas), answers `lifted`, and the admin then makes the frame cover the window. The admin shows
through, under the modal's backdrop, and the modal is centered over the whole admin, as
Shopify's. `modal.unlift` puts the styles back. Shopify draws modals in a frame of its own; lsf
cannot move the app's DOM out of its frame without cutting React's handlers off. A modal's `src`
is an `<iframe name="lsf-modal:<n>">` on the app's origin, whose page takes the opener's
`shopify`.

**Intents**: `shopify.intents.invoke('edit:shopify/Product', {value})` (and `…/ProductVariant`,
`…/Collection`, or the object form) asks the admin, which fetches the resource from
`/__lsf/admin/resources` (`id:` or `variant_id:`) and shows it in a dialog: lsf has no editor,
so closing it completes the activity with `{code: 'closed'}`. Another intent completes with
`{code: 'error'}`, its message saying lsf does not simulate it, and joins the banner.

The resource picker reads `GET /__lsf/admin/resources`: one query of the shop
(`products` with their `variants` and `media`, or `collections`) run with every scope and no cost
limit (`graphql::prepare` and `execute`, not `graphql::run`), then written in the shape of the
types, with the names App Bridge kept from the older Admin API (`images[].originalSrc`,
`totalVariants`, `availablePublicationCount`, `inventoryManagement`, a variant's `weight`). The
picker's search box, `query` and `filter` (`draft`, `archived`, `hidden`) are one search in
Shopify's syntax; `selectionIds` are fetched by id (`id:`, `variant_id:`).

### What it says goes wrong

- Shopify's App Bridge in the frame: it posts `dispatch`, `getState`, `subscribe` to `*` before
  it knows its admin (`source` is its config), which the admin recognizes, and says what to add.
  Inside a frame it redirects nothing; it listens to Shopify's admin origins only (and
  `localhost`), so its `idToken()` fails after a minute and its `fetch()` waits.
- No `hello` 4 s after the frame loaded: the backend is down, the page failed (the library
  answers `410` to a bot's user agent, `401` to a revoked token), the frame was refused (the
  template's `Content-Security-Policy: frame-ancestors`), or the page loads no App Bridge.
- A call of what lsf does not simulate: listed in a banner.
- What the template lacks (`config::template_warnings`: no `setAbstractFetchFunc` in
  `app/shopify.server.ts`, no `boundary.headers` in `app/routes/app.tsx`): read again at each
  load of the admin and of a page of the frame (`GET /__lsf/admin/warnings`), so a fixed file
  loses its banner without a restart.
- A backend that bypasses lsf (`api::BackendWatch`): the frame's proxy reports a request with a
  session token (`id_token`, `Authorization: Bearer`) answered `400` or more (not `401`, the
  library's own flows, nor `410`, its bot answer) while lsf has received no token exchange and
  no Admin API call yet. A backend without `LSF_URL` does exactly that: its token exchange fails
  at Shopify (`500`), or its Admin API call does (Shopify's status, `404` for a shop it does not
  have). The terminal says it once; the admin shows it (App Bridge posts `failed` when a
  `fetch()` of the app fails, and the admin asks again) until a call of the backend arrives.

### What is not simulated

The admin itself (its pages, its search, its other apps); Polaris itself (lsf serves Shopify's,
or its copy of it, and does not draw a component of its own); `shopify.scanner`, `pos`,
`intents.register` and the intents other than `edit:` of a product, a variant or a collection,
`webVitals`, `support`,
`reviews`, `scopes.request` and `revoke`, `app.extensions`, `shopifyQL`, `sidekick`, `tools`
(they reject or warn with "lsf does not simulate shopify.<name>"); Direct API access
(`fetch("shopify:admin/api/…")`); `extension:` links; the mobile and POS environments; the
admin's keyboard shortcuts; `?shopify-reload=` by `fetch` and
`document.write`. `shopify.user()` has `accountAccess` `"Account owner"` or `"Full access"`,
lsf's choice: shopify.dev's example says `"full"`, and the CDN script leaves it to the admin.

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
| `Market` | `stable_id("market", handle)` of a declared market, `stable_id("market", "FR")` (upper-case code) of a country in none (the checkout's); `marketCreate`'s: `stable_id("market", handle)` too, the id the store sync reads |
| `MarketWebPresence` (primary domain), `AppCatalog` (Online Store) | `stable_id("market_web_presence", host)`, `stable_id("app_catalog", "online-store")`; the catalogs share one counter (`Catalog/<n>` names any) |
| `Metaobject` | `stable_id("metaobject", "<type>/<handle>")` (the checkout's, lsf-core's `metaobject_by_reference`), the type as the store data writes it (`$app:author` for the app's `app--<id>--author`: `content::data_type`) |
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
| `DeliveryProfile` (general), its `DeliveryLocationGroup`, `DeliveryZone`, `DeliveryCountry`, `DeliveryMethodDefinition`, `DeliveryRateDefinition` | `stable_id("delivery_profile", "general")`; `stable_id("delivery_location_group", "general")`, `stable_id("delivery_zone" \| "delivery_country", "general/rest-of-world")`, `stable_id("delivery_method_definition" \| "delivery_rate_definition", "general/<rate handle>")` |
| inline objects with an id | `MailingAddress/<lsf-core address id>?model_name=CustomerAddress` (address book) or `Address` (order); `ImageSource`/`CollectionImage`/`ArticleImage` + lsf-core's image id; `MenuItem`: `stable_id("menu_item", "<menu>/<i>/<j>…")`; `MarketRegionCountry`: `stable_id("market_region", code)`; others `stable_id(<snake type>, …)` |

Every id is a function of the data: the same data gives the same records every run.

The records the Admin API creates take their ids from a counter per type (`state::first_id`),
and in a shop other than the default one the counter is moved by the shop's slot times
`ID_STRIDE` (`ShopState::set_id_slot`): ids are unique across shops, as on Shopify. The slot is
a number of the domain alone (`state::id_slot`), one of 80 million from 10¹³ (a million ids per
type each, 14 digits), so that the order shops are made in does not change their ids; the next
free one when two live domains share it, once in 80 million pairs. An id
lsf derives from a name in the default shop (a metafield, a metaobject, a menu, a market) is a
new one of the counter in another shop (`ShopState::created_gid`). Charges count from 1 in the
default shop and from the shop's offset in another.

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
  `isRedeemable` (enabled, not expired by the shop's day in its time zone, a balance left;
  computed when it is read, as the clock runs), `lastCharacters`,
  `maskedCode`, `customer`, `recipientAttributes` when the recipient is a customer (`recipient`
  is a `Customer!`: the message and nickname of a card for someone else are not kept),
  `templateSuffix`, dates.

#### The catalog's mutations (`domains/catalog/`)

One submodule per area, each with its mutations and its custom fields: `products`,
`product_set`, `variants`, `options`, `media`, `collections`, `collection_sources` (the sources
of 2026-10 and the shareable ones), `collection_duplicate`, `bundles`, `combined_listings`,
`feeds` (product feeds), `inventory`, `locations`, `publications`, `legacy_publications` (the
deprecated `productPublish` and the three others), `selling_plans`, `gift_cards`,
`gift_card_extras`, `queries` (sort keys, search filters, the fields that take an argument) and
`extra_queries`. `records` is what they share: reading inputs, and making
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
- **Metafields** of the catalog's inputs (`productCreate`, `productUpdate`, `productSet` and its
  variants, `productVariantsBulkCreate`/`Update`, collections, locations, selling plans; and
  likewise the customers' and the app family's, in `customers::metafields` and
  `app::metafields`) are
  checked and kept as `metafieldsSet` checks and keeps them: `records::check_metafields` and
  `records::set_metafields` run the content family's `content::metafields::check_owned_inputs`
  and `content::metafields::set` (types, definitions and their validations, the app's
  namespaces), with the errors at the input's path and their codes fitted to the payload's code
  enum (`productSet`'s are `INVALID_METAFIELD`). Like `metafieldsSet`, they create no definition.
- **Partial updates** (`allowPartialUpdates`, `productCreateMedia`) call `m.keep_changes()`.
- **Publication events** name no record: `emit_with` with the product (or collection) and the
  publication as snapshot (`publications.rs`).
- **Collection sources**: a conditions source (`CollectionConditionsSource/<n>`) keeps its
  inclusion and exclusion inline (`matchType`, `conditions`: inline objects of their type with
  ids of it, `CollectionSourceInclusionConditionProductTag/<n>`; `selections`: `{product,
  variantIds}`), `shareable`, `app` (a shareable one's) and the collection that owns it
  (`lsfCollection`); a sub-collection source (`CollectionSubCollectionsSource/<n>`) its
  `collections`. A collection lists them in `sources`, and is then a smart collection whose
  membership is their union (`collection_sources::member`); `apply_rules` evaluates the
  collections whose sources read other collections after the others. A collection without
  source records answers the one source it amounts to, built (`legacy_source`: its rules as
  typed conditions, else its products as selections), and makes it a record when it gets more
  sources.
- **Bundles** (`bundles.rs`): a component is a `ProductVariantComponent` record
  (`productVariant`, `quantity`), which the parent variant lists in `productVariantComponents`
  (`requiresComponents: true`; the product's `hasVariantsThatRequiresComponents`). A product
  fixed bundle keeps its components on the product (`bundleComponents`, inline, as the schema
  reads them; `bundleConsolidatedOptions`) and the input it was made from (`lsfBundle`), which
  `productBundleUpdate` makes its options and variants again from. `refresh_product` derives a
  bundle variant's `inventoryQuantity` (the fewest bundles its tracked components make) and
  `availableForSale` (every component sells), and then refreshes the products of which the
  product's variants are components (`bundles::refresh_parents`): every change of a component's
  stock reaches its bundles. The parent's own inventory item is not tracked: an order's line of
  a bundle takes its components' stock instead (`orders::model::stock_parts`: each component's
  item, its quantity per bundle), committed, shipped, moved and restocked part by part, and kept
  on the line per component item (`lsfStock.parts.<item> = {variant, perUnit, committed,
  shipped}`); `short_parts` is what `DECREMENT_OBEYING_POLICY` and billing attempts check. An
  inventory change of a component refreshes and compares its bundles' products too
  (`bundles::with_parents`, in `move_inventory` and the inventory mutations), and a bundle is in
  stock while its components make one, so it gets its own `variants/in_stock` and
  `variants/out_of_stock`. A deleted variant (`remove_variant`, `delete_product`) leaves the
  bundles it was a component of first (`bundles::removing_variants`: their component lists, then
  their products refreshed, with `products/update`), and a deleted bundle takes its
  `ProductVariantComponent`s along, so that no component names a variant that is gone.
- **Combined listings** (`combined_listings.rs`): `combinedListingRole` on the products, the
  parent's children under `lsfCombinedListing` (`[{product, parentVariant}]`), a child's parent
  under `lsfCombinedListingParent`.
- **Product feeds** (`feeds.rs`): `ProductFeed` records (`channelId`, `country`, `language`,
  `status`). A sync event names no record (`<feed GID>?product_id=<n>&sync=<uuid>`): its
  snapshot is the feed, the product (and a copy, for a deleted one) and the metadata, which
  `webhooks::payload::feeds` builds the payload from. `catalog::after_mutation` (run by the
  resolver after `markets::after_mutation`) turns the listing events of a mutation on a
  channel's publication (`product_listings/*`) and its stock changes into incremental syncs, and
  recomputes the feeds' statuses when a market, a locale or a channel changed.
- **The outside world**: the notifications of a gift card are emails in the shop's outbox
  (see "The outbox"), never sent.

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
  `roleAssignments`, dates, `metafields`. The data's counters, given without the orders behind
  them, are kept under `lsfOrdersBase`; `customers::refresh_company_location` (which
  `refresh_order` runs) adds the orders placed for the location (`purchasingEntity` a
  `PurchasingCompany`: a B2B order of the checkout) to them, and sums the company's
  `totalSpent` again.
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

### Segments, marketing, analytics, privacy and the storefront's checkouts

Each in a module of its own, whose header has the rules and their sources:

| Module | What |
|---|---|
| `segments` | `Segment` records (ids of the counter); `segments::language` parses the segment query language (a lexer and a recursive parser, errors `Query Line L Column C: …` at the clause at fault; parentheses nest 100 deep at most, `MAX_DEPTH`, as in the search syntax, since the parser recurses once per `(`; an offset reaches 10,000 years at most, `MAX_OFFSET_YEARS`, and the evaluator's date arithmetic is checked: a date past chrono's matches nothing), `segments::filters` lists the 33 filters of the reference (type, enum values, function parameters), `segments::evaluate` reads them on a customer (`Facts`: computed once per customer and request, from the customer, their addresses, orders and line items, company contacts, store credit accounts, abandoned checkouts and metafields; dates as whole days in the shop's zone). `customerSegmentMembers` pages the generic connection without its `query`, `sortKey` and `reverse` (they are the segment's), then adds `totalCount` and `statistics`. |
| `segments::saved_searches` | `SavedSearch` records (`searchTerms` and `filters` split from the query by `search::Query`); `resolve::search` applies the query of a connection's `savedSearchId` under the connection's own (`(saved) AND (query)`); `resolve::selection` reads the `ids` \| `search` \| `savedSearchId` of a bulk mutation (a saved search of the mutation's resource, `query_for`). |
| `customer_merge` | `customerMerge` (the kept customer by shopify.dev's rules; the other's orders, draft orders, addresses, metafields, tags and note moved, then deleted), `customerMergePreview`, `customerMergeJobStatus` (a memo per job), `Customer.mergeable`; `customers/purchasing_summary`. |
| `privacy` | Data erasure (a memo per customer: pending 10 days, then redacted by `process_due_erasures`), `dataSaleOptOut`, the WhatsApp consent (on the customer, copied to `defaultPhoneNumber` by `customers::contact_fields`), `ConsentPolicy` records, the privacy features (a memo). |
| `marketing` | `MarketingActivity` and `MarketingEvent` records (the app's remote id, channel handle and engagements under `lsf…` keys), the errors of `MarketingActivityUserErrorCode` with the code's description as message. |
| `analytics` | `AnalyticsAnnotation`, `AnalyticsTarget` records; `analytics::shopifyql`, a ShopifyQL subset (`FROM sales`) over a row per line sold, per order (its shipping and taxes) and per refund; a condition's `AND` and `OR` hold their operands in a list and a chain of `NOT` is counted, so that only parentheses nest it, 100 deep at most (`MAX_DEPTH`); offsets within 10,000 years (`MAX_OFFSET_YEARS`), the date arithmetic checked (a date past chrono's is `Syntax Error: '…' is not a date`). |
| `staff` | The shop's owner, `StaffMember/1` (`staff::owner_gid`; the platform family seeds the record, the tokens' user, and `staff::seed` makes it the shop's `accountOwner`), `CompanyLocationStaffMemberAssignment` records, and the deprecated tax registration mutations on the companies family's `change_tax_settings`. |
| `checkouts` | `AbandonedCheckout` records of the local checkout (one per cart token and order generation, a memo naming it), `Abandonment` records for checkouts of customers, the cart events (no record: the event's snapshot), what is abandoned when read (`is_listed`: contact, and more than ten minutes since the last change by the shop's clock), the three-month cleanup. |

**What follows a change** (`segments::follow`): the mutation framework calls it after each
mutation (`resolve::ShopResolver`, before the events are merged), and `AdminApi::change_shop`
after the changes the server makes (an order of the checkout, a cart, a checkout). From the
events since the mutation began, it finds the customers they concern (their own topics, the
customer of an order, a refund, a company contact or a checkout), evaluates every segment again
for them against the members it remembers (`segments/members/<GID>`, a memo), and emits
`customer.joined_segment` and `customer.left_segment` under a key that names no record
(`segment-membership/<customer>/<segment>`), so that one customer's moves in two segments are
two events. It also emits `customers/purchasing_summary` when an order of the customer changed
and their numbers did, only when the app subscribes to the topic (`Webhooks::subscribes`): the
topic follows every order flow, which the other families' tests and webhooks do not expect.

**Time**: the due erasures and the abandoned checkouts past three months are the shop's tick's
(see "The shop's clock"). **Audit events** (`api::audit`): after each GraphQL request, when the app
subscribes to `audit_events/admin_api_activity`, an event built from the request (no change of
the shop) is handed to the webhooks.

**The server** (`crates/cli/src/server`): `Visit::update_session` compares the cart's lines, note
and attributes before and after a change and reports a changed cart (`admin::record_cart`); the
checkout's `compute` reports what it computed (`admin::record_checkout`, which emits only when
the fields differ from the record's), and `place_order` completes it once the cart is taken.

### Orders (`domains/orders.rs`)

An order of the data or of the checkout is first an `OrderDraft`; `insert_order` makes its
records, so both are the same shape.

- **`Order`**: `name`, `number` (1001 for `#1001`), `createdAt`, `processedAt`, `updatedAt`,
  `cancelledAt` (the creation date of a cancelled order, the only date the data has) with
  `cancelReason: OTHER` *(chosen)*, `confirmed`, `test: false`, `email`, `phone`, `customer`,
  `purchasingEntity`, `customerAcceptsMarketing`, `currencyCode`, `presentmentCurrencyCode` (the
  shop's currency, unless the order is presented in another: `lsfPresentment`, see below),
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
- **`FulfillmentOrder`** (every order has one, at the order's location: for the seed's, the
  first; for an order the Admin API makes without one, `model::fulfilling_location`, the primary
  location when it can give every line, else the first active one that can): `status` (`OPEN`,
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
  server's `place_order` has (the `CheckoutResult`, the time, the customer, the contact, the
  address, the note, the attributes, the payment method's name as the gateway, the company
  location of a B2B buyer, which makes `purchasingEntity` a `PurchasingCompany`), keeps the
  session's number when no order of the shop has it, else gives the shop's next one
  (`model::next_order_number`, which `orderCreate` and a completed draft order take too: no two
  orders share a name; an id given and taken gets a new one too), a GID of the
  shop's counter, as `orderCreate`'s (not made of the number), answers its GID and number
  (`RecordedOrder`; the checkout links its thank-you page to that GID and shows the session's
  number, which a test reads the same every run), links a guest whose email
  is a customer's (ignoring case) to that customer, as Shopify does (Help Center behaviour, not
  on shopify.dev), refreshes the customer, and emits `orders/create`,
  `order_transactions/create` for the payment and, when paid in full, `orders/paid`. It commits
  no stock: the local checkout keeps the stock of the data *(chosen: test purchases never sell a
  product out)*. `lsf`'s `place_order` calls it through `AdminApi::record_order`.
- **Cart metafields copied onto the order**: before `orders/create`, `copy_cart_metafields` sets
  on the order each metafield of the cart (`PlacedOrder::metafields`, what the checkout UI
  extensions wrote) whose namespace (as stored: `$app:x` is `app--<app id>--x`) and key an ORDER
  definition has with `capabilities.cartToOrderCopyable.enabled`, as Shopify does from 2026-04
  ("Use metafield capabilities"); nothing without such a definition. It is set with
  `content::metafields::set` (linked to its definition), of the definition's type, the value
  checked against the type and the definition's validations: a value they refuse is not copied
  *(chosen: shopify.dev does not say what Shopify does with it)*. The server's order status page
  reads the order's metafields (`content::metafields::of_owner`) as `order` entries of
  `appMetafields`, for this order and for those of the store data.

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
- **Presentment** (`model::Presentment`, an order a subscription contract in another currency
  bills): the family computes every amount in the shop's currency, as `shopMoney`;
  `model::apply_presentment`, which `insert_order` and `refresh_order` run, rewrites the
  `presentmentMoney` of every `MoneyBag` of the order and of the records it holds (two records
  deep: line items, transactions, refunds and their lines, fulfillments, fulfillment orders,
  returns) as the shop amount at the order's rate, rounded per amount. A flow that builds its
  amounts in the shop's currency (a refund, an edit, a return) therefore leaves the order in its
  presentment currency once it refreshes it. Orders in the shop's currency are untouched.
- **Stock** (the levels of the catalog, changed in place): an order that claims stock commits it
  (`available` to `committed`: `orderCreate` with a `DECREMENT_…` behaviour, a completed draft
  order, units an edit adds), and each line item records what it committed, by location, and
  what of it shipped (`lsfStock`: `{"committed": {"<location GID>": 2}, "shipped": 0}`; a
  bundle's line, its components', under `parts`: see "Bundles").
  Shipping takes units out of `committed` and `on_hand` as far as the line committed them at
  that location; a restock puts back what the line took (`CANCEL` from what it committed,
  `RETURN` to `on_hand` as far as its units had shipped from stock); a fulfillment order moved
  takes the line's commitment along. An order of the store data or of the checkout took nothing,
  so shipping or restocking it leaves the stock as it is, never moving what another order
  committed. Each change of a tracked item is followed as an inventory mutation of the catalog
  follows one (`catalog::after_inventory_change`, then `catalog::product_changed`): the
  variants' `availableForSale`, `variants/in_stock` or `variants/out_of_stock` when the stock
  crossed zero, the smart collections of `VARIANT_INVENTORY`, then `inventory_levels/update`.
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
  field for it; its delivery rates (`checkout.shipping_rates`) are the setting
  `deliveryRates`, which `draftOrderAvailableDeliveryOptions` offers.

#### Payments, store credit, disputes and the clock

The modules `payments`, `store_credit`, `shopify_payments`, `risk`, `requested_edits`,
`edit_extras`, `draft_extras`, `return_extras` and `tax` of `domains/orders/` (and
`gift_card_extras` of the catalog) carry the rest of the family; each module's header says
what shopify.dev documents and what lsf chose. What they keep:

- **Payment terms** (`PaymentTerms`, `PaymentSchedule`, of the nine `PaymentTermsTemplate`s the
  seed makes with the ids of `paymentTermsTemplates`' example) name their order or draft; the
  order or draft names them in `paymentTerms`. A schedule's amounts follow its order:
  `refresh_order` runs `payments::refresh_terms` (`balanceDue` the outstanding amount,
  `totalBalance` what is still to pay or authorize, `completedAt` once nothing is left, the due
  date of terms due on fulfillment once fulfilled). Their dates for the webhooks are
  `lsfCreatedAt` and `lsfUpdatedAt`: GraphQL's types have none. A draft's terms move to its
  order (`payments::attach_to_order`), issued then if they were not.
- **Tender transactions**: every successful sale, capture, refund or change makes one
  (`payments::record_tender`, from `model::add_transaction` and for the seed's transactions in
  `insert_order`), and with it Shopify Payments' balance transaction when it processed the
  money (`shopify_payments::record_balance`: not through a manual gateway). The seed's make
  their records without events; the transactions' events go with `order_transactions/create`.
- **Mandates** are not records: an order's vaulted payment methods are the checkout's card
  (`lsfMandates` on an order that left a balance) and its customer's `CustomerPaymentMethod`s,
  read through the schema's fields; their ids are made of the order and the method.
- **Store credit**: `StoreCreditAccount` per owner and currency (the owner lists them in
  `storeCreditAccounts`), credit and debit transactions as records, expirations inline (their
  type is not a `Node`) in the account's `transactions`.
- **Shopify Payments**: one `ShopifyPaymentsAccount` and one `ShopifyPaymentsBankAccount` (the
  seed's; what they take from the shop is read when they are, the shop being seeded after the
  orders), `ShopifyPaymentsBalanceTransaction`s, `ShopifyPaymentsPayout`s,
  `ShopifyPaymentsDispute`s and their `ShopifyPaymentsDisputeEvidence`.
- **Returns**: `ExchangeLineItem`s (`lsfUnitPrice`, `lsfDiscount`), restocking and return
  shipping fees inline, a `ReverseFulfillmentOrder` per open return (the seed's open and closed
  returns too) with its line items (`lsfReturnLineItem`), `ReverseDelivery` and its line items,
  `ReverseFulfillmentOrderDisposition`s (`lsfLineItem`; the reverse fulfillment order's
  `lsfLastDispositions` are what its webhook lists). `returns/update` reads the return's
  `lsfUpdateRemovals`.
- **Requested order edits** (`RequestedOrderEdit` and its removals, `lsfQuantityAtRequest`),
  **risk assessments** (inline in the order's `lsfRiskAssessments`), **attribution definitions**
  (`OrderAttributionDefinition`), **the tax app's configuration** (a `TaxAppConfiguration` under
  the key `TaxAppConfiguration/<app id>`: its type has no id).
- **The clock**: `orders::tick`, the family's part of the shop's tick (see "The shop's clock"):
  `payment_schedules/due` once per schedule that came due (`lsfDueSent`), store credit that
  expired, a payout per day of Shopify Payments the shop's day left behind.
- **The emails** Shopify would send (reminders, invoices, store credit, return labels, order
  links) are in the shop's outbox (see "The outbox").
- **What the outside world does** that no mutation does (a bank's dispute and its decision,
  Shopify Protect, a buyer's link request, the merchant turning a tax app on) are functions of
  the domain (`shopify_payments::open_dispute`, `risk::set_protect`, `risk::request_link`,
  `tax::activate`) that `api::simulate` runs through `AdminApi::change_shop`, which delivers
  their webhooks.

### Logistics (`domains/logistics.rs`)

Seeded, so that a shop has what every Shopify shop has:

- **`DeliveryProfile`**, the general one: `name: "General profile"`, `default: true`, `version`,
  `createdAt`, `profileLocationGroups` (inline `DeliveryProfileLocationGroup`s: the
  `locationGroup` and its `locationGroupZones`, inline `DeliveryLocationGroupZone`s with their
  `zone` and `methodDefinitions`). One **`DeliveryLocationGroup`** with every location that is not
  a fulfillment service's, one **`DeliveryZone`** "Rest of world" *(chosen)* with one
  **`DeliveryCountry`** (`code.restOfWorld: true`), and a **`DeliveryMethodDefinition`** (with
  its **`DeliveryRateDefinition`**) per shipping rate of `checkout.shipping_rates`, in order,
  keeping the rate's handle under `lsfHandle`: the store data offers its rates to every address.
  Pickup and local delivery rates are the locations' on Shopify, not methods of a profile.
- **`DeliveryPromiseSetting`** (no id: kept under its type's name): delivery dates off, no
  processing time.
- The shop's shipping packages, which have no GraphQL type: a memo (`logistics/shipping-packages`)
  with Shopify's "Sample box" (`gid://shopify/ShippingPackage/1`, default) *(its size is lsf's)*.

What the mutations make (`domains/logistics/`: `transfers`, `shipments`, `stock`,
`fulfillment_orders`, `constraint_rules`, `labels`, `carriers`, `profiles`, `delivery`; each
module's header has the rules and the sources):

- **`InventoryTransfer`** (`name` `#T<n>`, `status`, `origin` and `destination`
  `LocationSnapshot`s inline, `referenceName`, `note`, `tags`, `dateCreated`, `lineItems`,
  `shipments`, `totalQuantity`, `receivedQuantity`, `metafields`) and
  **`InventoryTransferLineItem`** (`inventoryItem`, `title`, `totalQuantity`, `shippedQuantity`,
  `pickedForShipmentQuantity`, `processableQuantity`, `shippableQuantity`), refreshed from the
  shipments by `transfers::refresh` after every change. **`InventoryShipment`** (`name`
  `T<n>-<k>`, `status`, `barcode`, `tracking`, dates, the totals) and
  **`InventoryShipmentLineItem`** (`quantity`, `acceptedQuantity`, `rejectedQuantity`,
  `canceledQuantity`, `unreceivedQuantity`). GraphQL has no link from a shipment to its transfer:
  lsf keeps it under `lsfTransfer` (the webhooks' `inventory_transfer_id`).
- **Stock** (`logistics::stock`, through `orders::model::move_inventory`, which follows a change
  as the catalog's inventory mutations do): ready to ship commits at the origin; in transit, the
  units leave the origin's `committed` (what the transfer committed, kept per line under
  `lsfCommitted`; a transfer marked ready to ship is `lsfCommits`) or `available`, and `on_hand`,
  and enter the destination's `incoming` (a level made when the destination did not stock the
  item, `inventory_levels/connect`); received, accepted units become `available` and `on_hand`.
  `transfers::settle` keeps what a transfer commits equal to what it has not shipped while it is
  ready to ship or in progress, and nothing otherwise. A move at a location the item is not
  stocked at moves nothing (`model::move_inventory` never falls back to another level when a
  location is given), and no level is made at an inactive location. `locationDeactivate` with a
  `destinationLocationId` moves the levels there, then the open transfers that start or end at
  the location (`transfers::relocate`: their snapshot names the destination,
  `inventory_transfers/updated`), so that a shipment in transit is received where its incoming
  units went, as Shopify relocates "incoming movements".
- **`FulfillmentOrder`**s split, merged, rerouted (the order's committed stock moved with
  `model::move_line_commitment`), with `fulfillBy`, and `lsfProgress` (the reported progress,
  which keeps it `IN_PROGRESS`: `model::refresh_fulfillment_order` reads it) and `lsfPrepared`.
- **`DeliveryProfile`** and its parts (`DeliveryLocationGroup`, `DeliveryZone`,
  `DeliveryCountry`, `DeliveryProvince`, `DeliveryMethodDefinition`, `DeliveryRateDefinition`,
  `DeliveryParticipant`, `DeliveryCondition` with its `conditionCriteria` a `MoneyV2` or a
  `Weight`), the variants of a custom profile under `lsfVariants` (the general profile holds
  those no other holds). Counts, items and unassigned locations are computed when read.
- **`DeliveryCarrierService`** (`lsfUpdatedAt` for its `UPDATED_AT` sort key: the type has no
  `updatedAt`), **`FulfillmentConstraintRule`** (`function` the key of its `ShopifyFunction`,
  `metafields`), **`DeliveryPromiseProvider`**, **`DeliveryPromiseParticipant`**
  (`lsfBrandedPromiseHandle`), **`ShippingLabelPurchaseResult`** (done, `PURCHASED`) and
  **`ShippingLabel`**.
- **The outbox**: what Shopify would send or buy (a pickup notice, a label, a shipping
  confirmation) is in the shop's outbox (see "The outbox"); `GET /__lsf/admin` also shows each
  shop's `shippingPackages`.
- **Events**: a topic that says what changed (`inventory_shipments/receive_items`'s quantities
  before and after, `inventory_transfers/add_items`, `fulfillment_orders/merged`, …) is emitted
  with its context as the event's snapshot (`logistics::emit_change`: `happenedAt` and
  `lsfLogistics`), which the payload reads from `__previous`: two changes of a record in one
  request each keep theirs. Changes without a topic (a transfer created or deleted, a carrier
  service, a barcode, a deadline, a rule, a package) are emitted under names no subscription
  can name (`inventory_transfers/create`, `carrier_services/update`, `lsf/changed`, …).
- **Errors**: shopify.dev documents these mutations' errors by their codes; `logistics::coded`
  writes the description of the code in the request's schema (the payload's own code enum), the
  first sentence of it. The payloads without codes (delivery profiles, packages, promise
  participants) answer messages of lsf's.

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
  app's configuration, of which lsf reads the order ones only (below). Being definitions, they
  check what `metafieldsSet` writes to the same owner type, namespace and key (the type must be
  theirs).
- **`MetafieldDefinition`**, declared: the order definitions of `shopify.app.toml`
  (`[order.metafields.<namespace>.<key>]`, `config::AppConfig::order_metafields`;
  `content::seed_declared_definitions`), seeded before the inferred ones with the id the seed
  gives (`metafield_definition_gid`): `app` is the app's reserved namespace, another namespace
  stays as written (shopify.dev's TOML reference allows `app` only, its page on capabilities
  writes `custom`: both are read *(chosen)*); `type`, `name` (else the key, humanized),
  `description` and `capabilities.cart_to_order_copyable` are read, not `validations` nor
  `access` (an app-owned definition's defaults). A declaration without `type` is left out with
  a warning of the loader. The definitions of other owner types are not read: only the order
  ones change what lsf does (the copy of the cart's metafields). Shopify keeps declared
  definitions read-only through the Admin API; lsf does not *(gap)*. An uninstall deletes those of
  the reserved namespace, a reinstall creates them again under new ids (see "App").
- **`Metaobject`**: `type` (an app's own type, `$app:author` in the store data, is
  `app--<app id>--author`, as Shopify writes it; store sync writes it back as `$app:author`),
  `handle`, `displayName` (its `name` or `title` field, else the handle humanized), `definition`, `fields` (inline `MetaobjectField`s: `key`, `type`, `value`,
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
| `files` | `fileCreate`, `fileUpdate`, `fileDelete`; `files`' sort keys and search text. The staged uploads are the app family's: its upload endpoint keeps the bytes in the shop (a `Blob` under the target's key), and `files::staged_upload(state, resource_url)` reads them back for `fileCreate` and `fileUpdate`: the name is the key's last segment (the target's `filename`), the MIME type the upload's, the size the bytes'. Kept in the shop, an upload goes with a reset and is another shop's only there. |

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
  without an app every scope of the versions lsf knows, `auth::scopes::ALL_SCOPES`, the list the access
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
  **`Validation`** only for a Function the store data declares an owner of
  (`checkout.functions.<handle>.enabled`, `true` or `false`), as a shop already set up has them:
  `title` (the Function's name, a `t:` key read from its default locale), `enabled` as the data
  says, `functionId`, `shopifyFunction`, `blockOnFailure: false`, and the metafields
  `checkout.functions` gives it. `enabled: false` on a cart transform makes none (it cannot be
  turned off). Otherwise none, as on a shop the app was just installed on: the app creates them,
  and an app that checks whether it is set up sees that it is not. Without an app, none.
- **Uninstalled** (`domains::app::uninstall`): the app's Functions go with every object that
  runs them, as Shopify removes an uninstalled app's Functions and a Function goes "including all
  associated function owners" (shopify.dev, "About Shopify Functions"; staff on the community
  forums: the cart transforms "should be automatically removed", "the job that deletes the price
  rules and their associated codes runs asynchronously after an app is uninstalled"; a payment
  customization left active was "an issue on our end"). `remove_function_owners` deletes, with
  their metafields, the records of `FUNCTION_OWNER_TYPES` (`CartTransform`,
  `DeliveryCustomization`, `PaymentCustomization`, `Validation`, `FulfillmentConstraintRule`)
  and the discount nodes whose discount is a `DiscountAutomaticApp` or a `DiscountCodeApp`,
  those of the seed included; Shopify's own discount types stay. lsf does it at once (Shopify, in
  the background). A reinstall brings none back (a reinstalled app "had to register them again
  through the API"), nor does a reseed while the app is uninstalled (`AdminApi::reseed` removes
  them again from those shops). The store sync then writes `checkout.discounts: []` and every
  Function without an owner; the server's checkout also runs none of the app's Functions while
  the default shop has the app uninstalled (`CheckoutConfig::uninstalled`, which covers a
  scenario's own owners and `extensions`). `domains::storefront::remove_on_uninstall` deletes the
  app's script tags ("all of the script tags that it created are automatically removed",
  shopify.dev, `ScriptTag`) and its web pixel (a reinstalled app calls `webPixelCreate` again),
  emitting `CHANGED`: the next sync gives the theme no script tag.
  `content::remove_app_owned` deletes the metafield definitions of the app's reserved namespaces
  (`app--<app id>` and `app--<app id>--<name>`, never another app's), declared
  (`seed_declared_definitions`) or created (`metafieldDefinitionCreate`), unpinned first; their
  metafields stay with `definition: null`, and each definition is kept aside as a memo
  (`lsf/uninstalled-definition:<owner type>/<namespace>/<key>`, not a record: no GraphQL type, no
  sync). A reinstall (`note_installs`, for each shop it installs the app on again, through
  `change_shop` with the tokens locked, which come before the shops) runs `app::reinstall`:
  `content::restore_app_owned` creates each kept definition again under a new id, unpinned, in the
  order of the old ids, and links back the metafields of its owner type, namespace and key; one
  made meanwhile under the same three stays. `content::remove_app_owned_metaobject_definitions`
  does the same for the metaobject definitions of the app's reserved types (`app--<app id>--<type>`),
  from the store data or `metaobjectDefinitionCreate`: each goes with its entries into a memo
  (`lsf/uninstalled-metaobject-definition:<type>`), the entries out of the records too, since a
  `Metaobject` has a non-null `definition`; the reinstall
  (`content::restore_app_owned_metaobject_definitions`, after the metafield definitions) creates the
  definition under a new id, puts the entries back under their ids, now naming it, and renames
  the old id in the metafield definitions' validations. A new seed of a shop the app is not installed on runs
  `app::reseed_uninstalled` (the Function owners and the reserved definitions again: the memos went
  with the old state, so only the declared definitions come back on reinstall). Why, and the
  sources: "App metafields after an uninstall" below. `uninstall` answers what it removed
  (`Uninstalled`: `removedFunctionOwners`, `removedScriptTags`, `removedWebPixels`,
  `removedMetafieldDefinitions`, `removedMetaobjectDefinitions`, `retainedMetaobjects` of the
  control endpoint), `reinstall` what came back (`Reinstalled`, which the terminal says). The UI extensions and the app proxy are
  not records: the server hides the extensions while the default shop has the app uninstalled
  (`place_extensions` in lsf-cli's `server/checkout.rs`) and answers the storefront's `404` on the
  proxy's path (`not_installed` in `server/app_proxy.rs`: a store gets the proxy with the app, a
  change of `prefix` or `subpath` reaching "only new installations", shopify.dev, "About app
  proxies"), and `AdminApi::with_notices` lets the terminal say when an app is uninstalled from a
  shop (`finish_uninstall`, plus `uninstalled_proxy_reason` for the default shop of an app with an
  `[app_proxy]`) or installed again (`note_installs`, with the definitions brought back and the
  proxy's path). The words are `lsf_app::config::NOT_INSTALLED` and the reasons next to it.

**App metafields after an uninstall.** Decided from the most authoritative source, in this order:
shopify.dev, the changelog, Shopify's staff.

- shopify.dev, "Manage metafield definitions", section "App uninstall and reinstall"
  (<https://shopify.dev/docs/apps/build/metafields/definitions#app-uninstall-and-reinstall>,
  read 2026-11-08): "When a user uninstalls your app, Shopify deletes its app-owned metafield
  definitions and temporarily retains the metafields and their values without a definition. This
  applies to any app-owned definition, whether declared in `shopify.app.toml` or created with the
  GraphQL Admin API. Cleanup of the retained data runs asynchronously." "If the same app is
  reinstalled while that data is still retained, then Shopify recreates the definitions and
  associates the existing metafields with them." TOML definitions: "Shopify deletes them during
  uninstall and creates them again on reinstall. The new definitions have new GIDs"; GraphQL
  definitions: "can persist after uninstall until asynchronous cleanup removes them". "Shopify
  doesn't guarantee how long metafields and their values are retained after an uninstall." The
  same page defines app-owned: "Use the reserved `$app` namespace. The app controls the
  definition", against merchant-owned, "any non-reserved namespace". The metaobject page says the
  same of app-owned metaobject definitions
  (<https://shopify.dev/docs/apps/build/metaobjects/manage-metaobject-definitions#app-uninstall-and-reinstall>).
- The changelog has no entry on it. The section was added in August 2026: Wes-Dev-Shopify on
  community.shopify.dev ("App-owned TOML metafield definitions: what happens after
  uninstall/reinstall?", 2026-08-12) first observed the metafield and its value kept, then wrote
  "Uninstalling deletes app-owned metafield definitions" and linked the new sections.
- Older threads (2024 to May 2026: "Metafields/metaobjects definitions with reserved namespace
  left after app uninstall", "App-reserved metafield definitions blocking product/variant updates
  after app uninstall") report reserved definitions left behind, relaying a reply of Shopify that
  "merchants might still rely on the data", and no staff post there says otherwise; they predate
  the documented behaviour, which lsf follows. Of the posts read as saying app metafields are
  cleaned up, none is a staff statement on app-owned metafields: Paige-Shopify ("App-owned
  metafields randomly disappearing after a few days?", 2026-02) only answers "Outside of
  uninstalling the app, this is the first I'm hearing of it."; David_Arbias, no staff mark ("What
  happens to app-owned collection sources after the app owning it was uninstalled?", 2026-08-01),
  writes that a metafield "under `currentAppInstallation` … gets deleted together with any
  resource owned by the app", which is about app-data metafields, not the reserved namespaces;
  Kellan-Shopify ("Orphaned app-owned metaobject definitions cannot be deleted even after
  reinstalling app", 2026-03-03) speaks of "a process in place to clear out any orphaned
  metaobjects on our end".

So lsf deletes the reserved definitions (`$app`, `$app:<name>`), keeps their metafields, and a
reinstall creates the definitions again under new ids; the definitions of other namespaces are
the merchant's, and no source says an uninstall touches them: they stay. lsf deletes the created
definitions at once, as it deletes the Function owners Shopify removes in the background, and
keeps the values until a new seed, since it has no clock to expire them by. The metaobject page
says the same of the app's metaobject definitions (`$app:<type>`) and their entries, and lsf does
the same to them; their entries are kept out of the records meanwhile, which no query could
reach anyway (`Metaobject.definition` is non-null, `metaobjects(type:)` needs a definition). lsf
does not read `[metaobjects.app.<type>]` of `shopify.app.toml` *(gap)*. The app-data metafields of the installation (`AppInstallation`) stay: no shopify.dev page says
what an uninstall does to them, and the one post that does is not Shopify's.

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

#### Managed pricing (`domains/app/pricing.rs`, `api/pricing.rs`)

Shopify keeps an app's plans in the Partner Dashboard and hosts the page where the merchant picks
one (`admin.shopify.com/store/<store>/charges/<app handle>/pricing_plans`). lsf takes the plans
from `--pricing-plans <file>` (TOML, or JSON for `.json`; `Pricing::read`) or from
`PUT /__lsf/admin/pricing-plans` (`Pricing::from_json`), checked as a whole: a wrong declaration
says every fault (`plans[1] (pro): price is 0 or more`) and changes nothing. `AdminApi` holds
them for every shop (`with_pricing`, `pricing`, `set_pricing`): the file's, which `DELETE` puts
back, and those that apply. A reset of the shops keeps them, as it keeps the stubbed URLs.

- **The pages** are pages of lsf's admin (`embedded::render`, so the same top bar, navigation and
  title bar): `GET /admin/charges/<app>/pricing_plans` lists the plans, the shop's current one
  marked (`pricing::current`: its active subscription, and the `planHandle` of its recurring
  line); `…/pricing_plans/<plan>` is the plan to approve, whose form posts to
  `POST /__lsf/admin/pricing-plans/<plan>/select`, as the charge page's form posts to its
  control endpoint. `admin.js` maps Shopify's URLs of the admin (`https://admin.shopify.com/store/
  <store>/…`, `https://<store>.myshopify.com/admin/…`) that App Bridge asks the admin to open
  (`navigate` with `target: '_top'`) to lsf's `/admin/…?shop=<store>.myshopify.com`.
- **Choosing a plan** (`pricing::select`) makes the records of the Billing API: an
  `AppSubscription` named after the plan (`test` on a development store, which lsf's shops are,
  in legacy mode), its trial, one `AppSubscriptionLineItem` whose `AppRecurringPricing` has the
  plan's `price`, `interval` and `planHandle`; it is activated as an approved charge is
  (`billing::activate_until`, which sets the dates before the status). The shop's other active
  subscription is `CANCELLED` once the new one is active, so that the Partner API's history sees
  one change of plan (`SUBSCRIPTION_UPDATED`), not an end and a start. Down to a
  free plan, the new subscription keeps the paid one's `currentPeriodEnd` ("a downgrade doesn't
  start a new billing cycle"). The number is the next charge's (`billing::next_charge`): the
  `charge_id` of the welcome link. Each change emits `app_subscriptions/update` (legacy) or
  `CHANGED`.
- **The welcome link** (`pricing::welcome_url`): a path, or none, is the app's page in lsf's
  admin with the admin's `shop`; a URL gets `shop` last. `charge_id` (legacy) and `plan_handle`
  are appended. It is the subscription's `returnUrl` too.
- **The Billing API** creates no charge while plans are declared: `AdminApi::managed_pricing`
  goes to the resolver (`ShopResolver::with_managed_pricing`, then `Mutation::managed_pricing`),
  and `appSubscriptionCreate` and `appPurchaseOneTimeCreate` answer the user error developers
  report (`pricing::BILLING_API_REFUSED`).
- **`legacy`** (`true` by default) is what Shopify keeps for the apps enrolled before April
  2026: `charge_id`, the webhook, test subscriptions. `false` is what its page says of the apps
  enrolled since, and of every app after April 28, 2026: `plan_handle` alone, no webhook; such an
  app reads the Partner API (below), and the record stays in the Admin API too.
- **The mutations** see how the app bills through `pricing::Billing` (`BillingApi`, `Legacy`,
  `AppPricing`; `AdminApi::billing`, then `ShopResolver::with_billing` and `Mutation::billing`).
  `Billing::subscription_topic` is the topic of a change of a subscription:
  `app_subscriptions/update`, or `CHANGED` under `AppPricing`, whose page says "Shopify App
  Pricing doesn't use webhooks to notify your app of subscription changes" and sends the app to
  the Partner API for "status changes that happen outside of a redirect (such as cancellations
  or freezes)": `appSubscriptionCancel` uses it, as `pricing::select` does. An uninstall cancels
  without a webhook in every mode (`billing::cancel_on_uninstall`): the app's subscriptions go
  with it.

#### The Partner API (`admin/partner.rs`, `api/partner.rs`, `domains/app/partner.rs`)

An app on Shopify App Pricing reads its plan from the Partner API (`activeSubscription`) and its
history from it (`events`, and the older `app { events }`). lsf serves it at Shopify's path on
its own origin, `/<partner id>/api/<version>/graphql.json`: `api::partner::handle` checks the
version (2026-07, 2026-10 and 2027-01, whose introspections are the same), the token
(`X-Shopify-Access-Token`, `partner::token`) and the Partner ID (`partner::partner_id`), both the
same every run for the client id, and answers Shopify's errors (`401` "Invalid access token",
`404` "Invalid API version", `400` past 50000 characters). The module's header has the sources,
what the pages contradict, and lsf's choices.

- **The schema** is Shopify's whole 2026-10 one, from the pinned `@shopify/dev-mcp`
  (`tools/admin/partner_schema.mjs` writes `schemas/partner/2026-10/`; `mise run
  partner:schema`), parsed by `admin::schema` and run by `admin::graphql` without a cost
  (`partner::execute`): validation and introspection are Shopify's. The resolver answers
  `activeSubscription`, `events` and `App.events`; another root field answers
  `LSF_NOT_SIMULATED`.
- **The data** is read once per request (`partner::Organization`, `ShopView::of` for each shop
  in turn, `AdminApi::read_shops`): the shop's active subscription of managed pricing
  (`domains::app::partner::active_subscription`, computed at the shop's clock: the trial, the
  billing cycle), and its history.
- **The history** is a memo of the shop (`lsf/partner-events`, a list: not a record, so no type,
  no webhook, no sync), written as the changes happen, so that a later change does not rewrite
  an event: `billing::set_status` calls `partner::status_changed` on every change of a charge's
  status (the `App.events` types, the Billing API's included; for a subscription with a
  `planHandle`, `SUBSCRIPTION_CREATED`, `_UPDATED` when another plan is active, `_CANCELED` when
  none is), `app::uninstall` and `app::reseed_uninstalled` add `RELATIONSHIP_UNINSTALLED`,
  `app::reinstall` `RELATIONSHIP_INSTALLED`, and the seed the install the shop starts with, at
  `Seed::epoch`. A reset gives back the seed's.

### Markets, catalogs and channels (`domains/markets.rs`)

The content family seeds the markets and the locales; this family completes them
(`markets::seed`, after the app's seed) and owns what sells in them. Each module of
`domains/markets/` says in its header what shopify.dev documents of its mutations and where lsf
differs; `markets::tests` tests every mutation over the demo store, `tests/markets.rs` the shape
of what they make and what syncs.

- **`Market`**, completed: `priceInclusions` (taxes in the price when the shop's prices include
  them), `currencySettings.roundingEnabled: false`, the conditions it has not
  (`companyLocationsCondition: null`, …), `delivery`, `returnPolicyProfile: null`. `regions` and
  `conditions.regionsCondition.regions` hold the same inline `MarketRegionCountry`s; `type`
  follows the conditions. A market's catalogs and web presences are read from the catalogs and
  presences that name it (`markets`): one place holds each link.
- **`MarketWebPresence`**: the primary domain's, for every market and every locale of the shop
  (`defaultLocale` the primary one, the others alternate), then those the mutations make. Its
  `rootUrls` are recomputed by `web_presences::refresh`; `refresh_domains` keeps each `Domain`'s
  `marketWebPresence` and `localization` (its presence's locales, else the primary domain's),
  which `domains/update` reports. `ShopLocale.marketWebPresences` is read from the presences.
- **The backup region**: the shop's setting `backupRegion` (a country code), the shop's
  country by default. `markets_backup_region/update` names no record: its snapshot is the
  region.
- **Catalogs**: `MarketCatalog` (`markets`), `CompanyLocationCatalog` (`companyLocations`),
  `AppCatalog` (`apps`, `channels`), with `title`, `status`, `priceList`, `publication`,
  `operations: []`. The Online Store's publication gets the Online Store's app catalog, and the
  fields the catalog family's seed leaves out (`autoPublish: false`, `supportsFuturePublishing:
  false`: lsf publishes on it at once).
- **`PriceList`**: `name`, `currency`, `parent` (`adjustment`, `settings`), `catalog`,
  `createdAt`, `fixedPricesCount`, and what GraphQL reads through custom fields:
  `lsfFixedPrices` (`[{variant, price, compareAtPrice}]`), `lsfQuantityRules`
  (`[{variant, minimum, maximum, increment}]`), `lsfPriceBreaks` (the `QuantityPriceBreak`
  records: `minimumQuantity`, `price`, `priceList`, `variant`). `PriceList.prices` lists every
  variant, fixed or relative; contextual pricing is computed when it is read
  (`price_lists::contextual_price`). A deleted variant leaves every price list
  (`markets::removing_variants`, run by `delete_product` and `remove_variant`): its fixed price,
  its rule and its price breaks go, `fixedPricesCount` is counted again, `price_lists/update`.
- **`Channel`** (`channelCreate`): its account, specification, handle, `activeRegions` (the
  countries of the active region markets), `app`, and its own `Publication` (`lsfChannel` names
  the channel back, `supportsFuturePublishing: true`, `lsfScheduled` its scheduled listings) and
  `AppCatalog` (`lsfPublication`, `lsfCatalog`).
- **Without an id**: `ProductResourceFeedback` under `feedback/<product GID>/<channel GID or
  app>`, `AppFeedback` under `feedback/shop/<channel GID or app>`, `MarketLocalizableResource`
  (`resourceId`, `lsfLocalizations`) under `market_localizations/<resource GID>`.
- **Listings**: the catalog family's publish path (`catalog::publish_on`, shared by
  `publishablePublish`, `publicationUpdate` and the current channel's mutations) asks
  `markets::scheduled` before it changes a publication (a future date on a channel's
  publication schedules the listing) and tells `markets::listed` after (the listing events);
  `delete_product` and `delete_collection` call `markets::removing`. What follows any mutation
  is `markets::after_mutation`, which the resolver runs after each one that emitted events,
  before their repeats are merged: the products created join the publications with
  `autoPublish`, a listed product or collection changed is `…_listings/update`, and the
  scheduled listings whose date has passed are listed. A listing event names no record
  (`<publishable GID>?publication_id=<n>`): its snapshot is the product or collection, with the
  listing (`lsfListing`: `publishedAt`, `publishAt`).
- Events without a topic in 2026-10 (catalogs, price lists, publications, web presences, market
  localizations, resource feedback, `channels/create`, `channels/update`, a full sync) are
  emitted under a topic no subscription can name, as the content family's.

### Subscriptions (`domains/subscriptions/`)

The Subscriptions API and the customers' saved payment methods. Nothing is seeded (the store data
has no subscriptions) but the checkout's shipping rates, kept in the setting
`subscriptionDeliveryRates` for the delivery options. One module per area: `model` (the shapes
and amounts every contract-like record shares, the errors), `payment_methods`, `drafts`,
`contracts`, `cycles`, `billing`, `calculation`, `checkout`; each module's header says what
shopify.dev documents of its mutations and its sources; `tests` tests every mutation as it
succeeds and as it refuses (the shop left as it was).

- **`CustomerPaymentMethod`**: its key is Shopify's form, `gid://shopify/CustomerPaymentMethod/
  <32 hex digits>` (a UUID of the shop and a counter, without its dashes: the token of the
  webhooks). `customer`, `instrument` (inline: a `CustomerCreditCard`, a
  `CustomerPaypalBillingAgreement` or a `CustomerShopPayAgreement`, which names its method in
  `lsfMethod` for `isRevocable`), `revokedAt`, `revokedReason`, `mandates: []`. What a charge
  does with it is `lsfOutcome` (`success`, `challenge`, or an error code), read from the test card
  when it is made; a vault session (`POST /__lsf/admin/card-sessions`) is the card itself,
  base64url-encoded without its number, so the vault keeps no state.
- **`SubscriptionContract`** (`new_gid`): `status`, `customer`, `customerPaymentMethod`,
  `currencyCode`, `billingPolicy` (with `cadence`), `deliveryPolicy`, `deliveryMethod` (inline,
  its `__typename` one of the union's), `deliveryPrice`, `lines` (inline `SubscriptionLine`s,
  `gid://shopify/SubscriptionLine/<uuid>`), `discounts` (inline `SubscriptionManualDiscount`s,
  each with its `entitledLines` and the ids it lists under `lsfLineIds`), `customAttributes`,
  `note`, `nextBillingDate`, `revisionId` (a string, from `"1"`: every committed change adds one),
  `originOrder`, `orders`, `billingAttempts`, `app`, `lastPaymentStatus`,
  `lastBillingAttemptErrorType`, `latestCommittedProjectedOrderTotals`, dates. `model::refresh`
  derives each line's `discountAllocations` and `lineDiscountedPrice` and the projected totals,
  for contracts, drafts, calculated and edited contracts alike.
- **Billing cycles** are not records: `cycles` computes them from the contract's `lsfSchedule`
  (segments `{fromIndex, start, firstEnd, interval, count, day, anchored}`: a new billing policy
  starts a segment at the current cycle) in the shop's time zone, and keeps their edits in
  `lsfCycleEdits` (`{"<index>": {skipped, billingAttemptExpectedDate, reason, contract,
  concatenated, concatenatedInto}}`). Its date arithmetic is checked: a cycle that would start
  or end after 9999 in UTC (`LAST_YEAR`, lsf's bound) does not exist, so a huge index answers
  `CYCLE_INDEX_OUT_OF_RANGE`, and a first interval past it (an `intervalCount` of 2^31 − 1 weeks)
  ends on December 30, 9999 at midnight in the shop's zone. The events of a cycle are keyed
  `gid://shopify/SubscriptionBillingCycle/<contract n>?index=<i>`, which names no record: their
  payload is built from the snapshot, the cycle as `to_value` answers it.
- **`SubscriptionDraft`** (`new_gid`): a contract-like record, with `originalContract`,
  `lsfRevision` (the revision it was made from), `lsfOriginal` (its lines and discounts then,
  for `linesAdded` and the others), `lsfCycle` and `lsfConcatenated` for a cycle draft,
  `lsfCommitted`. Its `discounts` may be `SubscriptionAppliedCodeDiscount`s, which keep the manual
  discount they become on commit (`lsfManual`).
- **`SubscriptionContractCalculationSuccess`** (or `…Failure`), under
  `gid://shopify/SubscriptionContractCalculation/<n>`: the `calculatedContract` (a
  `SubscriptionCalculatedContract`), `projectedOrderTotals`, `warnings`, `deliveryOptions`, and
  what its commit needs: `lsfTarget` (create, update, or a cycle), `lsfRevision`, `lsfDiscovery`,
  `lsfCommitted`. The mutation answers an inline `…Pending` of the same id.
- **`SubscriptionBillingAttempt`** (`new_gid`): `subscriptionContract`, `idempotencyKey`,
  `originTime`, `state` (inline, the union's), `order`, `ready`, `completedAt`, the deprecated
  `errorCode`, `errorMessage`, `processingError`, `nextActionUrl`, `transactions`, and
  `lsfCycleIndex`, `lsfAnchor` and `lsfMembers` (the contracts a concatenated cycle bills). The
  mutation answers the record pending, inline; the record has the outcome. Its order is made by
  `orders::insert_order` from an `OrderDraft` of the cycle's contract, its stock committed with
  `orders::model::commit_line`, `orders/create`, `order_transactions/create` (with the sale's
  `tender_transactions/create`) and `orders/paid` emitted as `orderCreate` emits them. A contract in a presentment currency (another than the
  shop's that the shop enables) bills an order presented in it: the draft's amounts are the
  contract's converted to the shop's currency at `markets::rate_bps` (a market's manual rate,
  else 1.0), and the order keeps its currency and rate (`OrderDraft.presentment`,
  `lsfPresentment: {currency, rateBps}`, `presentmentCurrencyCode`).
- **`Job`** of a bulk charge or search: done, the cycles it found under `lsfBillingCycles`.
- **The emails** lsf does not send are in the shop's outbox (see "The outbox"):
  `GET /__lsf/admin/subscriptions` lists them (`payment_methods::emails`) with the charges (the
  billing attempts).
- **From the checkout**: `AdminApi::record_order` runs `checkout::contracts_from_order` after
  `orders::record_placed_order`, in the same change of the shop.
- Events without a webhook topic (a draft changed, a calculation made, a challenge opened, a bulk
  job) emit `domains::app::CHANGED`.

### Point of sale (`domains/pos.rs`)

The seed runs last (`pos::seed`): it reads the locations another family made.

- **`PointOfSaleDevice`**, one per active location that is no fulfillment service:
  `stable_id("point_of_sale_device", <location GID>)` *(chosen: the Admin API cannot make one)*,
  `cashDrawer`, `activePaymentSession`, `fiscalDeviceIdentifier: null`, the location under
  `lsfLocation`. What lsf would have pushed to the POS app, which it never sends, is in the
  shop's outbox (see "The outbox", channel `pos`).
- **The staff member** the sessions name is the shop's owner, `StaffMember/1`
  (`staff::owner_gid`, seeded by the platform family).
- **Reason codes**: a `CashManagementDefaultReasonCode` per value of its enum and a
  `CashManagementSystemReasonCode` per system value (`stable_id("cash_management_reason_code",
  "default/<CODE>" | "system/<CODE>")`); the custom ones the mutation makes.
- What the mutations make: `CashDrawer` (`name`, `location`, `balance`),
  `PointOfSaleDevicePaymentSession` (the times, staff, notes, counted and expected amounts, the
  totals, and `cashActivities` inline: `CashCountActivity` and `CashAdjustmentActivity` with
  their `__typename`). `cashTrackingSession(s)` read the same sessions as `CashTrackingSession`s
  of their number. Events have topics no subscription can name (`cash_drawers/create`, …).

### Themes (`domains/themes.rs`)

The theme lsf serves is the shop's main theme, and the design question is what a write to it
does. **Decision: the directory is the main theme's files, read from disk; what the Admin API
writes to it is an in-memory overlay that the storefront serves on top of the directory, which is
never written to.**

- *Why not refuse writes to the main theme* (Shopify's error for a locked theme): an app that edits
  the live theme (a theme app's setup, a page builder writing `templates/*.json`, a backend that
  adds an `assets/` file) is a common case, and lsf exists to test it: it must see its write on the
  storefront. *Why not write to disk*: the directory is the developer's work, under version
  control, often open in an editor; a test run must not change it, and a reset must undo it.
- *Feasible at no cost*: every read of the theme goes through `lsf_core::theme::ThemeFiles`
  (`read`, `exists`, `list`, `version`, and `bytes` for the CDN's assets). `ThemeFiles` holds an
  optional `FileOverlay` (`set_overlay`): by path, a file's bytes (and its text, an `Arc<str>`
  stable for the life of the overlay, so that the theme's caches of parsed Liquid and JSON, keyed
  by the source's `Arc`, rebuild only what changed) or `None` for a file deleted; `exclusive` for a
  theme that replaces the directory. A read looks at an `AtomicBool` first: without an overlay,
  the common case, a read costs one atomic load more. Setting one expires what is derived from the
  files (`Theme::cached_version`, `cached_text` check `overlay_changed_since`, also for files read
  once, `--static`).
- *Records*: the local theme is `OnlineStoreTheme/1` (`lsf_core::urls::THEME_ID`, the
  `Shopify.theme.id` of `content_for_header`), `MAIN`, named by `theme_info.theme_name` of its
  `config/settings_schema.json`; the server gives its directory (`AdminApi::with_theme`, which
  amends the seed: `ShopState::amend_seed`, not a change store sync sees; again after a reseed).
  Its record keeps the overlay (`lsfFiles`: by filename, the blob of its bytes, digest, size,
  dates, or `{deleted: true}`); another theme's record keeps all its files there. The bytes are
  blobs of the shop, keyed by their MD5: a duplicate shares them, a reset forgets them.
- *What the storefront serves* (`themes::storefront_overlay`, `AdminApi::storefront_overlay`):
  the main theme of the default shop. The local theme without writes: nothing (the directory
  alone). With writes: the overlay. Another theme published: all its files, exclusively. Its
  `generation` is a hash of the filenames and digests it holds
  (`storefront_overlay_generation`, which reads no byte): the server makes a new overlay only
  when it moved, from the one it serves (`storefront_overlay_from`), whose files it keeps where
  their blob is the same `Arc` (no byte is copied, the text `Arc` the Liquid caches are keyed by
  stays). `OnlineStoreTheme.files` lists the filenames first (`files::listed`, no file read),
  pages them, and reads, digests and encodes only the files of the page. The server applies it where store sync builds the store
  (`ServerState::sync_theme`, when the count of changes moved, under the build lock, and after
  each reseed of `reload`, which drops the Admin API's files and script tags), only when the
  generation differs; live reload's token includes it. The CDN serves an asset of the overlay
  as it is (`/cdn/shop/t/1/assets/…`; an image without the CDN's transforms).
- *What it costs* (release builds of the commit before themes and of this one, the fixture theme
  of lsf-theme, the demo store, the two servers' renders interleaved by batches of 25, 1000 each,
  medians of `Server-Timing: render`, on 2026-10-31 on a machine loaded with other builds):
  without an overlay, `/` 168 → 166 µs, `/collections/all` 326 → 331 µs, a product 199 → 198 µs,
  a search 128 → 128 µs revalidating; 77 → 77, 366 → 361, 94 → 95, 60 → 60 µs with `--static`:
  within ±5 µs (±1.5 %) either way, which is the noise. With an overlay of two files and a script
  tag, `/` renders in 163 to 172 µs against 161 to 165 µs before (alternated runs): a few µs,
  most of it the 640 bytes of the script tag's loader.
- *Which theme the page names*: `Shopify.theme` of `content_for_header`, the `theme` object and
  the asset URLs (`/cdn/shop/t/<id>/…`) name the main theme (`themes::storefront_identity`,
  `AdminApi::storefront_theme`, given to `lsf_theme::Theme::set_identity` by
  `sync_theme`): its id and name while another theme is published, the name `themeUpdate` gave
  the local theme. `schema_name` stays the files' `theme_info`. Without either, the directory:
  id 1, the name its files give.
- *Sources from a URL* (`crate::sources`): the `source` of `themeCreate`, the `URL` bodies of
  `themeFilesUpsert` and the `url` of `urlRedirectImportCreate` are found in the parsed request
  (its query and variables, through fragments; only for the fields the token may run; a query
  that names none of the three is not parsed again) and fetched by the api layer before it locks
  the shop: 8 at a time, 10 s each, 30 s for all of them. The mutation reads them from the
  request's `Fetched` (`ShopResolver::with_fetched`, `Mutation::fetched`). A download within the
  mutation held the shop, and every request waiting on it (carts and checkouts recorded by the
  storefront too), for as long as it took: 500 s for 50 slow bodies, and a URL to lsf itself
  waited on itself until the timeout. lsf's own upload URLs (`/__lsf/admin/uploads/…`) are read
  from the shop's blobs, never fetched; a URL that was not prefetched (a line of a bulk
  operation) is refused, and says why. Only public addresses are fetched from (Shopify cannot
  reach the developer's machine): `sources::PublicOnly`, the ureq resolver of every connection,
  redirects included, refuses a host that resolves to the loopback, a private network,
  link-local, unique-local, unspecified, multicast or reserved addresses (IPv6 ones embedding
  an IPv4 address as that one), so that what is checked is what is connected to; proxies of the
  environment are off. `--admin-fetch-local` (`AdminApi::with_fetch`) lifts it. A refused
  source answers what an unreachable one does: `INVALID_ZIP`, `FILE_VALIDATION_ERROR`,
  `NOT_FOUND`.
- *Stubbed URLs* (`sources::Stubs`, on the `AdminApi`): `prefetch` answers a URL a stub has
  without fetching it, wherever it points, for a backend's own URL that a test has no server
  for. Configured ones (`--admin-source <url>=<file>`, `AdminApi::with_sources`) are read from
  their file at each use; registered ones (`PUT /__lsf/admin/sources?url=`, `api/simulate.rs`)
  hold the body, override a configured one, and go with `DELETE`. Only the developer names a
  file: an endpoint that took a path would let any client of the machine read its files back
  as theme files. One set for the server, not per shop (the internet is the same for every
  shop), so a reset keeps it. URLs are matched by `sources::stub_key`: scheme and host
  lowercased, the fragment dropped, the query kept.
- *What does not follow*: Shopify processes a theme from a URL in the background
  (`processing: true` until done): lsf's are done when the mutation answers.

### The online store and the app platform (`domains/storefront.rs`, `store_admin.rs`, `platform.rs`)

| Module | Records | Ids, keys |
|---|---|---|
| `storefront` | `ScriptTag`, `WebPixel`, `ServerPixel`, `StorefrontAccessToken` | the counter of each type; a storefront token's secret is a UUID of the shop and its id, without dashes |
| `store_admin` | `UrlRedirectImport` (its rows under `lsfRows`), `StandardMetafieldDefinitionTemplate` (seeded: shopify.dev's 16, by their ids) | the counter; a template by its id (`…Template/2` is the care guide) |
| `platform` | `StaffMember/1` (the shop owner, seeded), `CheckoutProfile/1` and `CheckoutAndAccountsConfiguration/1` (seeded), `AndroidApplication`/`AppleApplication` (ids of `MobilePlatformApplication`, the type by platform, as the reference's), `DelegateAccessToken` (key `delegate-access-token/<hash>`, never the token). `FulfillmentConstraintRule` is the logistics family's, `ProductResourceFeedback` the markets family's (`feedback/<product>/<channel>`) | the counter |

- **Redirect imports** (`store_admin::redirects`): the CSV is read as RFC 4180 has it (a quoted
  cell holds commas, line breaks and `""`), and a submit looks each row's path up in a map of
  the shop's redirects made once (`navigation::set_redirect`, which `add_redirect` runs after its
  own lookup): 100,000 rows take about half a second, where a scan per row took longer than a
  quarter of an hour, the shop locked.
- **App extensions** `lsf_app::config` lists without reading (`App::other`): the web pixel
  extension's `[settings.fields]` and the Flow triggers' `[[settings.fields]]` are read from their
  `shopify.extension.toml` at seed (`platform::extensions`, `[settings]` of the file when it is
  below `[[extensions]]`, as Shopify CLI reads it) into settings of the shop (`webPixelExtension`,
  `flowTriggers`).
- **The outside world**: what Shopify would hand to a service lsf does not run is never sent.
  The state Shopify would show is kept (a server pixel `CONNECTED`, a mobile app), and what
  would have been sent (a Flow trigger) is in the shop's outbox (see "The outbox"). EventBridge
  and Pub/Sub subscriptions' deliveries are in the webhooks' log, `not_sent`. The association
  files of mobile apps are served by the storefront (`/.well-known/…`,
  `AdminApi::association_file`).
- **Script tags** reach `content_for_header` through `lsf_theme::Theme::set_script_tags`,
  set by the server from the default shop (`AdminApi::storefront_scripts`) as the overlay is.
  They are not store data: a script tag is the app's, not the theme's or the data's, and the
  store data has no section for it.
- **Mutations run through others**: `standardMetafieldDefinitionEnable` and
  `standardMetaobjectDefinitionEnable` run `metafieldDefinitionCreate` and
  `metaobjectDefinitionCreate` within the same mutation (`Mutation::resolver`, as a bulk mutation
  runs its lines); the deprecated EventBridge and Pub/Sub subscription mutations run
  `webhookSubscriptionCreate`/`Update` with their arguments rewritten (`FieldContext::
  with_arguments`); `stagedUploadTargetGenerate(s)` run `stagedUploadsCreate`; bulk deletes and
  bulk tags by search run the `urlRedirects` and `discountNodes` connections. One set of checks,
  of records and of events, the deprecated shapes kept in the payloads.

## The shop's clock (`domains::tick`)

A shop's time is its clock (`ShopState::now`): the system's, or frozen at an instant (the store
data's `now`, or `POST /__lsf/admin/clock`, which `api::simulate` answers; a reset puts the store
data's back). What waits on time is done by one function, `domains::tick`, which each family's
work joins:

- `orders::tick`: payment schedules due, store credit expired, Shopify Payments payouts;
- `app::billing::expire_stale`: charges pending for more than two days;
- `markets::publish_due_listings`: the scheduled listings whose date passed (also after every
  mutation that emitted events);
- `privacy::process_due_erasures`: the data erasures ten days old, answered as the customers
  were before, for `customers/redact` (a compliance topic: `AdminApi::tick` triggers it to the
  app's compliance subscriptions, at the shop's time);
- `checkouts::delete_expired`: the abandoned checkouts three months old (also when a checkout is
  recorded).

It runs before every GraphQL request and when the clock is moved, under the shop's lock; what it
writes is a change like a mutation's: its events are delivered with the request's, and store
sync is told (a listing published, a customer redacted). What is worked out when it is read (an
abandoned checkout's ten minutes, a subscription's next billing date, a discount's or a gift
card's dates, `PaymentTerms.overdue`) needs no tick. A webhook's `filter` counts `now` from the
event, by the shop's clock.

## The outbox (`outbox`)

lsf never sends what Shopify sends the outside world (webhooks go to this machine or
`--app-url` only): what Shopify would have sent or bought is recorded in the shop's outbox
instead, by one API every family uses (`outbox::record(state,
Entry::email(what, to).subject(..).body(..).about(..))`, `Entry::new(channel, what)` for the
other channels), and read by one (`outbox::entries(state, channel)`, `outbox::clear`).

- **An entry** is `{id, at, channel, to, what, about}`, with `subject` and `body` for an email
  (the words of another channel in `body`) and `data` for what else would have gone out. `id`
  counts per shop and is never given twice (the memo keeps the last one past a `DELETE`); `at` is
  the shop's clock; `what` is a stable name a test matches (`payment_reminder`,
  `shipping_label`, `flow_trigger`, `cash_drawer_assigned`, `customer_data_erased`). The
  channels (`outbox::Channel`): `email`, `carrier` (a label bought), `flow` (a Flow trigger),
  `pos` (what Shopify pushes to a POS device), `data_erasure`.
- **A memo of the shop** (`lsf/outbox`, the last 1000 entries): a mutation that fails takes its
  entries back with the undo log, a reset empties it, the store sync never sees it.
- **Read** by `GET /__lsf/admin/outbox?shop=&channel=` (`DELETE` forgets them), its alias
  `GET /__lsf/admin/emails` (`channel=email`, the list named `emails`), `GET /__lsf/admin`
  (`shops[].outbox`), and `GET /__lsf/admin/subscriptions` (the Subscriptions API's emails).
  `api::simulate` answers the first two.
- **A record keeps** what its GraphQL type answers (an abandonment's `emailState` and
  `emailSentAt`, which the app reports); what no query reads goes to the outbox.

## Events and webhooks (`webhooks`)

A change emits events: `{topic, shop, gid, at, id, snapshot}` (`products/update`, `orders/create`,
…), after the mutation succeeded. `Webhooks::emit(event, lookup)` turns an event into a delivery
for each subscription of the shop whose topic matches and whose `filter` matches the payload:

- **Topics**: the 227 values of the `WebhookSubscriptionTopic` enum of each version with their
  written form, read from the description of each value in its introspection
  (`tools/admin/webhook_topics.mjs --version <v>` writes `schemas/admin/<v>/topics.json`): the split
  between resource and action is not mechanical (`CUSTOMERS_EMAIL_MARKETING_CONSENT_UPDATE` is
  `customers_email_marketing_consent/update`, `CUSTOMER_TAGS_ADDED` is `customer.tags_added`).
- **Subscriptions**: those of `shopify.app.toml` (`[[webhooks.subscriptions]]`, app-wide, one per
  topic, `api_version` of `[webhooks]`), and those the backend creates through the Admin API
  (`webhookSubscriptionCreate`, per shop, in `webhooks::Registry`, which gives the
  `WebhookSubscription` record of each). Compliance topics (`customers/data_request`,
  `customers/redact`, `shop/redact`) are only triggered on demand.
- **Uninstall** (`POST /__lsf/admin/uninstall`), as shopify.dev describes it: `app/uninstalled`
  is delivered through the subscriptions the shop has, then those made with
  `webhookSubscriptionCreate` are deleted ("When an app uninstalls, Shopify automatically
  performs cleanup tasks, such as deleting WebhookSubscription objects", `appUninstall`), and
  `Webhooks::emit` delivers nothing more for the shop (`Registry::is_installed`): the
  subscriptions of `shopify.app.toml` are "applied uniformly across every shop that installs
  your app" ("Manage webhook subscriptions"), and stay the app's. Installing it again (the
  control endpoint, OAuth, a token exchange: `AdminApi::note_installs` after each, the tokens
  locked) brings them back; the shop's are gone. `shop/redact`, "48 hours after a store owner
  uninstalls your app" ("Privacy law compliance"), is sent on demand. A trigger is not an
  event: it is sent whether the app is installed or not. Before `app/uninstalled`, the uninstall
  changes the shop's records (`domains::app::uninstall`, for the control endpoint and
  `appUninstall` alike): the subscription is cancelled (no webhook), and every object that runs
  one of the app's Functions is deleted with its metafields (`remove_function_owners`, see
  "App"), each emitting `CHANGED`, which no subscription receives.
- **Where**: a `uri` starting with `/` joins the app URL (`--app-url`, else `application_url`),
  trailing slashes removed, as Shopify's CLI prepends `application_url`; an absolute one under
  `application_url` (or on its origin) moves to `--app-url` when it is given; another HTTP(S) URL
  is kept. `pubsub://`, `arn:aws:events:` and `kafka://` are logged as not sent, with why.
  **Only to this machine or to `--app-url`** (`AppUrls::may_send_to`): a URL whose host is not the
  loopback (`subscriptions::is_loopback_host`: `localhost`, a name under it, `127.0.0.0/8`, `::1`;
  not `0.0.0.0` nor `::ffff:127.0.0.1`), as the HTTP client reads it, is sent only when it
  is on the origin of `--app-url`, or is an address given explicitly (`--address`, a trigger's
  `address`). Anything else (a relative `uri` joined to a remote `application_url`, another host,
  a subscription the backend made) is logged `not_sent` with `subscriptions::off_machine` (start
  lsf with `--app-url`), and `lsf app webhook trigger` refuses it before sending anything (exit
  2), with `off_machine_with` (give `--app-url` or `--address`: it starts no server): a webhook
  is signed with the app's real secret when `.env` has it. One test of the host decides everywhere the rule
  applies: the `uri` of `webhookSubscriptionCreate` and `…Update` (`is_local_http`), the
  deliveries, `lsf app webhook trigger` and its `--address`, and the `webhooks:` line of the
  banner (`Webhooks::summary`). A name under `localhost` (RFC 6761) connects to the loopback,
  `127.0.0.1` then `::1`, whatever the system resolver says (`delivery::LocalhostNames`), and
  the request keeps its `Host`.
- **Payload**: the JSON of Shopify's webhooks, which is the REST representation of the resource
  (`id` as a number, `admin_graphql_api_id`, snake_case, prices as strings with two decimals,
  enums in lowercase, tags as one string), in the key order of Shopify's example payload of the
  topic. `webhooks::payload` builds it from the record through a `Lookup` (the records a record
  names by GID, and the `Shop`), so that it needs nothing of the state but its records: the
  builders are there, by family, rather than in `domains/`. A test compares the payload of every
  topic lsf builds with Shopify's example of each version (`tests/fixtures/webhooks/examples-<v>.json`,
  written by `tools/admin/webhook_examples.mjs`), key for key and value for value, with the
  differences listed and explained (the reference has no example of `subscription_billing_attempts
  /*`: the generator adds those of Shopify's subscriptions reference app, the same resource for
  the three topics). `include_fields` keeps only those fields (`variants.price`
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
  `X-Shopify-API-Version` (the version the payload is built in: the subscription's as the server
  serves it, with a note when it is another; see "Webhooks per version"), `X-Shopify-Webhook-Id` (one per delivery, the same across its retries),
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
- **Control**: `GET /__lsf/webhooks` lists the deliveries (`?topic=`, `?shop=` short or full,
  `?state=`, `?since=<id>`, `?resource=<gid>`, the record of the event, kept in each delivery's
  `resource`; `?wait=<n>` waits until n of those that match every filter are done, `?timeout=<ms>`
  at most, 10 s by default, and answers `408` when they were not), with `last`, the newest id of
  the log (ids are never given twice), for the next `since`; `DELETE /__lsf/webhooks` forgets them,
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
and replace. `SourceData::build(options, &[overlay])` over the data the files gave (or
`load_with_overlays(source, options, &[overlay])`, which reads them first) gives the store with
the changes. `None` when nothing the store data expresses changed.

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
| `metaobjects` | `Metaobject` (type, handle, fields, template suffix). A draft entry is removed. An app's own type is `$app:<type>`, as its namespaces are; lsf-core finds it by that name and by `app--<any id>--<type>`, the one a theme uses. |
| `selling_plan_groups` | `SellingPlanGroup` (name, app id, options, products) with its `SellingPlan`s (name, description, options, pricing policies, recurring or not, checkout charge) |
| metafields | of every owner above, of variants and company locations, of the shop and of the declared markets: the namespace of the app (`app--<id>--config`) as `$app:config`, references as the keys the data names them by (handle, `blog/article`, `type/handle`, path of `files/`, variant id, email, company name), JSON types as JSON |
| `shop` | name, email, description, the five policies of the data, metafields (merged field by field; a metafield removed is in `removed.shop_metafields`) |
| `localization` | rewritten when a market or a locale changed: the markets declared in the data or created by the Admin API (renamed or not; not a country's market of its own), active and of type `REGION`, with their countries (a country they add is added) and metafields; each country in the currency its active region market sells in (its own with `localCurrencies`, else the market's base currency, a country's own market included); the published `ShopLocale`s as the languages, the primary first |
| `checkout.discounts` | rewritten when an app discount changed: every `DiscountAutomaticApp` and `DiscountCodeApp` active by the shop's clock (`startsAt` ≤ now < `endsAt`), in id order, with its `id`, the handle of its Function (`appDiscountType.functionId`), title, first code, classes and metafields |
| `checkout.functions` | a Function whose owner changed: `enabled` (`true` when one of its `CartTransform`, `DeliveryCustomization`, `PaymentCustomization` and `Validation` objects runs it, `false` when every one is disabled, left out when it has none), and the metafields of the enabled ones (of the disabled ones when none is), in creation order, a later one's taking the place of an earlier one's. The checkout runs a Function that is not a discount only through an enabled owner. |
| `checkout.shipping_rates` | rewritten when a delivery profile changed: the rates of the data in their order, each shipping rate replaced by the method definition the seed made of it in the general profile's zones that ship to the rest of the world (left out when gone or inactive), then that zone's other active method definitions with a flat price and no condition, their handle made of their name (`logistics::profiles::checkout_rates`) |

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
- **Owners of one Function**: two customizations of one Function share one set of settings
  and run it once; on Shopify each runs it with its own metafields.
- **Products**: SEO, the category, bundle components (a bundle syncs as a product of its own,
  available while its components are: the checkout sells it as one line, which a cart transform
  Function may expand, `tests/bundles.rs`) and combined listings, product feeds, the inventory
  item's cost and
  customs data, media taken from a URL outside `files/`, the recommendations a product of the
  data sets (written again, it gets the default ones), whether an untracked variant is in stock
  at one location and not another (it is wherever it is stocked once its inventory changed).
- **Collections**: rules and sources (lsf-core resolves rules; a changed automated collection, or
  one made of sources, is written as a manual one with the products it has), publication (an
  unpublished collection stays), SEO.
- **Customers**: the note, the language, tax exemptions, SMS consent, a customer without an
  email (removed from the storefront), the roles of a contact beyond the locations it orders
  for, a second contact.
- **Companies**: the note, the billing address, catalogs and price lists (the local checkout
  gives a company location buyer the prices and quantity rules of the data).
- **Content**: a blog's feed, comments that are not published, menu items of a type the data
  has no field for (a metaobject, a customer account page: written by URL); `main-menu` and
  `footer` always exist (lsf-core adds them back).
- **Markets**: a country's market of its own (its metafields; its currency syncs), the rest of
  the currency settings (a manual rate, rounding: the storefront converts no price), price
  inclusions, a market of company locations or locations (the storefront has no such buyer),
  web presences (the storefront has one address and serves every language under
  `/<locale>/`), catalogs, price lists and their fixed prices, quantity rules and price breaks
  (one price and one rule per variant in the store data, for every buyer), market
  localizations, the backup region, channels and their listings, resource feedback, an
  unpublished locale (not served).
- **Locations**: fulfillment services; the "Shop location" lsf adds to a store without locations
  becomes a location of the data once it changes.
- **The shop**: anything else (domains, currency, time zone, plan: the Admin API does not
  change them).
- **Subscriptions**: contracts, drafts, calculations, billing attempts and saved payment methods
  (the store data has none of them, and the local checkout charges no saved method). The stock a
  billing attempt's order commits syncs, as any order's.
- Segments, saved searches, marketing activities and events, analytics annotations and targets,
  consent policies and privacy settings, abandoned checkouts and abandonments, staff assignments:
  none is store data. A merge, an erasure, a data sale opt-out and a WhatsApp consent change
  customers, which sync as customers do (the WhatsApp and the opt-out are not in the data).
- **Point of sale**: devices, cash drawers, reason codes, payment sessions.
- **Logistics**: inventory transfers and shipments (the stock they move syncs), fulfillment
  orders, fulfillment constraint rules, shipping labels, carrier services (the checkout does not
  call them), delivery promises, shipping packages; delivery profiles beyond the general
  profile's flat rates to the rest of the world: other zones and profiles, conditional rates,
  carrier services' rates (the store data has one list of rates, offered to every address).
- Metafield and metaobject definitions, files (`files/` is on disk), translations, webhooks,
  publications other than the Online Store.
- Themes, theme files and script tags are not store data: the storefront gets them through the
  theme it serves (see "Themes"), not the overlay of the store data.
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
  `isActive`, and `isFulfillmentService: false` (a fulfillment service's location is not one of
  the store data).

### Ids in the store data

A record created with an id of the counter keeps it in the store, for the entities whose data
has an `id` (products, variants, collections, pages, blogs, articles, comments, customers and
their addresses, companies and their locations, locations, selling plans, images, media other
than images, discounts). The others take the id lsf-core derives from their key, which the
mutation that creates one must give it: `metaobject_gid(type, handle)`, `menu_gid(handle)`,
`market_gid(handle)`, a gift card `stable_id("gift_card", code)`, an image of a product
`stable_id("media", "<handle>/<position>")`. Their id changes in the store when their key does
(a metaobject's handle, a menu's handle), and the images of a product follow their positions.

The ids of what the Admin API did not touch do not change: the overlay writes only what
changed, every entity it writes carries its id, a renamed entity stays where it was, and a
removal keeps the ids and the dates of the others (an undated entity after a removed one keeps
the date its position gave it). Sessions and carts that name variants by id keep working.

### Server wiring

The plan was to project after each change, under the shop's lock, and to build the store lazily.
The projection is lazy too: on the store of 3,000 products of the timing below it takes 40 to 70 ms,
which every mutation would have paid on the Admin request's path, ten times for a burst of ten.

1. **At start** (`ServerState::new`), the files are read once (`lsf_core::store::load::
   SourceData::read`: parsed, checked and merged) and kept (`Loaded::source`, with the
   `LoadOptions` the store was built with); the store built from them is the *base*
   (`Loaded::base`): it seeds the shops (`AdminApi::new`) and turns sync on
   (`AdminApi::sync_store(base)`). The store the storefront sees starts as the base. Every
   later build starts from that data, never from the files on disk: `--static` reads them once,
   and without `--watch` a file changed on disk but not reloaded does not leak into a store
   whose overlay was computed against the old base (the files' image sizes are kept too).
2. **A change of the default shop is counted, not projected.** `ShopState::revision` grows with
   every write (`insert`, `insert_as`, `remove`, `get_mut`, `touch`, a reset), so `graphql` and
   `change_shop` (an order the checkout records goes through it) tell a request that wrote from
   one that only read by comparing it before and after, and count the first kind
   (`AdminApi::store_changes`, an atomic counter) while the shop is still locked. A reset
   (`DELETE /__lsf/admin`) counts too. Only the default shop's `LiveShop` (`default: true`, kept
   through a reseed that moves it) counts: the other shops never sync. A change that does not go
   through `graphql` or `change_shop` must count itself the same way (`AdminApi::install` writes
   only the installation's scopes, which do not sync, and does not). **The time** changes what
   the overlay says too (an app discount runs between its `startsAt` and `endsAt`): a request
   that moved the clock counts (`POST /__lsf/admin/clock`), and on the system clock the overlay
   says when it goes stale by itself (`sync::next_moment`, the next start or end of an app
   discount), which the count passes once that instant is past (`StoreSync::changes`, no lock).
3. **The next request that reads the store** (`dispatch`, after the Admin API's own paths, and
   `observe_changes`, which live reload calls) compares the count with the one its store was
   built at (`ServerState::admin_changes`): equal, it goes on (one atomic read). Different, it
   takes the build lock (`ServerState::rebuilding`, which `reload` takes too) and asks
   `AdminApi::synced`, which projects the shop under its lock when the count moved since the
   last projection (`api::sync::StoreSync::update`), keeps the overlay when it is the same
   (`Overlay: PartialEq`), and moves the generation on only when it is not. A new generation is
   built (`loaded.source.build(&loaded.load_options, &[overlay])`, or the base itself for no
   overlay) outside the shop's lock, and swapped into `Loaded` whole: a page rendered meanwhile
   has the `Arc<Loaded>` it started with. A burst of mutations is projected and built once, by
   the first request that follows it; an order recorded at the checkout makes the same overlay
   and builds nothing (`an_order_placed_at_the_checkout_does_not_build_the_store_again`).
4. **Sessions with their own data** keep it (`SessionEntry::data`, with the generation their
   store was built at). Their store is `[sync_overlay, Overlay::new(OVERLAY_FILE, data)]` on
   the same data (`ServerState::session_store`, which `PUT /__lsf/session` uses too), built again on the
   session's next request when the generation moved. A session named only by
   `GET /__lsf/checkout?session=` sees its store as of its own last request.
5. **Live reload**: the token has the generation of the store built (`Loaded::synced`), and
   `observe_changes` builds before it computes the token: the pages reload once the store is
   built, and find it so.
6. **The files change** (without `--static`, or `POST /__lsf/reload`): `reload_and_reseed`
   reads the files again and loads the new base, reseeds
   (`AdminApi::reseed`; the terminal says what was dropped), moves sync to the new base
   (`sync_store`, which counts a change), applies the theme the reseeded shop has
   (`sync_theme`: the directory, without the Admin API's files or script tags), and builds
   from `synced()`: the store is the new base, unless the backend changed the shop in between.
   The watch (`refresh_if_changed`) keeps the files as they were last read
   (`server::data_files::DataFiles`: path, size, modification time) and sorts what changed by
   `lsf_core::store::load::role_of`: the store's own data reseeds as above; checkout scenarios
   alone (`checkouts/*.json`, which the seed does not read) go through `reload_scenarios`,
   which reads the files and moves sync to the new base without reseeding, so the shops keep
   the backend's changes and the store is the new files with the same overlay; a file the store
   does not read (`schema/`, `README.md`) does nothing. The line says which files changed and
   what was done. `POST /__lsf/reload` always reseeds.
7. **What the store data refuses** of the overlay: the problems of the store built with it that
   the files alone do not have (`Applied::diagnostics`) are printed when they differ from the
   last build's, and given to `AdminApi::sync_built`, which `GET /__lsf/admin` shows in
   `storeSync.built`. The store is built from what is valid, as from files with errors. Two
   entities of one key in the overlay (two companies of one name, which Shopify allows and the
   store data, keyed by name, does not) are both written, so that the build says so: an
   overlay entity replaces one of its key only when the overlay did not write that one
   (`lsf_core::store::load`).
8. The checkout's configuration comes from the store (`config_from_store`): with the synced
   store, it has the discounts and the settings of the Functions the Admin API changed, and a
   scenario's `checkout` still goes on top of them.

`GET /__lsf/admin` gives `storeSync`: `enabled`, `generation`, `pending` (counted, not projected
yet), `overlay` (`written`: entities by section; `removed`: their keys), `built` (`generation`,
`ms`, `diagnostics`).

**What a mutation costs** (release build, through the handlers, the fixture theme):

| Store | Records | Mutation (Admin request) | Next page: projection and build | Page without a change |
|---|---|---|---|---|
| Demo (8 products) | 251 | 0.5 ms | 0.3 ms (build 0.1 to 0.2 ms) | 0.1 ms |
| 3,000 products of 12 variants | 198,000 | 0.06 ms (300 to 400 ms with the copy of the shop a mutation made before the undo log) | 100 ms (build 56 to 58 ms, the projection most of the rest) | 0.1 ms |

- Measured on 2026-10-31 (three alternated runs, medians, a loaded machine): the build was
  0.3 to 0.4 ms and 170 to 210 ms when it read the files again (`load_with_overlays` on the
  source), the next page 0.5 ms and 240 to 270 ms. Building from the data read once
  (`SourceData::build`: a copy of the merged data, the overlay, the build) skips reading,
  parsing and checking 3,000 files. An earlier measure, before the build's recommendations were
  fixed and on another machine, had the large store at 290 to 370 ms.

- The build was cubic before: a product's default recommendations checked every product against
  the list so far before keeping ten. 2,000 products took 1.2 to 2 s to load, 3,000 products 5 to
  6 s; now 180 ms and 300 to 370 ms (`lsf-core`, `build_recommendations`).
- The 400 ms a mutation took on the large store were not sync's: the mutation framework cloned
  the whole shop before the first write of a mutation, to undo it on `userErrors`. The undo log
  of "Mutations" replaced it: on an Apple M4 Pro, in release, a `productUpdate` went from 180 ms
  to 0.06 ms, and a failure that changed a product and its 12 variants and created a product
  rolls back in 0.04 ms (1.2 ms when it also removed a variant: removing one of 198,000 records
  shifts the others, forward and back). `tests/mutation_cost.rs` measures it
  (`cargo test --release -p lsf-app --test mutation_cost -- --ignored --nocapture`).
- Seeding 3,000 products takes 1.7 to 3.4 s, at start and on each reseed, not on a mutation.

## Crate layout

The Admin API is the `admin` module of `lsf-app` (`crates/app/src/admin`), whose module paths
below are relative to it. It reads the store of `lsf-core` and the rest of `lsf-app`: the app
(`config::App`, whose `config` is the backend's part of `shopify.app.toml`) and the checkout's
Functions and types. It serves nothing itself: `lsf` routes the requests to [`api`], as it does
for the checkout.

| Module | Role |
|---|---|
| `version` | The versions lsf knows (`KNOWN`: each one's status, files and adapters), the ones a server serves (`Served`), which one answers a request; `version::adapt`, how a version reads the records differently. |
| `schema` | The schema of each version, parsed at its first request from `schemas/admin/<version>/admin.graphql` (generated by `tools/admin/admin_schema.mjs` from the pinned `@shopify/dev-mcp`, with its descriptions, for introspection; embedded Brotli-compressed). |
| `graphql` | Parsing (a parser of its own), validation, variables, execution, introspection, cost: a GraphQL server over a `Resolver`. Knows nothing of Shopify's data. |
| `state` | Shops, records, ids, the clock, events, the changes since the seed; the seed from `lsf_core::Store` (`state::seed`, which runs the seed of each domain). |
| `sync` | Store sync: the records changed since the seed, projected to an overlay of the store data. |
| `resolve` | The generic resolution of records, connections and root fields, and the registry of custom resolvers. |
| `domains/` | One module per family of resources: its seed, its custom resolvers, its mutations (`catalog`, `orders`, `customers`, `content`, `app`, `markets`, `subscriptions`, `logistics`, `segments`, `customer_merge`, `privacy`, `marketing`, `analytics`, `staff`, `checkouts`, `pos`, `platform`, `store_admin`, `storefront`, `themes`). Its webhook payloads are in `webhooks::payload`, built from records alone. |
| `search` | Shopify's search syntax. |
| `auth` | Tokens, OAuth, token exchange, session tokens, access scopes, HMAC. |
| `webhooks` | Topics, subscriptions and their registry, payloads (by family), signing, the delivery worker and its log, the `/__lsf/webhooks` endpoints, the sample records of `lsf app webhook trigger`. |
| `outbox` | What lsf records instead of sending it to the outside world, in a memo of the shop (see "The outbox"). |
| `throttle` | The cost bucket of each shop and app. |
| `api` | One HTTP request in, one answer out: the Admin API endpoints, OAuth, and the `/__lsf/admin` and `/__lsf/webhooks` control endpoints. `api::sync`: store sync as a server runs it (the count of changes, the overlay made when asked). `api::simulate`: the clock, the outbox, and what buyers, banks and Shopify do. `api::embedded`: the admin of an embedded app and its resource picker's list. |
| `assets` | The files of the admin of an embedded app and lsf's App Bridge (`assets/admin/`), embedded in the binary. Their tests: `assets/tests/admin.test.mjs`, `app-bridge.test.mjs`. |
