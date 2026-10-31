# Admin backend: a Shopify app backend run end to end against lsf

The backend of a Shopify app, written with Shopify's own libraries, and a script that runs it
against lsf's [Admin API](../../docs/admin.md): it starts lsf and the backend, plays the merchant
and the buyer, checks every answer, prints a table, and stops both. Nothing here talks to
Shopify, and nothing is deployed.

```bash
cargo build -p lsf                       # once, and after changing lsf
cd examples/admin-backend
npm install
npm run e2e
```

Or `mise run admin:e2e` from the root of the repository, which does the three. A run takes about
2 seconds and exits with `1` when a check fails.

```text
result    check                                                                       detail
--------  --------------------------------------------------------------------------  ----------
PASS      auth: embedded launch → authenticate.admin (token exchange)                 home page rendered "Local Supply Co.", offline session stored
PASS      auth: OAuth authorize → callback → access token                             token for local-supply-co.myshopify.com, …
…
KNOWN     billing: billing.require from a page (exit-iframe)                          the template's exit-iframe page only redirects to https admin URLs, …

36 passed, 0 failed, 1 known gaps
```

`E2E_VERBOSE=1` prints the logs of lsf and the backend, `E2E_ONLY=proxy` runs the checks whose
name has `proxy`, `LSF_BIN=…` runs another lsf binary.

## What is in it

