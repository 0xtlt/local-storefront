# Admin API

The backend of a Shopify app talks to Shopify through the **GraphQL Admin API** and receives
**webhooks**. While it runs, local-storefront serves a local Admin API (version **2026-10**)
over the store data, with OAuth, token exchange and session tokens, and delivers the webhooks of
the app to its backend, signed as Shopify signs them. The backend runs and is tested offline: no
development store, no tunnel, the same data every run, and the failures that are hard to provoke
on a real store (a missing scope, throttling, an uninstall) on demand.

A product the backend creates through the Admin API is a product of the local shop; an order
placed in the [local checkout](checkout.md) is an order the Admin API lists, and its
`orders/create` webhook reaches the backend.

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

## Point the backend at lsf

Shopify's libraries build the URL of a shop as `https://<shop>/admin/api/<version>/graphql.json`
(and `https://<shop>/admin/oauth/access_token` for tokens), and some force `https` on whatever
host they are given. lsf listens on `http://127.0.0.1:9292` and serves every shop there: the
token, not the host, picks the shop. So the way in is the library's HTTP layer: send the
requests for `*.myshopify.com` to lsf. Use API version `2026-10`; lsf answers another with a
`404` that says so.

The recipes read the address of lsf from an environment variable of your own (`LSF_URL` here),
so that the same code talks to Shopify when it is not set.

### React Router app (`@shopify/shopify-app-react-router`) and `@shopify/shopify-api`

Every request of `@shopify/shopify-api` (the Admin API clients, token exchange, OAuth) goes
through its runtime's fetch function, which `setAbstractFetchFunc` of
`@shopify/shopify-api/runtime` replaces (`lib/clients/admin/graphql/client.ts`,
`lib/utils/fetch-request.ts` of 15.0.1). The node adapter sets it to `globalThis.fetch` when it is
imported, so replace it after that import, in `app/shopify.server.ts`:

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
`POST /__lsf/admin/session-token`.

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
(the same token now reads less, and `app/scopes_update` is delivered).

**Uninstall.** `POST /__lsf/admin/uninstall` revokes the app's tokens on the shop and delivers
`app/uninstalled`; `POST /__lsf/admin/install` installs it again.

## Webhooks

- **Subscriptions**: those of `shopify.app.toml` (`[[webhooks.subscriptions]]`, every shop), and
  those the backend makes with `webhookSubscriptionCreate` (per shop; listed by
  `webhookSubscriptions`, which, as on Shopify, does not list those of the file). A subscription
  made through the Admin API takes `https://` and, locally, `http://` to this machine.
- **Events**: what the Admin API changes (`productCreate` → `products/create`,
  `inventory_items/create`, `inventory_levels/connect`; `productUpdate` and `tagsAdd` →
  `products/update`; `productDelete` → `products/delete`), an order placed at the checkout
  (`orders/create`, then `orders/paid` when it is paid in full), an uninstall, a change of scopes.
- **Payloads** are Shopify's JSON for the topic (the REST representation of the resource), built
  from the record as the change left it, cut to `include_fields`, and checked against `filter`.
  A topic whose payload lsf does not build is logged as not sent, never sent with a made-up body.
- **Headers**: `X-Shopify-Topic`, `X-Shopify-Hmac-Sha256` (base64 HMAC-SHA256 of the body with the
  client secret), `X-Shopify-Shop-Domain`, `X-Shopify-API-Version: 2026-10`,
  `X-Shopify-Webhook-Id`, `X-Shopify-Event-Id`, `X-Shopify-Triggered-At`, `User-Agent:
  Shopify-Captain-Hook`. Every library's verification accepts them with the shared secret.
- **Delivery** is from a background worker, in order, retried after 1, 5 and 15 seconds when the
  backend does not answer `2xx` within 5 seconds.

On demand: `POST /__lsf/webhooks/trigger {"topic": "orders/create", "id"?: "<gid>"}` sends the
webhook of a record of the shop (a representative one without `id`), and
`npx local-storefront app webhook trigger --topic orders/create` does it from the command line,
without a server. See the [control API](control-api.md) and the [CLI](cli.md#app-webhook-trigger).

## What the answers say

- Errors are Shopify's, word for word where they are known: parse and validation errors with
  `locations` and `extensions.code`, `you must provide one of first or last`, `first cannot
  exceed 250. …`, `ACCESS_DENIED`, `THROTTLED`, `MAX_COST_EXCEEDED`. A mutation's validation errors
  are its `userErrors`, and a mutation that fails changes nothing.
- `extensions.cost`: `requestedQueryCost`, `actualQueryCost` and `throttleStatus`, as Shopify
  computes them, from a leaky bucket per shop and app (`--admin-throttle`).
- `extensions.search`: Shopify's warnings when a `query:` names a field the connection does not
  have ("Invalid search field for this query."); the field is then ignored, as on Shopify.
- `extensions.lsf`, only lsf's, only when there is something to say: `defaulted`, the paths of
  the fields answered with a default because the store data does not give them (and
  `defaultedFields`, as `Type.field`), and `notes`, what lsf did not simulate in the request (an
  input it ignored, a search filter it does not read).
- A mutation lsf does not simulate answers an error with `extensions.code: LSF_NOT_SIMULATED`, never
  a silent success. `GET /__lsf/admin` lists those it simulates. The REST Admin API
  (`/admin/api/2026-10/products.json`) answers `404`: lsf serves the GraphQL Admin API only.

## What lsf simulates

Every query of 2026-10 is answered from the records of the shop: a field the store data does not
give is a default, reported in `extensions.lsf.defaulted`. The mutations, and the queries that
need more than reading records, are listed by family below.

### Products, collections and inventory

### Orders, fulfillments and draft orders

### Customers and companies

### Metafields, metaobjects and content

### The app: billing, discounts, Functions and bulk operations

### App proxy

## Testing

- **Wait for a webhook**: `GET /__lsf/webhooks?topic=products/create&wait=1` answers once one is
  delivered (or `408` after `?timeout=`), with what was sent and what the backend answered.
  `DELETE /__lsf/webhooks` forgets them.
- **Start clean**: `DELETE /__lsf/admin` puts every shop back to the store data, forgets the
  subscriptions the backend made and the tokens OAuth issued; the printed token works again.
- **Isolate parallel tests**: `POST /__lsf/admin/shops {"shop": "test-1"}` gives each test a shop
  of its own, seeded from the same data, and its token.
- **Fixed dates**: with `now` in the store data, every record created or changed is dated then,
  and the same run gives the same answers.
- **Throttling**: `--admin-throttle legacy` (1000 points, 50 a second) reaches `THROTTLED`
  sooner; `--admin-throttle off` never does.
- The store data edited while the server runs seeds the shops again: what the Admin API changed is
  dropped, and the server says so.
