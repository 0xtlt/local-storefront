# Quickstart: an Admin API backend

From the backend of a Shopify app to a local GraphQL Admin API (2026-10) it queries, webhooks it
receives, and a test that runs in CI. No development store, no tunnel. This page needs no theme
and no React: a sync job, a worker or any server works the same way.

## 1. Start lsf for the app

lsf reads the app's `shopify.app.toml`: the client id, the scopes the token gets, and the
webhook subscriptions.

```toml
client_id = "0123456789abcdef0123456789abcdef"
handle = "my-backend"
application_url = "https://my-app.trycloudflare.com"

[access_scopes]
scopes = "read_products,write_products,read_orders"

[webhooks]
api_version = "2026-10"

  [[webhooks.subscriptions]]
  topics = ["products/create", "app/uninstalled"]
  uri = "/webhooks"
```

The token gets these scopes and no others, as on Shopify: with `read_orders` alone, an order
reads, but its `customer` (`read_customers`), its returns (`read_returns`) and its fulfillment
orders (`read_merchant_managed_fulfillment_orders`) answer `ACCESS_DENIED`, which names the scope
to add. Add the scopes your backend reads, then restart lsf or call `POST /__lsf/admin/install`.

It signs webhooks with the client secret: `SHOPIFY_API_SECRET` of the environment or of the
app's `.env`, where `shopify app env pull` writes it. In the app's folder:

```bash
npm install --save-dev local-storefront
npx local-storefront serve --app . --only admin --app-url http://localhost:3000
```

```text
app:    . (no extensions: the Admin API, webhooks and the app proxy)
data:   built-in demo store (the `init` command writes an editable copy)
serving: admin
admin:  http://127.0.0.1:9292/admin/api/2026-10/graphql.json
        X-Shopify-Access-Token: shpat_…   (shop: local-supply-co.myshopify.com)
        client secret: from SHOPIFY_API_SECRET of ./.env
        GraphiQL: http://127.0.0.1:9292/__lsf/graphiql   (queries in the browser, the token filled in)
webhooks: 2 subscriptions of shopify.app.toml → http://localhost:3000
embedded: http://127.0.0.1:9292/admin/apps/my-backend   (the app in lsf's admin; …)
ready:  http://127.0.0.1:9292/   (status and control API: http://127.0.0.1:9292/__lsf)
```

`--only admin` serves the Admin API alone. `--app-url` is where your backend listens: webhooks
go there, and nowhere off this machine. `app dev --app . --app-url …` serves the app's checkout
too.

What the Admin API creates lives in memory: a restart of lsf puts every shop back to the store
data. The tokens survive it: the backend's stored sessions keep working.

## 2. Query it

The token is printed, and is the same every run for this app: a test can hard-code it.
`GET /__lsf/admin` gives it too, as `shops[0].token`.

```bash
curl -s http://127.0.0.1:9292/admin/api/2026-10/graphql.json \
  -H 'Content-Type: application/json' -H "X-Shopify-Access-Token: $TOKEN" \
  -d '{"query": "{ shop { name } }"}'
```

With `@shopify/admin-api-client`, which always builds an `https://` URL, rewrite the URL in its
fetch. The same code talks to Shopify when `LSF_URL` is not set:

```js
// query.mjs
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

const { data } = await client.request(`{ products(first: 3) { nodes { title } } }`);
console.log(data.products.nodes);
```

```bash
LSF_URL=http://127.0.0.1:9292 SHOPIFY_ACCESS_TOKEN=shpat_… node query.mjs
```

