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
- `extensions.cost` on every answer: `requestedQueryCost`, `actualQueryCost`, `throttleStatus`
  (`maximumAvailable`, `currentlyAvailable`, `restoreRate`), computed as Shopify documents it
  (scalars and enums 0, objects 1, connections 2 + `first`/`last` × the cost of a node, a
  mutation 10), a leaky bucket per shop and app. A query over 1000 points is refused
  (`MAX_COST_EXCEEDED`), an empty bucket answers `THROTTLED`. `--admin-throttle off` (or the store
  data) turns it off; the plan sets the size of the bucket.
- `extensions.lsf` (only lsf adds it, and only when there is something to say): the fields that
  were answered with a default because lsf does not simulate them (`defaulted`: their paths), so
  that a developer never mistakes a default for data.

## Shops, apps and tokens

- **One shop by default**: the store data's (`shop.permanent_domain`, e.g.
  `my-shop.myshopify.com`), on which the app is installed when the server starts, with the
  scopes of `shopify.app.toml` (`[access_scopes] scopes`; every scope when there is no app).
- **More shops for isolation**: `POST /__lsf/admin/shops {"shop": "test-1"}` creates
  `test-1.myshopify.com`, seeded from the same store data, with the app installed, and answers its
  token. Parallel tests each get a shop of their own, as storefront tests each get a session.
  A token belongs to one shop: the token, not the host, picks the shop of a request (the backend
  sends every shop to the same `http://127.0.0.1:9292`).
- **Access tokens**: the one printed at start (deterministic for a shop and an app, so a test can
  hard-code it), and those the OAuth endpoints issue. Offline tokens `shpat_…`, online tokens
  `shpua_…`.
- **OAuth**, as Shopify: `GET /admin/oauth/authorize` approves at once and redirects to the
  `redirect_uri` (checked against `[auth] redirect_urls`) with `code`, `hmac`, `host`, `shop`,
  `state`, `timestamp`; `POST /admin/oauth/access_token` exchanges the code, or a session token
  (token exchange, `urn:ietf:params:oauth:grant-type:token-exchange`, offline or online access
  token, expiring or not), or the client credentials. Every answer is Shopify's JSON.
