# Quickstart: a public app (React Router template)

Shopify's [React Router app template](https://github.com/Shopify/shopify-app-template-react-router)
run end to end on your machine: the embedded app in a local admin, its webhooks and Events,
billing, a theme app extension in a theme, the app proxy, and a Playwright test. No development
store, no tunnel, no Partner account.

## 1. Set up the template

```bash
git clone https://github.com/Shopify/shopify-app-template-react-router my-app   # or `shopify app init`
cd my-app
npm install
npm run setup                                   # the Prisma session storage
npm install --save-dev local-storefront
```

lsf and the backend both read the app's `.env`. `shopify app env pull` writes it; by hand:

```ini
SHOPIFY_API_KEY=0123456789abcdef0123456789abcdef
SHOPIFY_API_SECRET=local-secret
SCOPES=write_products
```

`shopify app config link` fills `client_id`, `name` and `handle` in `shopify.app.toml`. Without
it, add `name = "My app"` and `handle = "my-app"` there: the admin's URL and the plan page use
the handle.

## 2. Make the one edit

The library sends the Admin API and the token exchange to `https://<shop>`. Send them to lsf, in
the body of `app/shopify.server.ts`, after the imports:

```ts
import { setAbstractFetchFunc } from "@shopify/shopify-api/runtime";

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
```

Without `LSF_URL`, the app talks to Shopify as before. Until the edit is there, lsf says so at
start and in its admin. Nothing else changes: lsf's admin gives the pages its App Bridge and
Polaris.

## 3. Run lsf and the backend

```bash
npx local-storefront app dev --app . --app-url http://localhost:3000
LSF_URL=http://127.0.0.1:9292 SHOPIFY_APP_URL=http://localhost:3000 PORT=3000 npx react-router dev
```

Not `npm run dev`: that is `shopify app dev`, which talks to Shopify. Open the `embedded:` URL
lsf prints, <http://127.0.0.1:9292/admin/apps/my-app>. The app opens in a local admin, with its
navigation and title bar. Click **Generate a product**: the admin shows "Product created".

## 4. Webhooks and Events

The template's `[events]` subscription runs on the price change of **Generate a product**: the
backend prints `Received PRODUCT Event for local-supply-co.myshopify.com`. The webhooks of
`shopify.app.toml` go to `--app-url`, signed with `SHOPIFY_API_SECRET`:

```bash
curl -X POST http://127.0.0.1:9292/__lsf/admin/uninstall   # app/uninstalled: the template deletes its sessions
curl -X POST http://127.0.0.1:9292/__lsf/admin/install
curl http://127.0.0.1:9292/__lsf/webhooks                  # every delivery, and what the backend answered
```

## 5. Billing

With managed pricing, declare the plans of the Partner Dashboard in a file:

```toml
# pricing.toml
[[plans]]
handle = "free"
name = "Free"
price = 0

[[plans]]
handle = "pro"
name = "Pro"
price = 9
trial_days = 7
welcome_link = "/app"
```

Start lsf with `--pricing-plans pricing.toml`, and send the merchant to the plan page as on
Shopify:

```ts
// app/routes/app.plans.tsx
import type { LoaderFunctionArgs } from "react-router";
import { authenticate } from "../shopify.server";

export const loader = async ({ request }: LoaderFunctionArgs) => {
  const { redirect } = await authenticate.admin(request);
  return redirect("shopify://admin/charges/my-app/pricing_plans", { target: "_top" });
};
```

