# Admin backend: a Shopify app backend run end to end against lsf

The backend of a Shopify app, written with Shopify's own libraries, and a script that runs it
against lsf's [Admin API](../../docs/admin.md): it starts lsf and the backend, plays the merchant
and the buyer, checks every answer, prints a table, and stops both. Nothing here talks to
Shopify, and nothing is deployed.

```bash
cargo build -p lsf-cli   # once, and after changing lsf
cd examples/admin-backend
npm install
npm run e2e
```

Or `mise run admin:e2e` from the root of the repository, which does the three and installs the
headless Chromium the Polaris checks run in (`npx playwright-core install --only-shell --no-remove
chromium`, which does nothing once it is there). A run takes about 8 seconds and exits with `1`
when a check fails.

```text
result    check                                                                       detail
--------  --------------------------------------------------------------------------  ----------
PASS      auth: embedded launch → authenticate.admin (token exchange)                 home page rendered "Local Supply Co.", offline session stored
PASS      auth: OAuth authorize → callback → access token                             token for local-supply-co.myshopify.com, …
…
PASS      billing: billing.require from a page (exit-iframe)                          exit-iframe page opens Shopify's confirmation URL, which lsf's admin serves

54 passed, 0 failed, 0 known gaps
```

`E2E_VERBOSE=1` prints the logs of lsf and the backend, `E2E_ONLY=proxy` runs the checks whose
name has `proxy`, `LSF_BIN=…` runs another lsf binary. Without it, the run takes the lsf that
`cargo build` made: under `CARGO_TARGET_DIR` when it is set, else `target/debug/lsf`.

## What is in it

| Path | What it is |
|---|---|
| `shopify.app.toml` | The app: client id, scopes, `[webhooks]` subscriptions (relative and absolute `uri`s, the compliance topics), `[app_proxy]`. |
| `.env` | `SHOPIFY_API_SECRET`, the client secret, where `shopify app dev` writes it: lsf reads it there, and so does the backend. A local secret, for this app that is never deployed. |
| `server/` | The backend. `shopify.mjs` makes the Shopify objects as the React Router template's `app/shopify.server.ts` does; `index.mjs` has the routes; `api.mjs` the Admin API calls, family by family; `lsf.mjs` points the libraries at lsf; `embedded.mjs` is the embedded page, what the template's `app/routes/app.tsx` renders with `AppProvider`. |
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
- **Managed pricing**: plans set by `PUT /__lsf/admin/pricing-plans`, for these checks only (the
  Billing API above is closed while they are declared); in Chromium, the app's
  `shopify://admin/charges/admin-backend/pricing_plans` link → lsf's plan selection page → Pro →
  Approve → the welcome link with `charge_id` and `plan_handle`, which the page's loader reads;
  `currentAppInstallation.activeSubscriptions` has Pro and `app_subscriptions/update` its
  `plan_handle`. Then `/app/plans`, whose loader calls the template's
  `redirect("shopify://admin/charges/…", { target: "_top" })`: App Bridge opens
  `https://admin.shopify.com/store/…/pricing_plans`, which lsf's admin opens as its own page;
  down to Free, Pro is cancelled, and both webhooks arrive. With `legacy = false`, a plan ended
  without the app (`POST /__lsf/admin/pricing-plans/cancel`) leaves the backend no active
  subscription, and no webhook. Then the backend reads its plan from lsf's Partner API
  (`GET /api/billing/partner`: `activeSubscription` and `events` for its app and shop GIDs, read
  from the Admin API), Pro in its trial, then `null` once the plan ends, and the history
  `SUBSCRIPTION_CANCELED`, `SUBSCRIPTION_CREATED`.
- **Bulk operations and files**: `bulkOperationRunQuery` → poll → the JSONL; a staged JSONL →
  `bulkOperationRunMutation`; `stagedUploadsCreate` → multipart `POST` → `fileCreate` → the bytes
  back.
- **Webhooks**: those of `shopify.app.toml`, of `registerWebhooks` (in `afterAuth`, as the
  template does) and of `webhookSubscriptionCreate`, each verified by `authenticate.webhook`; a
  forged HMAC (`401`); `lsf app webhook trigger` for the three compliance topics;
  `POST /__lsf/webhooks/trigger`; uninstall and install again.
- **Uninstall**: the checkout runs none of the app's Functions and loads none of its extensions,
  and says why; a reinstall clears it.
