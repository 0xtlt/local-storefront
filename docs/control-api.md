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
| `POST /__lsf/admin/uninstall` | Uninstalls the app from a shop (`{"shop"?}`, the default one without): its tokens stop working, `app/uninstalled` is delivered. | `{ "ok": true, "shop", "revoked": 1 }` |
| `POST /__lsf/admin/install` | Installs the app again (`{"shop"?, "scopes"?}`): its default token works again, with the app's scopes or those given (a list, or `"read_products,read_orders"`); `app/scopes_update` is delivered when they changed. | `{ "ok": true, "shop", "token", "scopes", "scopesChanged" }` |
| `POST /__lsf/admin/session-token` | A session token (App Bridge's `id_token`) of a shop's user, `{"shop"?, "user"?}`, signed with the client secret: for a test to call the backend's authenticated routes. | `{ "token", "authorization", "shop", "claims" }` |
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
| `data` | Store data for this session only, applied over the files. See [testing](testing.md#data-for-one-test-only). |
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
| `apiVersion`, `graphql` | `2026-10`, and the path of the GraphQL Admin API. |
| `defaultShop` | The shop of the store data (`shop.permanent_domain`). |
| `app` | `clientId`, `appUrl` (where webhooks and redirects go), `scopes` (what the app is installed with). |
| `throttle` | `enabled`, `maximumAvailable`, `restoreRate`. |
| `webhooks` | `summary`, `appSubscriptions` (those of `shopify.app.toml`), `deliveries` (how many the log has). |
| `mutations` | The mutations lsf simulates. Another answers `LSF_NOT_SIMULATED`. |
| `shops` | Per shop: `shop`, `installed`, `scopes`, `token` (the default one), `changed` (whether the Admin API changed it), `records`, `counts` (records by type), `tokens` (every valid token: `mode`, `source`, `scopes`, `expiresAt`, `user`), `subscriptions` (those the backend made). |

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
