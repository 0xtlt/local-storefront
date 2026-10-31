# Control API

While `npx local-storefront serve` runs, tests and tools talk to it under `/__lsf`. Nothing
there is ever delayed by `--throttle`, nor are the Admin API and OAuth under `/admin` (the Admin
API has its own cost bucket). How to use it in tests: [testing](testing.md); the backend of an
app: [Admin API](admin.md).

## Endpoints

| Request | What it does | Answer |
|---|---|---|
| `PUT /__lsf/session` | Sets the state of the caller's session. The body is a [session](data-reference.md#session): `customer`, `company_location`, `cart`, `country`, `data`, `throttle`. | `200` `{ "ok": true, "session": …, "diagnostics": [] }` (the warnings of `data`, such as a field it ignores), or `422` with diagnostics |
| `GET /__lsf/session` | The state of the caller's session. | The session |
| `DELETE /__lsf/session` | Resets the session to what the data files define. | `{ "ok": true }` |
| `DELETE /__lsf/sessions` | Forgets every session, with its cart, its checkout and the order it placed: a clean server between two test runs. | `{ "ok": true, "sessions": 3 }` |
| `GET /__lsf/login?customer=<email>&return_to=<path>` | Logs the browser in as a customer, then redirects to `return_to` (default `/account`). Also `default` or `none`. A link a person can click. The [checkout](checkout.md#log-in-as-a-customer) of the session follows. | A redirect |
| `GET /__lsf/status` | The theme, the data and its diagnostics, the templates, the customers, every page. | JSON, below |
| `POST /__lsf/reload` | Reads the data files again. | The diagnostics of the data |
| `GET /__lsf/timings` | How long templates, sections and blocks took so far. See [performance](performance.md#many-requests). | JSON |
| `DELETE /__lsf/timings` | Forgets those times. | `{ "ok": true }` |
| `GET /__lsf/profile?path=<path>` | The profile of a page, as the session sees it. `points=1`, `html=1`, `lines=1`, `all=1`, `runs=<n>`, `section_id=<id>`, `cost=<rules>`, `format=speedscope`. See [performance](performance.md#in-the-browser). | Text, HTML or JSON |
| `GET /__lsf/checkout?session=<id>` | The state of a session's [checkout](checkout.md#test-it), when the server runs with `--app`. The caller's session without `session`. | JSON |
| `GET /__lsf/checkouts` | The home page of the checkout: the extensions of the app and its [checkout scenarios](checkout.md#scenarios). `?format=json` for what it says, `?locale=fr` for a language. With `--app`. | HTML or JSON |
| `GET /checkout?scenario=<name>` | Opens a checkout for a scenario, in a new session whose cookie it sets. With `--app`. | A redirect |
| `GET /__lsf/checkout/assets/<file>` | The files of the checkout page, which are in the binary. With `--app`. | The file |
| `GET /__lsf/extensions/<handle>/<file>` | A file of the build (`dist/`) of a checkout UI extension. With `--app`. | The file |
| `GET /__lsf/schema/<kind>` | The JSON Schema of a kind of data file: `store`, `product`, `collection`, `page`, `blog`, `customer`, `menu`, `session`. | JSON Schema |
| `GET /__lsf` | The status as a page for a browser, with a customer chooser and links to every page and its profiles. | HTML |
| `GET /__lsf/admin` | The [Admin API](admin.md): its shops, their tokens, record counts and webhook subscriptions, the app, the throttle, the mutations lsf simulates. | JSON, [below](#admin-api) |
| `DELETE /__lsf/admin` | Puts every shop of the Admin API back to the store data, forgets the subscriptions the backend made and the tokens OAuth issued, and installs the app again (the printed token works). | `{ "ok": true, "shops": [...] }` |
| `POST /__lsf/admin/shops` | A new shop for one test, seeded from the same data, with the app installed: `{"shop": "test-1"}` makes `test-1.myshopify.com`. | `201` `{ "shop", "token", "graphql" }`; `409` when it exists, `422` for a bad name |
| `GET /__lsf/admin/clock?shop=`, `POST /__lsf/admin/clock` | A shop's clock (`{shop, now, frozen}`), and moving it: `{"advance": "31d"}` (seconds, or `s`, `m`, `h`, `d`) or `{"now": "<RFC 3339>"}` (`at` too), with `"shop"?`. The clock then stays at that instant, and what waits on time happens, with its webhooks: a checkout left ten minutes is abandoned, a data erasure ten days old is processed (`customers/redact`), payment schedules that came due fire `payment_schedules/due`, store credit expires, Shopify Payments pays out the days left behind, the app's charges pending for two days expire, scheduled listings whose date passed are listed, abandoned checkouts three months old are deleted (`checkouts/delete`). The same runs before every Admin API request, by the shop's clock, frozen or not. `DELETE /__lsf/admin` puts the clock back. | `{ "ok", "shop", "now", "frozen", "erased", "deletedCheckouts" }`; `422` for a bad body or a time past the year 9999 |
| `POST /__lsf/admin/uninstall` | Uninstalls the app from a shop (`{"shop"?}`, the default one without), as Shopify does: its tokens stop working, `app/uninstalled` is delivered, the subscriptions the backend made on the shop are deleted, and nothing more is delivered for the shop until the app is installed again. An app already uninstalled delivers nothing. | `{ "ok": true, "shop", "revoked": 1, "deletedSubscriptions": 2 }` |
| `POST /__lsf/admin/install` | Installs the app again (`{"shop"?, "scopes"?}`): its default token works again, with the app's scopes or those given (a list, or `"read_products,read_orders"`), and the subscriptions of `shopify.app.toml` deliver for the shop again (OAuth and a token exchange install it too); `app/scopes_update` is delivered when they changed. | `{ "ok": true, "shop", "token", "scopes", "scopesChanged" }` |
| `GET /__lsf/admin/outbox?shop=&channel=`, `DELETE …` | The [outbox](admin.md#the-outbox): what Shopify would have sent to the outside world, which lsf records instead (emails, shipping labels, Flow triggers, POS messages, data erasures), oldest first, the last 1000; of one channel (`email`, `carrier`, `flow`, `pos`, `data_erasure`) or all. `DELETE` forgets them; a reset empties it. | `{ "shop", "count", "outbox": [{ "id", "at", "channel", "to", "what", "about", "subject"?, "body"?, "data"? }] }`; `DELETE`: `{ "ok": true, "shop", "outbox": 3 }`; `422` for another channel |
| `POST /__lsf/admin/session-token` | A session token (App Bridge's `id_token`) of a shop's user, `{"shop"?, "user"?}`, signed with the client secret: for a test to call the backend's authenticated routes. | `{ "token", "authorization", "shop", "claims" }` |
| `GET /__lsf/admin/charges/<n>?shop=` | The page where the merchant approves a charge of the app (a subscription, a new capped amount, a one-time purchase): what the `confirmationUrl` of the billing mutations opens. `?format=json` for what it shows. | HTML or JSON |
| `POST /__lsf/admin/charges/<n>/approve`, `…/decline` | The merchant's decision (`?shop=` or `{"shop"}`): the charge becomes `ACTIVE` or `DECLINED`, its webhook is delivered. From the page's form, a `303` to the charge's `returnUrl` with `charge_id`, as Shopify redirects. | `{ "ok": true, "charge", "redirect" }`; `409` when it is not waiting for the merchant |
| `POST /__lsf/admin/uploads/<shop>`, `PUT /__lsf/admin/uploads/<shop>/<key>` | Where clients upload to the targets of `stagedUploadsCreate`: the form of a `POST` target (multipart, the `file` last), or the body of a `PUT` one. `GET …/<key>` gives the file back. | `201` (XML, as Google Cloud Storage answers), `200` |
| `GET /__lsf/admin/bulk/<shop>/<n>.jsonl` | The result of a bulk operation: its `url`. | JSON Lines |
| `POST /__lsf/admin/card-sessions` | lsf's card vault, as Shopify's (`https://deposit.shopifycs.com/sessions`): `{"credit_card": {"number", "first_name", "last_name", "month", "year", "verification_value"}}`. The id is the `sessionId` of `customerPaymentMethodCreditCardCreate` and `…Update`; the number says what charging the card does (the [test cards](admin.md#test-cards)). | `201` `{ "id" }`; `422` for what is not a card |
| `GET /__lsf/admin/subscriptions?shop=` | What the Subscriptions API did not send nor charge for real: its emails (`customerPaymentMethodSendUpdateEmail`, `paymentInstrumentSendAddEmail`, as in the outbox, the link in `data.url`), and every billing attempt with its outcome, its order and what it charged. | `{ "shop", "emails": [...], "charges": [...] }` |
| `GET /__lsf/admin/billing-attempts/<n>/challenge?shop=` | The 3D Secure page of a challenged billing attempt: its `nextActionUrl`. Opening it makes the challenge `ON_SESSION_CHALLENGED`. `?format=json` for what it says. | HTML or JSON |
| `POST /__lsf/admin/billing-attempts/<n>/challenge/complete`, `…/fail` | The customer passed (or failed) the challenge (`?shop=`): the attempt bills its order (`subscription_billing_attempts/success`) or fails with `AUTHENTICATION_FAILED` (`/failure`). | `{ "attempt", "state", "order", "errorCode" }`; `409` when it is not challenged |
| `GET`, `POST /__lsf/admin/payment-methods/<token>/update?shop=` | The page of a payment method's update link (`customerPaymentMethodGetUpdateUrl`, the update email): a new card, `{"credit_card": {…}}` or the page's form, replaces the method's; `customer_payment_methods/update` is delivered. | HTML or `{ "ok": true, "customerPaymentMethod" }` |
| `GET /__lsf/admin/emails?shop=`, `DELETE …` | The outbox's emails: `/__lsf/admin/outbox?channel=email`. | `{ "shop", "count", "emails": [...] }`; `DELETE`: `{ "ok": true, "shop", "emails": 2 }` |
| `POST /__lsf/admin/disputes`, `POST /__lsf/admin/disputes/<n>/won`, `…/lost`, `…/accepted` | A buyer's bank opens a dispute on an order (`{"order", "amount"?, "type"?: "CHARGEBACK" or "INQUIRY", "reason"?}`: `disputes/create`), then decides it (`disputes/update`; won, a chargeback's amount comes back to the balance). | `201` `{ "ok": true, "shop", "dispute" }`, `200`; `422` for a bad order, `409` for a dispute decided already |
| `POST /__lsf/admin/orders/protect`, `POST /__lsf/admin/orders/link-request` | Shopify Protect changes its view of an order (`{"order", "status"?, "eligibility"?}`: `orders/shopify_protect_eligibility_changed`); the buyer asks for a new link to the order's status page (`{"order"}`: the email in the outbox, `orders/link_requested`). | `{ "ok": true, "shop", "order" }`; `422` otherwise |
| `POST /__lsf/admin/tax-app/activate` | The merchant turns on the tax app that `taxAppConfigure` made `READY`: `ACTIVE`, `tax_services/update`. | `{ "ok": true, "shop", "state" }`; `409` when it is not ready |
| `GET /__lsf/admin/launch?shop=&path=` | Redirects to the app URL as the admin opens an embedded app: `embedded`, `hmac`, `host`, `id_token`, `locale`, `session`, `shop`, `timestamp`. | A redirect |
| `GET /__lsf/webhooks` | The webhook deliveries, oldest first: `?topic=orders/create`, `?shop=`, `?state=delivered\|failed\|pending\|not_sent`; `?wait=<n>` waits until n of them are done (`?timeout=<ms>`, 10 s by default; `408` when they are not). | `{ "count", "deliveries": [...] }` |
| `DELETE /__lsf/webhooks` | Forgets the deliveries. | `{ "ok": true, "deliveries": 3 }` |
| `POST /__lsf/webhooks/trigger` | Sends one webhook: `{"topic", "id"?, "shop"?, "address"?, "payload"?, "wait"?}`. The payload is built from the record `id` names, else a representative one of the topic, else `payload` as given; it goes to `address`, else to the subscriptions of the topic. | `{ "count", "deliveries": [...] }`; `404` without a record or a subscription, `422` for a topic lsf does not build |

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
| `routes` | Every page the data creates, as `npx local-storefront routes` lists them. |

A test runner can wait on this URL to know the server is up.

## Admin API

`GET /__lsf/admin`:

| Key | Value |
|---|---|
| `apiVersion`, `graphql` | The version the server names (`2026-10`, unless `--admin-versions` leaves it out), and the path of its GraphQL Admin API. |
| `apiVersions` | Every version lsf knows: `version`, `status` (`stable`, `release candidate`), `served`, `graphql`. |
| `defaultShop` | The shop of the store data (`shop.permanent_domain`). |
| `app` | `clientId`, `appUrl` (where webhooks and redirects go), `scopes` (what the app is installed with). |
| `throttle` | `enabled`, `maximumAvailable`, `restoreRate`. |
| `webhooks` | `summary`, `appSubscriptions` (those of `shopify.app.toml`), `deliveries` (how many the log has). |
| `mutations` | The mutations lsf simulates. Another answers `LSF_NOT_SIMULATED`. |
| `storeSync` | [What the storefront sees](admin.md#what-the-storefront-and-the-checkout-see) of the default shop's changes: `enabled`, `generation` (moves when the overlay does; `0` is the files), `pending` (a change waits for the next page), `overlay` (`written`: entities by section, `removed`: their keys; `null` while the storefront is the files), `built` (`generation`, `ms`, `diagnostics`: what the store data refused). |
| `shops` | Per shop: `shop`, `installed`, `scopes`, `token` (the default one), `changed` (whether the Admin API changed it), `records`, `counts` (records by type), `tokens` (every valid token: `mode`, `source`, `scopes`, `expiresAt`, `user`), `subscriptions` (those the backend made), `outbox` (the shop's [outbox](admin.md#the-outbox), as `GET /__lsf/admin/outbox` lists it). |

A delivery of `GET /__lsf/webhooks`: `id`, `webhookId`, `eventId`, `topic`, `shop`,
`subscription`, `url`, `apiVersion`, `triggeredAt`, `request` (`headers`, `body`: exactly what was
signed), `attempts` (`status`, `durationMs`, `response`, `error`), `state` and `reason`, `notes`.

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
