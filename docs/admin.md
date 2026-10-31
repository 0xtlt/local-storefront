# Admin API

The backend of a Shopify app talks to Shopify through the **GraphQL Admin API** and receives
**webhooks**. While it runs, local-storefront serves a local Admin API (version **2026-10**, and
[2027-01 on demand](#api-versions)) over the store data, with OAuth, token exchange and session tokens, and delivers the webhooks of
the app to its backend, signed as Shopify signs them. The backend runs and is tested offline: no
development store, no tunnel, the same data every run, and the failures that are hard to provoke
on a real store (a missing scope, throttling, an uninstall) on demand.

A product the backend creates through the Admin API is a product of the local shop, on the
theme's pages at once ([what the storefront and the checkout
see](#what-the-storefront-and-the-checkout-see)); an order placed in the [local
checkout](checkout.md) is an order the Admin API lists, and its `orders/create` webhook reaches
the backend.

## Start

```bash
npx local-storefront app dev --app ../my-app --app-url http://localhost:3000
```

```text
ready:  http://127.0.0.1:9292/   (extensions and checkout scenarios; …)
admin:  http://127.0.0.1:9292/admin/api/2026-10/graphql.json
        X-Shopify-Access-Token: shpat_4f…   (shop: my-shop.myshopify.com)
webhooks: 3 subscriptions of shopify.app.toml → http://localhost:3000
```

Every server serves the Admin API (`serve` too, with a theme). With `--app`, the client id, the
scopes and the webhooks are those of `shopify.app.toml`; without, a local client id
(`lsf-local-client-id`), every scope, and no subscription.

`app dev` serves the app alone: its checkout, its customer account, the Admin API and the app
proxy, but no theme, so no storefront pages (`/products/…` answers `404`). To see on the theme's
pages what the backend changes, serve the theme with the app:

```bash
npx local-storefront serve --theme ../my-theme --app ../my-app --app-url http://localhost:3000
```

`examples/admin-backend` of the repository is a backend written with Shopify's libraries (the
React Router package and `@shopify/shopify-api`), with a script that runs it against lsf end to
end: every recipe below is checked there.

- **The shop** is the store data's: `shop.permanent_domain` (`my-shop.myshopify.com`), seeded
  from the products, collections, customers, orders, metafields, metaobjects, markets, discounts
  and Functions of the data. The ids are those the storefront, the checkout and the Function
  inputs use.
- **The token** printed is an offline token the app is installed with, with the scopes of
  `[access_scopes] scopes`. It is the same every run for a shop and an app: a test can hard-code
  it.
- **The client secret** signs webhooks, OAuth answers and session tokens. Give lsf the backend's:
  `--client-secret`, or `SHOPIFY_API_SECRET` in the environment or in the app's `.env` (where
  `shopify app dev` and the app templates read it). Without one, lsf signs with
  `lsf-local-client-secret` and says so at start.