<http://127.0.0.1:9292/__lsf/graphiql> is a query editor with the token and the schema's docs,
offline. Ruby, Python and the React Router package: [point the backend at
lsf](admin.md#point-the-backend-at-lsf).

## 3. Receive webhooks

A mutation sends its webhooks to `--app-url`, signed as Shopify signs them. Check the HMAC on the
raw bytes of the body:

```js
// receiver.mjs: node --env-file=.env receiver.mjs
import { createServer } from "node:http";
import { createHmac, timingSafeEqual } from "node:crypto";

createServer(async (req, res) => {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const body = Buffer.concat(chunks);
  const digest = createHmac("sha256", process.env.SHOPIFY_API_SECRET).update(body).digest();
  const header = Buffer.from(req.headers["x-shopify-hmac-sha256"] ?? "", "base64");
  if (header.length !== digest.length || !timingSafeEqual(header, digest)) {
    return res.writeHead(401).end();
  }
  console.log(req.headers["x-shopify-topic"], JSON.parse(body).id);
  res.writeHead(200).end();
}).listen(Number(process.env.PORT ?? 3000));
```

A `productCreate` now prints `products/create 8397442660771`. To send one without a mutation:

```bash
npx local-storefront app webhook trigger --topic app/uninstalled --app-url http://localhost:3000
```

`GET /__lsf/webhooks` lists every delivery, with what the backend answered. A `401` there names
the secret lsf signed with.

## 4. Write a test

Reset the shop before each test, read `last` before the action, and wait for the webhook it sends:

```js
// test/products.test.mjs: node --test
import { test, beforeEach } from "node:test";
import assert from "node:assert/strict";

const LSF = process.env.LSF_URL ?? "http://127.0.0.1:9292";
const lsf = (method, path, body) =>
  fetch(LSF + path, { method, body: body && JSON.stringify(body) }).then((r) => r.json());

beforeEach(() => lsf("DELETE", "/__lsf/admin")); // every shop back to the store data

test("productCreate sends products/create", async () => {
  const { shops } = await lsf("GET", "/__lsf/admin");
  const { last } = await lsf("GET", "/__lsf/webhooks"); // the newest delivery so far

  const answer = await fetch(`${LSF}/admin/api/2026-10/graphql.json`, {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-Shopify-Access-Token": shops[0].token },
    body: JSON.stringify({ query: `mutation { productCreate(product: {title: "Mug"}) { product { id } } }` }),
  }).then((r) => r.json());
  const id = answer.data.productCreate.product.id;

  const { deliveries } = await lsf("GET", `/__lsf/webhooks?topic=products/create&since=${last}&wait=1`);
  assert.equal(deliveries[0].resource, id);
  assert.equal(deliveries[0].state, "delivered"); // the receiver checked the HMAC
});
```

`DELETE /__lsf/admin` puts every shop back to the store data: records, clock, outbox and tokens.
It revokes the tokens the backend got by token exchange or OAuth; the printed one keeps working.
Tests that run in parallel each take a shop of their own (`POST /__lsf/admin/shops`):
[a shop per test](admin.md#a-shop-per-test).

## 5. Move the clock

```bash
curl -X POST http://127.0.0.1:9292/__lsf/admin/clock -d '{"advance": "31d"}'
```

The shop's time moves on and stays there: payment schedules come due, checkouts are abandoned,
app subscriptions renew, each with its webhooks. `DELETE /__lsf/admin` puts the clock back.

## 6. Run it in CI

```yaml
# .github/workflows/backend.yml
name: backend
on: [push, pull_request]
jobs:
  backend:
    runs-on: ubuntu-latest
    env:
      SHOPIFY_API_SECRET: test-secret # lsf signs with it, the receiver checks with it
    steps:
      - uses: actions/checkout@v7
      - uses: actions/setup-node@v7
        with:
          node-version: 24
          cache: npm
      - run: npm ci
      - run: |
          npx local-storefront serve --app . --only admin --app-url http://localhost:3000 --static --quiet &
          node receiver.mjs &
          until curl -sf http://127.0.0.1:9292/__lsf/status > /dev/null; do sleep 0.2; done
          node --test
```

## Next

- [Admin API](admin.md): the libraries, tokens and OAuth, webhooks, a shop per test, orders from
  the checkout, billing, the app proxy, API versions.
- [Admin API reference](admin-reference.md): what each mutation does, and what lsf leaves out.
- [Control API](control-api.md#apps-checkout-and-admin-api): every `/__lsf/admin` endpoint.
- [Quickstart: a public app](quickstart-app.md), for the React Router template.
