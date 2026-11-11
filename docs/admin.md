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

The embedded app's own pages open in [lsf's admin](#preview-the-embedded-app): an admin around
the app, with a local App Bridge, so that its session tokens, title bar, navigation, save bar,
toasts and resource picker work offline too.

This guide starts lsf for a backend, points Shopify's libraries at it, previews the embedded app,
and tests the backend with it. What each mutation does, family by family, is in the [Admin API
reference](admin-reference.md).

## Start

```bash
npx local-storefront app dev --app ../my-app --app-url http://localhost:3000
```

```text
serving: checkout, admin
admin:  http://127.0.0.1:9292/admin/api/2026-10/graphql.json
        X-Shopify-Access-Token: shpat_4f…   (shop: local-supply-co.myshopify.com)
        client secret: from SHOPIFY_API_SECRET of ../my-app/.env
        GraphiQL: http://127.0.0.1:9292/__lsf/graphiql   (queries in the browser, the token filled in)
webhooks: 3 subscriptions of shopify.app.toml → http://localhost:3000
embedded: http://127.0.0.1:9292/admin/apps/my-app   (the app in lsf's admin; its pages load http://127.0.0.1:9292/__lsf/admin/app-bridge.js)
ready:  http://127.0.0.1:9292/   (extensions and checkout scenarios; …)
```

With `--app`, the client id, the scopes and the webhooks are those of the app's
`shopify.app.toml`, and the start lines of the Admin API come before `ready:`. Without
`--app-url`, the `webhooks:` line says when they lead off this machine:
`→ https://my-app.trycloudflare.com, not this machine: not sent (give --app-url http://localhost:<port>)`.
The parts lsf reads:

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
- **The client id** is `client_id` of `shopify.app.toml`. The app template ships
  `client_id = ""`: lsf then takes `SHOPIFY_API_KEY` of the environment or of the app's `.env`
  (the backend's apiKey, which the session tokens' `aud` must be), and says so in a
  `client id:` line. With neither, it uses `lsf-local-client-id`, and warns at start, on the
  home page and in lsf's admin. The admin names the app by `handle`, else by `name` in lower
  case with dashes (`Gift Notes` is `/admin/apps/gift-notes`), else by the client id.
- **The client secret** signs webhooks, OAuth answers and session tokens. Give lsf the backend's:
  `--client-secret`, or `SHOPIFY_API_SECRET` in the environment or in the app's `.env` (where
  `shopify app env pull` writes it, and the app templates read it). Without one, lsf signs with
  `lsf-local-client-secret`. The `client secret:` line says which: `from --client-secret`,
  `from SHOPIFY_API_SECRET of the environment`, `from SHOPIFY_API_SECRET of <app>/.env`, or
  `lsf-local-client-secret   (lsf's own: …)`.

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

The Admin API is the admin part of lsf, and needs `--app`: `serve` without `--app` serves the
theme alone, and its `/admin/…` and `/__lsf/admin` answer `404` with an `X-LSF-Reason` that says
so. A backend whose tests need no theme and no checkout runs the Admin API alone:

```bash
npx local-storefront serve --app ../my-app --only admin --app-url http://localhost:3000
```