- **Embedded page**: lsf prints where the app opens in its admin; `/admin/apps/admin-backend`
  opens the app in a frame at a new launch, through lsf's frame proxy; the backend sends the
  library's own `frame-ancestors` (Shopify's admin only), which the proxy drops, and the page
  loads lsf's App Bridge, Shopify's tag included; `fetch()` to `/api/settings` with App Bridge's
  session token; the resource picker's list has the new product; the library's bounce page,
  thrown past the page's headers, is framed and loads lsf's App Bridge; `shopify:admin/products`
  lands on lsf's page, which says it has none. What the page does in a browser (the title bar,
  the navigation, toasts, the picker, the save bar, the modal) is tested on lsf's App Bridge
  itself (`crates/app/assets/tests/app-bridge.test.mjs`).
- **Polaris**: the page loads Polaris from lsf's `/__lsf/cdn/shopifycloud/polaris-2.js`
  (`POLARIS_URL`, AppProvider's and `shopifyApp`'s `polarisUrl`), which redirects to Shopify's
  CDN while lsf is online; in Chromium (`playwright-core`), with every request to
  `cdn.shopify.com` refused, once `PUT /__lsf/polaris` sets lsf offline: the components are
  defined and their icons and fonts come from lsf; a page still on the CDN's Polaris gets lsf's
  copy from lsf's App Bridge; lsf and the app on `localhost`, on two ports, with no CORS error;
  `DELETE /__lsf/polaris` puts lsf back online.
- **App proxy**: `/apps/reviews/*` through the storefront, verified by
  `authenticate.public.appProxy`, answered with `liquid(…)` in the theme, the logged-in customer;
  a form `POST` through the proxy, then its redirect; a forged signature (`400`); while the app
  is uninstalled, the storefront's `404` and nothing to the backend, until a reinstall.

## Run the backend by hand

```bash
../../target/debug/lsf app dev --app . --app-url http://localhost:3000     # or `serve --theme theme --app .` for the storefront
LSF_URL=http://127.0.0.1:9292 APP_BRIDGE_URL=http://127.0.0.1:9292/__lsf/admin/app-bridge.js \
  POLARIS_URL=http://127.0.0.1:9292/__lsf/cdn/shopifycloud/polaris-2.js npm start
open http://127.0.0.1:9292/admin/apps/admin-backend                        # the embedded page, in lsf's admin
```

In lsf's admin, the page's title bar and navigation are the admin's; "Show a toast", "Pick
products" (the resource picker, with the shop's products), "About this app" (a `ui-modal`), the
app's Products page (a `fetch()` behind `authenticate.admin`) and its Settings page (a
`data-save-bar` form: change it, and the admin shows its save bar) use lsf's App Bridge. Under
**Preview**, the admin links to the checkout, and to the storefront with `serve --theme theme`.
`http://127.0.0.1:9292/__lsf/admin/launch` opens the app's page alone, through token exchange;
its App Bridge then sends it to the admin, as Shopify's does.

The backend reads its client id and scopes from `shopify.app.toml` and its secret from `.env`, as
lsf does; `SHOPIFY_API_KEY`, `SCOPES`, `SHOPIFY_APP_URL` and `PORT` (what `shopify app dev` sets)
win over them. Without `LSF_URL`, the same code talks to Shopify. The Partner API is read at
`PARTNER_API_URL` (`https://partners.shopify.com` without it) with `PARTNER_ID` and
`PARTNER_API_TOKEN`: lsf's are in `GET /__lsf/admin/partner`, and the run passes them.

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
5. **The `confirmationUrl` of a charge** is Shopify's, on the shop's admin
   (`https://<shop>/admin/charges/…`): the library's exit-iframe page accepts it, and lsf's admin
   serves its path. The run opens it on lsf's origin.
6. **The embedded page in lsf's admin** needs nothing of its own: lsf's admin opens it through a
   proxy that drops the `frame-ancestors` of `addDocumentResponseHeaders` (on every page, the
   library's thrown ones included) and puts lsf's App Bridge and Polaris in the page.
   `server/embedded.mjs` still loads lsf's App Bridge from `APP_BRIDGE_URL` first, and Polaris
   from `POLARIS_URL`, as a page opened outside lsf's frame needs.

## What does not work yet

Nothing known. A gap is shown as `KNOWN` in the table, listed in `KNOWN` of `e2e/run.mjs` with
its reason here; one that starts to pass is reported, to be removed from the list.

## Library versions

`@shopify/shopify-app-react-router` 4.0.0, `@shopify/shopify-api` 15.0.1,
`@shopify/admin-api-client` 2.0.1, `@shopify/shopify-app-session-storage-memory` 8.0.1,
`react-router` 7.18.5 (the package's peer: `^7.6.2`), Node 26.
