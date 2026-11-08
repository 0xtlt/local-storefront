# Control API

While `npx local-storefront serve` (or `app dev`) runs, tests and tools talk to it under `/__lsf`. Nothing
there is ever delayed by `--throttle`, nor are the Admin API and OAuth under `/admin` (the Admin
API has its own cost bucket). How to use it in tests: [testing](testing.md); the backend of an
app: [Admin API](admin.md).

Bodies are JSON: send `Content-Type: application/json`. A body that is JSON is read as JSON
without it too (`fetch` sends a string as `text/plain`, `curl -d` as a form).

## Endpoints

### Storefront and tests

| Request | What it does | Answer |
|---|---|---|
| `PUT /__lsf/session` | Sets the state of the caller's session. The body is a [session](data-reference.md#session): `customer`, `company_location`, `cart`, `country`, `data`, `throttle`. | `200` `{ "ok": true, "session": …, "diagnostics": [] }` (the warnings of `data`, such as a field it ignores), or `422` with diagnostics |
| `GET /__lsf/session` | The state of the caller's session. | The session |
| `DELETE /__lsf/session` | Resets the session to what the data files define. | `{ "ok": true }` |
| `DELETE /__lsf/sessions` | Forgets every session, with its cart, its checkout and the order it placed: a clean server between two test runs. | `{ "ok": true, "sessions": 3 }` |
| `GET /__lsf/login?customer=<email>&return_to=<path>` | Logs the browser in as a customer, then redirects to `return_to` (default `/account`). Also `default` or `none`. A link a person can click. The [checkout](checkout.md#log-in-as-a-customer) of the session follows. | A redirect |
| `GET /__lsf/status` | The theme, the data and its diagnostics, the templates, the customers, every page. | JSON, below |
| `POST /__lsf/reload` | Reads the data files again, and seeds the shops of the Admin API again from them whatever changed (without `--static`, a change on disk is read by itself, and a change of checkout scenarios alone keeps the shops: [when the data changes on disk](cli.md#when-the-data-changes-on-disk)). | The diagnostics of the data |
| `GET /__lsf/timings` | How long templates, sections and blocks took so far. See [performance](performance.md#many-requests). | JSON |
| `DELETE /__lsf/timings` | Forgets those times. | `{ "ok": true }` |
| `GET /__lsf/profile?path=<path>` | The profile of a page, as the session sees it. `points=1`, `html=1`, `lines=1`, `all=1`, `runs=<n>`, `section_id=<id>`, `cost=<rules>`, `format=speedscope`. See [performance](performance.md#in-the-browser). | Text, HTML or JSON |
| `GET /__lsf/schema/<kind>` | The JSON Schema of a kind of data file: `store`, `product`, `collection`, `page`, `blog`, `customer`, `menu`, `session`. | JSON Schema |
| `GET /__lsf` | The status as a page for a browser, with a customer chooser and links to every page and its profiles. | HTML |

### Apps: checkout and Admin API

The checkout needs `--app`; the Admin API runs on every server, with the app of `--app` when
there is one ([Admin API](admin.md)). First what a backend's tests use, then the rest. How a test
uses them: [test your backend](admin.md#test-your-backend).

| Request | What it does | Answer |
|---|---|---|
| `GET /__lsf/checkout?session=<id>` | The state of a session's [checkout](checkout.md#test-it), when the server runs with `--app`. The caller's session without `session`. Once the order is placed (`POST /checkouts/cn/<token>/complete`, which answers `{redirect, order: {id, number, name}}`), `status` is `completed` and `order` has its `id` (the `admin_graphql_api_id` of its `orders/create`), `number`, `processedAt`, `totalReceived`, `totalOutstanding`. **Here `number` is a string, `"#1004"`**: the session's number, as the thank-you page shows it. In the answer of `complete`, `number` is an integer, `1004`, and `name` is the string, `"#1004"`: the Admin API's, which can differ (`1005` when the shop has a `#1004` already). To match the two, use `id`, which both have. The state's `number` is a string since 0.2.0, and stays one. | JSON |
| `GET /__lsf/checkouts` | The home page of the checkout: the extensions of the app and its [checkout scenarios](checkout.md#scenarios). `?format=json` for what it says, `?locale=fr` for a language. With `--app`. | HTML or JSON |
| `GET /checkout?scenario=<name>` | Opens a checkout for a scenario, in a new session whose cookie it sets. With `--app`. | A redirect |
| `GET /__lsf/checkout/assets/<file>` | The files of the checkout page, which are in the binary. With `--app`. | The file |
| `GET /__lsf/extensions/<handle>/<file>` | A file of the build (`dist/`) of a checkout UI extension. With `--app`. | The file |
| `GET /__lsf/admin` | The [Admin API](admin.md): its shops, their tokens, record counts and webhook subscriptions, the app, the throttle, the mutations, each shop's outbox. | JSON, [below](#admin-api) |
| `DELETE /__lsf/admin` | Puts every shop of the Admin API back to the store data (its clock and its outbox too), forgets the subscriptions the backend made and the tokens OAuth issued, and installs the app again (the printed token works). It revokes every token of the app, those of token exchange too, and sends no `app/uninstalled`: the backend's stored tokens are answered `401` ([reset between runs](admin.md#reset-between-runs)). | `{ "ok": true, "shops": [...] }` |
| `POST /__lsf/admin/shops` | A new shop for one test, seeded from the same data, with the app installed: `{"shop": "test-1"}` makes `test-1.myshopify.com`. Each is a copy of the seed (about 2 MB for the demo store), kept until it is deleted: a reset keeps it. | `201` `{ "shop", "token", "graphql" }`; `409` when it exists, `422` for a bad name |
| `DELETE /__lsf/admin/shops/<shop>` | Deletes a shop `POST /__lsf/admin/shops` made (`test-1` or `test-1.myshopify.com`) and frees what it held: its records, its tokens and installation, the subscriptions the backend made on it. Its tokens, those of token exchange too, are then answered `401`, and no `app/uninstalled` is sent: `POST /__lsf/admin/uninstall` first, for the backend to hear it. The name can be taken again. The default shop is the store data's and stays (`DELETE /__lsf/admin` puts it back). | `{ "ok": true, "shop", "revoked", "deletedSubscriptions" }`; `404` for no such shop, `409` for the default shop |
| `POST /__lsf/admin/session-token` | A session token (App Bridge's `id_token`) of a shop's user, `{"shop"?, "user"?}`, signed with the client secret: for a test to call the backend's authenticated routes. | `{ "token", "authorization", "shop", "claims" }` |
| `GET /__lsf/admin/launch?shop=&path=` | Redirects to the app URL as the admin opens an embedded app: `embedded`, `hmac`, `host`, `id_token`, `locale`, `session`, `shop`, `timestamp`. | A redirect |
| `GET /admin/apps/<app>/<path>?shop=&user=&locale=` | [lsf's admin](admin.md#preview-the-embedded-app) with the embedded app open at `<path>`, in a frame at a new launch URL (a new signed query and session token) on every load. `<app>` is the app's `handle`, or its client id. `shop`, `user` (a staff id, 1 the account owner) and `locale` are the admin's; the other parameters go to the app. `GET /admin` and `GET /admin/apps` redirect to it. An app with `embedded = false`, or without a URL, gets a page that says so. | HTML |
| `GET /admin/<other>` | A page of the admin that lsf does not have (`/admin/products/123`, where a `shopify:admin/products/123` link of the app goes): it says so, with a link back to the app. | HTML, `404` |
| `GET /__lsf/admin/app-bridge.js` | lsf's App Bridge, which the app's pages load before Shopify's to run in lsf's admin. `Access-Control-Allow-Origin: *`, `Cache-Control: no-cache`. | JavaScript |
| `GET /__lsf/cdn/<path>` | Where the app's pages load [Polaris](admin.md#polaris-without-the-internet) from: `<path>` is a path of `https://cdn.shopify.com/` (`shopifycloud/polaris-2.js`). By the Polaris mode: `online`, a redirect there, the query kept; `offline`, lsf's copy of Polaris 2.0.0 and 1.1.0 (`polaris.js`, `polaris-1.js`, `polaris-2.js` and the versioned names), its icons and its fonts, with `https://cdn.shopify.com/` in the scripts and stylesheets rewritten to lsf's `/__lsf/cdn/`. `Access-Control-Allow-Origin: *`; `HEAD` too, and `OPTIONS` preflights (`204`, the `Access-Control-Request-Headers` allowed, `Max-Age` a day). | `302` (`Cache-Control: no-store`); the file (`no-cache` for scripts and stylesheets, a year for icons and fonts); `404` in plain text for a path lsf has no copy of |
| `GET /__lsf/polaris` | The Polaris mode: `mode` (`online` or `offline`), `default` (the mode lsf started in: `--polaris`, `LSF_POLARIS`), `cdn` (`/__lsf/cdn/`), `scripts` (each copy's `version`, `path` and `aliases`), `files` (how many lsf has). | JSON |
| `PUT /__lsf/polaris` | Sets the Polaris mode, at once for every page: `{"mode": "offline"}` or `"online"` (`?mode=` too; `POST` as well). `DELETE /__lsf/polaris` puts back the mode lsf started in; `DELETE /__lsf/admin` and `DELETE /__lsf/sessions` leave it. `Access-Control-Allow-Origin: *`: lsf's App Bridge reads it from the app's page. | `{ "ok": true, "mode" }`; `422` for another mode |
| `GET /__lsf/admin/resources?shop=&type=&query=&first=&after=` | What the resource picker of lsf's admin lists: the shop's products with their variants (`type=product` or `variant`) or its collections (`type=collection`), by title, in the shape `shopify.resourcePicker` answers (`Product`, `ProductVariant`, `Collection` of `@shopify/app-bridge-types`). `query` is in Shopify's search syntax; `first` from 1 to 250 (50 by default), `after` the `endCursor` of the page before. Every scope, no cost limit: it is the admin's, not the app's. | `{ "resources", "pageInfo": { "hasNextPage", "endCursor" } }`; `400` for another type or a bad `first`, `404` for no such shop |
| `GET /__lsf/admin/assets/<file>` | The files of lsf's admin page (`admin.js`, `admin.css`), which are in the binary. | The file |
| `GET /__lsf/admin/charges/<n>?shop=` | The page where the merchant approves a charge of the app (a subscription, a new capped amount, a one-time purchase): what the `confirmationUrl` of the billing mutations opens. `?format=json` for what it shows. | HTML or JSON |
| `POST /__lsf/admin/charges/<n>/approve`, `…/decline` | The merchant's decision (`?shop=` or `{"shop"}`; charges count from 1 in each shop, and without `shop` the default shop's is meant: keep the `?shop=` of the `confirmationUrl`): the charge becomes `ACTIVE` or `DECLINED`, its webhook is delivered. From the page's form, a `303` to the charge's `returnUrl` with `charge_id`, as Shopify redirects. | `{ "ok": true, "charge", "redirect" }`; `409` when it is not waiting for the merchant |
| `GET /admin/charges/<app>/pricing_plans?shop=` | The plan selection page of [managed pricing](admin.md#managed-pricing) in lsf's admin: the declared plans, the shop's current one marked, a button to choose each (`…/pricing_plans/<plan>` is the page that approves one). Where a `shopify:admin/charges/<app>/pricing_plans` link of the app goes, and Shopify's URL of it (`https://admin.shopify.com/store/<store>/charges/<app>/pricing_plans`) opened by App Bridge. `<app>` is the app's handle or client id. Without plans, it says how to declare them. | HTML; `404` without plans, for another app or plan |
| `GET /__lsf/admin/pricing-plans?shop=` | The plans of managed pricing: `plans` (`handle`, `name`, `price`, `currency`, `interval`, `trial_days`, `free`, `features`, `welcome_link`), `legacy`, `source` (the file of `--pricing-plans`, `"request"`, or `null`), `managedPricing` (plans are declared: the Billing API creates no charge), `page`, and the shop's `current` subscription (`id`, `name`, `plan`). | JSON |
| `PUT /__lsf/admin/pricing-plans` | Declares the plans while lsf runs (`POST` too): `{"plans": [...], "legacy"?: false}`, the fields of the [`--pricing-plans` file](admin.md#managed-pricing). Every shop's; a reset (`DELETE /__lsf/admin`) keeps them. `DELETE` puts back those of `--pricing-plans`, or none. | The same JSON with `"ok": true`; `400` for a body that is not JSON, `422` with what is wrong |
| `POST /__lsf/admin/pricing-plans/<plan>/select` | The merchant chooses a plan (`?shop=` or `{"shop"}`, the default shop without): a new `ACTIVE` subscription replaces the current one (`CANCELLED`), `app_subscriptions/update` is delivered for each (unless `legacy` is `false`). From the page's form, a `303` to the plan's welcome link with `charge_id` and `plan_handle`, as Shopify redirects. | `{ "ok": true, "shop", "plan", "subscription": { "id", "chargeId", "name", "status", "test", "trialDays", "trialEndsAt", "currentPeriodEnd" }, "replaced", "redirect" }`; `404` for no plans or no such plan, `409` when the shop is on the plan already |
| `POST /__lsf/admin/pricing-plans/cancel` | The shop's plan ends as if outside the app (`?shop=` or `{"shop"}`, the default shop without): its active subscription is `CANCELLED` at once, `app_subscriptions/update` is delivered (unless `legacy` is `false`). Shopify's plan page has no such button (the merchant chooses another plan or uninstalls): a test's stand-in for a plan that ends while the app stays installed, without the app's token ([managed pricing](admin.md#managed-pricing)). | `{ "ok": true, "shop", "plan", "subscription": { "id", "chargeId", "name", "status", … }, "webhook" }` (`webhook`: the topic delivered, or `null`); `404` for no plans, `409` when the shop has no active subscription |
| `POST /__lsf/admin/uninstall` | Uninstalls the app from a shop (`{"shop"?}`, the default one without), as Shopify does: its tokens stop working, its subscription is cancelled, its Functions go with every object that runs them (its customizations, validations, cart transforms, fulfillment constraint rules and app discounts, which a reinstall does not bring back), its script tags and web pixel are deleted, the metafield definitions of its reserved namespaces are deleted (their metafields stay, and a reinstall creates the definitions again under new ids), `app/uninstalled` is delivered, the subscriptions the backend made on the shop are deleted, and nothing more is delivered for the shop until the app is installed again. While it is uninstalled, the shop's checkout runs none of its Functions, and the checkout, its thank-you page and the customer account render none of its UI extensions, each saying "the app is not installed on the shop", and its app proxy's path is the storefront's `404`; a reinstall renders them again and forwards the proxy ([uninstall](admin.md#tokens)). An app already uninstalled delivers nothing. | `{ "ok": true, "shop", "revoked": 1, "deletedSubscriptions": 2, "removedFunctionOwners": 1, "removedScriptTags": 0, "removedWebPixels": 0, "removedMetafieldDefinitions": 0 }` |
| `POST /__lsf/admin/install` | Installs the app again (`{"shop"?, "scopes"?}`): its default token works again, with the app's scopes or those given (a list, or `"read_products,read_orders"`), and the subscriptions of `shopify.app.toml` deliver for the shop again (OAuth and a token exchange install it too); `app/scopes_update` is delivered when they changed. | `{ "ok": true, "shop", "token", "scopes", "scopesChanged" }` |
| `GET /__lsf/webhooks` | The webhook deliveries, oldest first, filtered by `topic`, `shop`, `state`, `resource` and `since`; `wait=<n>` waits until n of them are done. See [webhook deliveries](#webhook-deliveries). | `{ "count", "deliveries", "last", "waited"? }`; `408` when fewer are done; `422` at once for a wait that could only time out |
| `DELETE /__lsf/webhooks` | Forgets the deliveries. Their ids are not given again. | `{ "ok": true, "deliveries": 3 }` |
| `POST /__lsf/webhooks/trigger` | Sends one webhook: `{"topic", "id"?, "shop"?, "address"?, "payload"?, "wait"?}`. The payload is built from the record `id` names, else a representative one of the topic, else `payload` as given; it goes to `address`, else to the subscriptions of the topic. Without `wait`, it answers at once, the deliveries `pending`; with `"wait": true`, once they are done (`delivered`, `failed`, `not_sent`), or after `?timeout=<ms>` (10 seconds by default) with those left still `pending`. | `{ "count", "deliveries": [...] }`, `200` either way; `404` without a record or a subscription, `422` for a topic lsf does not build |
| `GET /__lsf/admin/clock?shop=`, `POST /__lsf/admin/clock` | A shop's clock (`{shop, now, frozen}`), and moving it: `{"advance": "31d"}` (seconds, or `s`, `m`, `h`, `d`) or `{"now": "<RFC 3339>"}` (`at` too), with `"shop"?`. The clock then stays at that instant, and what waits on time happens, with its webhooks: a checkout left ten minutes is abandoned, a data erasure ten days old is processed (`customers/redact`), payment schedules that came due fire `payment_schedules/due`, store credit expires, Shopify Payments pays out the days left behind, the app's charges pending for two days expire, scheduled listings whose date passed are listed, abandoned checkouts three months old are deleted (`checkouts/delete`). The same runs before every Admin API request, by the shop's clock, frozen or not. `DELETE /__lsf/admin` puts the clock back. | `{ "ok", "shop", "now", "frozen", "erased", "deletedCheckouts" }`; `422` for a bad body or a time past the year 9999 |
| `GET /__lsf/admin/outbox?shop=&channel=`, `DELETE …` | The [outbox](admin-reference.md#the-outbox): what Shopify would have sent to the outside world, which lsf records instead (emails, shipping labels, Flow triggers, POS messages, data erasures), oldest first, the last 1000; of one channel (`email`, `carrier`, `flow`, `pos`, `data_erasure`) or all. `DELETE` forgets them; a reset empties it. | `{ "shop", "count", "outbox": [{ "id", "at", "channel", "to", "what", "about", "subject"?, "body"?, "data"? }] }`; `DELETE`: `{ "ok": true, "shop", "outbox": 3 }`; `422` for another channel |
| `GET /__lsf/admin/emails?shop=`, `DELETE …` | The outbox's emails: `/__lsf/admin/outbox?channel=email`. | `{ "shop", "count", "emails": [...] }`; `DELETE`: `{ "ok": true, "shop", "emails": 2 }` |

What Shopify, buyers and banks do outside the Admin API, and the files it serves:

| Request | What it does | Answer |
|---|---|---|
| `POST /__lsf/admin/uploads/<shop>`, `PUT /__lsf/admin/uploads/<shop>/<key>` | Where clients upload to the targets of `stagedUploadsCreate`: the form of a `POST` target (multipart, the `file` last), or the body of a `PUT` one. `GET …/<key>` gives the file back. | `201` (XML, as Google Cloud Storage answers), `200` |
| `GET /__lsf/admin/bulk/<shop>/<n>.jsonl` | The result of a bulk operation: its `url`. | JSON Lines |
| `PUT /__lsf/admin/sources?url=<url>` | [Stubs a URL](admin-reference.md#stubbed-urls): the body is what `themeCreate`, a `URL` body of `themeFilesUpsert` or `urlRedirectImportCreate` reads there from now on, never downloaded. Every shop's; a reset keeps it. Encode the URL (`encodeURIComponent`). | `{ "ok": true, "url", "size" }`; `422` for a URL that is not `http(s)`, or a staged upload of lsf |
| `GET /__lsf/admin/sources`, `GET …?url=` | The stubbed URLs, those of `--admin-source` included; with `?url=`, one's bytes. | `{ "count", "sources": [{ "url", "size", "from" }] }`, `from` the file or `"request"`; the bytes; `404` for a URL not stubbed |
| `DELETE /__lsf/admin/sources`, `DELETE …?url=` | Forgets the stubs requests made, all of them or one. Those of `--admin-source` stay. | `{ "ok": true, "removed": 2 }` |
| `POST /__lsf/admin/card-sessions` | lsf's card vault, as Shopify's (`https://deposit.shopifycs.com/sessions`): `{"credit_card": {"number", "first_name", "last_name", "month", "year", "verification_value"}}`. The id is the `sessionId` of `customerPaymentMethodCreditCardCreate` and `…Update`; the number says what charging the card does (the [test cards](admin-reference.md#test-cards)). | `201` `{ "id" }`; `422` for what is not a card |
| `GET /__lsf/admin/subscriptions?shop=` | What the Subscriptions API did not send nor charge for real: its emails (`customerPaymentMethodSendUpdateEmail`, `paymentInstrumentSendAddEmail`, as in the outbox, the link in `data.url`), and every billing attempt with its outcome, its order and what it charged. | `{ "shop", "emails": [...], "charges": [...] }` |
| `GET /__lsf/admin/billing-attempts/<n>/challenge?shop=` | The 3D Secure page of a challenged billing attempt: its `nextActionUrl`. Opening it makes the challenge `ON_SESSION_CHALLENGED`. `?format=json` for what it says. | HTML or JSON |
| `POST /__lsf/admin/billing-attempts/<n>/challenge/complete`, `…/fail` | The customer passed (or failed) the challenge (`?shop=`): the attempt bills its order (`subscription_billing_attempts/success`) or fails with `AUTHENTICATION_FAILED` (`/failure`). | `{ "attempt", "state", "order", "errorCode" }`; `409` when it is not challenged |
| `GET`, `POST /__lsf/admin/payment-methods/<token>/update?shop=` | The page of a payment method's update link (`customerPaymentMethodGetUpdateUrl`, the update email): a new card, `{"credit_card": {…}}` or the page's form, replaces the method's; `customer_payment_methods/update` is delivered. | HTML or `{ "ok": true, "customerPaymentMethod" }` |
| `POST /__lsf/admin/disputes`, `POST /__lsf/admin/disputes/<n>/won`, `…/lost`, `…/accepted` | A buyer's bank opens a dispute on an order (`{"order", "amount"?, "type"?: "CHARGEBACK" or "INQUIRY", "reason"?}`: `disputes/create`), then decides it (`disputes/update`; won, a chargeback's amount comes back to the balance). | `201` `{ "ok": true, "shop", "dispute" }`, `200`; `422` for a bad order, `409` for a dispute decided already |
| `POST /__lsf/admin/orders/protect`, `POST /__lsf/admin/orders/link-request` | Shopify Protect changes its view of an order (`{"order", "status"?, "eligibility"?}`: `orders/shopify_protect_eligibility_changed`); the buyer asks for a new link to the order's status page (`{"order"}`: the email in the outbox, `orders/link_requested`). | `{ "ok": true, "shop", "order" }`; `422` otherwise |
| `POST /__lsf/admin/tax-app/activate` | The merchant turns on the tax app that `taxAppConfigure` made `READY`: `ACTIVE`, `tax_services/update`. | `{ "ok": true, "shop", "state" }`; `409` when it is not ready |

## Sessions

A session holds a cart, the customer logged in, the country, and data of its own. It follows
the `_lsf_session` cookie, which the server sets on the first response: each browser context
gets its own. A client without cookies names its session with a header:

```bash
curl -H 'x-lsf-session: my-test' http://127.0.0.1:9292/cart.js
```

`PUT /__lsf/session` replaces the state of the session:

```json
{
  "customer": "jane.doe@example.com",
  "cart": { "items": [{ "variant": "TEE-BLK-M", "quantity": 2 }], "note": "Gift wrap" },
  "country": "FR"
}
```

| Field | Value |
|---|---|
| `customer` | An email, `"default"` (the first customer of the data) or `"none"`. |
| `company_location` | For a B2B customer, the location they buy for. |
| `cart` | `items`, `note`, `attributes`. Each item has a `variant` (a SKU, an id, or a product handle for its first variant), and optionally a `quantity`, `properties` and a `selling_plan`. See [Cart](data-reference.md#cart). |
| `country` | The country the visitor shops from, by ISO code. |
| `data` | Store data for this session only, applied over the files and over what the [Admin API](admin.md#what-the-storefront-and-the-checkout-see) changed. See [testing](testing.md#data-for-one-test-only). |
| `throttle` | Delays for this session only. See [testing](testing.md#simulating-a-slow-network). |

A wrong body answers `422`, with the diagnostics `npx local-storefront validate --format json`
gives:

```json
{
  "ok": false,
  "errors": 1,
  "warnings": 0,
  "diagnostics": [
    {
      "severity": "error",
      "code": "unknown_variant",
      "file": "(request body)",
      "path": "/cart/items/0/variant",
      "message": "\"TEE-BLK-XXL\" is neither the SKU of a variant nor the handle of a product",
      "hint": "did you mean \"TEE-BLK-XL\"?"
    }
  ]
}
```

## Status

`GET /__lsf/status`:

| Key | Value |
|---|---|
| `theme` | The theme folder. |
| `data` | Where the data comes from. |
| `data_diagnostics` | `{ ok, errors, warnings, diagnostics }`, as `npx local-storefront validate --format json`. |
| `throttle` | The server's delays. |
| `templates` | The templates of the theme. |
| `locales` | The languages of the store. |
| `customers` | Each customer's `email` and `company`. |
| `counts` | How many `products`, `collections`, `pages`, `blogs`, `customers`, `sessions`. |
| `routes` | Every page the data creates, as `npx local-storefront routes` lists them: each one renders. Not `/account`, a page of lsf's, not of the theme. |

A test runner can wait on this URL to know the server is up.

## Admin API

`GET /__lsf/admin`:

| Key | Value |
|---|---|
| `apiVersion`, `graphql` | The version the server names (`2026-10`, unless `--admin-versions` leaves it out), and the path of its GraphQL Admin API. |
| `apiVersions` | Every version lsf knows: `version`, `status` (`stable`, `release candidate`), `served`, `graphql`. |
| `defaultShop` | The shop of the store data (`shop.permanent_domain`). |
| `app` | `clientId`, `appUrl` (where webhooks and redirects go), `scopes` (what the app is installed with), `pricingPlans` (`plans`, `legacy`, `source` of [managed pricing](admin.md#managed-pricing); `null` without). |
| `throttle` | `enabled`, `maximumAvailable`, `restoreRate`. |
| `webhooks` | `summary`, `appSubscriptions` (those of `shopify.app.toml`), `deliveries` (how many the log has). |
| `mutations` | The mutations lsf simulates: every one of the versions it serves ([what simulated means](admin-reference.md#what-lsf-simulates)). |
| `storeSync` | [What the storefront sees](admin.md#what-the-storefront-and-the-checkout-see) of the default shop's changes: `enabled`, `generation` (moves when the overlay does; `0` is the files), `pending` (a change waits for the next page), `overlay` (`written`: entities by section, `removed`: their keys; `null` while the storefront is the files), `built` (`generation`, `ms`, `diagnostics`: what the store data refused). |
| `shops` | Per shop: `shop`, `installed`, `scopes`, `token` (the default one), `changed` (whether the Admin API changed it), `records`, `counts` (records by type), `tokens` (every valid token: `mode`, `source`, `scopes`, `expiresAt`, `user`), `subscriptions` (those the backend made), `outbox` (the shop's [outbox](admin-reference.md#the-outbox), as `GET /__lsf/admin/outbox` lists it). |

## Webhook deliveries

`GET /__lsf/webhooks` lists the deliveries since the last `DELETE /__lsf/webhooks`, of every
shop, oldest first. The filters combine:

| Parameter | Keeps |
|---|---|
| `topic=orders/create` | One topic. |
| `shop=test-1` | One shop, by its name (`test-1`, in any case) or its domain (`test-1.myshopify.com`). |
| `state=delivered` | One state: `delivered`, `failed`, `pending` (still being tried) or `not_sent` (lsf did not send it, and `reason` says why: a topic it does not build, an address off this machine, an EventBridge or Pub/Sub subscription). |
| `resource=<gid>` | The deliveries about one record (`gid://shopify/Order/…`), as their `resource` names it. |
| `since=<id>` | The deliveries after this one, by `id`. Not a number: `422`. |

`wait=<n>` makes the request wait until n deliveries match every filter and are done:
`delivered`, `failed` or `not_sent`. It counts what the log already holds, the deliveries from
before the request included: with no `since`, `wait=1` can answer at once with an old one. The
answer lists every delivery that matches, those still `pending` too. `timeout=<ms>` bounds the wait, 10 seconds by default; then the answer is `408`, with the same
body. A delivery that the backend fails is retried after 1, 5 and 15 seconds, and stays `pending`
until its last attempt, about 21 seconds after the first: to see it fail, give `timeout=25000`.
Its `reason` is the last failure meanwhile (`answered 503`), then says how many attempts were
made (`answered 503 (4 attempts)`). For a `401` or `403`, it adds the secret lsf signed with:
`answered 401 (4 attempts): lsf signed it with the client secret from SHOPIFY_API_SECRET of
../my-app/.env: does the backend verify with the same one?`.

A wait that could only time out answers `422` at once: no delivery of the log matches, and none
can come, the `shop` being none the Admin API has, or no subscription of the app covering the
`topic` on that shop (on any shop without `shop`), in `shopify.app.toml` or through the Admin
API. A topic nothing subscribes to is never logged.

```json
{
  "count": 1,
  "deliveries": [{ "id": 24, "topic": "orders/create", "shop": "local-supply-co.myshopify.com", "resource": "gid://shopify/Order/8987513318340", "state": "delivered", … }],
  "last": 24,
  "waited": { "for": 1, "complete": true, "timeoutMs": 10000 }
}
```

`last` is in every answer, the `408` too: the newest id of the log, `0` before the first
delivery. Ids are never given twice, even after a `DELETE`: read `last` before an action, then
`since=<last>&wait=1` waits for what the action sends only. `waited` is there with `wait`. A
delivery has `id`, `webhookId`, `eventId`, `topic`, `shop`, `resource` (the GID of the record
of the event, `null` for one about no record, as `app/uninstalled`), `subscription`, `url`,
`apiVersion`, `triggeredAt`, `request` (`headers`, `body`: exactly what was signed), `attempts`
(`status`, `durationMs`, `response`, `error`), `state` and `reason`, `notes`.

## Response headers

Pages and other responses carry headers a test can check:

| Header | Meaning |
|---|---|
| `x-lsf-template` | The template that rendered the page (`product`, `product.alternate`, `404`). |
| `x-lsf-not-found` | Why a page is the `404` page: `no product with the handle "x"`, `no page at "/x"`, the metaobject type with no template... Absent on every other response. |
| `x-lsf-liquid-errors` | The number of Liquid errors in the page. Absent when there are none. |
| `x-lsf-placeholder` | `1`: the image is a placeholder, the file is not in `shopify-local/files/`. |
| `x-lsf-throttle` | The delay that was applied, as `1000ms`. |
| `server-timing` | How long the server took. See [performance](performance.md#render-times). |