`--only checkout` runs the checkout without the Admin API, and `--only theme,admin` the theme
and the Admin API without the checkout ([what runs](cli.md#what-runs)). An option of the Admin
API (`--app-url`, `--pricing-plans`, …) with an `--only` that leaves it out stops the start:
`error: --app-url needs the admin part, which --only leaves out`.

[`examples/admin-backend`](https://github.com/0xtlt/local-storefront/tree/main/examples/admin-backend)
is a backend written with Shopify's libraries (the React Router package and
`@shopify/shopify-api`), with a script that runs it against lsf end to end
(`mise run admin:e2e` from the root of the repository): the JavaScript recipes of "[Point the
backend at lsf](#point-the-backend-at-lsf)" are checked there.

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

### JavaScript: `@shopify/admin-api-client`

A backend that only calls the Admin API (a sync job, a worker, a script) needs this recipe
alone: the token is the one printed at start, and there is no OAuth to set up.

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
loader's `admin.graphql(…)` queries lsf. To see the page as the merchant does, with its App
Bridge working (the session token of `fetch()`, the title bar, the navigation), open it in
[lsf's admin](#preview-the-embedded-app). A test calls a route the front end fetches with
`Authorization: Bearer <session token>`: mint one with `POST /__lsf/admin/session-token`, one
per request. And `authenticate.admin` answers `410 Gone`
to a client whose `User-Agent` looks like a bot (the `isbot` package): curl's, Node's `fetch`'s
and Playwright's headless Chromium (`HeadlessChrome`) do; send a browser's. In lsf's frame, the
frame's proxy says Chrome for headless Chromium.

lsf reads the template when it starts: an `app/shopify.server.ts` without `setAbstractFetchFunc`,
or an `app/routes/app.tsx` without `boundary.headers`, is a warning on the home page and in lsf's
admin, which says what to add. Three things of the template to know:

- **Export the route's `headers`.** `export const headers: HeadersFunction = (headersArgs) =>
  boundary.headers(headersArgs);` on a route (the template has it on `app.tsx`) passes the
  library's headers on. Without it, a `401` of `billing.request` loses
  `X-Shopify-API-Request-Failure-Reauthorize-Url`, and App Bridge has nowhere to send the
  merchant: the Upgrade button does nothing.
- **Another port.** `vite.config.ts` reads `PORT` before React Router loads `.env`, and its HMR
  port is `64999`: give both on the command line, `PORT=3001 SHOPIFY_APP_URL=http://localhost:3001
  npx react-router dev`, and change `64999` (to `Number(process.env.HMR_PORT || 64999)`) to run
  two apps at once.
- **Versions.** The template ships `@shopify/shopify-app-react-router` 3.x (`^3.0.2`), whose
  `AppProvider` loads Polaris 1 (`polaris.js`); 4.0.0 loads `polaris-2.js`. lsf serves both, and
  nothing else here differs between them.

- **Billing.** `billing.require` and `billing.request` work in the loader of a page (the
  template's gate in `app.tsx`) and in a route the front end fetches. The `confirmationUrl` lsf
  answers is Shopify's, on the shop's admin
  (`https://<shop>/admin/charges/<app id>/<n>/RecurringApplicationCharge/confirm_recurring_application_charge?signature=…`),
  which the template's exit-iframe page accepts; lsf's admin serves that path and shows lsf's
  charge page. A fetched route answers `401` with it in
  `X-Shopify-API-Request-Failure-Reauthorize-Url`, which App Bridge opens. After the approval,
  a `returnUrl` on Shopify's admin (the library's default) or on the app comes back to the app
  in lsf's admin. In a test, approve it without the page ([approve a charge](#approve-a-charge)).
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

### GraphiQL in the browser

`http://127.0.0.1:9292/__lsf/graphiql` is a query editor for the local Admin API: the shop, its
token and the API version are filled in from `GET /__lsf/admin` (pick another shop or a served
version), **Docs** browses the schema of the version, **Cost debug** sends
`Shopify-GraphQL-Cost-Debug: 1` (`extensions.cost.fields`), and Ctrl+Enter (⌘+Enter) runs the
query. It is lsf's own page, not Shopify's GraphiQL app: it loads nothing from the internet. The
home page of the app links to it (**More**), and a browser that opens the GraphQL endpoint
itself gets the `401` Shopify answers without a token, with an `X-LSF-Reason` that points there.

### Types from the schema (GraphQL Codegen)

`@graphql-codegen/cli` reads the schema from lsf's introspection, as from Shopify's: point it at
`http://127.0.0.1:9292/admin/api/2026-10/graphql.json` with the `X-Shopify-Access-Token`
header. Use `graphql@16`, as `@shopify/api-codegen-preset` does: `graphql@17` refuses Shopify's
2026-10 schema itself ("Interface field HasMetafields.metafield is not deprecated, so
implementation field MediaImage.metafield must not be deprecated", and the same of
`ChannelDefinition.id`, which `Node.id` is not). Those deprecations are Shopify's: lsf's schema
is the introspection of Shopify's Admin API, unchanged, and the same codegen fails against a
development store.

```bash
npm install --save-dev graphql@16
```

## Preview the embedded app

`http://127.0.0.1:9292/admin/apps/<handle>` is lsf's admin with the app open: the shop's name and
the app's navigation on the left, the app's title bar, and the app in a frame, at the app's URL
(`--app-url` on the path of `application_url`) with the query the admin signs. Each load is a
new launch: a new `hmac`, a new session token in `id_token`. The page's path follows the app's:
`/admin/apps/my-app/app/settings` opens the app at `/app/settings`, and reloading it opens the
app there again. The `serve` and `app dev` banners print its URL (`embedded:`), and the home
page of the app has an **Admin** button to it.

Under **Preview**, the navigation links to what the server runs, each in a new tab: **Online
Store** (`/`) with the theme, and **Checkout** (`/__lsf/checkouts`, the scenarios and the
extensions of the app) with the checkout. So a change the app makes, such as a payment
customization created, is one click from the checkout that shows it. They are the default
shop's: the admin of [another shop](#a-shop-per-test) has none, and `--only admin` has none
([what runs](cli.md#what-runs)). The navigation has no other section of Shopify's admin
(Orders, Products, Customers, Settings): lsf has no page for them, and a `shopify:admin/…` link
of the app lands on a page that says so.

| Query | What it does |
|---|---|
| `shop` | The shop: `test-1` or `test-1.myshopify.com` ([a shop per test](#a-shop-per-test)). The store data's by default. |
| `user` | The staff member the session token is for: `1`, the account owner, by default. |
| `locale` | The `locale` the app is opened with: the shop's primary locale by default. |

The other parameters go to the app. `<handle>` is `handle` of `shopify.app.toml`, or the client
id; `/admin`, `/admin/` and `/admin/apps` redirect there. An app with `embedded = false` gets a page that
links to its launch instead, as Shopify opens it in a tab of its own.

### Load lsf's App Bridge

The app's page runs App Bridge, Shopify's script, which talks to Shopify's admin only. lsf
serves its own, `/__lsf/admin/app-bridge.js`, which talks to lsf's admin: load it first in the
page's `<head>`, before any other script.

```html
<script src="http://127.0.0.1:9292/__lsf/admin/app-bridge.js"></script>
<script src="https://cdn.shopify.com/shopifycloud/app-bridge.js" data-api-key="…"></script>
```

Shopify's script, when it loads after it, finds `shopify.ready` and does nothing (its first line
returns when App Bridge is already there): the page keeps its tag, and loads nothing else of
Shopify's. Offline, the tag does not load, and lsf's App Bridge is all there is. lsf's reads its
configuration where Shopify's does (`data-api-key` of the CDN tag, `<meta name="shopify-api-key">`,
`<script type="shopify/config">`, the page's `shop`, `host` and `locale`, then
`sessionStorage`), and takes the admin's origin from its own `src`: open the admin on the host
name the page loads it from (`127.0.0.1` and `localhost` are two origins).

Give `--app-url` an address on this machine (`http://localhost:3000`), not a tunnel: Chrome
treats a page served from a public address as public, and blocks its requests to `127.0.0.1` or
`localhost` (lsf's App Bridge, Polaris from `/__lsf/cdn/`) until the browser grants it local
network access.

**The frame goes through a proxy.** lsf's admin opens the app's frame on a port of its own
(`embedded:` names it: `its frame is http://127.0.0.1:52011, a proxy of http://localhost:3000`),
which forwards to `--app-url`. Shopify's libraries let only Shopify's admin frame the app: every
page `@shopify/shopify-app-react-router` answers has `Content-Security-Policy: frame-ancestors
https://<shop> https://admin.shopify.com …`, those it writes itself included (the bounce page
`/auth/session-token`, the page of `redirect(url, {target: "_top"})`, the exit-iframe page), which
a Response thrown from a loader carries past `entry.server.tsx`. No local origin is
`https://<shop>`. The proxy changes, in what the backend answers:

- `frame-ancestors` leaves `Content-Security-Policy`, and `X-Frame-Options` goes;
- `https://cdn.shopify.com/shopifycloud/app-bridge.js` is lsf's App Bridge, and
  `https://cdn.shopify.com/shopifycloud/polaris…` is lsf's `/__lsf/cdn/` (in the pages and in
  `Link`): Shopify's CDN online, lsf's copy offline;
- the backend's origin is the proxy's (in the pages, `Location`, the `shopify-reload` of the
  bounce page, `X-Shopify-API-Request-Failure-Reauthorize-Url`), so that the frame stays on it.

The backend sees each request as if it came to it (`Host`, `Origin` and `Referer` are its own).
So the React Router template runs in lsf's admin as it ships: no edit of `entry.server.tsx`,
`app.tsx` or `polarisUrl`, and no `APP_BRIDGE_URL`. Its one edit is
[the fetch function](#javascript-the-react-router-app-and-shopifyshopify-api), for the Admin API.
The proxy forwards HTTP, not WebSockets: an HMR socket on the app's own port does not pass (the
template's has a port of its own). `LSF_ADMIN_FRAME=direct` opens the backend in the frame
without the proxy.

A page opened outside lsf's frame (`/__lsf/admin/launch`, a test that fetches `--app-url`) gets
none of this: it loads lsf's App Bridge itself, as above, with
`APP_BRIDGE_URL=http://127.0.0.1:9292/__lsf/admin/app-bridge.js` read by
`@shopify/shopify-app-react-router/adapters/node` for the pages the library writes, and a
`<script src={appBridgeUrl} />` before `AppProvider`.
[`examples/admin-backend`](https://github.com/0xtlt/local-storefront/tree/main/examples/admin-backend)
does it by hand (`server/embedded.mjs`), and leaves the library's `frame-ancestors` as they are.

When the app's page runs Shopify's App Bridge instead (`LSF_ADMIN_FRAME=direct`), lsf's admin
sees what it posts and says so in a banner, with the line to add; when no App Bridge answers (the backend is down, the page
failed, the frame was refused), it says that. Inside lsf's frame, Shopify's script redirects
nothing (it sends the page to Shopify's admin only when it is not in a frame), but it listens to
Shopify's admin only: `shopify.idToken()` and every authenticated `fetch()` wait, for a minute
and for ever.

Outside an admin (the page opened alone, as `/__lsf/admin/launch` does), lsf's App Bridge sends
the page to lsf's admin, as Shopify's sends it to Shopify's, unless `shopify.config` has
`disabledFeatures: ["auto-redirect"]`.

### Polaris without the internet

Polaris, the web components of the app's pages (`<s-page>`, `<s-button>`, …), is a script of
Shopify's CDN, and loads its icons and its fonts from the CDN too. lsf serves the CDN's paths at
`/__lsf/cdn/<path>`, `<path>` being the path on `https://cdn.shopify.com/`: the page loads Polaris
from `http://127.0.0.1:9292/__lsf/cdn/shopifycloud/polaris-2.js`, and lsf's Polaris mode decides
what it gets.

| Mode | What `/__lsf/cdn/<path>` answers |
|---|---|
| `online`, the default | A `302` to `https://cdn.shopify.com/<path>`, the query kept: Shopify's Polaris, as without lsf. |
| `offline` | lsf's copy, which is in the binary: Polaris 2.0.0 (`shopifycloud/polaris-2.js`, `polaris-2.0.js`) and 1.1.0 (`shopifycloud/polaris.js`, `polaris-1.js`, `polaris-1.1.js`), every icon they name, and their fonts. In the scripts and stylesheets, `https://cdn.shopify.com/` becomes `http://<lsf>/__lsf/cdn/`: the icons and the fonts come from lsf too. A path lsf has no copy of is a `404` that says so. |

The mode is `--polaris offline` (or `LSF_POLARIS=offline`) of `serve` and `app dev`, and lsf
prints a `polaris:` line when it starts offline. While lsf runs, the
[control API](control-api.md) reads and changes it, for every page at once: `GET /__lsf/polaris`,
`PUT /__lsf/polaris` with `{"mode": "offline"}`, and `DELETE /__lsf/polaris` for the mode lsf
started in. `DELETE /__lsf/admin` and `DELETE /__lsf/sessions` keep it, as they keep stubbed URLs.
The copy is what the CDN served on 2026-11-08 (`crates/cli/assets/shopify-cdn`, whose README has
its hashes): lsf never downloads Polaris, and offline is the copy of the lsf you run. Online, the
page gets Shopify's current one.

In lsf's frame, the proxy sends the page's Polaris (`polaris.js` of the template's 3.x,
`polaris-2.js` of 4.0.0) and its `Link` preload to `/__lsf/cdn/`: nothing to change. Outside it,
`polarisUrl` (`@shopify/shopify-app-react-router` 2.1.0 or later) goes on `AppProvider` and on
`shopifyApp` (for the `Link` header), set to `http://127.0.0.1:9292/__lsf/cdn/shopifycloud/polaris.js`
(the same file the app loads). A page with a `Content-Security-Policy` allows lsf's origin where
it allows `cdn.shopify.com`: `script-src` (Polaris), `style-src` (the stylesheets of its fonts),
`font-src` (the fonts) and `connect-src` (Polaris fetches its icons). lsf answers with
`Access-Control-Allow-Origin: *`, and answers the preflight of a `fetch()` with headers
(`OPTIONS`: `204`, the headers asked for allowed, kept a day), so the app's origin may differ
from lsf's.

A page that still loads Polaris from `cdn.shopify.com` runs offline too: when that script fails
to load and lsf is `offline`, lsf's App Bridge loads lsf's copy of the same path, once, and says
so in the console. It never loads one when Polaris is already defined, nor when lsf is `online`.
[`examples/admin-backend`](https://github.com/0xtlt/local-storefront/tree/main/examples/admin-backend)
loads Polaris from `POLARIS_URL`, and its end-to-end run checks both in Chromium, with every
request to `cdn.shopify.com` refused.

### What App Bridge does in lsf's admin

lsf's App Bridge has every name of `@shopify/app-bridge-types` 0.7.4 (the types of the CDN
script). In lsf's admin:

| API | What it does |
|---|---|
| `shopify.idToken()` | A session token of the admin's shop and user, minted by lsf and signed with the client secret, valid a minute. |
| `fetch()` | Adds `Authorization: Bearer <session token>`, `X-Requested-With: XMLHttpRequest` and `Accept-Language` to the requests to the page's host (on any port) or its subdomains, over the page's protocol, and to `appOrigins`; not to others, nor when the request has an `Authorization` already, nor to lsf's own origin (the admin the script comes from, its `/__lsf/cdn/` included), which on Shopify is never the app's host, unless the page itself is on lsf's origin. Asks a new token once when the answer has `X-Shopify-Retry-Invalid-Session-Request`, and sends the admin to `X-Shopify-API-Request-Failure-Reauthorize-Url` when there is one. `disabledFeatures: ["fetch"]` leaves `fetch` alone. As the CDN script 0.41.4 does. |
| `shopify.config`, `shopify.origin`, `shopify.ready`, `shopify.environment` | The configuration as the page gives it; lsf's admin's origin; ready once the admin answered; `{embedded: true, mobile: false, pos: false, intent: false}`. |
| `shopify.user()` | `{id, name, email, accountAccess, locale}` of the admin's user: `"Account owner"` for user 1, `"Full access"` for another. |
| `shopify.toast.show()`, `.hide()` | Toasts at the bottom of the admin, five seconds by default, `isError`, an `action` that calls `onAction`; `onDismiss` when the toast goes. |
| `shopify.loading()` | The loading bar at the top of the admin. |
| Title bar | The admin's title bar: `<ui-title-bar title>` and its buttons and links (`variant="primary"`, `"breadcrumb"`, `tone="critical"`, `<section label>` menus), else `<s-page heading>` and its slots (`primary-action`, `secondary-actions` with an `s-menu` by `commandFor`, `breadcrumb-actions`, `accessory`), else `document.title`. A click in the admin clicks the page's element. |
| Navigation | The admin's navigation, under the app's name: the links of `<s-app-nav>` or `<ui-nav-menu>` (`a`, `s-link`), relative, `app:` or of the page's origin; `rel="home"` is the app's name. A click in the admin clicks the page's link, so that the app's router follows it. The admin's URL follows the app's path (`history.pushState`, `replaceState`). |
| Save bar | `<form data-save-bar>` shows the admin's save bar while a field differs from what it was (Save submits the form, Discard resets it; `data-discard-confirmation` asks first); `<ui-save-bar id>` with `shopify.saveBar.show()`, `.hide()`, `.toggle()` (its `variant="primary"` button is Save, the other Discard; `discardConfirmation`). While it shows, a link or a nav item asks the merchant first, and `shopify.saveBar.leaveConfirmation()` resolves when they leave. |
| `shopify.resourcePicker()` | A picker of the shop's products (with their variants), variants or collections, as the Admin API has them: `action`, `multiple` (a number is a maximum), `query`, `filter` (`query`, `variants`, `draft`, `archived`, `hidden`), `selectionIds`. Answers `Product[]`, `ProductVariant[]` or `Collection[]` of `@shopify/app-bridge-types`, a product's `variants` the ones picked; `undefined` when cancelled. |
| `shopify.picker()` | A picker of the app's own items: `heading`, `items`, `headers`, `multiple`; `selected` resolves with their ids. |
| `shopify.scopes.query()` | `granted` (the scopes installed on the shop), `required` and `optional` (those of `shopify.app.toml`). |
| Modals | `<ui-modal>` (its children, or `src`; its `<ui-title-bar>` is its title and buttons), `<s-app-window src>`, and `<s-modal>` while Polaris is not there (offline): drawn over the app, in its frame, the admin around dimmed. `shopify.modal.show(id)`, `.hide()`, `.toggle()`; Polaris's `s-modal` opens itself. A page of a modal's `src` shares the frame's App Bridge. |
| Links | `shopify:admin/products` (and `shopify://admin/…`, `window.open`) goes to lsf's page of it, which says lsf has no admin page there. `app:/path` is a path of the app. `target="_top"` leaves the admin. A plain `<a>` the app's router does not follow reloads the frame without the session token, on Shopify too: the console says so. Without Polaris, an `s-link`, `s-button` or `s-clickable` with `href` dispatches `shopify:navigate`, as Polaris does, for `AppProvider` to route. |
| The bounce page | `?shopify-reload=<url>` (the library's `/auth/session-token`) loads `<url>` again with a new `id_token`. |

What lsf does not simulate rejects with, or warns, `lsf does not simulate shopify.<name>`, and
the admin lists it in a banner: `shopify.scanner`, `shopify.pos`, `shopify.intents.invoke` and
`.register`, `shopify.webVitals`, `shopify.support`, `shopify.reviews`, `shopify.scopes.request`
and `.revoke`, `shopify.app.extensions()`, `shopify.shopifyQL`, `shopify.sidekick`,
`shopify.tools`, `fetch()` of `shopify:admin/api/…` (Direct API access), and `extension:` links.
`shopify.intents.request.value` is `null`: the page never runs as an intent.

Polaris web components (`s-page`, `s-button`, `s-text-field`, …) are Shopify's own, which lsf
serves rather than simulates: Shopify's CDN through `/__lsf/cdn/`, or lsf's copy of it
([without the internet](#polaris-without-the-internet)). When no Polaris loads (a page on the
CDN, no internet, lsf `online`), they show as plain elements with their text, and lsf's App
Bridge reads what it needs of them all the same (the title bar of `s-page`, the links of
`s-app-nav`), opens `s-modal`s and follows `s-link`s itself.

## Tokens

| Token | How to get it |
|---|---|
| The default token of a shop | Printed at start, in `GET /__lsf/admin` (`shops[].token`), and the answer of `POST /__lsf/admin/shops`. Offline, never expiring, the scopes of the app. |
| A session token (App Bridge's `id_token`) | `POST /__lsf/admin/session-token` `{"shop"?, "user"?}`: HS256 with the client secret, the claims of shopify.dev's "ID tokens". |
| An access token by token exchange | `POST /admin/oauth/access_token` with the session token, as the React Router template and the libraries do: offline or online, expiring or not. |
| An access token by the authorization code grant | `GET /admin/oauth/authorize?client_id=&scope=&redirect_uri=&state=` approves at once and redirects to `redirect_uri` with `code`, `hmac`, `host`, `shop`, `state`, `timestamp`; then `POST /admin/oauth/access_token`. `redirect_uri` must be one of `[auth] redirect_urls`, or one of them on the origin of `application_url` moved to the origin of `--app-url`, the same path: with `https://my-app.trycloudflare.com/auth/callback` in the file and `--app-url http://localhost:3000`, `http://localhost:3000/auth/callback` is accepted, and `http://localhost:3000/other/callback` is not. An app with no `[auth] redirect_urls` accepts none. |
| Refresh tokens, client credentials | `grant_type=refresh_token`, `grant_type=client_credentials`, as Shopify. |
| A delegate access token | `delegateAccessTokenCreate`: `shppa_…`, a subset of the parent's scopes, expiring with it or sooner (see "[The app's installation](admin-reference.md#the-apps-installation-and-shopifys-other-services)"). |

OAuth without a library (in Python, say): send the browser to
`$LSF_URL/admin/oauth/authorize?client_id=…&scope=…&redirect_uri=…&state=…` (with `&shop=` for a
shop other than the default one), and POST the code of the callback to
`$LSF_URL/admin/oauth/access_token`, `{"client_id", "client_secret", "code"}`: the code knows its
shop.

**Tokens survive a restart of lsf.** lsf keeps its tokens in memory, and the backend keeps the
ones it got in its session storage. So every token lsf issues (offline, expiring offline and its
refresh token, client credentials) is derived from the client id, the client secret, the shop,
its kind and its rank: the first offline token of a run is the first of every run. A token lsf
does not know is looked for among those it would issue next, and taken back: the backend's
stored session works after lsf restarts, with the scopes the app is installed with now (an
expiring one for an hour from then), and the terminal says
`admin:  took back an access token of an earlier run of lsf (shpat_3cbbea…)`. A token revoked
in the run (an uninstall, a reset, a shop deleted), or one of this run that expired, is never
taken back; a token of another secret is unknown. Online tokens are not taken back: they last a
day, and the libraries exchange a new session token after a `401`.

Every access token is `shpat_` and 32 hex digits, as Shopify's. A token belongs to one shop. A
request without a valid token is answered `401`
`{"errors": "[API] Invalid API key or access token (unrecognized login or wrong password)"}`, with
an `X-Lsf-Reason` header saying why in plain words. A token or a code that expired, and a refresh
token that was retired, are refused as such for a day; then lsf forgets them, and refuses them as
tokens it never issued. For a revoked token, the reason says what revoked it and how to get
another: `this access token of test-1.myshopify.com was revoked when the shop was deleted
(DELETE /__lsf/admin/shops/test-1): run the token exchange or OAuth again …` (an uninstall and
a reset say so too). The server prints it once per token, `--quiet` or not:
`admin:  401 for a revoked access token: …`.

**Scopes are enforced.** A field or a mutation the token's scopes do not cover answers
`ACCESS_DENIED` with Shopify's message ("Access denied for products field. Required access:
`read_products` access scope."), for root fields, nested ones (an order's `customer` needs
`read_customers`) and `node`. `write_x` covers `read_x`. To see what the backend does with fewer
scopes: `POST /__lsf/admin/install {"scopes": "read_orders"}` installs the app again with them
(every token of the app on the shop now reads less, the printed one and those of token exchange
and OAuth alike, and `app/scopes_update` is delivered).

**Uninstall.** `POST /__lsf/admin/uninstall` does what Shopify does when a merchant uninstalls
the app: it revokes the app's tokens on the shop, cancels its subscription, removes its
Functions with every object that runs them, its script tags and its web pixel, stops rendering
its UI extensions (below), delivers `app/uninstalled` (through the
subscriptions the shop had), then deletes the subscriptions the backend made with
`webhookSubscriptionCreate` ("Shopify automatically performs cleanup tasks, such as deleting
WebhookSubscription objects", [`appUninstall`](https://shopify.dev/docs/api/admin-graphql/2026-10/mutations/appUninstall)).
From then on nothing is delivered for the shop: an order placed at the checkout is recorded, and
no `orders/create` goes out, not even through `shopify.app.toml`, whose subscriptions apply to
"every shop that installs your app". Installing it again (`POST /__lsf/admin/install`, OAuth, or a
token exchange) brings back the subscriptions of `shopify.app.toml`; those of the shop are gone,
for the backend to make again, as on Shopify. `shop/redact`, which Shopify sends 48 hours after
an uninstall, is sent on demand: `POST /__lsf/webhooks/trigger {"topic": "shop/redact"}`.

**The app's Functions stop with the uninstall.** Shopify removes an uninstalled app's
extensions, Functions included, and a Function goes "including all associated function owners"
([About Shopify Functions](https://shopify.dev/docs/apps/build/functions)). So the uninstall
deletes, with their metafields, every object that runs one of the app's Functions: its payment
and delivery customizations, validations, cart transforms, fulfillment constraint rules, and its
app discounts (`DiscountAutomaticApp`, `DiscountCodeApp`), whether the app created them or the
store data declared them (`checkout.functions.<handle>.enabled`, `checkout.discounts`). The
answer counts them (`removedFunctionOwners`). The discounts of Shopify's own types (a
`DiscountCodeBasic` the app made) stay. While the app is uninstalled, the shop's checkout runs
none of its Functions, whatever a scenario declares or names in its `extensions`: the
checkout's Functions dialog lists each with "the app is not installed on the shop". A reinstall
gets none of those objects back, as on Shopify, where a reinstalled app "had to register them
again through the API" ([community](https://community.shopify.com/t/shopify-function-doesnt-work-when-reinstalling-the-app/261798)):
the app creates them again, as on a first install. Shopify removes them in the background
after the uninstall ("the job that deletes the price rules and their associated codes runs
asynchronously", Shopify's staff on [community.shopify.dev](https://community.shopify.dev/t/discountredeemcodebulkadd-phantom-redemption-codes/32591));
lsf removes them at once.

**The app's own metafield definitions go too; their metafields stay.** "When a user uninstalls
your app, Shopify deletes its app-owned metafield definitions and temporarily retains the
metafields and their values without a definition. This applies to any app-owned definition,
whether declared in `shopify.app.toml` or created with the GraphQL Admin API", and "If the same app
is reinstalled while that data is still retained, then Shopify recreates the definitions and
associates the existing metafields with them" ([Manage metafield definitions, App uninstall and
reinstall](https://shopify.dev/docs/apps/build/metafields/definitions#app-uninstall-and-reinstall)).
lsf does the same:

| Definition | On Shopify | In lsf |
|---|---|---|
| In the app's reserved namespace (`$app`, `$app:<name>`, stored `app--<app id>…`), declared in `shopify.app.toml` | Deleted during the uninstall; created again on reinstall, under a new GID ("a definition ID from before the uninstall no longer resolves") | Deleted by the uninstall (`removedMetafieldDefinitions`); the reinstall creates it again under a new id, with its metafields |
| In the app's reserved namespace, made with `metafieldDefinitionCreate` | Deleted too, in the background ("can persist after uninstall until asynchronous cleanup removes them"); created again by a reinstall while the data is retained | Deleted at once, like the declared ones; the reinstall creates it again under a new id |
| In any other namespace (`custom`, one the app chose) | The merchant's definition: nothing says an uninstall touches it | Stays, id and metafields included |
| The metafields (values) of the app's definitions | Kept a while without a definition ("Shopify doesn't guarantee how long") | Kept, without a definition, until the store data is seeded again; a new seed while the app is uninstalled keeps the declared definitions deleted and drops the ones the app made |

While the app is uninstalled, an order does not copy the cart metafields of a deleted `$app`
definition (`cart_to_order_copyable`): the definition is gone. Look definitions up by owner type,
namespace and key after a reinstall, as shopify.dev advises, not by an id kept from before. The
app-data metafields of its installation (`AppInstallation`) stay: no shopify.dev page says an
uninstall deletes them.

**The app's own metaobject definitions go too; their metaobjects are kept.** Shopify says the
same of metaobjects: "When a user uninstalls your app, Shopify deletes its app-owned metaobject
definitions and temporarily retains the metaobjects and their values without a definition", and a
reinstall "recreates the definitions and associates the existing metaobjects with them", under new
GIDs ([Manage metaobject definitions, App uninstall and
reinstall](https://shopify.dev/docs/apps/build/metaobjects/manage-metaobject-definitions#app-uninstall-and-reinstall)).
lsf does the same:

| Definition | On Shopify | In lsf |
|---|---|---|
| Of the app's reserved types (`$app:<type>`, stored `app--<app id>--<type>`), from the store data or `metaobjectDefinitionCreate` | Deleted (the GraphQL ones in the background); created again on reinstall, under a new GID | Deleted by the uninstall (`removedMetaobjectDefinitions`); the reinstall creates it again under a new id |
| Of any other type (`author`, `size_chart`) | The merchant's | Stays, id and entries included |
| The metaobjects of the app's definitions | Kept a while without a definition | Kept aside, ids and values included (`retainedMetaobjects`), until the store data is seeded again; the reinstall gives them back to the new definition |

While the app is uninstalled, its metaobjects are out of reach: a `Metaobject` has a non-null
`definition`, and `metaobjects(type:)` takes the type of "a definition that already exists in the
shop". So no query lists them, and the storefront does not render them. lsf does not read
`[metaobjects.app.<type>]` of `shopify.app.toml`: the app's types come from the store data and from
the Admin API. A metafield definition whose validation names the deleted definition
(`metaobject_definition_id`) names the new one after the reinstall. Look definitions up by type
after a reinstall, as shopify.dev advises.

**The app's extensions go with it too.** While the app is not installed on the default shop, lsf
renders none of its UI extensions, as Shopify removes an uninstalled app's extensions. Its theme
app blocks are files of the theme, which lsf leaves as they are:

| Extension | On Shopify | In lsf while uninstalled | After a reinstall |
|---|---|---|---|
| Checkout UI extensions, thank-you and order status targets | They stop rendering; a reinstall within 24 hours restores the active ones with their settings ([staff on community.shopify.dev](https://community.shopify.dev/t/activated-checkout-extensions-do-not-deactivate-after-app-uninstall/35571)); later, the merchant places them again in the checkout editor | The checkout and its thank-you page load none, an order placed before the uninstall included | They render again: lsf has no checkout editor, so it always restores them |
| Customer account UI extensions | Placed in the same checkout and accounts editor | The customer account loads none; an extension's full page (`/account/pages/<handle>`) answers `404` with the reason | They render again |
| Theme app extensions (app blocks, app embeds) | "Blocks associated with the apps are automatically and entirely removed from online store themes" ([UX guidelines](https://shopify.dev/docs/apps/build/online-store/theme-app-extensions/ux)); app embeds are off after an install until the merchant turns them on | They still render: they are the theme's files (and `app.embeds` of the store data), which lsf never writes ([theme app extensions](storefront.md#theme-app-extensions)) | Nothing changes |
| Web pixel | Removed with the app; a reinstalled app calls `webPixelCreate` again | The uninstall deletes the `WebPixel` record (`removedWebPixels`); the storefront runs no web pixel anyway | The app creates it again |
| Script tags | "When an app is uninstalled from a shop, all of the script tags that it created are automatically removed" ([`ScriptTag`](https://shopify.dev/docs/api/admin-graphql/2026-10/objects/ScriptTag)) | The uninstall deletes them (`removedScriptTags`): `content_for_header` stops loading them at once | The app creates them again |
| Admin UI extensions, admin links | Admin links are deleted ([`appUninstall`](https://shopify.dev/docs/api/admin-graphql/2026-10/mutations/appUninstall)) | lsf's admin renders neither | - |
| App proxy | A store gets it with the app: a change of `prefix` or `subpath` applies "only to new installations" (on a dev store, "uninstall and reinstall the app", [About app proxies](https://shopify.dev/docs/apps/build/online-store/app-proxies)), so the uninstall removes it and the storefront answers the path with its `404` page | `/<prefix>/<subpath>` is the theme's `404` page (with `X-LSF-Reason`), and nothing reaches the backend ([App proxy](#app-proxy)) | Forwarded again, on the path of `shopify.app.toml` |

Each place that shows extensions says why they are gone, "the app is not installed on the shop":

- the checkout: a banner (`gift-message, order-notice: not loaded. …`), the "Extensions not
  loaded" list of the Functions dialog, and the panel of `?targets=all`; its state has
  `"extensions": []` and `"uninstalled": {"reason", "extensions"}` (`null` while installed);
- the customer account: the same banner and panel;
- the home page (`/` with `--app` and no theme, `/__lsf/checkouts` otherwise): a banner, a line in
  Problems, and `app not installed` on each Function and UI extension; its JSON has
  `"installed": false` and `"uninstalled"`;
- the terminal, unless `--quiet`: `admin:  local-supply-co.myshopify.com: the app is not installed
  on the shop: its UI extensions do not render and its Functions do not run until it is installed
  again (…)`, then, for an app with an `[app_proxy]`, `… uninstalling it removed its app proxy, so
  /apps/reviews is the storefront's 404 …`; and `… the app is installed again: its UI extensions
  render again, …; its 2 metafield definitions are back, under new ids; its metaobject definition
  is back, under a new id, with its metaobjects; /apps/reviews goes to its app proxy again` after a
  reinstall.

## Webhooks

- **Subscriptions**: those of `shopify.app.toml` (`[[webhooks.subscriptions]]`, every shop the
  app is installed on), and those the backend makes with `webhookSubscriptionCreate` (per shop,
  deleted when the app is uninstalled; listed by `webhookSubscriptions`, which, as on Shopify,
  does not list those of the file). A subscription
  made through the Admin API takes `https://` and, locally, `http://` to this machine.
- **Where they go**: a `uri` that is a path joins `--app-url` (else `application_url`); an
  absolute one on the origin of `application_url` (the tunnel of a `shopify app dev` that is not
  running) moves to `--app-url`; another goes where it says. **Webhooks stay on this machine**:
  lsf sends one to the loopback (`localhost`, `*.localhost`, `127.0.0.0/8`, `::1`), to the
  scheme, host and port of `--app-url`, or to an address given on purpose (`--address`,
  `address` of `POST /__lsf/webhooks/trigger`). A name under `localhost` (`app.localhost`) is
  sent to the loopback whatever your system's resolver says, with its own `Host` header: the
  name is reserved for the loopback (RFC 6761). `0.0.0.0` and `::ffff:127.0.0.1` do not count as
  this machine. Anything else (a production backend, a stale tunnel) is not sent, and
  `GET /__lsf/webhooks` logs it `not_sent`, with the reason `lsf sends webhooks to this machine
  or to --app-url only: <url> is neither; start lsf with --app-url http://localhost:<port> (your
  backend)`. So give `--app-url`: without it, the subscriptions of `shopify.app.toml` point at
  `application_url`, usually a tunnel, and lsf sends nothing there.
- **Shopify's Events** (`[events]`, `[[events.subscription]]`, which the app template ships for
  `webhooks.products.update.tsx`): lsf delivers those of topic `Product` (`create`, `update`,
  `delete`), when a product or a variant changes through the Admin API. An `update` is sent
  when a field of `triggers` changed (`product.variants.price`, `product.variants.*`,
  `product.*`; the product's `title`, `handle`, `status`, `tags`, `vendor`, `productType`,
  `descriptionHtml`, `templateSuffix`, and the variants' `title`, `price`, `compareAtPrice`,
  `sku`, `barcode`, `inventoryPolicy`, `taxable`, `position`), one delivery per variant changed.
  The body has `topic`, `action`, `handle`, `fields_changed` and `query_variables`
  (`productId`, `variantsId`), and `data` (and `errors`), what `query` answers with them, with
  the app's scopes; a `query_filter` that does not match sends nothing (`not_sent`, with why).
  The headers are Shopify's for Events: `Shopify-Topic: Product`, `Shopify-Action`,
  `Shopify-Handle`, `Shopify-Hmac-Sha256`, …, without `X-`; `authenticate.webhook` reads them
  (`topic` is `PRODUCT`). `GET /__lsf/webhooks?topic=Product` lists them. Another topic is
  warned about at start, and nothing is sent for it.
- **Events**: what the Admin API changes (`productCreate` → `products/create`,
  `inventory_items/create`, `inventory_levels/connect`; any other change of a product, such as
  `productUpdate`, `tagsAdd` or `metafieldsSet` on it → `products/update`; `productDelete` →
  `products/delete`), an order placed at the checkout
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
  backend does not answer `2xx` within 5 seconds. The delivery stays `pending` through the
  retries, its `reason` the last failure, and becomes `failed` after the last attempt, about 21
  seconds after the first.

Without a library, verify the HMAC on the raw bytes of the body, before parsing it: the JSON
parsed and serialized again is not what was signed. In Python:

```python
import base64, hashlib, hmac

def verify_webhook(raw_body: bytes, header: str, secret: str) -> bool:
    digest = hmac.new(secret.encode(), raw_body, hashlib.sha256).digest()
    return hmac.compare_digest(base64.b64encode(digest).decode(), header or "")

# Flask: verify_webhook(request.get_data(), request.headers.get("X-Shopify-Hmac-Sha256"), SECRET)
```

**The backend answers `401`** (or `403`): the two secrets differ. lsf signs with
`--client-secret` (`LSF_CLIENT_SECRET`), else `SHOPIFY_API_SECRET` of the environment, else the
one of the app's `.env`, else `lsf-local-client-secret`. lsf says so where you look, naming the secret it signed with as the start line `client secret:`
does:

- the server prints one line at the first refusal of each delivery, `--quiet` or not:
  `webhooks: products/create → http://localhost:3000/webhooks answered 401: lsf signed it with
  the client secret from SHOPIFY_API_SECRET of ../my-app/.env: does the backend verify with the
  same one?`;
- the delivery's `reason` in `GET /__lsf/webhooks` says the same: `answered 401: lsf signed it
  with …` while it is `pending`, `answered 401 (4 attempts): lsf signed it with …` once `failed`;
- `lsf app webhook trigger` prints `failed: answered 401 (1 attempt): lsf signed it with …`.

On demand: `POST /__lsf/webhooks/trigger {"topic": "orders/create", "id"?: "<gid>"}` sends the
webhook of a record of the shop (a representative one without `id`: for `orders/create`, the
first order), and the command line does it without a server. In the demo store, the first order,
`#1001`, is fulfilled already: its fulfillment order is `CLOSED`, and a handler's
`fulfillmentCreate` answers a `userError`. To test a fulfillment handler, [place an
order](#place-an-order), or give the `id` of an unfulfilled one:
`{"topic": "orders/create", "id": "gid://shopify/Order/7888001690128"}` (`#1002`).

```bash
npx local-storefront app webhook trigger --topic app/uninstalled --app ../my-app --app-url http://localhost:3000
```

The topic must be one `shopify.app.toml` subscribes to (or give `--address <url>`). Without
`--app-url`, a subscription that leads off this machine is not sent: the command sends nothing,
prints `error: lsf sends webhooks to this machine or to --app-url only: <url> is neither; give
--app-url http://localhost:<port> (your backend) or --address <url>` and exits `2`.
`--format json` prints each delivery as `GET /__lsf/webhooks` lists it. See the
[control API](control-api.md) and the [CLI](cli.md#app-webhook-trigger).

## Test your backend

A test of the backend runs lsf and the backend side by side. It gives itself a shop, calls the
backend as the app's front end does, places orders as a buyer, and waits for the webhooks lsf
delivers back. Tests that run in parallel each take a shop of their own.

### Start lsf and the backend

Fix both ports: the backend reads lsf's address when it starts (`LSF_URL`), and lsf delivers the
webhooks to the backend's (`--app-url`). Both need the same client secret. lsf reads
`SHOPIFY_API_SECRET` of the app's `.env` (its start line `client secret:` says where it found
one); the backend reads that file only when something loads it:

- `npx react-router dev`, the React Router template's dev server, loads it into the backend
  (`SHOPIFY_API_KEY`, `SHOPIFY_API_SECRET`, `SCOPES`), after `npm run setup` (its Prisma session
  storage);
- `npm run start` (`react-router-serve` of a build) reads no `.env`: run `npm run build` first,
  and pass the three in `env`;
- a plain Node server reads it with `node --env-file=.env server.mjs`.

With Playwright:

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
      // The backend, without the Shopify CLI: the React Router template's dev server.
      command: "npx react-router dev",
      url: "http://localhost:3000/",
      env: { PORT: "3000", LSF_URL: "http://127.0.0.1:9292", SHOPIFY_APP_URL: "http://localhost:3000" },
      reuseExistingServer: !process.env.CI,
    },
  ],
});
```

The backend's `url` must answer without a session token: Playwright waits for a `2xx`, `3xx` or
`400` to `403`. Without Playwright, start both, then wait until `GET /__lsf/status` answers (or
`ready:` is printed on stderr) and the backend's port does: with pytest, a fixture does it
([in Python](#in-python)).

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
  const answer = await fetch(LSF + path, {
    method,
    headers: { "Content-Type": "application/json" },
    body: body && JSON.stringify(body),
  });
  if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${await answer.text()}`);
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

// The deliveries that match the filters, once `wait` of them are done. Throws on a `408`
// (fewer were done than `wait` asks) and on a `422` (a wait that could only time out).
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
tests and of earlier runs included, so `wait=1` can answer with an old one at once. `wait`
counts only the deliveries that match every filter. Filter it:

- `shop=`: a test shop, by its name (`test-1`, in any case) or its domain.
- `resource=<gid>`: the deliveries about one record (the order a test placed, the product it
  made), as each delivery's `resource` gives it. Shops seeded from the same data give the same
  ids, so two test shops can make the same product GID: give `shop=` with it. Ids come back
  after a reset too: after `DELETE /__lsf/admin`, the next order has the id of the run before's,
  whose delivery the log still has. Give `since=` with it, or reset the log as well.
- `since=<id>`: the deliveries after that one. Every answer has `last`, the newest id of the
  log (`0` before the first). Ids are never given twice, even after a `DELETE`: read `last`
  before the action, and `since=<last>` waits for what the action sends only. For the second
  webhook of a topic, pass the `id` of the first: a `last` read after both were sent skips the
  second.

A delivery the backend fails is retried after 1, 5 and 15 seconds, and stays `pending` until its
last attempt, about 21 seconds on, its `reason` the last failure (`answered 401: lsf signed it
with …` names the secret). `wait` gives up after `timeout` (10 seconds by default) with a `408`
and the same body: to see a failure's last attempt, give `timeout=25000`. Then assert `state`,
and `attempts.at(-1).status` for what the backend answered.

A wait that could only time out answers `422` at once, when the log has nothing that matches: a
`shop=` lsf does not have (`there is no shop nosuchshop.myshopify.com: …`), or a topic no
subscription of the app covers on that shop, in `shopify.app.toml` or through the Admin API
(``no subscription to `inventory_items/create` for test-1.myshopify.com, …``). Such a topic is
never logged: subscribe to it to wait on it.

### Place an order

The [local checkout](checkout.md) is the storefront's: its orders land in the default shop
(`defaultShop` of `GET /__lsf/admin`), whatever shops a test made, and their `orders/create`
goes to the backend. A test that places one names its own session (`x-lsf-session`) and waits
for the webhook about its order:

```js
const headers = { "x-lsf-session": `order-${crypto.randomUUID()}`, "Content-Type": "application/json" };
const buyer = (method, path, body) =>
  fetch(LSF + path, { method, headers, body: body && JSON.stringify(body), redirect: "manual" });

const { last: since } = await webhooks({}); // the newest delivery before this order
const session = await buyer("PUT", "/__lsf/session", { cart: { items: [{ variant: "TEE-BLK-S", quantity: 1 }] } });
if (!session.ok) throw new Error(JSON.stringify((await session.json()).diagnostics)); // a SKU of your data?
const checkout = (await buyer("GET", "/checkout")).headers.get("location"); // /checkouts/cn/<token>
await buyer("POST", `${checkout}/update`, {
  operations: [
    { type: "email", value: "buyer@example.com" },
    { type: "shippingAddress", address: { lastName: "Buyer", address1: "1 Main St", city: "New York", countryCode: "US", provinceCode: "NY", zip: "10001" } },
    { type: "paymentMethod", handle: "card" },
  ],
});
const completed = await buyer("POST", `${checkout}/complete`);
const { order, errors } = await completed.json(); // order: { id, number, name }
if (!completed.ok) throw new Error(JSON.stringify(errors)); // what is missing, or the Function that failed
const { deliveries } = await webhooks({ topic: "orders/create", resource: order.id, since, wait: 1 });
```

- `TEE-BLK-S` is a SKU of the demo store; use one of your data. A SKU the data lacks answers
  `422`, and the `diagnostics` say so (`"MUG-RED" is neither the SKU of a variant nor the handle
  of a product`, `did you mean "MUG-RST"?`).
- `complete` needs an email, a payment method and, for a cart that ships, an address with a
  last name, a first line and a city. Otherwise it answers `422` with the state, whose `errors`
  say what is missing.
- `order` is the order as the Admin API recorded it. `order.id` is the `admin_graphql_api_id`
  of its `orders/create`: match by it, never by the number. The thank-you page and the
  checkout's `.json` keep the session's number: each session numbers its orders after the store
  data's (`#1004` in every session), while the Admin API gives the next number no order has
  (`#1004`, then `#1005`, …). So `complete` can answer `#1005` for a thank-you page that says
  `#1004`.
- A Function of the app that fails blocks `complete`: its error is in `errors`. A test that
  only needs an order serves the Functions it needs and no other (`--extension <handle>`,
  repeated), or none: `--no-functions` serves the UI extensions and the Admin API, and runs no
  Function.
- The backend processes the order in the default shop: fulfilling it takes the scope
  `write_merchant_managed_fulfillment_orders` ([orders](admin-reference.md#orders-fulfillments-and-draft-orders)).
  An order in a test shop is one the test makes there with `orderCreate`.

### Approve a charge

The backend's billing request answers `401` with the charge's `confirmationUrl` in
`X-Shopify-API-Request-Failure-Reauthorize-Url`, Shopify's, on the shop's admin:
`https://test-1.myshopify.com/admin/charges/5662588352328/1/RecurringApplicationCharge/confirm_recurring_application_charge?signature=…`.
Its fourth segment is the charge. Charges count from 1 in each shop, so approve that one, on
its shop:

```js
const answer = await callBackend(shop, "POST", "/api/billing/request");
const confirmation = new URL(answer.headers.get("x-shopify-api-request-failure-reauthorize-url"));
const charge = confirmation.pathname.split("/")[4];
await fetch(`${LSF_URL}/__lsf/admin/charges/${charge}/approve?shop=${confirmation.host}`, { method: "POST" });
```

In a browser, open the URL on lsf's origin (`confirmation.href.replace(confirmation.origin,
LSF_URL)`): lsf's admin redirects it to the charge page, `/__lsf/admin/charges/<n>?shop=`. The
subscription is then `ACTIVE`, and `app_subscriptions/update` is delivered if the app subscribes
to it. `/decline` declines it.

**Billing cycles follow the shop's clock.** Once `currentPeriodEnd` passes (the trial's end,
then every 30 days, or 365 for an annual plan), the next request starts the next cycle: the end
moves on, the usage balance starts again from nothing, and a limited discount has an interval
less. No webhook is sent for it: Shopify sends `app_subscriptions/update` for a change of status
or of capped amount. Move the clock ([the shop's clock](admin-reference.md#the-shops-clock)) to
see a renewal.

### Managed pricing

An app on managed pricing (Shopify now calls it Shopify App Pricing) has no billing code: its
plans are in the Partner Dashboard, Shopify hosts the page where the merchant picks one, and the
app sends the merchant there:

```js
// The template's loader, or `open("shopify://admin/charges/my-app/pricing_plans", "_top")` in the page.
return redirect(`shopify://admin/charges/${appHandle}/pricing_plans`, { target: "_top" });
```

lsf has no Partner Dashboard: declare the plans in a file and start lsf with
`--pricing-plans pricing.toml` (or `LSF_PRICING_PLANS`; JSON for a `.json` file):

```toml
# pricing.toml: the plans of the Partner Dashboard.
[[plans]]
handle = "free"            # plan_handle: in the subscription, the webhook and the welcome link
name = "Free"              # the subscription's name
price = 0                  # 0 (or free = true): a free plan
features = ["One market"]

[[plans]]
handle = "pro"
name = "Pro"
price = 4.99               # USD unless `currency` says otherwise
interval = "monthly"       # or "annual"
trial_days = 7
features = ["Every market", "Rules by customer tag"]
welcome_link = "/welcome"  # a path of the app, or a full URL; the app's home without it
```

A test sets them while lsf runs: `PUT /__lsf/admin/pricing-plans` with
`{"plans": [{"handle": "pro", "name": "Pro", "price": 4.99, "trial_days": 7}]}` (the same fields),
`DELETE` puts back those of the file, read again. Unless `--static`, lsf reads the file again when
it changes, before the next request (`pricing plans reloaded`); plans a `PUT` set stay until the
`DELETE`. lsf prints the page
at start (`pricing: 2 plans …`).

The page is at `/admin/charges/<app handle>/pricing_plans` of [lsf's admin](#preview-the-embedded-app).
A `shopify:admin/charges/…` link of the app, and Shopify's own URL of the page
(`https://admin.shopify.com/store/<store>/charges/<app handle>/pricing_plans`, which the
template's `redirect` opens), land there. Build the link with the app's handle (`handle` of
`shopify.app.toml`): on Shopify a client id in its place fails, so lsf answers it with a `404`
and an `X-LSF-Reason` header. Only an app whose `shopify.app.toml` has no handle gets its page at
its client id, since lsf knows no other name for it. The merchant chooses a plan and approves it,
as on Shopify:

- the shop gets a new `ACTIVE` subscription named after the plan, `test: true` (lsf's shops are
  development stores), with the plan's trial and one `AppRecurringPricing` line
  (`price`, `interval`, `planHandle`). `currentAppInstallation.activeSubscriptions` lists it;
- the plan it replaces is `CANCELLED` at once: a shop has one subscription. Down to a free plan,
  the new one keeps the paid plan's `currentPeriodEnd`;
- `app_subscriptions/update` is delivered for each (`plan_handle` in the payload);
- the browser goes to the plan's welcome link with `charge_id` (the subscription's number) and
  `plan_handle`: a path is the app's page in lsf's admin
  (`/admin/apps/<handle>/welcome?shop=…&charge_id=3&plan_handle=pro`), a full URL gets `shop`
  too.

While plans are declared, `appSubscriptionCreate` and `appPurchaseOneTimeCreate` answer the user
error Shopify gives an app on managed pricing: "Managed Pricing Apps cannot use the Billing API
(to create charges)." `appSubscriptionCancel` and uninstalling the app still cancel the
subscription: `appSubscriptionCancel` delivers `app_subscriptions/update` (`CANCELLED`), the
uninstall nothing (the app's webhook subscriptions go with it). A test chooses a plan without
the page:

```js
await fetch(`${LSF}/__lsf/admin/pricing-plans/pro/select`, { method: "POST", body: JSON.stringify({ shop }) });
// → { ok, subscription: { id, chargeId, name, status, … }, replaced: [...], redirect }
```

A test ends the shop's plan the same way, as if it ended outside the app (the app's
`appSubscriptionCancel` needs its token; this does not):

```js
await fetch(`${LSF}/__lsf/admin/pricing-plans/cancel`, { method: "POST", body: JSON.stringify({ shop }) });
// → { ok, plan, subscription: { id, status: "CANCELLED", … }, webhook: "app_subscriptions/update" }
```

The shop's active subscription is `CANCELLED` at once, `currentAppInstallation.activeSubscriptions`
is empty, and `app_subscriptions/update` (`CANCELLED`, the plan's `plan_handle`) is delivered,
none with `legacy = false` (`webhook: null`): the case of an app that has to notice by itself that
the merchant has no plan. A `409` when the shop has no active subscription. Shopify has no exact
equivalent. Its plan page has no button to end a plan: the merchant chooses another plan
(a free one, if the app has it) or uninstalls the app, and Shopify's help center says that "app
plan changes are managed within the app itself or through the app developer's website, not from
Settings > Apps". The route stands for a plan that ends while the app stays installed (Shopify's
support, a store closed), which Shopify App Pricing counts among the "cancellations, freezes, or
plan expirations" it sends no webhook for. The status is `CANCELLED`, which the Admin API gives
to every subscription that ends ("the app being uninstalled, a new app subscription being
activated, or a direct cancellation by the app"); `EXPIRED` is for a charge never approved. No
button on lsf's page either.

**Shopify changed this in April 2026.** Its page now says that for the apps enrolled since,
and for every app after April 28, 2026, the welcome link gets `plan_handle` alone, no
`app_subscriptions/update` is sent, and the app reads its plan from the Partner API's
`activeSubscription`. Its "legacy features" section still lists `charge_id`, the webhook and test
subscriptions for the apps enrolled before. lsf does what those apps get, unless the declaration
says `legacy = false`: then `plan_handle` alone, no webhook, `test: false`. No webhook means none
for any change of the subscription: a plan chosen, `appSubscriptionCancel` from the app, an
uninstall by the merchant. Shopify's page: "Shopify App Pricing doesn't use webhooks to notify
your app of subscription changes", and for "status changes that happen outside of a redirect
(such as cancellations or freezes), query the Partner API"; Partner support told a developer
that `app_subscriptions/update` "belongs to the older Billing API and is not supported for App
Pricing" ([community](https://community.shopify.dev/t/app-pricing-app-events-usage-billing-request-for-documented-answers-on-undocumented-behaviors/36911)).
lsf still records the subscription in the Admin API either way, and serves the Partner API
(below) in both modes.

#### Read the plan from the Partner API

An app on Shopify App Pricing reads its plan from the [Partner
API](https://shopify.dev/docs/api/partner): `activeSubscription(appId:, shopId:)` for the plan now,
and `events` for the history ([Shopify App
Pricing](https://shopify.dev/docs/apps/launch/billing/shopify-app-pricing#query-subscription-data)).
lsf serves it at Shopify's path, on its own origin. The app changes the origin alone:

```text
Shopify:  POST https://partners.shopify.com/{partner_id}/api/2026-10/graphql.json
lsf:      POST http://127.0.0.1:9292/{partner_id}/api/2026-10/graphql.json
          X-Shopify-Access-Token: {partner API client token}
```

lsf prints the URL and the token at start, after the `pricing:` line:

```text
partner: http://127.0.0.1:9292/9880163/api/2026-10/graphql.json   (Partner API: activeSubscription and events of gid://shopify/App/3662721844947; token prtapi_lsf_9c17…)
```

`GET /__lsf/admin/partner` has the same, with each shop's GID: a test reads them there.

- **The Partner ID** is seven digits, and **the token** is `prtapi_lsf_` and 32 hex digits.
  Shopify's real tokens are `prtapi_` and 32 hex digits, which GitHub's push protection refuses in
  a commit; the `lsf_` keeps lsf's token out of that shape, so your repository can hold it. Both
  are the same every run for the app's client id: a test can hard-code them, or read them from
  `GET /__lsf/admin/partner` (`partnerId`, `token`). Another token, or another number in the
  path, answers Shopify's `401` `{"errors": [{"message": "Invalid access token", "extensions":
  {"code": "401"}}]}`; `X-Lsf-Reason` says which part is wrong.
- **The app's GID** is the Admin API's: `gid://shopify/App/<n>`, what
  `currentAppInstallation { app { id } }` answers. **A shop's GID** is its `shop { id }`. So the
  app can read both from the Admin API, as the [example
  backend](https://github.com/0xtlt/local-storefront/tree/main/examples/admin-backend) does
  (`GET /api/billing/partner` in `server/index.mjs`). The older `app(id:)` takes
  `gid://partners/App/<n>`, the same number.
- **Versions**: 2026-07, 2026-10 and 2027-01, which have the same schema. Another version answers
  `404`. The schema is Shopify's whole 2026-10 one, so introspection and validation work as on
  Shopify.

```js
// The app's own code: only the origin changes.
const PARTNER_API = process.env.PARTNER_API_URL ?? "https://partners.shopify.com";
const answer = await fetch(`${PARTNER_API}/${process.env.PARTNER_ID}/api/2026-10/graphql.json`, {
  method: "POST",
  headers: { "Content-Type": "application/json", "X-Shopify-Access-Token": process.env.PARTNER_API_TOKEN },
  body: JSON.stringify({
    query: `query($appId: ID!, $shopId: ID!) {
      activeSubscription(appId: $appId, shopId: $shopId) { trialEndsAt items { handle } } }`,
    variables: { appId, shopId },
  }),
});
```

What lsf answers, from the plans chosen, changed and ended, the trials, the installs and the
uninstalls of every shop:

| Query | What lsf answers |
|---|---|
| `activeSubscription(appId:, shopId:)` | The shop's active subscription of managed pricing: one item, the plan (`handle`, the plan's name as `description`, a `FlatRatePrice`); `billingPeriod`; `trialEndsAt` while the trial runs, then `null`; `currentBillingCycle` `null` during the trial, then the cycle the shop's clock is in (periods of 30 days, or 365 for an annual plan, from the end of the trial or the approval); `legacySubscriptionId` the Admin API's `AppSubscription` GID while the plans keep the legacy features, `null` with `legacy = false`; `cancelAtEndOfCycle: false` and `pendingUpdate: null`. `null` without a plan, and for a subscription of the Billing API (below). "Shop not found" for a shop lsf does not have. |
| `events(filter:, orderBy:, first:, after:, …)` | The history of every shop, newest first (`OCCURRED_AT_ASC` turns it), the last 30 days by each shop's clock unless `occurredAtMin` or `occurredAtMax` says otherwise (365 days at most), 250 a page at most: `RELATIONSHIP_INSTALLED` and `RELATIONSHIP_UNINSTALLED` (`Relationship`); `SUBSCRIPTION_CREATED` (a first plan), `SUBSCRIPTION_UPDATED` (a change of plan, one event) and `SUBSCRIPTION_CANCELED` (the plan ended, with `cancelEffectiveOn`), each with its `plan` (`SubscriptionStatus`). `shopId` needs `subjectId`, as on Shopify ("You must specify a subjectId when filtering by shopId."). |
| `app(id:) { events(types:, shopId:, …) }` | The older system, newest first, with no default window: the relationship events, and one per change of a charge, the Billing API's included (`SUBSCRIPTION_CHARGE_ACCEPTED` and `…_ACTIVATED` on an approval, `…_CANCELED`, `…_DECLINED`, `…_EXPIRED`, `ONE_TIME_CHARGE_…`), each with its `charge` (`id`, `name`, `amount`, `test`, `billingOn`). |

An uninstall records the end of the plan, then `RELATIONSHIP_UNINSTALLED`; a reinstall
`RELATIONSHIP_INSTALLED`. Each shop starts with the install of the seed, dated 2024-01-01 12:00
UTC like the seed's records: outside the default window. `DELETE /__lsf/admin` puts each shop's
history back to that one event.

Where Shopify's pages disagree, lsf follows the reference. The Shopify App Pricing page says
`activeSubscription` covers a subscription "through Shopify App Pricing or the Billing API"; the
[Active subscription](https://shopify.dev/docs/api/partner/2026-07/active-subscription) reference
says "The shop must have an active managed pricing contract for the app". So lsf answers `null`
for a subscription the Billing API made. The 2026-10 reference names the number of the path the
Partner ID, "the number in your Partner Dashboard URL", not the Dev Dashboard's organization ID;
older pages call it `{organization_id}`.

Shopify's [Historical events](https://shopify.dev/docs/api/partner/2026-07/historical-events)
example selects `state` on both `Relationship` and `SubscriptionStatus`. Their types differ
(`RelationshipState!`, `SubscriptionStatusState!`), and lsf refuses that as the GraphQL spec and
graphql-ruby do (`Field 'state' has a return_type conflict`). Alias one:
`... on Relationship { relationship: state }`.

The rest of the Partner API (`transactions`, `transaction`, `publicApiVersions`, the mutations
`appSubscriptionCancel` and `appCreditCreate`) answers an error with `extensions.code:
LSF_NOT_SIMULATED`. No `CHARGE_…`, `EARNING_…` or `CREDIT_…` event either: no money moves. lsf
does not throttle (Shopify allows four requests a second).

lsf does not simulate: the App Events API (usage meters), usage charges, private plans and their
stores, the eight-plan limit, prices in local currencies, plans that toggle between monthly and
yearly (declare two), proration and deferred billing (every change is at once, no money moves,
and a plan ended is ended at once: no `cancelAtEndOfCycle`, no `pendingUpdate`), freezes, the
Partner API's other queries and mutations (above), and the 180-day memory of a trial used before
(each plan's trial applies every time).

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
every token the backend got by token exchange or OAuth, and send no `app/uninstalled`. After
`DELETE /__lsf/admin`, the printed token, `shops[].token`, keeps working: the reset installs the
app again with it. The backend's session storage still holds the old token, which the Admin API
now answers `401` (its `X-Lsf-Reason` says why). Shopify's React Router package then deletes the
session, so its next request exchanges a new token: only one call fails. Code that keeps its
tokens itself fails until they are gone. So do one of these:

- empty the backend's session storage after a reset (the React Router template's: the `Session`
  table of its Prisma database);
- name the test shops anew in each run (`products-${crypto.randomUUID()}`), as above;
- end each test with `POST /__lsf/admin/uninstall {"shop"}`, then delete the shop: the backend's
  `app/uninstalled` handler deletes the sessions, as on Shopify.

### Dates, throttling and the outbox

- **Dates follow the system clock** unless the store data sets `now`: a record the backend
  creates is dated at the time it ran (`createdAt`, `updatedAt`, to the second), so two runs
  give two dates. With `now` in the store data, every record created or changed is dated then,
  and the same run gives the same answers.
- **Moving the clock freezes it.** [`POST /__lsf/admin/clock`](admin-reference.md#the-shops-clock)
  `{"advance": "31d"}` or `{"now": "…"}` moves the shop's time on (a checkout abandoned, a
  payment due, a charge expired) and leaves it there: the answer says `"frozen": true`, and every
  record after it is dated at that instant until the next move. A test that moves the clock so
  reads the same dates every run. `DELETE /__lsf/admin` puts back the store data's clock (the
  system's, or `now`).
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
    session = buyer.put(LSF_URL + "/__lsf/session", json={"cart": {"items": [{"variant": sku, "quantity": 1}]}})
    if not session.ok:  # 422: a SKU of your data?
        raise RuntimeError(session.json()["diagnostics"])
    checkout = LSF_URL + buyer.get(LSF_URL + "/checkout", allow_redirects=False).headers["location"]
    buyer.post(checkout + "/update", json={"operations": [
        {"type": "email", "value": email},
        {"type": "shippingAddress", "address": {"lastName": "Buyer", "address1": "1 Main St",
            "city": "New York", "countryCode": "US", "provinceCode": "NY", "zip": "10001"}},
        {"type": "paymentMethod", "handle": "card"},
    ]})
    completed = buyer.post(checkout + "/complete")
    if not completed.ok:  # 422: what is missing, or the Function that failed
        raise RuntimeError(completed.json()["errors"])
    return completed.json()["order"]  # id, number, name

def webhooks(**filters):  # topic=, shop=, resource=, since=, wait=, timeout=
    answer = requests.get(LSF_URL + "/__lsf/webhooks", params=filters)
    if not answer.ok:  # 408: fewer are done than `wait` asks; 422: it could only time out
        raise RuntimeError(f"{answer.status_code} {answer.text}")
    return answer.json()

since = webhooks()["last"]  # the newest delivery before the order
order = place_order()
[delivery] = webhooks(topic="orders/create", resource=order["id"], since=since, wait=1)["deliveries"]
assert delivery["state"] == "delivered"
```

The storefront does not show what the backend did to the order ([orders do not
sync](admin-reference.md#orders-fulfillments-and-draft-orders)): read it back with the Admin
API. Here, after a backend that fulfills the order before it answers its `orders/create`:

```python
token = requests.get(LSF_URL + "/__lsf/admin").json()["shops"][0]["token"]
answer = requests.post(LSF_URL + "/admin/api/2026-10/graphql.json", headers={"X-Shopify-Access-Token": token}, json={
    "query": "query($id: ID!) { order(id: $id) { displayFulfillmentStatus fulfillments { trackingInfo { number } } } }",
    "variables": {"id": order["id"]},
})
assert answer.json()["data"]["order"]["displayFulfillmentStatus"] == "FULFILLED"
```

With pytest, a fixture starts lsf once for the run, and resets it before each test ([reset
between runs](#reset-between-runs)). Start the backend the same way, on the port of
`--app-url`:

```python
# tests/conftest.py: lsf for the whole run, and a clean state for each test.
import subprocess, time, requests, pytest

LSF_URL = "http://127.0.0.1:9292"

@pytest.fixture(scope="session")
def lsf():
    server = subprocess.Popen(["npx", "local-storefront", "app", "dev", "--app", ".", "--port", "9292",
                               "--app-url", "http://localhost:3000", "--static", "--quiet"])
    for _ in range(100):
        try:
            if requests.get(LSF_URL + "/__lsf/status").ok:
                break
        except requests.ConnectionError:
            pass
        time.sleep(0.1)
    else:
        server.terminate()
        raise RuntimeError("lsf did not start: its lines above say why")
    yield LSF_URL
    server.terminate()
    server.wait()

@pytest.fixture(autouse=True)
def clean(lsf):
    for path in ("/__lsf/admin", "/__lsf/webhooks", "/__lsf/sessions"):
        requests.delete(lsf + path).raise_for_status()
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
- **While the app is uninstalled** (`POST /__lsf/admin/uninstall`, `appUninstall`): no proxy. A
  store gets the proxy with the app ("Configuring an app proxy requires the `write_app_proxy`
  access scope"; a change of `prefix` or `subpath` applies "only to new installations", and on a
  dev store means "uninstall and reinstall the app", [About app
  proxies](https://shopify.dev/docs/apps/build/online-store/app-proxies)), so uninstalling the app
  removes it, and the storefront answers the path as any path it has no page for. lsf answers the
  theme's `404` page (a `POST`, `404 Not found`), sends nothing to the backend, and says why in
  `X-LSF-Reason` and the terminal: `app proxy: the app is not installed on the shop: uninstalling
  it removed its app proxy, so /apps/reviews is the storefront's 404 and no request reaches the
  backend until it is installed again (…)`. A reinstall forwards it again. Shopify documents no
  page for this case: the 404 is what a storefront answers on a path no app proxy holds.

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
  leaves them. **A product the Admin API creates is published nowhere**, as on Shopify:
  `publishablePublish` on the Online Store's publication puts it on the storefront, and needs
  `write_publications` (and `read_publications` to find the publication with `publications`).
  An app whose `[access_scopes]` leaves them out creates products the storefront never shows:
  `productCreate` and `productSet` then say so in `extensions.lsf.notes`;
- a customer it creates can log in, at `/__lsf/login?customer=<email>` and at the checkout;
- a metafield it sets is read by Liquid (`product.metafields.custom.care`, `shop.metafields…`)
  and by the Function inputs: the `$app:config` settings of a discount, a validation, a
  customization or a cart transform; one it sets on an order is read by the customer account UI
  extensions on the order status page (`appMetafields`, `owner_type = "ORDER"`);
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
| Shop: name, email, description, policies | Domains, currency, time zone, plan |

Each family of the [reference](admin-reference.md#what-lsf-simulates) says what of it syncs.
For contributors, the full list, field by field, is in `crates/app/docs/admin/design.md` ("Store
sync").

- **Only the default shop syncs.** The shops of `POST /__lsf/admin/shops` exist to isolate tests:
  what they change stays in the Admin API.
- **The files do not change.** lsf builds the store from the files with the Admin API's changes on
  top; nothing is written to disk. `DELETE /__lsf/admin` (and the store data edited, unless
  `--static`, or `POST /__lsf/reload`) puts the storefront back to the files, and the theme back
  to the directory, without the files and script tags the Admin API added. A checkout scenario
  added or edited (`checkouts/`) is not store data: lsf reads the scenarios again and keeps what
  the Admin API changed ([when the data changes on disk](cli.md#when-the-data-changes-on-disk)).
- **A session with its own data** (`PUT /__lsf/session` with `data`) sees its data on top of the
  Admin API's changes.
- **An order placed at the checkout** is recorded in the Admin API, and the store is not built
  again for it: orders do not sync (the account pages show it from the checkout, as they do
  without the Admin API). Its number is the session's: each session numbers its orders after
  the store data's, whatever the others do, so a test reads the same number on the thank-you
  page every run ([testing](testing.md)). The Admin API records it under that number when none
  of its orders has it, else under its next one, that of `orderCreate` and draft orders: no two
  orders share a name there. Two sessions so show `#1004` on their thank-you pages while the
  Admin API names their orders `#1004` and `#1005` (`#1005` and `#1006` after an
  `orderCreate`); `complete` answers the Admin API's number. A
  guest whose email is a customer's places the order as that customer, as on Shopify: the
  customer's `numberOfOrders` and `amountSpent` count it.
- **The cart's metafields** (those the checkout UI extensions wrote) are copied onto the order,
  as Shopify does since 2026-04, when an ORDER definition has the same namespace and key with
  `cartToOrderCopyable` enabled; without one, nothing is copied. The definition comes from
  `metafieldDefinitionCreate` (`ownerType: ORDER`, `capabilities: {cartToOrderCopyable:
  {enabled: true}}`) or from `shopify.app.toml`, which lsf reads for this
  (`[order.metafields.app.gift_message]`, `type`, `capabilities.cart_to_order_copyable = true`;
  `app` is the app's `$app`). The order's metafield has the definition's type; a value the
  definition refuses (its type, its validations) is not copied. `order { metafield(namespace:
  "$app", key: "gift_message") }` reads it, and so does `appMetafields` on the order status
  page ([checkout](checkout.md#customer-account)).
- **When the store data refuses a change** (an email it does not take, two variants with the
  same options, two companies of one name, which Shopify allows), the storefront shows the rest,
  the terminal says what was refused (once), and so does `GET /__lsf/admin`
  (`storeSync.built.diagnostics`).
- **What it costs**: a request of the backend only counts the change. The next request that
  reads the store builds it again, once for a burst of mutations: under 1 ms on the demo store,
  about 0.1 s for 3,000 products of 12 variants. It builds on the files as the server read them
  at start (or at the last reload), never on the files on disk: with `--static`, a data file
  edited since is not seen until a reload (`POST /__lsf/reload`). Pages served without a change
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
is left (`crates/app/docs/admin/design.md`, "Add a version").

## Reference

The [Admin API reference](admin-reference.md) has the rest:

- [what the answers say](admin-reference.md#what-the-answers-say): errors, `extensions.cost`,
  `extensions.lsf`;
- [what lsf simulates](admin-reference.md#what-lsf-simulates), family by family: products,
  orders and fulfillments, delivery, customers, subscriptions, metafields, markets, point of
  sale, billing, discounts and bulk operations, themes, the app's installation;
- [the shop's clock](admin-reference.md#the-shops-clock) and [the
  outbox](admin-reference.md#the-outbox).
