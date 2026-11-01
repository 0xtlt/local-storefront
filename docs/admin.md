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

This guide starts lsf for a backend, points Shopify's libraries at it, and tests the backend
with it. What each mutation does, family by family, is in the [Admin API
reference](admin-reference.md).

## Start

```bash
npx local-storefront app dev --app ../my-app --app-url http://localhost:3000
```

```text
admin:  http://127.0.0.1:9292/admin/api/2026-10/graphql.json
        X-Shopify-Access-Token: shpat_4f…   (shop: local-supply-co.myshopify.com)
webhooks: 3 subscriptions of shopify.app.toml → http://localhost:3000
ready:  http://127.0.0.1:9292/   (extensions and checkout scenarios; …)
```

With `--app`, the client id, the scopes and the webhooks are those of the app's
`shopify.app.toml`, and the start lines of the Admin API (with one that says where the client
secret came from) come before `ready:`. The parts lsf reads:

```toml
client_id = "…"
application_url = "https://my-app.trycloudflare.com"   # the tunnel of shopify app dev: --app-url replaces it

[access_scopes]
scopes = "read_products,write_products,read_orders"     # the token has these, and no others

[webhooks]
api_version = "2026-10"

  [[webhooks.subscriptions]]
  topics = ["products/create", "orders/create"]
  uri = "/webhooks"                                      # a path: joined to --app-url

  [[webhooks.subscriptions]]
  topics = ["app/uninstalled"]
  uri = "/webhooks/app/uninstalled"
```

- **The shop** is the store data's: its `shop.permanent_domain`, `local-supply-co.myshopify.com`
  for the demo store (`defaultShop` of `GET /__lsf/admin`), seeded from the products,
  collections, customers, orders, metafields, metaobjects, markets, discounts and Functions of
  the data. The ids are those the storefront, the checkout and the Function inputs use.
- **The token** printed is an offline token the app is installed with, with the scopes of
  `[access_scopes] scopes`: no scope, no access. It is the same every run for a shop and an app:
  a test can hard-code it.
- **The client secret** signs webhooks, OAuth answers and session tokens. Give lsf the backend's:
  `--client-secret`, or `SHOPIFY_API_SECRET` in the environment or in the app's `.env` (where
  `shopify app env pull` writes it, and the app templates read it). Without one, lsf signs with
  `lsf-local-client-secret`.