| Path | What it is |
|---|---|
| `shopify.app.toml` | The app: client id, scopes, `[webhooks]` subscriptions (relative and absolute `uri`s, the compliance topics), `[app_proxy]`. |
| `.env` | `SHOPIFY_API_SECRET`, the client secret, where `shopify app dev` writes it: lsf reads it there, and so does the backend. A local secret, for this app that is never deployed. |
| `server/` | The backend. `shopify.mjs` makes the Shopify objects as the React Router template's `app/shopify.server.ts` does; `index.mjs` has the routes; `api.mjs` the Admin API calls, family by family; `lsf.mjs` points the libraries at lsf. |
| `e2e/run.mjs` | The run: the checks, in order, and the table. `e2e/browser.mjs` keeps cookies and says it is Chrome. |
| `theme/` | A theme of five templates, for the checks of the storefront: what the Admin API changed, on the product page; the app proxy's Liquid inside `layout/theme.liquid`. |
| `extensions/discount-tag-percentage` | A discount Function (a copy of the checkout playground's, module included), which the backend configures with `discountAutomaticAppCreate`. |

The backend runs the React Router package (`@shopify/shopify-app-react-router`) without React
Router: a plain `node:http` server hands each request, as a Web `Request`, to the library
(`authenticate.admin`, `authenticate.webhook`, `authenticate.public.appProxy`, `billing`), as a
loader does, and answers the `Response` it returns or throws. The library is the same; there is
no Vite build to wait for. The authorization code grant, which the React Router package does not
have, is done with `@shopify/shopify-api` (`auth.begin`, `auth.callback`), and the bad-token and
throttling checks use `@shopify/admin-api-client` directly.

## What it checks

- **Auth**: the admin's launch of the embedded app (`/__lsf/admin/launch`) → `authenticate.admin`
  → token exchange → `admin.graphql`; OAuth (authorize → callback → token); `decodeSessionToken`
  + `tokenExchange` for an online token, and an expiring offline one refreshed with
  `refreshToken`; a bad session token (`401` and the retry header); a revoked access token (the
  library drops the session, the next request exchanges again).
- **Errors**: a bad token with `@shopify/admin-api-client` (`401`); `extensions.cost`;
  `MAX_COST_EXCEEDED` and `THROTTLED` on a shop of its own; fewer scopes after
  `POST /__lsf/admin/install` → `app/scopes_update` and `ACCESS_DENIED`.
- **Catalog**: products read with pagination; `productCreate`, `productVariantsBulkUpdate`,
  `productUpdate`, `inventorySetQuantities` at the primary location, `publishablePublish`; then
  the product page and `/products/<handle>.js` of the storefront; `productDelete`.
- **Customers and content**: `customerCreate`, `metafieldsSet`, `metaobjectDefinitionCreate`,
  `metaobjectUpsert` (`$app:review`); the customer logs in on the storefront and the app proxy's
  Liquid reads their metafield.
- **Discounts and orders**: `discountAutomaticAppCreate` for the Function; an order of the new
  product at the local checkout, with the Function's 20 % off; `fulfillmentCreate` and
  `refundCreate` (from `suggestedRefund`) on it; an imported order (`orderCreate`) fulfilled and
  refunded, and the stock it took on the product page.
- **Billing**: `billing.request` → the approval page → Approve → the return URL →
  `billing.check`.
- **Bulk operations and files**: `bulkOperationRunQuery` → poll → the JSONL; a staged JSONL →
  `bulkOperationRunMutation`; `stagedUploadsCreate` → multipart `POST` → `fileCreate` → the bytes
  back.
- **Webhooks**: those of `shopify.app.toml`, of `registerWebhooks` (in `afterAuth`, as the
  template does) and of `webhookSubscriptionCreate`, each verified by `authenticate.webhook`; a
  forged HMAC (`401`); `lsf app webhook trigger` for the three compliance topics;
  `POST /__lsf/webhooks/trigger`; uninstall and install again.
- **App proxy**: `/apps/reviews/*` through the storefront, verified by
  `authenticate.public.appProxy`, answered with `liquid(…)` in the theme, the logged-in customer;
  a form `POST` through the proxy, then its redirect; a forged signature (`400`).

## Run the backend by hand

```bash
../../target/debug/lsf app dev --app . --app-url http://localhost:3000     # or `serve --theme theme --app .` for the storefront
LSF_URL=http://127.0.0.1:9292 npm start
open http://127.0.0.1:9292/__lsf/admin/launch                              # the app's home, through token exchange
```

The backend reads its client id and scopes from `shopify.app.toml` and its secret from `.env`, as
lsf does; `SHOPIFY_API_KEY`, `SCOPES`, `SHOPIFY_APP_URL` and `PORT` (what `shopify app dev` sets)
win over them. Without `LSF_URL`, the same code talks to Shopify.

## What a developer has to know

What it took to point a backend written with these libraries at lsf, also in
[`docs/admin.md`](../../docs/admin.md#point-the-backend-at-lsf):

1. **The fetch function** of `@shopify/shopify-api` (`setAbstractFetchFunc`) is replaced in the
   body of `server/shopify.mjs`, after the imports: `@shopify/shopify-app-react-router/server`
   sets it when it is imported, and undoes an override made before.
2. **OAuth's browser redirect** goes to `https://<shop>/admin/oauth/authorize`: the
   `/oauth/install` route sends it to lsf when `LSF_URL` is set.
3. **Session tokens last a minute**: the run mints one for each request of the front end.
4. **The library answers `410 Gone` to bots** (`isbot`): curl's and Node's user agents included.
   The run says it is Chrome.
5. **Billing from a page** does not work (below): the backend requests it from a route the front
   end fetches.

## What does not work yet

Shown as `KNOWN` in the table; a known gap that starts to pass is reported, to be removed from
the list in `e2e/run.mjs`.

- **`billing.require` / `billing.request` in the loader of a page.** The template sends the
  browser to its exit-iframe page, which only redirects to the admin over `https`
  (`admin.shopify.com`, `<shop>.myshopify.com/admin`, `isTrustedExitIframeDestination` of
  4.0.0); lsf's charge page is `http://127.0.0.1:9292/__lsf/admin/charges/<n>`, refused with
  "Invalid exit-iframe destination. Refusing to redirect." From a fetched route it works.

## Library versions

`@shopify/shopify-app-react-router` 4.0.0, `@shopify/shopify-api` 15.0.1,
`@shopify/admin-api-client` 2.0.1, `@shopify/shopify-app-session-storage-memory` 8.0.1,
`react-router` 7.18.5 (the package's peer: `^7.6.2`), Node 26.