- **Session tokens** (App Bridge's `id_token`): `POST /__lsf/admin/session-token` mints one
  (HS256 with the client secret, Shopify's claims), so that a test can call the authenticated
  routes of the backend; `GET /__lsf/admin/launch` redirects to the app URL with the query an
  embedded app is opened with (`embedded`, `hmac`, `host`, `id_token`, `locale`, `session`,
  `shop`, `timestamp`).
- **Access scopes are enforced**: a field or a mutation the token's scopes do not cover answers
  `ACCESS_DENIED`, with the scope it requires (`write_*` implies `read_*`). A table maps the root
  fields and mutations to their scope.

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

- A field holds its GraphQL value: scalars as Shopify writes them (`Money` and `Decimal` as
  strings, `DateTime` in RFC 3339 with `Z`), enums as their name, embedded objects (`MoneyV2`,
  `MailingAddress`, `SEO`, …) as nested objects, references to other records as their GID, a
  connection or a list of records as an array of GIDs.
- A record of a type with an interface or a union keeps `__typename`; nested objects take theirs
  from the field type, or from their own `__typename` when the field type is abstract.
- New records get ids from a counter per type that starts above every seeded id. Every change
  sets `updatedAt` from the clock of the shop: the store data's `now` when it has one (a test reads
  the same dates every run), the system clock otherwise.
- Store data edited while the server runs (`--watch`) reseeds every shop: what the Admin API
  changed is dropped, and the terminal says so. `DELETE /__lsf/admin` reseeds on demand.

### How a field is resolved (`resolve`)

The executor asks the resolver for each field. The resolver of the domain owning a type
handles what needs code (arguments, computed values, filters); everything else is generic:

1. A custom resolver registered for `(type, field)` answers it.
2. Otherwise the record's own value, converted by the schema type: a GID for an object type is the
   record it names (`null` when the record is gone), an array for a connection type is paginated
   (`first`, `last`, `after`, `before`, `reverse`, `sortKey`, `query`), an array for a list is
   mapped.
3. Otherwise a default: `null` for a nullable field; for a non-null one, an empty list or
   connection, `""`, `false`, `0`, `"0.0"` for `Money`/`Decimal`, the first enum value, or the
   defaults of an object type, recursively. The path goes into `extensions.lsf.defaulted`, except
   for fields that are empty by nature (an empty connection of a record that has none).

Root fields follow conventions before custom code: `products` (a connection of `Product`) lists
every record of the type; `product(id:)` reads one; `node(id:)` and `nodes(ids:)` read any;
`productsCount` counts with the same `query`. Cursors are opaque (base64 of the position and the
GID), stable across requests while the list does not change.

### Search syntax (`search`)

The `query` argument of connections, and the `filter` of webhook subscriptions, use Shopify's
search syntax: terms, `field:value`, comparators (`:<`, `:<=`, `:>`, `:>=`), `AND`, `OR`, `NOT`,
`-`, parentheses, quoted phrases, `*` prefix and existence (`field:*`), dates (`created_at:>2026-01-01`,
`created_at:>-7d`?). `search` parses it into a tree and matches a record through a per-type table
of the fields each connection documents (`title`, `tag`, `status`, `vendor`, `sku`, `email`, …);
an unknown field matches nothing, as Shopify. A free term matches the type's default text fields.

### Mutations (`mutations`)

A mutation is a function from the state and its arguments to its payload, run while the shop is
locked. It validates as Shopify does and answers `userErrors` (`field`, `message`, `code`) when
the input is wrong, changes nothing when it fails, and returns the **events** of what it
changed. A mutation lsf does not simulate answers a GraphQL error with
`extensions.code = "LSF_NOT_SIMULATED"`, never a silent success.

## Events and webhooks (`webhooks`)

A change emits events: `{topic, shop, gid, at}` (`products/update`, `orders/create`, …), after
the mutation succeeded. Events become deliveries for each subscription of the shop whose topic
matches and whose `filter` matches the payload:

- **Subscriptions**: those of `shopify.app.toml` (`[[webhooks.subscriptions]]`, app-wide,
  `api_version` of `[webhooks]`), and those the backend creates through the Admin API
  (`webhookSubscriptionCreate`, per shop). Compliance topics (`customers/data_request`,
  `customers/redact`, `shop/redact`) are only triggered on demand.
- **Payload**: the JSON of Shopify's webhooks, which is the REST representation of the resource
  (`id` as a number, `admin_graphql_api_id`, snake_case, prices as strings), built from the
  record when the event is delivered; `include_fields` keeps only those fields. Topics whose
  payload lsf does not build are listed, not sent.
- **Headers**: `X-Shopify-Topic`, `X-Shopify-Hmac-Sha256` (base64 HMAC-SHA256 of the body with
  the client secret), `X-Shopify-Shop-Domain`, `X-Shopify-API-Version`, `X-Shopify-Webhook-Id`,
  `X-Shopify-Event-Id`, `X-Shopify-Triggered-At`, `X-Shopify-Name` for a named subscription,
  `Content-Type: application/json`, `User-Agent: Shopify-Captain-Hook`.
- **Delivery**: from a background worker, never from the thread of a request (the backend's
  handler may call the Admin API back). A `2xx` within 5 seconds is a success; anything else is
  retried a few times with a short back-off (Shopify retries 8 times over 4 hours), then marked
  failed. Each attempt is logged: the request, the status, the time, the start of the answer.
- **Control**: `GET /__lsf/webhooks` lists the deliveries (`?topic=`, `?wait=<n>` waits until there
  are n, for tests), `DELETE /__lsf/webhooks` forgets them, `POST /__lsf/webhooks/trigger`
  sends one (`{"topic", "id"?}`), as `lsf app webhook trigger` does from the command line.

## Store sync

Admin API changes that the store data can express are written back into the store the
storefront and the checkout see: a product created by the backend is on the theme's collection
page, an app discount created with `discountAutomaticAppCreate` runs its Function at the
checkout with the metafields it was given, a customer created is one the checkout can log in as.
The state projects its changed records to a store-data overlay, applied with
`lsf_core::store::load::load_with_overlay` (which learns to remove entities). The checkout
reports placed orders to the state, which records them and emits `orders/create`.

## Crate layout

`lsf-admin` (`crates/admin`) depends on `lsf-core` (the store) and `lsf-checkout` (the app:
`app::App`, whose `config` is the backend's part of `shopify.app.toml`, and the Functions). It
serves nothing itself: `lsf` routes the requests to [`api`], as it does for the checkout.

| Module | Role |
|---|---|
| `version` | The API versions lsf serves (2026-10), from the path of a request. |
| `schema` | The schema of the Admin API, parsed once from `schemas/2026-10/admin.graphql` (generated by `tools/admin/admin_schema.mjs` from the pinned `@shopify/dev-mcp`, with its descriptions, for introspection). |
| `graphql` | Parsing, validation, execution, introspection, cost: a GraphQL server over a `Resolver`. Knows nothing of Shopify's data. |
| `state` | Shops, records, ids, the clock, events; the seed from `lsf_core::Store`. |
| `resolve` | The generic resolution of records, connections and root fields, and the registry of custom resolvers. |
| `domains/` | One module per family of resources: its seed, its custom resolvers, its mutations, its webhook payloads (`catalog`, `orders`, `customers`, `content`, `app`). |
| `search` | Shopify's search syntax. |
| `auth` | Tokens, OAuth, token exchange, session tokens, access scopes, HMAC. |
| `webhooks` | Topics, subscriptions, payloads, signing, the delivery worker and its log. |
| `throttle` | The cost bucket of each shop and app. |
| `api` | One HTTP request in, one answer out: the Admin API endpoints, OAuth, and the `/__lsf/admin` and `/__lsf/webhooks` control endpoints. |