| Option | Environment variable | What it does |
|---|---|---|
| `--app-url <url>` | `LSF_APP_URL` | Where the backend listens. Webhooks with a relative `uri` go there, and an absolute `uri` on the origin of `application_url` (often the tunnel of a `shopify app dev` that is not running) is moved there. Without it, `application_url`, where lsf sends only when it is on this machine ([where webhooks go](#webhooks)). |
| `--client-secret <secret>` | `LSF_CLIENT_SECRET` | The client secret. |
| `--admin-throttle <plan>` | `LSF_ADMIN_THROTTLE` | The cost bucket: `store` (default), `standard`, `advanced`, `plus`, `enterprise`, `legacy`, `off`. |
| `--admin-versions <list>` | `LSF_ADMIN_VERSIONS` | The [API versions](#api-versions) served: `2026-10` alone by default, `2026-10,2027-01` with the release candidate. |
| `--admin-fetch-local` | `LSF_ADMIN_FETCH_LOCAL` | Sources from a URL (`themeCreate`, `themeFilesUpsert`, `urlRedirectImportCreate`) may be on this machine or its network. Off by default. |
| `--admin-source <url>=<file>` | | Answers a URL with a file instead of downloading it ([stubbed URLs](admin-reference.md#stubbed-urls)). Repeatable. |

`app dev` serves the app alone: its checkout, its customer account, the Admin API and the app
proxy, but no theme, so no storefront pages (`/products/…` answers `404`). To see on the theme's
pages what the backend changes, serve the theme with the app:

```bash
npx local-storefront serve --theme ../my-theme --app ../my-app --app-url http://localhost:3000
```

`serve` without `--app` serves the Admin API too, with a local client id
(`lsf-local-client-id`), every scope and no subscription, and prints nothing about it:
`GET /__lsf/admin` has its token.

[`examples/admin-backend`](https://github.com/0xtlt/local-storefront/tree/main/examples/admin-backend)
is a backend written with Shopify's libraries (the React Router package and
`@shopify/shopify-api`), with a script that runs it against lsf end to end: the JavaScript
recipes of "[Point the backend at lsf](#point-the-backend-at-lsf)" are checked there.

## Point the backend at lsf

Shopify's libraries build the URL of a shop as `https://<shop>/admin/api/<version>/graphql.json`
(and `https://<shop>/admin/oauth/access_token` for tokens), and some force `https` on whatever
host they are given. lsf listens on `http://127.0.0.1:9292` and serves every shop there: the
token, not the host, picks the shop. So the way in is the library's HTTP layer: send the
requests for `*.myshopify.com` to lsf. Use API version `2026-10` (or a version you serve with
`--admin-versions`); lsf answers another with a `404` that says so, and one Shopify no longer
supports with 2026-10, as Shopify does ([API versions](#api-versions)).

The recipes read the address of lsf from an environment variable of your own (`LSF_URL` here),
so that the same code talks to Shopify when it is not set. Those that take an access token read
`SHOPIFY_ACCESS_TOKEN`: the token printed at start, also `shops[0].token` of `GET /__lsf/admin`
([tokens](#tokens)).

### JavaScript: the React Router app and `@shopify/shopify-api`

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

Run the backend with the app's values (what `shopify app dev` would give it), without the
Shopify CLI: the template's `npm run dev` is `shopify app dev`, which talks to Shopify. In the
React Router template, after `npm run setup` (its Prisma session storage):

```bash
SHOPIFY_API_KEY=<client_id of shopify.app.toml> SHOPIFY_API_SECRET=<its secret> \
SCOPES=read_products,write_products SHOPIFY_APP_URL=http://localhost:3000 PORT=3000 \
LSF_URL=http://127.0.0.1:9292 npx react-router dev
```

Then open the app as the admin would: `http://127.0.0.1:9292/__lsf/admin/launch?path=/app`
redirects to `http://localhost:3000/app` with `embedded=1`, `shop`, `host`, `hmac` and a session
token in `id_token`, valid for a minute: follow the redirect at once. The template's
`authenticate.admin` reads the session token, exchanges it for an access token at lsf, and the
loader's `admin.graphql(…)` queries lsf. The App Bridge script of the page needs the real admin
around it; what the server does is all there. A route the front end calls with `fetch` is
authenticated with `Authorization: Bearer <session token>`: mint one with
`POST /__lsf/admin/session-token`, one per request. And `authenticate.admin` answers `410 Gone`
to a client whose `User-Agent` looks like a bot (the `isbot` package): curl's and Node's
`fetch`'s do; send a browser's.

- **Billing from a page.** `billing.request` (and `billing.require` with it) in the loader of a
  page sends the browser to the template's exit-iframe page, which only goes on to the admin
  over `https` (`admin.shopify.com`, `<shop>.myshopify.com/admin`): it refuses lsf's charge page
  (`http://127.0.0.1:9292/__lsf/admin/charges/<n>`) with "Invalid exit-iframe destination". Call
  it from a route the front end fetches: the library then answers `401` with the charge page in
  `X-Shopify-API-Request-Failure-Reauthorize-Url`, which App Bridge (or the test) opens. In a
  test, approve it without the page ([approve a charge](#approve-a-charge)).
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

### JavaScript: `@shopify/admin-api-client`

`createAdminApiClient` takes a `customFetchApi` (`dist/graphql/client.mjs` of 2.0.1). It always
builds an `https://` URL from `storeDomain` (`validateDomainAndGetStoreUrl` of
`@shopify/graphql-client` sets the protocol to `https`), so rewrite the URL in the fetch:

```js
import { createAdminApiClient } from "@shopify/admin-api-client";

const lsf = process.env.LSF_URL; // http://127.0.0.1:9292
const client = createAdminApiClient({
  storeDomain: "local-supply-co.myshopify.com",
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
session = ShopifyAPI::Auth::Session.new(shop: "local-supply-co.myshopify.com", access_token: token)
client = ShopifyAPI::Clients::Graphql::Admin.new(session: session)
client.query(query: "{ shop { name } }").body
```

### Python: the `ShopifyAPI` package

The `ShopifyAPI` package (12.7.0) builds `https://<shop>.myshopify.com/admin/api/<version>`, but
its `GraphQL` resource reads its `endpoint` at each `execute` (`shopify/resources/graphql.py`):
point it at lsf.

```python
import os, shopify

session = shopify.Session("local-supply-co.myshopify.com", "2026-10", os.environ["SHOPIFY_ACCESS_TOKEN"])
shopify.ShopifyResource.activate_session(session)
graphql = shopify.GraphQL()
if os.environ.get("LSF_URL"):
    graphql.endpoint = os.environ["LSF_URL"] + "/admin/api/2026-10/graphql.json"
print(graphql.execute("{ shop { name } }"))
```

Without a library, a POST is all it takes (with `requests`):

```python
import os, requests

LSF_URL = os.environ.get("LSF_URL", "http://127.0.0.1:9292")
token = requests.get(LSF_URL + "/__lsf/admin").json()["shops"][0]["token"]  # the default shop's

answer = requests.post(
    LSF_URL + "/admin/api/2026-10/graphql.json",
    headers={"X-Shopify-Access-Token": token},
    json={"query": "query($n: Int!) { products(first: $n) { nodes { title } } }", "variables": {"n": 3}},
).json()
```

### Any HTTP client: `fetch` and `curl`

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
| A delegate access token | `delegateAccessTokenCreate`: `shppa_…`, a subset of the parent's scopes, expiring with it or sooner (see "[The app's installation](admin-reference.md#the-apps-installation-and-shopifys-other-services)"). |

Every access token is `shpat_` and 32 hex digits, as Shopify's. A token belongs to one shop. A
request without a valid token is answered `401`
`{"errors": "[API] Invalid API key or access token (unrecognized login or wrong password)"}`, with
an `X-Lsf-Reason` header saying why in plain words. A token or a code that expired, and a refresh
token that was retired, are refused as such for a day; then lsf forgets them, and refuses them as
tokens it never issued.

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
- **Where they go**: a `uri` that is a path joins `--app-url` (else `application_url`); an
  absolute one on the origin of `application_url` (the tunnel of a `shopify app dev` that is not
  running) moves to `--app-url`; another goes where it says. **Webhooks stay on this machine**:
  one goes to another host only when that host is the origin of `--app-url` or `--address`.
  Anything else (a production backend, a stale tunnel) is not sent, and `GET /__lsf/webhooks`
  logs it `not_sent` with the reason. So give `--app-url`: without it, the subscriptions of
  `shopify.app.toml` point at `application_url`, usually a tunnel, and lsf sends nothing there.
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
    move it with `POST /__lsf/admin/clock {"advance": "11m"}` ([the shop's
    clock](admin-reference.md#the-shops-clock)).
- **Payloads** are Shopify's JSON for the topic (the REST representation of the resource), built
  from the record as the change left it, cut to `include_fields`, and checked against `filter`.
  A topic whose payload lsf does not build is logged as not sent, never sent with a made-up body.
- **Headers**: `X-Shopify-Topic`, `X-Shopify-Hmac-Sha256` (base64 HMAC-SHA256 of the body with the
  client secret), `X-Shopify-Shop-Domain`, `X-Shopify-API-Version` (the version the payload is
  built in: the subscription's, `2026-10` by default),
  `X-Shopify-Webhook-Id`, `X-Shopify-Event-Id`, `X-Shopify-Triggered-At`, `User-Agent:
  Shopify-Captain-Hook`. Every library's verification accepts them with the shared secret
  (`authenticate.webhook` of the React Router package).
- **Delivery** is from a background worker, in order, retried after 1, 5 and 15 seconds when the
  backend does not answer `2xx` within 5 seconds.

Without a library, verify the HMAC on the raw bytes of the body, before parsing it: the JSON
parsed and serialized again is not what was signed. In Python:

```python
import base64, hashlib, hmac

def verify_webhook(raw_body: bytes, header: str, secret: str) -> bool:
    digest = hmac.new(secret.encode(), raw_body, hashlib.sha256).digest()
    return hmac.compare_digest(base64.b64encode(digest).decode(), header or "")

# Flask: verify_webhook(request.get_data(), request.headers.get("X-Shopify-Hmac-Sha256"), SECRET)
```

**The backend answers `401`** (`failed: answered 401` in lsf's log): the two secrets differ. lsf
signs with `--client-secret` (`LSF_CLIENT_SECRET`), else `SHOPIFY_API_SECRET` of the
environment, else the one of the app's `.env`, else `lsf-local-client-secret`; without `--app`, no
`.env` is read. The start banner says where its secret came from.

On demand: `POST /__lsf/webhooks/trigger {"topic": "orders/create", "id"?: "<gid>"}` sends the
webhook of a record of the shop (a representative one without `id`), and the command line does it
without a server:

```bash
npx local-storefront app webhook trigger --topic app/uninstalled --app ../my-app --app-url http://localhost:3000
```

The topic must be one `shopify.app.toml` subscribes to (or give `--address <url>`). Without
`--app-url`, a subscription that leads off this machine is not sent: the command exits `2` and
says to give it. See the [control API](control-api.md) and the [CLI](cli.md#app-webhook-trigger).

## Test your backend

A test of the backend runs lsf and the backend side by side. It gives itself a shop, calls the
backend as the app's front end does, places orders as a buyer, and waits for the webhooks lsf
delivers back. Tests that run in parallel each take a shop of their own.

### Start lsf and the backend

Fix both ports: the backend reads lsf's address when it starts (`LSF_URL`), and lsf delivers the
webhooks to the backend's (`--app-url`). Both read the client secret from `SHOPIFY_API_SECRET` of
the app's `.env`. With Playwright:

```ts
// playwright.config.ts
import { defineConfig } from "@playwright/test";

export default defineConfig({
  fullyParallel: true,
  webServer: [
    {
      command: "npx local-storefront app dev --app . --port 9292 --app-url http://localhost:3000 --static --quiet",
      url: "http://127.0.0.1:9292/__lsf/status",
      reuseExistingServer: !process.env.CI,
    },
    {
      // The backend, without the Shopify CLI (the React Router template: `npx react-router dev`).
      command: "npm run start",
      url: "http://localhost:3000/",
      env: { PORT: "3000", LSF_URL: "http://127.0.0.1:9292", SHOPIFY_APP_URL: "http://localhost:3000" },
      reuseExistingServer: !process.env.CI,
    },
  ],
});
```

The backend's `url` must answer without a session token: Playwright waits for a `2xx`, `3xx` or
`400` to `403`. Without Playwright, start both, then wait until `GET /__lsf/status` answers (or
`ready:` is printed on stderr) and the backend's port does.

### A shop per test

Each test makes a shop of its own (`POST /__lsf/admin/shops`), seeded from the same store data,
with the app installed and the subscriptions of `shopify.app.toml`. It calls the backend with a
session token of that shop, as App Bridge does, and a browser's `User-Agent`:

```js
// tests/lsf.mjs: plain fetch, for Playwright, node:test or Vitest alike.
export const LSF = "http://127.0.0.1:9292";
export const BACKEND = "http://localhost:3000";
// The React Router package answers 410 Gone to a bot's User-Agent, Node's included.
const BROWSER = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

export async function lsf(method, path, body) {
  const answer = await fetch(LSF + path, { method, body: body && JSON.stringify(body) });
  return answer.json();
}

// { shop: "<name>.myshopify.com", token, graphql }
export const newShop = (name) => lsf("POST", "/__lsf/admin/shops", { shop: name });

// A request of the app's front end, with a session token of the shop (valid for a minute).
export async function callBackend(shop, method, path, body) {
  const { authorization } = await lsf("POST", "/__lsf/admin/session-token", { shop });
  return fetch(BACKEND + path, {
    method,
    headers: { Authorization: authorization, "User-Agent": BROWSER, "Content-Type": "application/json" },
    body: body && JSON.stringify(body),
  });
}

// The deliveries that match the filters, once `wait` of them are done.
export const webhooks = (filters) => lsf("GET", `/__lsf/webhooks?${new URLSearchParams(filters)}`);
```

```js
// tests/products.spec.mjs: `/api/products` stands for a route of your backend.
import { test, expect } from "@playwright/test";
import { callBackend, lsf, newShop, webhooks } from "./lsf.mjs";

test("a product the backend creates comes back as a webhook", async () => {
  const { shop } = await newShop(`products-${crypto.randomUUID().slice(0, 8)}`);
  try {
    const answer = await callBackend(shop, "POST", "/api/products", { title: "Mug" });
    expect(answer.status).toBe(200);

    const { deliveries } = await webhooks({ topic: "products/create", shop, wait: 1 });
    expect(deliveries[0].state).toBe("delivered");
    expect(deliveries[0].attempts.at(-1).status).toBe(200); // what the backend answered
  } finally {
    await lsf("POST", "/__lsf/admin/uninstall", { shop }); // app/uninstalled: the backend drops its session
    await lsf("DELETE", `/__lsf/admin/shops/${shop}`);
  }
});
```

The default shop's token, for a test that calls the Admin API itself, is `shops[0].token` of
`GET /__lsf/admin`; a test shop's is in the answer of `POST /__lsf/admin/shops`. Only the
default shop [syncs to the storefront](#what-the-storefront-and-the-checkout-see): what a test
shop changes stays in the Admin API.

### Wait for a webhook

`GET /__lsf/webhooks?topic=products/create&shop=test-1&wait=1` answers once one delivery that
matches the filters is done: `delivered`, `failed` or `not_sent` (a `pending` one is still being
tried). The log keeps every delivery since the last `DELETE /__lsf/webhooks`, those of other
tests and of earlier runs included, so `wait=1` can answer with an old one at once. Filter it:

- `shop=`: a test shop, by its name (`test-1`) or its domain.
- `resource=<gid>`: the deliveries about one record (the order a test placed, the product it
  made).
- `since=<id>`: the deliveries after that one. Each answer gives the id to pass next time, in
  `last`. <!-- check: since-field --> For the second webhook of a topic, pass the `last` of the
  first wait.

A delivery the backend fails is retried after 1, 5 and 15 seconds, and stays `pending` until its
last attempt, about 21 seconds on. `wait` gives up after `timeout` (10 seconds by default) with a
`408` and the same body: to see a failure's last attempt, give `timeout=25000`. Then assert
`state`, and `attempts.at(-1).status` for what the backend answered.

### Place an order

The [local checkout](checkout.md) is the storefront's: its orders land in the default shop
(`defaultShop` of `GET /__lsf/admin`), whatever shops a test made, and their `orders/create`
goes to the backend. A test that places one names its own session (`x-lsf-session`) and waits
for the webhook about its order:

```js
const headers = { "x-lsf-session": `order-${crypto.randomUUID()}`, "Content-Type": "application/json" };
const buyer = (method, path, body) =>
  fetch(LSF + path, { method, headers, body: body && JSON.stringify(body), redirect: "manual" });

await buyer("PUT", "/__lsf/session", { cart: { items: [{ variant: "TEE-BLK-S", quantity: 1 }] } });
const checkout = (await buyer("GET", "/checkout")).headers.get("location"); // /checkouts/cn/<token>
await buyer("POST", `${checkout}/update`, {
  operations: [
    { type: "email", value: "buyer@example.com" },
    { type: "shippingAddress", address: { lastName: "Buyer", address1: "1 Main St", city: "New York", countryCode: "US", provinceCode: "NY", zip: "10001" } },
    { type: "paymentMethod", handle: "card" },
  ],
});
const { order } = await (await buyer("POST", `${checkout}/complete`)).json(); // { id, number, name }
const { deliveries } = await webhooks({ topic: "orders/create", resource: order.id, wait: 1 });
```

- `TEE-BLK-S` is a SKU of the demo store; use one of your data.
- `complete` needs an email, a payment method and, for a cart that ships, an address with a
  last name, a first line and a city. Otherwise it answers `422` with the state, whose `errors`
  say what is missing.
- `order.id` is the `admin_graphql_api_id` of the order's `orders/create`: match by it, never by
  the number. The thank-you page numbers each session's orders after the store data's (`#1004`
  in every session), while the Admin API names them in turn (`#1005`, `#1006`, …).
- A Function of the app that fails blocks `complete`. A test that only needs an order serves the
  Functions it needs and no other: `--extension <handle>`, repeated.
- The backend processes the order in the default shop: fulfilling it takes the scope
  `write_merchant_managed_fulfillment_orders` ([orders](admin-reference.md#orders-fulfillments-and-draft-orders)).
  An order in a test shop is one the test makes there with `orderCreate`.

### Approve a charge

The backend's billing request answers `401` with lsf's charge page in
`X-Shopify-API-Request-Failure-Reauthorize-Url`: `http://127.0.0.1:9292/__lsf/admin/charges/1?shop=test-1.myshopify.com`.
Charges count from 1 in each shop, so approve the one of that URL, its `?shop=` kept:

```js
const answer = await callBackend(shop, "POST", "/api/billing/request");
const charge = new URL(answer.headers.get("x-shopify-api-request-failure-reauthorize-url"));
await fetch(`${charge.origin}${charge.pathname}/approve${charge.search}`, { method: "POST" });
```

The subscription is then `ACTIVE`, and `app_subscriptions/update` is delivered if the app
subscribes to it. `/decline` declines it. Billing from the loader of a page does not work
([billing from a page](#javascript-the-react-router-app-and-shopifyshopify-api)).

### Reset between runs

```bash
curl -X DELETE http://127.0.0.1:9292/__lsf/admin      # every shop back to the data: records, clock, outbox, tokens
curl -X DELETE http://127.0.0.1:9292/__lsf/webhooks   # the delivery log
curl -X DELETE http://127.0.0.1:9292/__lsf/sessions   # carts, checkouts and the order numbers of sessions
```

`DELETE /__lsf/admin` keeps the test shops (each a copy of the seed, about 2 MB for the demo
store); `DELETE /__lsf/admin/shops/<shop>` deletes one. After the three, a run places the same
orders, with the same ids, as the run before: a test can hard-code them.

**A reset revokes the backend's tokens.** `DELETE /__lsf/admin`, and deleting a shop, revoke
every token of the app on the shop, those the backend got by token exchange too, and send no
`app/uninstalled`. The backend's session storage still holds the old token, which the Admin API
now answers `401` (its `X-Lsf-Reason` says why). Shopify's React Router package then deletes the
session, so its next request exchanges a new token: only one call fails. Code that keeps its
tokens itself fails until they are gone. So do one of these:

- empty the backend's session storage after a reset (the React Router template's: the `Session`
  table of its Prisma database);
- name the test shops anew in each run (`products-${crypto.randomUUID()}`), as above;
- end each test with `POST /__lsf/admin/uninstall {"shop"}`, then delete the shop: the backend's
  `app/uninstalled` handler deletes the sessions, as on Shopify.

### Dates, throttling and the outbox

- **Fixed dates**: with `now` in the store data, every record created or changed is dated then,
  and the same run gives the same answers; [`POST /__lsf/admin/clock`](admin-reference.md#the-shops-clock)
  moves it on (a checkout abandoned, a payment due, a charge expired).
- **Throttling**: `--admin-throttle legacy` (1000 points, 50 a second) reaches `THROTTLED`
  sooner; `--admin-throttle off` never does.
- **What the backend would have sent** (emails, labels, Flow triggers) is in the shop's
  [outbox](admin-reference.md#the-outbox), for a test to read.
- The store data edited while the server runs seeds the shops again: what the Admin API changed
  is dropped, and the server says so.

### In Python

The same calls with `requests`. The checkout belongs to the session's cookie: keep it in a
`requests.Session`.

```python
import os, requests

LSF_URL = os.environ.get("LSF_URL", "http://127.0.0.1:9292")

def place_order(sku="TEE-BLK-S", email="buyer@example.com"):
    buyer = requests.Session()
    buyer.put(LSF_URL + "/__lsf/session", json={"cart": {"items": [{"variant": sku, "quantity": 1}]}})
    checkout = LSF_URL + buyer.get(LSF_URL + "/checkout", allow_redirects=False).headers["location"]
    buyer.post(checkout + "/update", json={"operations": [
        {"type": "email", "value": email},
        {"type": "shippingAddress", "address": {"lastName": "Buyer", "address1": "1 Main St",
            "city": "New York", "countryCode": "US", "provinceCode": "NY", "zip": "10001"}},
        {"type": "paymentMethod", "handle": "card"},
    ]}).raise_for_status()
    answer = buyer.post(checkout + "/complete")
    answer.raise_for_status()
    return answer.json()["order"]  # id, number, name

def wait_for_webhooks(**filters):  # topic=, shop=, resource=, since=, wait=, timeout=
    answer = requests.get(LSF_URL + "/__lsf/webhooks", params=filters)
    answer.raise_for_status()  # 408: fewer are done than `wait` asks
    return answer.json()["deliveries"]

order = place_order()
[delivery] = wait_for_webhooks(topic="orders/create", resource=order["id"], wait=1)
assert delivery["state"] == "delivered"
```

## App proxy

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
  `http://localhost:3000/proxy/product/1?page=2&shop=local-supply-co.myshopify.com&logged_in_customer_id=&path_prefix=%2Fapps%2Freviews&timestamp=1793440800&signature=…`.
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

In Python, parse the query with `keep_blank_values=True`: `parse_qsl` drops empty values by
default, and `logged_in_customer_id=` is empty for every visitor who is not logged in, so the
signature would never match. Flask's `request.args.to_dict()` loses repeated names the same way:
read `request.query_string`.

```python
import hashlib, hmac, time
from urllib.parse import parse_qsl

def verify_app_proxy(query_string: str, secret: str) -> bool:
    values, signature = {}, ""
    for name, value in parse_qsl(query_string, keep_blank_values=True):
        if name == "signature":
            signature = value
        else:
            values[name] = f"{values[name]},{value}" if name in values else value
    message = "".join(sorted(f"{name}={value}" for name, value in values.items()))
    expected = hmac.new(secret.encode(), message.encode(), hashlib.sha256).hexdigest()
    fresh = abs(time.time() - int(values.get("timestamp", 0))) <= 90
    return hmac.compare_digest(expected, signature) and fresh
```

To answer in the theme, send the Liquid with its content type. With Flask:

```python
from flask import Flask, Response, request

app = Flask(__name__)

@app.route("/proxy", defaults={"rest": ""}, methods=["GET", "POST"])
@app.route("/proxy/<path:rest>", methods=["GET", "POST"])
def proxy(rest):
    if not verify_app_proxy(request.query_string.decode(), SECRET):
        return "bad signature", 401
    return Response("<h1>Reviews of {{ shop.name }}</h1>", content_type="application/liquid")
```

Not simulated: the prefix and subpath a merchant can change in the admin (lsf serves those of
the file). Shopify does not document what `template` and `request.page_type` are on a proxied
page; lsf leaves them empty.

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
- the shipping rates of the general delivery profile it changes are the checkout's delivery
  options (see [delivery](admin-reference.md#inventory-transfers-fulfillment-and-delivery));
- a theme file it writes to the main theme (`themeFilesUpsert`, `themeFilesCopy`,
  `themeFilesDelete`) is served on top of the theme directory, which is never written to; a theme
  it publishes is served instead of the directory;
- a script tag it creates (`scriptTagCreate`, `ONLINE_STORE`) is loaded by `content_for_header`;
- a mobile app it declares (`mobilePlatformApplicationCreate`) is in
  `/.well-known/assetlinks.json` or `/.well-known/apple-app-site-association`;
- the open pages reload, with `--live-reload`.

| Syncs | Does not sync |
|---|---|
| Products: title, handle, description, vendor, type, tags, status and publication, options, variants (price, SKU, barcode, inventory policy, weight), stock per location, media from `files/`, collections and selling plan groups they are in | Orders and what belongs to them (`orderCreate`, draft orders, fulfillments, refunds, returns): the account pages show the orders of the data and of the checkout |
| Collections: title, handle, description, image, products in order | Discounts that are not app discounts (`DiscountCodeBasic`, …): the local checkout runs Functions only; of an app discount, `combinesWith`, usage limits and every code but the first |
| Customers: email, names, phone, tags, marketing consent, account state, addresses, their company | Gift cards the Admin API creates (changes to the data's cards sync) |
| Companies and their locations, pages, blogs and articles, menus, locations, metaobjects, selling plan groups | SEO, product categories, bundle components and combined listings (a bundle is a product of its own there), collection rules and sources (a changed automated collection becomes a manual one), product feeds, point of sale |
| Metafields of all of these, of the shop and of markets; markets, their countries and their currency; the published languages | Metafield and metaobject definitions, files, translations, publications other than the Online Store; catalogs, price lists, quantity rules, web presences, channels |
| Shop: name, email, description, policies | Domains, currency, time zone, plan; turning a Function off (its settings go, it still runs) |

Each family of the [reference](admin-reference.md#what-lsf-simulates) says what of it syncs.
For contributors, the full list, field by field, is in `crates/admin/docs/design.md` ("Store
sync").

- **Only the default shop syncs.** The shops of `POST /__lsf/admin/shops` exist to isolate tests:
  what they change stays in the Admin API.
- **The files do not change.** lsf builds the store from the files with the Admin API's changes on
  top; nothing is written to disk. `DELETE /__lsf/admin` (and the store data edited, with
  `--watch`, or `POST /__lsf/reload`) puts the storefront back to the files, and the theme back
  to the directory, without the files and script tags the Admin API added.
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
  reads `MERCHANT_READ` in 2027-01, and a collection source condition made with
  `metafieldInteger` (removed in 2027-01) reads as a
  `CollectionSourceInclusionConditionMetafieldInt`, its `value` a string.
- `GET /__lsf/admin` lists the versions lsf knows, their status and whether they are served
  (`apiVersions`).

For contributors: adding another version is a data drop and a line of code,
`mise run admin:version -- <version>` writes its files from Shopify's introspection and says what
is left (`crates/admin/docs/design.md`, "Add a version").

## Reference

The [Admin API reference](admin-reference.md) has the rest:

- [what the answers say](admin-reference.md#what-the-answers-say): errors, `extensions.cost`,
  `extensions.lsf`;
- [what lsf simulates](admin-reference.md#what-lsf-simulates), family by family: products,
  orders and fulfillments, delivery, customers, subscriptions, metafields, markets, point of
  sale, billing, discounts and bulk operations, themes, the app's installation;
- [the shop's clock](admin-reference.md#the-shops-clock) and [the
  outbox](admin-reference.md#the-outbox).