| Option | Environment variable | What it does |
|---|---|---|
| `--app-url <url>` | `LSF_APP_URL` | Where the backend listens. Webhooks with a relative `uri` go there, and an absolute `uri` on the origin of `application_url` (often the tunnel of a `shopify app dev` that is not running) is moved there. Without it, `application_url`. |
| `--client-secret <secret>` | `LSF_CLIENT_SECRET` | The client secret. |
| `--admin-throttle <plan>` | `LSF_ADMIN_THROTTLE` | The cost bucket: `store` (default), `standard`, `advanced`, `plus`, `enterprise`, `legacy`, `off`. |
| `--admin-versions <list>` | `LSF_ADMIN_VERSIONS` | The [API versions](#api-versions) served: `2026-10` alone by default, `2026-10,2027-01` with the release candidate. |

## API versions

| Version | Shopify's status | Served |
|---|---|---|
| `2026-10` | latest stable | by default |
| `2027-01` | release candidate | with `--admin-versions 2026-10,2027-01` |

Each version is served at `/admin/api/<version>/graphql.json` with its own schema: its fields,
mutations and enum values (a field of another version answers Shopify's `Field '…' doesn't
exist on type '…'`), its introspection, its access scopes, its search filters and its webhook
topics. `X-Shopify-API-Version` says which version answered.

```bash
npx local-storefront app dev --app ../my-app --admin-versions 2026-10,2027-01
```

```text
admin:  http://127.0.0.1:9292/admin/api/2026-10/graphql.json
        also API version 2027-01 (release candidate)
```

- **A version Shopify no longer supports** (`2025-10`, `2024-01`, …) is answered by the oldest
  stable version served, as Shopify [falls forward](https://shopify.dev/docs/api/usage/versioning)
  to "the oldest accessible stable version": `X-Shopify-API-Version` says `2026-10`, and
  `X-Lsf-Reason` why.
- **A version lsf does not serve** answers `404` with a JSON message that says which ones it
  serves: a version Shopify supports but lsf has no data for (`2026-07`), `unstable`, and
  `2027-01` without the option, whose message says how to serve it. lsf does not answer them
  with another version's schema.
- **Webhooks** are built in the version of their subscription: `[webhooks] api_version` of
  `shopify.app.toml`, or the version of the request that made it with
  `webhookSubscriptionCreate`. A version the server does not serve is built in 2026-10, and the
  delivery's notes (`GET /__lsf/webhooks`) say so. `lsf app webhook trigger --api-version
  2027-01` builds its payload in 2027-01.
- **The records are the same** in every version: what a 2027-01 request creates, a 2026-10 one
  reads. Where 2027-01 shows a record differently, lsf adapts it: a metaobject definition whose
  admin access is `PUBLIC_READ` or `PRIVATE` (removed in 2027-01, "Use MERCHANT_READ instead")
  reads `MERCHANT_READ` in 2027-01.
- `GET /__lsf/admin` lists the versions lsf knows, their status and whether they are served
  (`apiVersions`).

Adding another version is a data drop and a line of code: `mise run admin:version -- <version>`
writes its files from Shopify's introspection and says what is left
(`crates/admin/docs/design.md`, "Add a version").

## Point the backend at lsf

Shopify's libraries build the URL of a shop as `https://<shop>/admin/api/<version>/graphql.json`
(and `https://<shop>/admin/oauth/access_token` for tokens), and some force `https` on whatever
host they are given. lsf listens on `http://127.0.0.1:9292` and serves every shop there: the
token, not the host, picks the shop. So the way in is the library's HTTP layer: send the
requests for `*.myshopify.com` to lsf. Use API version `2026-10` (or a version you serve with
`--admin-versions`); lsf answers another with a `404` that says so, and one Shopify no longer
supports with 2026-10, as Shopify does ([API versions](#api-versions)).

The recipes read the address of lsf from an environment variable of your own (`LSF_URL` here),
so that the same code talks to Shopify when it is not set.

### React Router app (`@shopify/shopify-app-react-router`) and `@shopify/shopify-api`

Every request of `@shopify/shopify-api` (the Admin API clients, token exchange, OAuth) goes
through its runtime's fetch function, which `setAbstractFetchFunc` of
`@shopify/shopify-api/runtime` replaces (`lib/clients/admin/graphql/client.ts`,
`lib/utils/fetch-request.ts` of 15.0.1). `@shopify/shopify-app-react-router/server` sets it to
`fetch` when it is imported (it imports `@shopify/shopify-api/adapters/web-api`; the adapters of
`@shopify/shopify-api` do the same), so replace it in the body of `app/shopify.server.ts`, which
runs after every import. A module of its own that sets it, imported before `/server`, is undone
without a word, and the backend talks to Shopify:

```ts
import "@shopify/shopify-app-react-router/adapters/node";
import { setAbstractFetchFunc } from "@shopify/shopify-api/runtime";
import { ApiVersion, shopifyApp } from "@shopify/shopify-app-react-router/server";

// Local development against lsf: the Admin API and OAuth of every shop go to it.
const lsf = process.env.LSF_URL; // http://127.0.0.1:9292
if (lsf) {
  setAbstractFetchFunc((input, init) => {
    const url = new URL(input instanceof Request ? input.url : String(input));
    if (!url.hostname.endsWith(".myshopify.com")) return fetch(input, init);
    const headers = new Headers(init?.headers);
    headers.set("x-forwarded-host", url.host); // the shop, for OAuth
    return fetch(`${lsf}${url.pathname}${url.search}`, { ...init, headers });
  });
}

const shopify = shopifyApp({
  apiVersion: ApiVersion.October26,
  // … the rest of the template: apiKey, apiSecretKey, scopes, appUrl, sessionStorage
});
```

`@shopify/shopify-api` is a dependency of `@shopify/shopify-app-react-router`; with a package
manager that does not hoist (pnpm), add it to the app's dependencies, at the same version.

Run the backend with the app's values (what `shopify app dev` would give it):

```bash
SHOPIFY_API_KEY=<client_id of shopify.app.toml> SHOPIFY_API_SECRET=<its secret> \
SCOPES=read_products,write_products SHOPIFY_APP_URL=http://localhost:3000 \
LSF_URL=http://127.0.0.1:9292 npm run dev
```

Then open the app as the admin would: `http://127.0.0.1:9292/__lsf/admin/launch` redirects to the
app URL with `embedded=1`, `shop`, `host`, `hmac` and a session token in `id_token`. The
template's `authenticate.admin` reads the session token, exchanges it for an access token at
lsf, and the loader's `admin.graphql(…)` queries lsf. The App Bridge script of the page needs
the real admin around it; what the server does is all there. A route the front end calls with
`fetch` is authenticated with `Authorization: Bearer <session token>`: mint one with
`POST /__lsf/admin/session-token`. A session token is valid for a minute, as Shopify's: a test
mints one per request. And `authenticate.admin` answers `410 Gone` to a client whose
`User-Agent` looks like a bot (the `isbot` package): curl's and Node's `fetch`'s do; send a
browser's.

- **Billing from a page.** `billing.request` (and `billing.require` with it) in the loader of a
  page sends the browser to the template's exit-iframe page, which only goes on to the admin
  over `https` (`admin.shopify.com`, `<shop>.myshopify.com/admin`): it refuses lsf's charge page
  (`http://127.0.0.1:9292/__lsf/admin/charges/<n>`) with "Invalid exit-iframe destination". Call
  it from a route the front end fetches: the library then answers `401` with the charge page in
  `X-Shopify-API-Request-Failure-Reauthorize-Url`, which App Bridge (or the test) opens. In a
  test, `POST /__lsf/admin/charges/<n>/approve` approves without the page.
- **OAuth.** The React Router package has no authorization code grant (it installs through
  token exchange). `auth.begin` of `@shopify/shopify-api` redirects the browser to
  `https://<shop>/admin/oauth/authorize?…`, which no fetch function sees: send it to lsf
  instead, the same path and query. lsf approves and redirects to the callback, whose
  `auth.callback` gets the token through the fetch function.

  ```js
  const response = await shopify.auth.begin({ shop, callbackPath: "/auth/callback", isOnline: false, rawRequest: request });
  if (process.env.LSF_URL) {
    const authorize = new URL(response.headers.get("location"));
    response.headers.set("location", `${process.env.LSF_URL}${authorize.pathname}${authorize.search}`);
  }
  return response;
  ```

`authenticate.webhook` verifies the webhooks lsf delivers with `apiSecretKey`: give lsf the same
secret.

### `@shopify/admin-api-client`

`createAdminApiClient` takes a `customFetchApi` (`dist/graphql/client.mjs` of 2.0.1). It always
builds an `https://` URL from `storeDomain` (`validateDomainAndGetStoreUrl` of
`@shopify/graphql-client` sets the protocol to `https`), so rewrite the URL in the fetch:

```js
import { createAdminApiClient } from "@shopify/admin-api-client";

const lsf = process.env.LSF_URL; // http://127.0.0.1:9292
const client = createAdminApiClient({
  storeDomain: "my-shop.myshopify.com",
  apiVersion: "2026-10",
  accessToken: process.env.SHOPIFY_ACCESS_TOKEN,
  customFetchApi: lsf
    ? (url, init) => fetch(String(url).replace(/^https:\/\/[^/]+/, lsf), init)
    : fetch,
});

const { data, errors, extensions } = await client.request(
  `query { products(first: 3) { nodes { id title } } }`,
);
```

### Ruby: the `shopify_api` gem

Every request of the gem (`Clients::Graphql::Admin`, token exchange, OAuth, refresh, client
credentials) is made by `ShopifyAPI::Clients::HttpClient`, whose base URI is
`https://<api_host or shop>` (`lib/shopify_api/clients/http_client.rb` of 16.3.1): `api_host`
cannot reach a plain-HTTP server. Rewrite the URL of each request instead, in an initializer:

```ruby
# config/initializers/lsf.rb
if ENV["LSF_URL"] # http://127.0.0.1:9292
  module LsfAdminApi
    private

    def request_url(request)
      super.sub(%r{\Ahttps://[^/]+}, ENV.fetch("LSF_URL"))
    end
  end
  ShopifyAPI::Clients::HttpClient.prepend(LsfAdminApi)
end
```

```ruby
ShopifyAPI::Context.setup(api_key: ENV["SHOPIFY_API_KEY"], api_secret_key: ENV["SHOPIFY_API_SECRET"],
  host: "http://localhost:3000", scope: "read_products", is_embedded: true, is_private: false,
  api_version: "2026-10")
session = ShopifyAPI::Auth::Session.new(shop: "my-shop.myshopify.com", access_token: token)
client = ShopifyAPI::Clients::Graphql::Admin.new(session: session)
client.query(query: "{ shop { name } }").body
```

### Python

The `ShopifyAPI` package (12.7.0) builds `https://<shop>.myshopify.com/admin/api/<version>`, but
its `GraphQL` resource reads its `endpoint` at each `execute` (`shopify/resources/graphql.py`):
point it at lsf.

```python
import os, shopify

session = shopify.Session("my-shop.myshopify.com", "2026-10", os.environ["SHOPIFY_ACCESS_TOKEN"])
shopify.ShopifyResource.activate_session(session)
graphql = shopify.GraphQL()
if os.environ.get("LSF_URL"):
    graphql.endpoint = os.environ["LSF_URL"] + "/admin/api/2026-10/graphql.json"
print(graphql.execute("{ shop { name } }"))
```

Without a library, a POST is all it takes:

```python
import requests

answer = requests.post(
    "http://127.0.0.1:9292/admin/api/2026-10/graphql.json",
    headers={"X-Shopify-Access-Token": token},
    json={"query": "query($n: Int!) { products(first: $n) { nodes { title } } }", "variables": {"n": 3}},
).json()
```

### `fetch` and `curl`

```js
const response = await fetch("http://127.0.0.1:9292/admin/api/2026-10/graphql.json", {
  method: "POST",
  headers: { "Content-Type": "application/json", "X-Shopify-Access-Token": token },
  body: JSON.stringify({ query: "{ shop { name myshopifyDomain } }" }),
});
```

```bash
curl -s http://127.0.0.1:9292/admin/api/2026-10/graphql.json \
  -H 'Content-Type: application/json' -H "X-Shopify-Access-Token: $TOKEN" \
  -d '{"query": "{ shop { name } }"}'
```

`application/graphql` bodies (the query alone) and `GET ?query=` work too, as on Shopify.

## Tokens

| Token | How to get it |
|---|---|
| The default token of a shop | Printed at start, in `GET /__lsf/admin` (`shops[].token`), and the answer of `POST /__lsf/admin/shops`. Offline, never expiring, the scopes of the app. |
| A session token (App Bridge's `id_token`) | `POST /__lsf/admin/session-token` `{"shop"?, "user"?}`: HS256 with the client secret, the claims of shopify.dev's "ID tokens". |
| An access token by token exchange | `POST /admin/oauth/access_token` with the session token, as the React Router template and the libraries do: offline or online, expiring or not. |
| An access token by the authorization code grant | `GET /admin/oauth/authorize?client_id=&scope=&redirect_uri=&state=` approves at once and redirects to `redirect_uri` with `code`, `hmac`, `host`, `shop`, `state`, `timestamp`; then `POST /admin/oauth/access_token`. `redirect_uri` must be one of `[auth] redirect_urls` (also on the origin of `--app-url`). |
| Refresh tokens, client credentials | `grant_type=refresh_token`, `grant_type=client_credentials`, as Shopify. |

Every access token is `shpat_` and 32 hex digits, as Shopify's. A token belongs to one shop. A
request without a valid token is answered `401`
`{"errors": "[API] Invalid API key or access token (unrecognized login or wrong password)"}`, with
an `X-Lsf-Reason` header saying why in plain words.

**Scopes are enforced.** A field or a mutation the token's scopes do not cover answers
`ACCESS_DENIED` with Shopify's message ("Access denied for products field. Required access:
`read_products` access scope."), for root fields, nested ones (an order's `customer` needs
`read_customers`) and `node`. `write_x` covers `read_x`. To see what the backend does with fewer
scopes: `POST /__lsf/admin/install {"scopes": "read_orders"}` installs the app again with them
(every token of the app on the shop now reads less, the printed one and those of token exchange
and OAuth alike, and `app/scopes_update` is delivered).

**Uninstall.** `POST /__lsf/admin/uninstall` does what Shopify does when a merchant uninstalls
the app: it revokes the app's tokens on the shop, cancels its subscription, delivers
`app/uninstalled` (through the subscriptions the shop had), then deletes the subscriptions the
backend made with `webhookSubscriptionCreate` ("Shopify automatically performs cleanup tasks,
such as deleting WebhookSubscription objects", [`appUninstall`](https://shopify.dev/docs/api/admin-graphql/2026-10/mutations/appUninstall)).
From then on nothing is delivered for the shop: an order placed at the checkout is recorded, and
no `orders/create` goes out, not even through `shopify.app.toml`, whose subscriptions apply to
"every shop that installs your app". Installing it again (`POST /__lsf/admin/install`, OAuth, or a
token exchange) brings back the subscriptions of `shopify.app.toml`; those of the shop are gone,
for the backend to make again, as on Shopify. `shop/redact`, which Shopify sends 48 hours after
an uninstall, is sent on demand: `POST /__lsf/webhooks/trigger {"topic": "shop/redact"}`.

## Webhooks

- **Subscriptions**: those of `shopify.app.toml` (`[[webhooks.subscriptions]]`, every shop the
  app is installed on), and those the backend makes with `webhookSubscriptionCreate` (per shop,
  deleted when the app is uninstalled; listed by `webhookSubscriptions`, which, as on Shopify,
  does not list those of the file). A subscription
  made through the Admin API takes `https://` and, locally, `http://` to this machine.
- **Events**: what the Admin API changes (`productCreate` → `products/create`,
  `inventory_items/create`, `inventory_levels/connect`; `productUpdate` and `tagsAdd` →
  `products/update`; `productDelete` → `products/delete`), an order placed at the checkout
  (`orders/create`, `order_transactions/create`, then `orders/paid` when it is paid in full), an
  uninstall, a change of scopes.
- **The storefront's own events**, as the online store's cart and checkout fire them on
  Shopify, through the default shop:
  - `carts/create` when a cart of the theme gets its first line (`/cart/add.js`, the cart
    form, `/cart/update.js`), `carts/update` at each change of its lines, note or attributes;
    the payload's `id` and `token` are the cart's token (`/cart.js`'s), its prices the
    variants' in the Admin API;
  - `checkouts/create` when the [local checkout](checkout.md) of a cart is first shown,
    `checkouts/update` when what the buyer sees changes (lines, email, phone, address, delivery,
    payment, discount codes) and when the order is placed (`completed_at`); reading the checkout
    again sends nothing;
  - a checkout with contact information and no order for more than ten minutes ("A checkout is
    considered abandoned if it remains incomplete for more than ten minutes", Help Center) is in
    `abandonedCheckouts`, and, for a customer of the shop, has an `Abandonment`; three months on,
    the admin deletes it (`checkouts/delete`). The time is the shop's: with a store data `now`,
    move it with `POST /__lsf/admin/clock {"advance": "11m"}` ([control API](control-api.md)).
- **Payloads** are Shopify's JSON for the topic (the REST representation of the resource), built
  from the record as the change left it, cut to `include_fields`, and checked against `filter`.
  A topic whose payload lsf does not build is logged as not sent, never sent with a made-up body.
- **Headers**: `X-Shopify-Topic`, `X-Shopify-Hmac-Sha256` (base64 HMAC-SHA256 of the body with the
  client secret), `X-Shopify-Shop-Domain`, `X-Shopify-API-Version` (the version the payload is
  built in: the subscription's, `2026-10` by default),
  `X-Shopify-Webhook-Id`, `X-Shopify-Event-Id`, `X-Shopify-Triggered-At`, `User-Agent:
  Shopify-Captain-Hook`. Every library's verification accepts them with the shared secret.
- **Delivery** is from a background worker, in order, retried after 1, 5 and 15 seconds when the
  backend does not answer `2xx` within 5 seconds.

On demand: `POST /__lsf/webhooks/trigger {"topic": "orders/create", "id"?: "<gid>"}` sends the
webhook of a record of the shop (a representative one without `id`), and
`npx local-storefront app webhook trigger --topic orders/create` does it from the command line,
without a server. See the [control API](control-api.md) and the [CLI](cli.md#app-webhook-trigger).

## What the storefront and the checkout see

What the backend changes through the Admin API is what the storefront and the
[local checkout](checkout.md) of the running server show, from the next request on:

- a product it creates is on the theme's pages and in the collections it joined, once it is
  `ACTIVE` and published on the Online Store; set to `DRAFT` or `ARCHIVED`, or unpublished, it
  leaves them;
- a customer it creates can log in, at `/__lsf/login?customer=<email>` and at the checkout;
- a metafield it sets is read by Liquid (`product.metafields.custom.care`, `shop.metafields…`)
  and by the Function inputs: the `$app:config` settings of a discount, a validation, a
  customization or a cart transform;
- an app discount it creates (`discountAutomaticAppCreate`, `discountCodeAppCreate`) runs its
  Function at the checkout, with the metafields it was given, while it is active;
- the open pages reload, with `--live-reload`.

| Syncs | Does not sync |
|---|---|
| Products: title, handle, description, vendor, type, tags, status and publication, options, variants (price, SKU, barcode, inventory policy, weight), stock per location, media from `files/`, collections and selling plan groups they are in | Orders and what belongs to them (`orderCreate`, draft orders, fulfillments, refunds, returns): the account pages show the orders of the data and of the checkout |
| Collections: title, handle, description, image, products in order | Discounts that are not app discounts (`DiscountCodeBasic`, …): the local checkout runs Functions only; of an app discount, `combinesWith`, usage limits and every code but the first |
| Customers: email, names, phone, tags, marketing consent, account state, addresses, their company | Gift cards the Admin API creates (changes to the data's cards sync) |
| Companies and their locations, pages, blogs and articles, menus, locations, metaobjects, selling plan groups | SEO, product categories, bundles, collection rules (a changed automated collection becomes a manual one) |
| Metafields of all of these, of the shop and of markets; markets and their countries | Metafield and metaobject definitions, files, translations, publications other than the Online Store |
| Shop: name, email, description, policies | Domains, currency, time zone, plan; turning a Function off (its settings go, it still runs) |

The full list, field by field, is in the design notes of `lsf-admin`
(`crates/admin/docs/design.md`, "Store sync").

- **Only the default shop syncs.** The shops of `POST /__lsf/admin/shops` exist to isolate tests:
  what they change stays in the Admin API.
- **The files do not change.** lsf builds the store from the files with the Admin API's changes on
  top; nothing is written to disk. `DELETE /__lsf/admin` (and the store data edited, with
  `--watch`, or `POST /__lsf/reload`) puts the storefront back to the files.
- **A session with its own data** (`PUT /__lsf/session` with `data`) sees its data on top of the
  Admin API's changes.
- **An order placed at the checkout** is recorded in the Admin API, and the store is not built
  again for it: orders do not sync (the account pages show it from the checkout, as they do
  without the Admin API). Its number is the session's: each session numbers its orders after
  the store data's, whatever the others do, so a test reads the same number on the thank-you
  page every run ([testing](testing.md)). The Admin API records it under that number when none
  of its orders has it, else under its next one, that of `orderCreate` and draft orders: no two
  orders share a name there. Two sessions, or a session after an `orderCreate`, can so show
  `#1004` on their thank-you pages while the Admin API names their orders `#1005` and `#1006`. A
  guest whose email is a customer's places the order as that customer, as on Shopify: the
  customer's `numberOfOrders` and `amountSpent` count it.
- **When the store data refuses a change** (an email it does not take, two variants with the
  same options, two companies of one name, which Shopify allows), the storefront shows the rest,
  the terminal says what was refused (once), and so does `GET /__lsf/admin`
  (`storeSync.built.diagnostics`).
- **What it costs**: a request of the backend only counts the change. The next request that
  reads the store builds it again, once for a burst of mutations: under 1 ms on the demo store,
  about 0.1 s for 3,000 products of 12 variants. It builds on the files as the server read them
  at start (or at the last reload), never on the files on disk: with `--static`, or without
  `--watch`, a data file edited since is not seen until a reload. Pages served without a change
  in between cost nothing more. `GET /__lsf/admin` gives `storeSync`: the generation of the changes, whether a
  change waits for the next page (`pending`), what the overlay writes and removes, and how long
  the last build took.

## What the answers say

- Errors are Shopify's, word for word where they are known: parse and validation errors with
  `locations` and `extensions.code`, `you must provide one of first or last`, `first cannot
  exceed 250. …`, `ACCESS_DENIED`, `THROTTLED`, `MAX_COST_EXCEEDED`. A mutation's validation errors
  are its `userErrors`, and a mutation that fails changes nothing.
- `extensions.cost`: `requestedQueryCost`, `actualQueryCost` and `throttleStatus`, as Shopify
  computes them, from a leaky bucket per shop and app (`--admin-throttle`).
- `extensions.search`: Shopify's warnings when a `query:` names a field the connection does not
  have ("Invalid search field for this query."); the field is then ignored, as on Shopify.
- Cursors are opaque, and resume as Shopify's do: right after their item in the connection's
  sort order, even when the item was deleted in between (delete a page of products while paging,
  and `after: endCursor` answers the next ones, not those a page further).
- `extensions.lsf`, only lsf's, only when there is something to say: `defaulted`, the paths of
  the fields answered with a default because the store data does not give them (and
  `defaultedFields`, as `Type.field`), and `notes`, what lsf did not simulate in the request (an
  input it ignored, a search filter it does not read).
- A money input of ten trillion or more in the currency's unit (`Money`, `MoneyInput.amount`, a
  discount's amount, …), or a line or an order whose total would be, answers a `userError`
  ("Price must be less than 10000000000000") and changes nothing. Shopify publishes no limit:
  this one is lsf's, so that no total overflows.
- A mutation lsf does not simulate answers an error with `extensions.code: LSF_NOT_SIMULATED`, never
  a silent success. `GET /__lsf/admin` lists those it simulates. The REST Admin API
  (`/admin/api/2026-10/products.json`) answers `404`: lsf serves the GraphQL Admin API only.

## What lsf simulates

Every query of 2026-10 is answered from the records of the shop: a field the store data does not
give is a default, reported in `extensions.lsf.defaulted`. The mutations, and the queries that
need more than reading records, are listed by family below.

### Products, collections and inventory

A backend that manages a catalog through the Admin API works against lsf as against a store:
what it changes is read back by the queries, delivered as webhooks, and shown by the storefront
and the checkout (see "Store sync" below).

| Area | Mutations lsf simulates |
|---|---|
| Products | `productCreate`, `productUpdate` (with `media` and `metafields`), `productDelete`, `productSet` (synchronous or not, upsert by `id`, `handle` or `customId`), `productDuplicate`, `productChangeStatus` |
| Variants | `productVariantsBulkCreate` (its `strategy` included), `productVariantsBulkUpdate` (`allowPartialUpdates` included), `productVariantsBulkDelete`, `productVariantsBulkReorder` |
| Options | `productOptionsCreate` (`LEAVE_AS_IS`, `CREATE`), `productOptionUpdate` (`LEAVE_AS_IS`, `MANAGE`), `productOptionsDelete` (`DEFAULT`, `NON_DESTRUCTIVE`, `POSITION`), `productOptionsReorder` |
| Media | `productCreateMedia`, `productUpdateMedia`, `productDeleteMedia`, `productReorderMedia`, `productVariantAppendMedia`, `productVariantDetachMedia` |
| Collections | `collectionCreate`, `collectionUpdate`, `collectionDelete`, `collectionAddProducts`, `collectionAddProductsV2`, `collectionRemoveProducts`, `collectionReorderProducts` |
| Inventory | `inventoryAdjustQuantities`, `inventorySetQuantities`, `inventoryMoveQuantities`, `inventorySetOnHandQuantities`, `inventoryActivate`, `inventoryDeactivate`, `inventoryBulkToggleActivation`, `inventoryItemUpdate` |
| Locations | `locationAdd`, `locationEdit`, `locationActivate`, `locationDeactivate`, `locationDelete`, `locationLocalPickupEnable`, `locationLocalPickupDisable` |
| Publications | `publishablePublish`, `publishableUnpublish` |
| Selling plans | `sellingPlanGroupCreate`, `sellingPlanGroupUpdate`, `sellingPlanGroupDelete`, `sellingPlanGroupAddProducts`, `sellingPlanGroupRemoveProducts`, `sellingPlanGroupAddProductVariants`, `sellingPlanGroupRemoveProductVariants`, `productJoinSellingPlanGroups`, `productLeaveSellingPlanGroups`, `productVariantJoinSellingPlanGroups`, `productVariantLeaveSellingPlanGroups` |
| Gift cards | `giftCardCreate`, `giftCardUpdate`, `giftCardCredit`, `giftCardDebit`, `giftCardDeactivate` |

How they behave, as shopify.dev documents it:

- **Validation** answers Shopify's `userErrors` (`field`, `message`, `code`) and changes nothing:
  "The variant 'Red / S' already exists.", "Option 'Color' already exists.", "Can't manually
  remove products from a smart collection", "The location cannot be deleted while it is
  active.", … Where Shopify documents an error by its code only, the message is the code's
  description in the 2026-10 schema.
- **Metafields** given to `productCreate`, `productUpdate`, `productSet`, the variant
  mutations, collections, locations and selling plans (and to customers, app discounts and the
  owners of Functions) are checked as `metafieldsSet` checks them
  (see "Metafields, metaobjects and content"): the type and the value, a definition's type and
  validations, `$app` and another app's namespaces. The errors are the mutation's, at the path of
  the input (`["metafields", "0", "value"]`; `productSet`'s with the code `INVALID_METAFIELD`).
- **Derived values follow**: price ranges, `totalInventory`, `hasOnlyDefaultVariant`,
  `featuredMedia`, a variant's `title`, `displayName`, `position` and `inventoryQuantity`, an
  option value's `hasVariants`, a variant's `availableForSale` after an inventory change.
- **Inventory**: `on_hand` stays the sum of `available`, `committed`, `reserved`, `damaged`,
  `safety_stock` and `quality_control`; adjusting `available` changes `on_hand` with it, moving
  between two of them does not. `changeFromQuantity` must be given (`null` skips the check) and a
  stale one is `CHANGE_FROM_QUANTITY_STALE`. Reasons and ledger documents are checked. The answer
  is an `InventoryAdjustmentGroup` (`reason: "Inventory correction"`, `quantityAfterChange:
  null`, as the Admin API answers right after the change).
- **`@idempotent`**: the inventory mutations, `locationActivate` and `locationDeactivate` require
  it, as Shopify does since 2026-04. The same key with the same arguments within 24 hours answers
  the first answer again without doing anything; with other arguments,
  `IDEMPOTENCY_KEY_PARAMETER_MISMATCH`.
- **Smart collections** (`ruleSet`) list the products their rules match (`TAG`, `TITLE`, `TYPE`,
  `VENDOR`, `VARIANT_TITLE`, `VARIANT_PRICE`, `VARIANT_COMPARE_AT_PRICE`, `VARIANT_WEIGHT`,
  `VARIANT_INVENTORY`, `IS_PRICE_REDUCED`; all of them, or any with `appliedDisjunctively`), and
  follow the products every mutation changes. Custom collections keep their manual order.
- **Publishing**: a product created through the Admin API is unpublished; `publishablePublish`
  on the Online Store sets its `publishedAt` and `onlineStoreUrl`, `publishableUnpublish` clears
  them.
- **Asynchronous work is done at once**: `productSet(synchronous: false)` answers a
  `ProductSetOperation` already `COMPLETE`, `productReorderMedia`, `collectionAddProductsV2`,
  `collectionRemoveProducts`, `collectionReorderProducts` a `Job` already `done`.
- **Media given by URL** is recorded, not downloaded: `image.url` is the URL, the size unknown,
  and the medium is `READY` at once (Shopify answers `UPLOADED` and processes it later).
- **Queries**: `products` and `productVariants` take their documented sort keys
  (`INVENTORY_TOTAL`, `FULL_TITLE`, `SKU`, …) and search filters (`sku`, `barcode`, `price`,
  `inventory_total`, `is_price_reduced`, `out_of_stock_somewhere`, `published_status`,
  `option1`, `location_id`, …); `productByIdentifier` and `productVariantByIdentifier` find a
  `customId`; `productTags`, `productTypes`, `productVendors`, `inCollection`,
  `InventoryItem.inventoryLevel(locationId:)`, `publishedOnPublication`,
  `resourcePublications`, the `appliesTo…` fields of selling plan groups are answered.

**Webhooks**: `products/create`, `products/update`, `products/delete`, `collections/*`,
`inventory_items/*`, `inventory_levels/update`, `…/connect`, `…/disconnect`, `locations/*`
(`activate` and `deactivate` included), `variants/in_stock` and `variants/out_of_stock` (a
variant whose stock crosses zero), `product_publications/create` and `…/delete` (the same for
`collection_publications`), `selling_plan_groups/*`. A variant, option or media change is a `products/update`; an inventory
change is an `inventory_levels/update` only.

**Store sync**: products (with their status and Online Store publication), variants (price,
SKU, barcode, stock), options, images of `files/`, collections and their products, locations,
selling plan groups and the gift cards of the store data are written back to the store the
storefront and the checkout see. Not synced: smart collection rules (a changed smart collection
becomes a manual one with the products it has), collection publications (an unpublished
collection stays on the storefront), gift cards the Admin API creates (the store data keys a
card by its code, which Shopify answers once), media from a URL outside `files/`, SEO, the
category.

**Not simulated yet** (they answer `LSF_NOT_SIMULATED`): the new collection model of 2026-10
(`collection.sources`, `sourcesToCreate`, `collectionConditionsSourceCreate`, `…Update`,
`…Delete`), `collectionDuplicate`, `collectionPublish`, `collectionUnpublish`, `productPublish`,
`productUnpublish`, `publishablePublishToCurrentChannel`, `publishableUnpublishToCurrentChannel`,
`publicationCreate`, `publicationUpdate`, `publicationDelete`, bundles and combined listings
(`productBundleCreate`, `productBundleUpdate`, `combinedListingUpdate`,
`productVariantRelationshipBulkUpdate`), `productFullSync`, `productFeedCreate`,
`productFeedDelete`, `bulkProductResourceFeedbackCreate`, quantity rules and price breaks
(`quantityRulesAdd`, `quantityRulesDelete`, `quantityPricingByVariantUpdate`), inventory
transfers and shipments (`inventoryTransfer…`, `inventoryShipment…`), fulfillment services
(`fulfillmentServiceCreate`, `…Update`, `…Delete`), `giftCardProductSet`,
`giftCardSendNotificationToCustomer`, `giftCardSendNotificationToRecipient`. Inputs a simulated
mutation ignores (`category`, `linkedMetafield`, `combinedListingRole`, a future `publishDate`,
…) are said in `extensions.lsf.notes`. The collections of the store data are custom collections
for the Admin API (lsf-core resolves their rules into products). `productTags`,
`productTypes` and `productVendors` page 250 at most, as every connection of lsf (Shopify allows
1000 to 5000).

### Orders, fulfillments and draft orders

An order placed in the local checkout is an order of the Admin API at once (`orders/create`,
`order_transactions/create` for its payment, `orders/paid`), and the backend processes it as on
Shopify: it fulfills, refunds, edits, cancels and returns it; the order's totals, statuses,
fulfillment orders, transactions and stock follow, and each change fires its webhooks (the
specific topic first, then `orders/updated`, which Shopify sends "in addition to the more
specific webhooks").

- **Queries**: `orders` and `ordersCount` with the filters of the reference (`financial_status`,
  `fulfillment_status` with `shipped`, `partial`, `unshipped`, `unfulfilled`, `on_hold`, `status`
  with `open`, `closed`, `cancelled`, `not_closed`, `name` with or without `#`, `email`,
  `customer_id`, `sku`, `tag`, `created_at`, `processed_at`, `updated_at`, `gateway`,
  `current_total_price`, `return_status`, `delivery_method`, `location_id`, `test`,
  `source_name`, …) and every `OrderSortKeys`; `fulfillmentOrders` (open ones unless
  `includeClosed`), `assignedFulfillmentOrders`, `manualHoldsFulfillmentOrders`,
  `FulfillmentOrder.supportedActions` and `locationsForMove`, `returnableFulfillments`,
  `Order.suggestedRefund`, `draftOrders`.
- **Orders**: `orderCreate` (imported orders: lines by variant or custom, customer, addresses,
  shipping lines, tax lines, a discount code, transactions, `financialStatus`,
  `fulfillmentStatus`, `inventoryBehaviour`), `orderUpdate`, `orderCancel` (cancelled at once,
  its `job` done and its `jobResult` readable with `node`; authorizations voided, payments
  refunded with `refundMethod`, units restocked with `restock`), `orderClose`, `orderOpen`,
  `orderMarkAsPaid`, `orderCapture`, `transactionVoid`, `orderCustomerSet`,
  `orderCustomerRemove`, `orderInvoiceSend` (nothing sent), `orderDelete` (a cancelled or closed
  order).
- **Fulfillment**: `fulfillmentCreate` (with `lineItemsByFulfillmentOrder`, partial or whole;
  fulfillment orders close or stay in progress, the order is fulfilled or partially fulfilled),
  `fulfillmentTrackingInfoUpdate`, `fulfillmentCancel`, `fulfillmentEventCreate`;
  `fulfillmentOrderHold` and `ReleaseHold` (several holds, a hold of some items splits the
  fulfillment order), `Move`, `Cancel`, `Open`, `Reschedule`.
- **Fulfillment services**: `fulfillmentServiceCreate`, `Update`, `Delete` (with their
  location), and the requests between merchant and service:
  `fulfillmentOrderSubmitFulfillmentRequest`, `AcceptFulfillmentRequest`,
  `RejectFulfillmentRequest`, `SubmitCancellationRequest`, `AcceptCancellationRequest`,
  `RejectCancellationRequest`, `fulfillmentOrderClose`. lsf does not call the service's
  `callbackUrl`: the service reads `assignedFulfillmentOrders`.
- **Refunds**: `refundCreate` (line items with `CANCEL`, `RETURN` or `NO_RESTOCK`, shipping,
  transactions against the order's payments; a refund without transactions moves no money, as
  on Shopify).
- **Returns**: `returnRequest`, `returnApproveRequest`, `returnDeclineRequest`, `returnCreate`,
  `returnProcess` (with its refund), `returnClose`, `returnReopen`, `returnCancel`.
- **Order edits**: `orderEditBegin`, `orderEditAddVariant`, `orderEditAddCustomItem`,
  `orderEditSetQuantity`, `orderEditAddLineItemDiscount`, `orderEditRemoveDiscount`,
  `orderEditCommit` (`orders/edited`, with the lines added and removed).
- **Draft orders**: `draftOrderCreate`, `draftOrderUpdate`, `draftOrderDelete`,
  `draftOrderCalculate`, `draftOrderComplete` (an order, paid unless `paymentPending`),
  `draftOrderDuplicate`, `draftOrderInvoiceSend` (recorded as sent, nothing sent). Taxes are at
  the store data's `checkout.tax_rate`.
- **Webhooks**: `orders/create`, `/updated`, `/paid`, `/cancelled`, `/fulfilled`,
  `/partially_fulfilled`, `/edited`, `/delete`; `order_transactions/create`;
  `fulfillments/create`, `/update`; `fulfillment_events/create`; `fulfillment_holds/added`,
  `/released`; `fulfillment_orders/*` (holds, moves, splits, cancellations, requests to a
  service); `refunds/create`; `returns/request`, `/approve`, `/decline`, `/close`, `/reopen`,
  `/cancel` (`returns/process` fires, but has no example payload to build it from: it is logged
  as not sent); `draft_orders/create`, `/update`, `/delete`.

What lsf chose where Shopify does not say, or where a local tool must differ:

- The local checkout keeps the stock of the store data: its orders commit nothing, so test
  purchases never sell a product out. Orders the backend makes do: `orderCreate` with a
  `DECREMENT_…` `inventoryBehaviour`, a completed draft order, units an edit adds. Shipping
  takes the units the order committed; a restock puts back what the order took. An order that
  took nothing (of the store data or the checkout) ships and restocks without moving the stock. A variant an order
  sells out is out of stock as after an inventory mutation: `availableForSale` is `false`,
  `variants/out_of_stock` is sent, and the storefront shows it sold out.
- `orderCreate`'s `discountCode` makes a `DiscountCodeApplication` (its `value` as the
  reference's examples answer it) and the allocations on the lines (an amount off the order is
  spread over them by their totals) or on the shipping line. Its `allocationMethod`,
  `targetSelection` and `targetType` are lsf's: Shopify's examples do not show them.
- `orderCancel` is synchronous (Shopify's runs as a job). `orderCreate` with a
  `financialStatus` and no `transactions` records the transaction the status implies (a sale of
  the total for `PAID`). `fulfillmentCancel` does not put the shipped units back in stock.
- Messages Shopify does not publish are lsf's, in Shopify's form; those it does publish (in the
  reference's examples, or quoted from real answers) are word for word. The sources are in the
  doc comments of `crates/admin/src/domains/orders/`.
- The `returns/*` payloads list no return line items (Shopify's examples have none, and lsf does
  not know their shape): read them through the API.

Not simulated yet (they answer `LSF_NOT_SIMULATED`): `orderCreateManualPayment`,
`orderCreateMandatePayment`, `orderRiskAssessmentCreate`, `orderAttributionDefinitionUpsert`,
`orderAttributionDefinitionDelete`, `orderEditAddShippingLine`, `orderEditUpdateShippingLine`,
`orderEditRemoveShippingLine`, `orderEditRemoveLineItemDiscount`, `orderEditUpdateDiscount`,
`requestedOrderEditCreate`, `requestedOrderEditDecline`, `requestedOrderEditResolve`,
`fulfillmentOrderSplit`, `fulfillmentOrderMerge`, `fulfillmentOrderReportProgress`,
`fulfillmentOrderLineItemsPreparedForPickup`, `fulfillmentOrdersReroute`,
`fulfillmentOrdersSetFulfillmentDeadline`, `fulfillmentConstraintRuleCreate`,
`fulfillmentConstraintRuleUpdate`, `fulfillmentConstraintRuleDelete`, `shippingLabelPurchase`,
`shippingPackageUpdate`, `shippingPackageMakeDefault`, `shippingPackageDelete`, `returnRefund`, `returnLineItemRemoveFromReturn`, `removeFromReturn`,
`reverseDeliveryCreateWithShipping`, `reverseDeliveryShippingUpdate`,
`reverseFulfillmentOrderDispose`, `draftOrderBulkAddTags`, `draftOrderBulkRemoveTags`,
`draftOrderBulkDelete`, `draftOrderCreateFromOrder`, `draftOrderInvoicePreview`,
`paymentTermsCreate`, `paymentTermsUpdate`, `paymentTermsDelete`, `paymentReminderSend`,
`paymentScheduleCapture`, `disputeEvidenceUpdate`. Not computed yet: `returnCalculate`,
`Return.suggestedFinancialOutcome`, `requestedOrderEditCalculate`,
`draftOrderAvailableDeliveryOptions`, exchanges and reverse fulfillment orders of returns,
duties, store credit refunds, B2B draft orders, payment terms.

### Customers and companies

The customers and companies of the store data, and those the backend creates. What the backend
changes reaches the storefront and the checkout: a customer it creates logs in at the local
checkout by email, with the addresses, consent, tags and metafields it gave, and buys for the
company locations its roles give it.

**Queries**: `customers` (sort keys `NAME`, `LOCATION`, `CREATED_AT`, `UPDATED_AT`, `ID`; the
search filters of 2026-10: `email`, `phone`, `first_name`, `last_name`, `state`, `tag`,
`tag_not`, `country`, `accepts_marketing`, `orders_count`, `total_spent`, `customer_date`,
`order_date`, `updated_at`, `id`, and a free term on the names, email, phone and tags),
`customersCount` (which documents `id` only: another filter is ignored, with Shopify's warning),
`customer(id:)`, `customerByIdentifier` (`id`, `emailAddress`, `phoneNumber`, `customId`),
`companies` (sort keys `NAME`, `ORDER_COUNT`, `TOTAL_SPENT`, `SINCE_DATE`, …), `companiesCount`,
`company`, `companyLocations`, `companyLocation`, `companyContact`, `companyContactRole`, and the
contacts, locations, roles and role assignments of a company.

**Mutations**, as Shopify validates them (`userErrors` with Shopify's messages where they are
published, Rails' wording otherwise), with their webhooks:

| Mutations | What lsf does | Webhooks |
|---|---|---|
| `customerCreate`, `customerUpdate`, `customerSet` | A name, an email or a phone is required; email and phone are unique ("Email has already been taken") and checked ("Email is invalid", "Phone is invalid"). A new customer is `DISABLED`, not subscribed, in the shop's language. `tags` replace the tags; `metafields` set the customer's, checked as `metafieldsSet` checks them; the deprecated `addresses` replace the address book. `customerSet` creates, updates by `id`, or upserts by `email`, `phone` or `customId`. | `customers/create` or `customers/update`, `customer.tags_added` / `customer.tags_removed` for the tags that changed, the consent topics for a consent that changed |
| `customerDelete` | Refused for a customer with orders. Its company contacts go with it. | `customers/delete` |
| `customerAddressCreate`, `customerAddressUpdate`, `customerAddressDelete`, `customerUpdateDefaultAddress` | The first address is the default one; `setAsDefault` changes it; the default address cannot be deleted. `countryCode` gives the country's name; a province keeps its code when its name is not given. | `customers/update` |
| `customerEmailMarketingConsentUpdate`, `customerSmsMarketingConsentUpdate` | `SUBSCRIBED`, `UNSUBSCRIBED` or `PENDING`, for a customer with an email (a phone); `consentUpdatedAt` is now unless given. | `customers/update`, then `customers_email_marketing_consent/update` or `customers_marketing_consent/update` |
| `customerSendAccountInviteEmail` | The customer is `INVITED`. The email is not sent: `extensions.lsf.notes` says so. | `customers/update` |
| `customerGenerateAccountActivationUrl` | `https://<shop>/account/activate/<id>/<token>`; refused for an enabled account. | none |
| `customerAddTaxExemptions`, `customerRemoveTaxExemptions`, `customerReplaceTaxExemptions` | | `customers/update` |
| `tagsAdd`, `tagsRemove` on a customer | The webhook names only the tags added (removed). | `customers/update`, `customer.tags_added` / `customer.tags_removed` |
| `companyCreate` | The company with Shopify's two roles ("Location admin", "Ordering only"), a location (named after the company when none is given), and the contact given, a new customer, main contact, "Ordering only" at the location. | `companies/create`, `company_locations/create`, `customers/create`, `company_contacts/create`, `company_contact_roles/assign` |
| `companyUpdate`, `companyDelete`, `companiesDelete` | Deleting a company deletes its locations, contacts and roles; its customers stay. | `companies/update`; `company_contacts/delete`, `company_locations/delete`, `companies/delete` |
| `companyLocationCreate`, `companyLocationUpdate`, `companyLocationDelete`, `companyLocationsDelete`, `companyLocationAssignAddress`, `companyAddressDelete`, `companyLocationTaxSettingsUpdate`, `companyLocationAssignTaxExemptions`, `companyLocationRevokeTaxExemptions` | `billingSameAsShipping` (or both address types) gives one address for both. | `company_locations/create`, `/update`, `/delete` |
| `companyContactCreate`, `companyContactUpdate`, `companyContactDelete`, `companyContactsDelete`, `companyContactRemoveFromCompany`, `companyAssignCustomerAsContact`, `companyAssignMainContact`, `companyRevokeMainContact` | A contact's id is its customer's (the checkout's `purchasingCompany.contact.id`). Deleting a contact keeps its customer. | `company_contacts/create`, `/update`, `/delete`; `companies/update` for the main contact |
| `companyContactAssignRole(s)`, `companyLocationAssignRoles`, `companyContactRevokeRole(s)`, `companyLocationRevokeRoles` | One role per contact and location; the role and the location must be the contact's company's. | `company_contact_roles/assign`, `/revoke` |

A company location's `ordersCount` and `totalSpent` (and its company's) count the orders placed
for it at the local checkout by a B2B buyer, on top of what the store data says, as a customer's
`numberOfOrders` and `amountSpent` count theirs; a cancelled order costs nothing.

Every id that names nothing answers a `userError` (`Customer does not exist`, `Resource requested
does not exist.`), and a mutation that fails changes nothing. The order of the webhooks of one
mutation is lsf's: Shopify does not document it.

Merges, data erasure, the WhatsApp consent, segments and the staff and tax registrations of a
company location are in "Segments, marketing, analytics and privacy" below.

**Not simulated yet** (they answer `LSF_NOT_SIMULATED`): the payment methods
(`customerPaymentMethodCreditCardCreate`, `…CreditCardUpdate`, `…GetUpdateUrl`,
`…PaypalBillingAgreementCreate`, `…PaypalBillingAgreementUpdate`, `…RemoteCreate`, `…Revoke`,
`…SendUpdateEmail`) and store credit (`storeCreditAccountCredit`, `storeCreditAccountDebit`).
The WhatsApp consent of `customerCreate` is ignored, with a note.

### Segments, marketing, analytics and privacy

A backend that segments customers, runs campaigns, reports on sales or handles privacy requests
works against lsf as against a store: the segment query language is parsed and evaluated over the
customers of the shop, the marketing activities and analytics the app records are read back,
what Shopify would send to the outside world is recorded in an outbox, never sent.

**Segments** (`segmentCreate`, `segmentUpdate`, `segmentDelete`; `segments`, `segment`,
`segmentsCount`, `customerSegmentMembers`, `customerSegmentMembership`,
`customerSegmentMembersQueryCreate` and `customerSegmentMembersQuery`, `segmentFilters`,
`segmentFilterSuggestions`, `segmentValueSuggestions`):

- The query language of shopify.dev's "Segment query language reference": clauses joined by
  `AND` (tighter) and `OR`, parentheses, `=`, `!=`, `>`, `>=`, `<`, `<=`, `BETWEEN … AND …`,
  `CONTAINS`, `NOT CONTAINS`, `IS NULL`, `IS NOT NULL`, `MATCHES (…)` and `NOT MATCHES (…)`;
  quoted strings, numbers, dates, date-times, offsets (`-7d`, `-2w`, `-1m`, `+1y`), `today`,
  `yesterday`, the deprecated `30_days_ago`; at most 10 clauses. Dates are whole days in the
  shop's time zone (`customerSegmentMembers(timezone:)` for that query, UTC by default).
- The filters: `amount_spent`, `number_of_orders`, `customer_added_date`, `first_order_date`,
  `last_order_date`, `abandoned_checkout_date` (the local checkout's), `customer_account_status`,
  `email_subscription_status`, `sms_subscription_status`, `customer_tags` (case ignored),
  `customer_email_domain`, `customer_language` (`en` matches `en-GB`), `customer_countries`,
  `customer_regions` (`US-NY`), `customer_cities` (`US-NY-NewYorkCity`), `companies`,
  `created_by_app_id` (the app, for the customers it created), `product_subscription_status`,
  `orders_placed MATCHES (app_id, location_id, count, amount, sum_amount, date)`,
  `products_purchased MATCHES (id, tag, quantity, sum_quantity, date)`,
  `store_credit_accounts MATCHES (balance, currency, next_expiry_date, last_credit_date)`,
  `customer_within_distance MATCHES (coordinates, distance_km | distance_mi)`, and
  `anniversary('metafields.<namespace>.<key>')`. What lsf has no data for matches no customer:
  `predicted_spend_tier` and `rfm_group` (Shopify's models: `IS NULL` holds), `shopify_email.*`
  (lsf sends no email), `storefront_event.*` (the storefront records no views).
- A bad query is a user error on `query`, in Shopify's form: `Query Line 1 Column 0:
  'total_spent' filter cannot be found.` (the form and `'MATCHES' is unexpected.` are Shopify's,
  quoted from real answers; the other messages are lsf's).
- Members: `customerSegmentMembers(segmentId: | query: | queryId:)` with `sortKey`
  (`created_at`, `first_order_date`, `last_abandoned_order_date`, `last_order_date`,
  `number_of_orders`, `amount_spent`), `reverse`, `totalCount` and
  `statistics.attributeStatistics` (`amount_spent`, `number_of_orders`). A member's id is
  `gid://shopify/CustomerSegmentMember/<customer id>`. `customerSegmentMembersQueryCreate` runs
  at once: the query is `done`, and `queryId` reads the members it found then.
- **Webhooks**: `segments/create`, `/update`, `/delete`; `customer.joined_segment` and
  `customer.left_segment` when a change of a customer (their record, an order, a company
  contact, a checkout) moves them into or out of a segment. Creating or editing a segment moves
  nobody, and a member whose dates move them out as time passes leaves at their next change.

**Saved searches** (`savedSearchCreate`, `savedSearchUpdate`, `savedSearchDelete`; the
`…SavedSearches` queries of products, collections, customers, orders, draft orders, files, URL
redirects, discounts and redeem codes; `segmentMigrations`): the query is split into
`searchTerms` and `filters` as Shopify's example shows; a connection given `savedSearchId`
(`orders`, `products`, `abandonedCheckouts`, …) lists what the saved search's query keeps.
Customer saved searches fire `customer_groups/create`, `/update`, `/delete`.

**Customers**:

| Mutation | What lsf does | Webhooks |
|---|---|---|
| `customerMerge` (with `customerMergePreview`, `customerMergeJobStatus`, `Customer.mergeable`) | Shopify's rules of the customer kept (the override's email, the only email, an enabled account, the consent, else `customerTwoId`); refused for a company contact, gift cards, a subscription, store credit, payment methods, a multipass identifier, a pending erasure, merged tags over 250 or a note over 5000 characters. The kept customer gets the other's orders, draft orders, addresses, metafields, tags and note (or the overrides); the job is done at once. | `customers/update`, `customers/delete`, `customers/merge` |
| `customerRequestDataErasure`, `customerCancelDataErasure` | The erasure waits 10 days, cancellable meanwhile; then (by the shop's clock: `POST /__lsf/admin/clock`) the names, email, phone, note and addresses are redacted, consents `REDACTED`, and `customers/redact` is sent to the app. Each step is in the outbox. | `customers/update`, `customers/redact` |
| `customerWhatsAppMarketingConsentUpdate` | As the SMS consent: a phone is required, `SUBSCRIBED`, `UNSUBSCRIBED` or `PENDING`; it shows in `defaultPhoneNumber.whatsAppMarketingConsent`. | `customers/update`, `customers_whats_app_marketing_consent/update` |
| `dataSaleOptOut` | The customer of the email opts out (`dataSaleOptOut: true`); an email no customer has gets a customer of its own *(chosen)*. | `customers/update` or `customers/create` |
| `companyLocationAssignStaffMembers`, `companyLocationRemoveStaffMembers` | The shop's owner is a `StaffMember` (`Shop.accountOwner`); assignments show in `CompanyLocation.staffMemberAssignments`. | none |
| `companyLocationCreateTaxRegistration`, `companyLocationRevokeTaxRegistration` (deprecated) | As `companyLocationTaxSettingsUpdate`: the location's `taxSettings.taxRegistrationId`. | `company_locations/update` |

`customers/purchasing_summary` follows every change of a customer's sales (an order created,
paid, cancelled, edited, refunded): sent when the app subscribes to it, its numbers those of the
customer after the change.

**Marketing** (`marketingActivityUpsertExternal`, `…CreateExternal` (deprecated),
`…UpdateExternal`, `…DeleteExternal`, `marketingActivitiesDeleteAllExternal`,
`marketingActivityCreate` and `marketingActivityUpdate` (the deprecated extension activities),
`marketingEngagementCreate`, `marketingEngagementsDelete`; `marketingActivities` with
`marketingActivityIds`, `remoteIds` and `utm`, `marketingActivity`, `marketingEvent`,
`marketingEvents`): each external activity has its `MarketingEvent` (remote id, URLs, UTM
parameters, dates); `remoteId` names it in every mutation; the errors are
`MarketingActivityUserErrorCode`'s (a UTM campaign or remote id taken, UTM parameters that cannot
change, an activity outside a hierarchy without UTM or URL parameter, …); engagements add up to
the activity's `adSpend`; deleting all is a `Job` done at once. Nothing is sent to the channels.

**Abandoned checkouts**: `abandonedCheckouts` (filters `status`, `recovery_state`,
`email_state`, `created_at`, `updated_at`; sort keys `CUSTOMER_NAME`, `TOTAL_PRICE`, …),
`abandonedCheckoutsCount`, `abandonment`, `abandonmentByAbandonedCheckoutId`,
`abandonmentUpdateActivitiesDeliveryStatuses` and the deprecated `abandonmentEmailStateUpdate`
(the email state an app's marketing activity reports). See "Webhooks" for how the storefront makes
them.

**Analytics**: `analyticsAnnotationCreate`, `…Update`, `…Delete` (the app's annotations, of the
types apps may create: `campaign`, `product_launch`, …; `Shop.analyticsAnnotations`);
`analyticsTargetCreate`, `…Update`, `analyticsTargetsDelete` (a metric of the `sales` schema,
unique by metric, dates and filters; `analyticsTargets`, `presentmentExpectedValue` in the shop's
currency). `shopifyqlQuery` answers `FROM sales` over the shop's orders and refunds: `SHOW`,
`WHERE`, `GROUP BY`, `TIMESERIES` (periods filled), `SINCE`/`UNTIL`/`DURING`, `ORDER BY`,
`LIMIT`, `WITH TOTALS`; the metrics `gross_sales`, `discounts`, `returns`, `net_sales`,
`shipping_charges`, `taxes`, `total_sales`, `orders`, `average_order_value`, `quantity_ordered`,
`net_items_sold`, `customers`, `new_customers`, `returning_customers`, … and the dimensions of
time, order, customer, product and address; a column it does not know is the reference's
`Column Not Found: Column 'total_sale' not found`. `VISUALIZE` and `COMPARE TO` are ignored, with
a note; another schema than `sales` is a parse error.

**Privacy**: `consentPolicyUpdate` (once `privacyFeaturesDisable([COOKIE_BANNER])` turned
Shopify's banner off, as Shopify requires), `consentPolicy`, `consentPolicyRegions`;
`privacyFeaturesDisable` and `privacySettings`.

**The outbox**: lsf sends no email, no WhatsApp message, and erases nothing outside the shop.
What Shopify would have sent is listed per shop in `GET /__lsf/admin` (`shops[].outbox`: `at`,
`channel`, `to`, `what`, `about`).

**Webhooks**: also `customer_account_settings/update` (built from the shop's customer account
setting, sent on demand: no mutation changes it) and `audit_events/admin_api_activity` (each
GraphQL request, when the app subscribes to it; Shopify delivers it through Pub/Sub or
EventBridge only, which lsf logs as not sent).

**Store sync**: none of these records reach the storefront or the checkout (segments, saved
searches, marketing, analytics, consent policies, abandoned checkouts, staff assignments). A
merge, an erasure and a data sale opt-out change customers, which sync as customers do.

### Metafields, metaobjects and content

**Metafields.** `metafieldsSet` and `metafieldsDelete` work on every owner the schema allows
(products, variants, collections, customers, companies and their locations, orders, draft
orders, pages, blogs, articles, locations, markets, discounts, the owners of Functions, the
shop, the app's installation).

- Every type of 2026-10 is checked as Shopify checks it (`number_integer` `"330\n"` is
  "Value must be an integer."), and kept as the Admin API returns it: JSON compact, `money` with
  a string amount in the shop's currency, measurements as `{"value", "unit"}`, references as
  GIDs of records that exist. `metafieldDefinitionTypes` lists the types with their categories and
  validations.
- 25 inputs at most, all or nothing: one error and nothing is set. Errors have Shopify's `field`
  (`["metafields", "0", "value"]`) and `code` (`INVALID_VALUE`, `INVALID_TYPE`, `BLANK`,
  `APP_NOT_AUTHORIZED`, `STALE_OBJECT`, …).
- `namespace` omitted, `$app` and `$app:x` are the app's reserved namespaces (`app--<id>`,
  `app--<id>--x`), in mutations and queries alike; another app's is refused.
- `compareDigest`: `Metafield.compareDigest` gives it; given, the value must not have changed;
  `null`, the metafield must not exist yet.
- With a definition of the owner type, namespace and key, `type` may be left out, must be the
  definition's when given, and the value must pass its validations (`min`, `max`, `regex`,
  `choices`, `list.min`, …). Setting a metafield does not create a definition, through
  `metafieldsSet` or the `metafields` input of a product, a customer or an app discount. The
  store data's metafields come with definitions lsf infers (one per owner type, namespace and
  key).
- The token needs the access of the owner (`write_products` for a product's).
- A product's metafields (or a variant's) are written to the store: the theme's Liquid and the
  Function inputs read them, as every metafield of an owner the store data has.

**Metafield definitions.** `metafieldDefinitionCreate`, `metafieldDefinitionUpdate`,
`metafieldDefinitionDelete` (`deleteAllAssociatedMetafields`, required for the app's
namespaces and for reference types with metafields), `metafieldDefinitionPin`,
`metafieldDefinitionUnpin`; `metafieldDefinitions(ownerType:, namespace:, key:, pinnedStatus:,
sortKey:)` and `metafieldDefinition(identifier:)`. A definition created where metafields exist
takes them and checks them (`validationStatus`).

**Metaobjects.** `metaobjectDefinitionCreate`, `metaobjectDefinitionUpdate` (field definitions
created, updated and deleted; the entries follow), `metaobjectDefinitionDelete` (with its
entries), `metaobjectCreate`, `metaobjectUpdate`, `metaobjectUpsert`, `metaobjectDelete`,
`metaobjectBulkDelete` (done at once: its `job` is `done`); `metaobjects(type:)` with `sortKey`
and `query` (`fields.<key>:value`), `metaobjectByHandle`, `metaobjectDefinitionByType`,
`Metaobject.field(key:)` and `values`.

- `$app:author` is the app's type (`app--<app id>--author` in the answers, as on Shopify). Its
  entries reach the storefront: the theme reads them as `metaobjects['app--<app id>--author']`
  (with its real app id, which lsf does not need to know) or, in the app's extension,
  `metaobjects['$app:author']`; a Function as `shop.metaobject(handle: {type: "$app:author", …})`.
  The store data writes the type `$app:author`.
- Field values are checked against the field definitions as
  metafield values are; required fields must have one; `fields` patches, `values` replaces.
- A handle left out is made from the display name (`Jane Austen` → `jane-austen`, then
  `jane-austen-1`).
- With the `publishable` capability, an entry is a `DRAFT` until it is `ACTIVE`; only active
  entries are on the storefront.

**Online store.** `pageCreate`, `pageUpdate`, `pageDelete`; `blogCreate`, `blogUpdate`,
`blogDelete`; `articleCreate` (in a blog by `blogId`, or a new one from `blog: {title}`),
`articleUpdate`, `articleDelete`; `commentApprove`, `commentSpam`, `commentNotSpam`,
`commentDelete`; `menuCreate`, `menuUpdate`, `menuDelete`; `urlRedirectCreate`,
`urlRedirectUpdate`, `urlRedirectDelete`. Handles are made from titles, `redirectNewHandle`
creates the redirect, `metafields` inputs are checked as `metafieldsSet` checks them. Published
pages and articles, menus, and active metaobjects are written to the store the theme renders.

**Files.** `fileCreate` (up to 250, from a URL or a staged upload), `fileUpdate` (alt, filename,
content, the products a file is a medium of), `fileDelete`; `files` searched by name and alt,
sorted by `FILENAME`. A file is `READY` as soon as it is created: lsf has nothing to process, so
a backend that polls `fileStatus` sees it at once. Its URL is the source's. From a staged upload
(`originalSource` the target's `resourceUrl`, once the file is uploaded), the file has the name
the target was made for, the MIME type it was uploaded with, and its size (`originalFileSize`,
`originalSource.fileSize`); its URL serves the bytes.

**Translations.** `translationsRegister` and `translationsRemove`; `translatableResource`,
`translatableResources(resourceType:)`, `translatableResourcesByIds`, and `translations(locale:)`
of the resources. lsf translates products, collections, pages, blogs, articles, metafields and
metaobject fields of translatable types, policies, product options and their values, and menus.
The digest is the SHA-256 of the content, as Shopify's; the locale must be one of the shop's
other than its primary one. A translation is `outdated` once its resource changed. Translations
are not shown on the storefront.

**Webhooks.** `metafield_definitions/create`, `/update` (also pin and unpin), `/delete`;
`metaobjects/create`, `/update`, `/delete`; `products/update` when a product's or a variant's
metafields change. Shopify has no topic for pages, blogs, articles, comments, menus, redirects,
files, or the metafields of other owners.

**Not simulated yet** (`LSF_NOT_SIMULATED`): `standardMetafieldDefinitionEnable`,
`standardMetaobjectDefinitionEnable`, `shopLocaleEnable`, `shopLocaleUpdate`, `shopLocaleDisable`, `urlRedirectBulkDeleteAll`,
`urlRedirectBulkDeleteByIds`, `urlRedirectBulkDeleteBySavedSearch`,
`urlRedirectBulkDeleteBySearch`, `urlRedirectImportCreate`, `urlRedirectImportSubmit`,
`fileAcknowledgeUpdateFailed`, the `theme*` mutations, and the `market*` mutations. Nor are
metafield definition `constraints`, the effects of `capabilities` and `access` (kept as given),
and staff authors of articles (`author.userId`).

### The app: billing, discounts, Functions and bulk operations

**Billing.** The billing helpers of the React Router template (`billing.require`,
`billing.request`, `billing.check`, `billing.cancel`, `createUsageRecord`,
`updateUsageCappedAmount` of `@shopify/shopify-api`) work unchanged, from a route the front end
fetches (from the loader of a page, see ["Billing from a page"](#react-router-app-shopifyshopify-app-react-router-and-shopifyshopify-api)):

- `appSubscriptionCreate` (recurring and usage lines, a discount, `trialDays`, `test`) and
  `appPurchaseOneTimeCreate` make a `PENDING` charge and answer a `confirmationUrl` on lsf:
  `http://127.0.0.1:9292/__lsf/admin/charges/<n>?shop=…`. The page shows the charge; **Approve**
  (or **Decline**) sends the merchant back to `returnUrl` with `charge_id=<n>`, as Shopify does.
  In a test, `POST /__lsf/admin/charges/<n>/approve` (or `/decline`) does it without the page.
- Approved, a subscription is `ACTIVE` and the app's other active subscription `CANCELLED`;
  `currentAppInstallation { activeSubscriptions oneTimePurchases allSubscriptions }` list them. A
  charge left pending two days (by the shop's clock) is `EXPIRED`. Uninstalling the app
  (`POST /__lsf/admin/uninstall`) cancels its subscription.
- `appUsageRecordCreate` charges a usage line of an active subscription, up to its capped amount
  (`"Failed to create usage charge"` past it); the same `idempotencyKey` gives back the same
  record. `appSubscriptionLineItemUpdate` asks for a new capped amount, which the merchant
  approves on the same page. `appSubscriptionTrialExtend` and `appSubscriptionCancel` too.
- Webhooks: `app_subscriptions/update` (a status or a capped amount changed),
  `app_purchases_one_time/update`, `app_subscriptions/approaching_capped_amount` (the usage
  balance crossed 90%).
- No money moves: `prorate` credits nothing, and there are no billing cycles. A replacement
  Shopify defers to the next cycle (`APPLY_ON_NEXT_BILLING_CYCLE`, and some `STANDARD` ones)
  applies at once, and the usage balance never resets. `currentPeriodEnd` is the end of the trial,
  else 30 days (365 for an annual plan) after the approval.

**Discounts.** `discountAutomaticAppCreate`/`Update` and `discountCodeAppCreate`/`Update` take the
Function by `functionHandle` (or the deprecated `functionId`), which must be a discount Function
of the app; their `metafields` are the Function's configuration, checked as `metafieldsSet`
checks them (a definition of the `DISCOUNT` owner type applies). The store sync writes an active
app discount back to the store data's `checkout.discounts` (its Function, title, first code,
classes and metafields), so the local checkout runs it. Shopify's own types (basic, buy X get Y,
free shipping, automatic and code) are kept as records: the local checkout runs Functions only.
Also: `discountAutomaticActivate`/`Deactivate`/`Delete` and the code ones (a discount's `status`
follows its dates), `discountAutomaticBulkDelete`, `discountCodeBulkActivate`/`Deactivate`/
`Delete` (done at once: their `job` is `done`), `discountRedeemCodeBulkAdd`,
`discountCodeRedeemCodeBulkDelete`; the queries `discountNodes` (ids `DiscountNode/<n>`),
`discountNodesCount`, `discountNode`, `codeDiscountNodeByCode`, `discountCodesCount`,
`automaticDiscountNodes`, `codeDiscountNodes`, with their sort keys and search filters (`status`,
`type`, `method`, `title`, `code`, `tag`, the dates, …). Webhooks: `discounts/create`,
`discounts/update`, `discounts/delete`, `discounts/redeemcode_added`,
`discounts/redeemcode_removed`.

**Functions.** `shopifyFunctions(apiType:)` lists the app's Functions. Payment and delivery
customizations (`…Create`, `…Update`, `…Delete`, `…Activation`), validations (`validationCreate`,
`Update`, `Delete`) and cart transforms (`cartTransformCreate`, `Delete`) take the Function by
handle or id, check it implements their API, and keep its configuration in their metafields,
which the store sync writes back as the Function's settings in `checkout.functions` (the
settings of a disabled one are dropped, but its Function still runs at the local checkout).

**Bulk operations.** `bulkOperationRunQuery` runs the query against the shop, every page of every
connection, and writes JSON Lines as Shopify does (one line per node, nested connections on lines
of their own with `__parentId`); Shopify's restrictions are checked (a connection, five at most,
two levels deep, no top-level `node`). `bulkOperationRunMutation` runs the mutation once per line
of a staged upload. Both finish within the request: the payload says `CREATED`, as Shopify's
does, the next query reads `COMPLETED` with its `url`, `objectCount` and `fileSize`, and
`bulk_operations/finish` is delivered. The file is at
`http://127.0.0.1:9292/__lsf/admin/bulk/<shop>/<n>.jsonl`. `bulkOperation`, `bulkOperations`
(filtered by `status`, `operation_type`, `created_at`), `currentBulkOperation`.

**Staged uploads.** `stagedUploadsCreate` answers targets on lsf, in the form of Shopify's Google
Cloud Storage ones: a `POST` target (`url` and form `parameters`, the `file` last) or a `PUT`
target (the default). The bytes stay in the shop until it is reset, for
`bulkOperationRunMutation` (`stagedUploadPath`) and `fileCreate` (`originalSource`, the
`resourceUrl`).

Not simulated yet (they answer `LSF_NOT_SIMULATED`): `appUninstall`, `appRevokeAccessScopes`,
`discountBulkTagsAdd`/`Remove`, `fulfillmentConstraintRuleCreate`/`Update`/`Delete`, the
deprecated `stagedUploadTargetGenerate(s)`. Saved searches (`savedSearchId`), credits,
revenue attribution records and frozen subscriptions have no data behind them.

### App proxy

With `--app` and an `[app_proxy]` in `shopify.app.toml`, the storefront forwards
`/<prefix>/<subpath>` and every path below it to the backend, signed as Shopify signs it, and
renders the answer in the theme when it is Liquid. A backend that serves storefront pages or JSON
through its app proxy (reviews, loyalty pages, wishlists) is tested on the local storefront.

```toml
[app_proxy]
url = "/proxy"        # or https://<the tunnel of shopify app dev>/proxy
prefix = "apps"       # a, apps, community or tools
subpath = "reviews"
```

```text
proxy:  http://127.0.0.1:9292/apps/reviews → http://localhost:3000/proxy
```

- **Where it goes**: a `url` that is a path joins `--app-url` (else `application_url`), as
  Shopify's CLI prepends the app URL; one on the origin of `application_url` moves to
  `--app-url`, as webhooks do; another is used as given. `/apps/reviews/product/1?page=2` goes to
  `http://localhost:3000/proxy/product/1?page=2&shop=my-shop.myshopify.com&logged_in_customer_id=&path_prefix=%2Fapps%2Freviews&timestamp=1793440800&signature=…`.
- **The query**: the browser's, then Shopify's: `shop`, `logged_in_customer_id` (the customer the
  session is logged in as, empty when logged out), `path_prefix`, `timestamp` (now: Shopify's
  libraries refuse one more than 90 seconds off) and `signature`, the hex HMAC-SHA256 of the
  others with the client secret.
- **The request**: its method and body, and the browser's headers without `Cookie` (Shopify strips
  it), with `X-Forwarded-For` and `X-Forwarded-Host`. A localized URL (`/fr/apps/reviews`) is
  proxied too, with `Accept-Language: fr;q=1.0, en;q=0.5`; `path_prefix` stays `/apps/reviews`.
- **The answer**: `Content-Type: application/liquid` is rendered as the template of a page, with
  the objects of the request (`shop`, `customer`, `cart`, `request.locale`, `settings`), inside
  `layout/theme.liquid`; `{% layout none %}` (what `liquid(body, {layout: false})` of the
  templates sends) renders it alone, and so does `lsf app dev` without a theme. Anything else is
  passed on with its status and its headers, redirects included, but those Shopify strips:
  `Set-Cookie`, `Server`, `X-Powered-By`, `Date` and the rest of shopify.dev's list.
- **When the backend does not answer**: nothing listening is a `502` page, no answer within 10
  seconds a `504` page, each saying what failed and to start the backend on `--app-url`.
  Shopify documents no timeout for app proxies; 10 seconds is what it documents for its APIs.

In a React Router app, `authenticate.public.appProxy` verifies the request and `liquid` answers
in the theme:

```ts
// app/routes/proxy.$.tsx
export const loader = async ({ request }: LoaderFunctionArgs) => {
  const { liquid } = await authenticate.public.appProxy(request);
  return liquid("<h1>Reviews</h1><p>{{ shop.name }}, {{ customer.first_name | default: 'guest' }}</p>");
};
```

Without a library, a handler verifies the signature as shopify.dev's "Authenticate app proxies"
does: every parameter but `signature`, the values of a repeated name joined with commas,
`name=value` sorted and concatenated.

```js
import crypto from "node:crypto";
import express from "express";

function verifyAppProxy(url, secret) {
  const params = new URL(url, "http://localhost").searchParams;
  const signature = params.get("signature") ?? "";
  const values = new Map();
  for (const [name, value] of params) {
    if (name !== "signature") {
      values.set(name, values.has(name) ? `${values.get(name)},${value}` : value);
    }
  }
  const message = [...values].map(([name, value]) => `${name}=${value}`).sort().join("");
  const expected = crypto.createHmac("sha256", secret).update(message).digest("hex");
  return (
    expected.length === signature.length &&
    crypto.timingSafeEqual(Buffer.from(expected), Buffer.from(signature)) &&
    Math.abs(Date.now() / 1000 - Number(params.get("timestamp"))) <= 90
  );
}

const app = express();
app.get("/proxy/*rest", (req, res) => {
  if (!verifyAppProxy(req.originalUrl, process.env.SHOPIFY_API_SECRET)) {
    return res.status(401).send("bad signature");
  }
  res.type("application/liquid").send("<h1>Reviews of {{ shop.name }}</h1>");
});
app.listen(3000);
```

Not simulated: the prefix and subpath a merchant can change in the admin (lsf serves those of
the file). Shopify does not document what `template` and `request.page_type` are on a proxied
page; lsf leaves them empty.

## Testing

- **Wait for a webhook**: `GET /__lsf/webhooks?topic=products/create&wait=1` answers once one is
  delivered (or `408` after `?timeout=`), with what was sent and what the backend answered.
  `DELETE /__lsf/webhooks` forgets them.
- **Start clean**: `DELETE /__lsf/admin` puts every shop back to the store data, forgets the
  subscriptions the backend made and the tokens OAuth issued; the printed token works again, and
  the storefront is the files' again.
- **Isolate parallel tests**: `POST /__lsf/admin/shops {"shop": "test-1"}` gives each test a shop
  of its own, seeded from the same data, and its token.
- **Fixed dates**: with `now` in the store data, every record created or changed is dated then,
  and the same run gives the same answers.
- **Throttling**: `--admin-throttle legacy` (1000 points, 50 a second) reaches `THROTTLED`
  sooner; `--admin-throttle off` never does.
- The store data edited while the server runs seeds the shops again: what the Admin API changed is
  dropped, and the server says so.
