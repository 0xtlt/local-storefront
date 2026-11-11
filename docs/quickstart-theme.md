# Quickstart: a theme

From a theme folder to a storefront you edit live and a test that runs in CI. The examples use
Shopify's Horizon theme; any theme works the same way.

## 1. Serve the theme

In the theme folder:

```bash
npm install --save-dev local-storefront
npx local-storefront serve --live-reload
```

```text
theme:  .
data:   built-in demo store (the `init` command writes an editable copy)
serving: theme
ready:  http://127.0.0.1:9292/   (status and control API: http://127.0.0.1:9292/__lsf)
```

Open <http://127.0.0.1:9292>. With `--live-reload`, a page reloads when you save a file of the
theme or of the store data, and a small panel lists the errors of the page. Leave it off in
tests.

Write `npx local-storefront`, not `npx lsf`: `lsf` is another package on npm.

## 2. Write the store data

```bash
npx local-storefront init
```

```text
./shopify-local: wrote 16 files, kept 0 existing ones, refreshed schema/
```

`shopify-local/` is the store: `store.json`, one file per product in `products/`,
`customers.json`, `collections.json`, menus, pages and blogs. Edit them, or replace the demo's
products with yours. Then check them:

```bash
npx local-storefront validate
```

```text
./shopify-local: valid (8 products, 7 collections, 3 pages, 1 blog, 4 customers, 2 menus, 0 checkout scenarios)
```

A mistake has its file, its place and a hint, and the command exits with `1`:

```text
error[unknown_field]: unknown field "titel"
  --> products/blue-shirt.json at /titel
  hint: did you mean "title"?
```

## 3. Check and format the Liquid

```bash
npx local-storefront check          # 383 files checked, 0 problems
npx local-storefront fmt --check    # 28 files to format, 256 already formatted
```

`check` finds the Liquid Shopify's strict parser rejects (`check --fix` rewrites it), the filters
that do not exist, and the snippets, sections and translation keys the theme names but does not
have. `fmt --check` exits with `1` on a theme nobody formatted, as Horizon is: run
`npx local-storefront fmt` once, in a commit of its own.

## 4. Write a browser test

Each test gets its own cart and customer. One request puts the session in the state the test
needs.

```bash
npm install --save-dev @playwright/test
```

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';

export default defineConfig({
  fullyParallel: true,
  use: { baseURL: 'http://127.0.0.1:9393' },
  webServer: {
    command: 'npx local-storefront serve --port 9393 --static --quiet --strict',
    url: 'http://127.0.0.1:9393/__lsf/status',
    reuseExistingServer: !process.env.CI,
  },
});
```

```ts
// tests/cart.spec.ts
import { test, expect } from '@playwright/test';

test('a logged-in customer sees their cart', async ({ page }) => {
  // The demo store's customer and SKU: put your own.
  const response = await page.request.put('/__lsf/session', {
    data: {
      customer: 'jane.doe@example.com',
      cart: { items: [{ variant: 'TEE-BLK-M', quantity: 2 }] },
    },
  });
  expect(response.ok()).toBeTruthy(); // 422, with the reason, for a SKU the data does not have

  await page.goto('/cart');
  const lines = page.getByRole('main').getByRole('table');
  await expect(lines.getByRole('link', { name: 'Organic Cotton T-Shirt', exact: true })).toBeVisible();
});
```

```bash
npx playwright test
```

`--static` reads the files once, `--strict` refuses to start on store data with errors, and
port `9393` keeps the tests away from your dev server on `9292`.

## 5. Run it in CI

Commit `package.json`, `package-lock.json` and `shopify-local/`.

```yaml
# .github/workflows/storefront.yml
name: storefront
on: [push, pull_request]
jobs:
  storefront:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: actions/setup-node@v7
        with:
          node-version: 24
          cache: npm
      - run: npm ci
      - run: npx local-storefront validate
      - run: npx local-storefront check
      # - run: npx local-storefront fmt --check   # once the theme is formatted
      - name: Render every page, and fail on a Liquid error
        run: |
          npx local-storefront routes > /dev/null
          for route in $(npx local-storefront routes); do
            npx local-storefront render "$route" --strict > /dev/null ||
              { echo "::error::render failed: $route"; exit 1; }
          done
      - run: npx playwright install --with-deps chromium
      - run: npx playwright test
```

## What `shopify theme dev` does that lsf does not

- **The theme editor.** There is none: edit the JSON templates and `config/settings_data.json`
  by hand. `request.design_mode` is always `false`.
- **A real store.** The products, customers and orders are your JSON files, not a development
  store's. Nothing is uploaded, and there is no `theme push` or `theme pull`.
- **Shopify's checkout.** Without an app, `/checkout` sums up the cart.
- **Theme Check's other rules** (performance, accessibility): use `shopify theme check` for those.

## Next

- [Data format](data-format.md): products, customers, menus, metafields, markets.
- [Testing](testing.md): sessions, data for one test, slow networks.
- [Strict Liquid](strict-liquid.md), [formatting](formatting.md), [performance](performance.md).
- [Storefront](storefront.md): accounts, the password page, theme app extensions.
- [CLI reference](cli.md): every option and exit code.