`/admin/apps/my-app/app/plans` lands on lsf's plan page. **Choose Pro**, then **Approve**: the
shop gets an active `Pro` subscription, and the browser goes to `/app` with `charge_id` and
`plan_handle`. A test skips the page: `POST /__lsf/admin/pricing-plans/pro/select`. The Billing
API (`billing.request`): [approve a charge](admin.md#approve-a-charge).

## 6. A theme app extension and the app proxy

An app block is a file of a theme app extension: `extensions/rating/shopify.extension.toml` with
`name = "Rating"` and `type = "theme"`, and `extensions/rating/blocks/rating.liquid`:

```liquid
<p class="my-app-rating">{{ block.settings.heading }} ★★★★☆</p>

{% schema %}
{
  "name": "Rating",
  "target": "section",
  "settings": [{ "type": "text", "id": "heading", "label": "Heading", "default": "Rated" }]
}
{% endschema %}
```

A theme names it in a JSON template, as the theme editor writes it. In Horizon's
`templates/product.json`, add this section (`_blocks` is Horizon's section that takes app
blocks) and put `"rating"` in `order`:

```json
"rating": {
  "type": "_blocks",
  "blocks": {
    "rating_1": { "type": "shopify://apps/my-app/blocks/rating/0", "settings": { "heading": "Loved by customers" } }
  },
  "block_order": ["rating_1"]
}
```

For the app proxy, add to `shopify.app.toml`, and a route that answers Liquid:

```toml
[app_proxy]
url = "/proxy"
prefix = "apps"
subpath = "my-app"
```

```ts
// app/routes/proxy.$.tsx
import type { LoaderFunctionArgs } from "react-router";
import { authenticate } from "../shopify.server";

export const loader = async ({ request }: LoaderFunctionArgs) => {
  const { liquid } = await authenticate.public.appProxy(request);
  return liquid("<h1>Hello from my app</h1><p>{{ shop.name }}</p>");
};
```

Serve the theme with the app, in place of `app dev`:

```bash
npx local-storefront serve --theme ../horizon --app . --app-url http://localhost:3000
```

A product page shows the block, and <http://127.0.0.1:9292/apps/my-app> shows the proxy's answer
in the theme's layout. In the theme's folder, `npx local-storefront check --app ../my-app`
checks the app blocks the theme names, and `app build` checks the extension as Shopify does.

## 7. A Playwright test

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: 'e2e',
  workers: 1, // one lsf, one shop: the tests change it in turn
  webServer: [
    {
      command: 'npx local-storefront app dev --app . --port 9393 --app-url http://localhost:3001 --polaris offline --static --quiet',
      url: 'http://127.0.0.1:9393/__lsf/status',
    },
    {
      command: 'npx prisma migrate deploy && npx react-router dev',
      url: 'http://localhost:3001/',
      env: { PORT: '3001', SHOPIFY_APP_URL: 'http://localhost:3001', LSF_URL: 'http://127.0.0.1:9393' },
    },
  ],
});
```

```ts
// e2e/app.spec.ts
import { test, expect } from '@playwright/test';

const LSF = 'http://127.0.0.1:9393';

test('the app generates a product', async ({ page, request }) => {
  const { last } = await (await request.get(`${LSF}/__lsf/webhooks`)).json();

  await page.goto(`${LSF}/admin/apps/my-app/app`);
  await expect(page.frameLocator('iframe').getByText('Get started with products')).toBeVisible();
  await page.getByRole('button', { name: 'Generate a product' }).click(); // the admin's title bar
  await expect(page.getByText('Product created')).toBeVisible(); // the admin's toast

  // The price change sends the template's Product Event to webhooks.products.update.tsx.
  const events = await request.get(`${LSF}/__lsf/webhooks?topic=Product&since=${last}&wait=1`);
  const { deliveries } = await events.json();
  expect(deliveries[0].state).toBe('delivered');
});
```

`--polaris offline` serves Polaris from lsf: no request leaves the machine. To run the test next
to your dev server, give the template's HMR port (`64999` in `vite.config.ts`) a variable
([another port](admin.md#javascript-the-react-router-app-and-shopifyshopify-api)).

## Next

- [Admin API](admin.md): the embedded admin, App Bridge, tokens, webhooks, billing, managed
  pricing and the Partner API, the app proxy.
- [Storefront](storefront.md#theme-app-extensions): app embed blocks, app-data metafields, why a
  block renders nothing.
- [Quickstart: Functions and checkout UI extensions](quickstart-functions.md), for the app's
  extensions.
- [`examples/admin-backend`](https://github.com/0xtlt/local-storefront/tree/main/examples/admin-backend):
  a backend with Shopify's libraries, run against lsf end to end.
