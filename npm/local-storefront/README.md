# local-storefront

Run Shopify themes and Shopify apps on your machine, without Shopify. From your theme, your app
and JSON store data, local-storefront serves the storefront, the checkout of your app and a local
GraphQL Admin API. There is no development store, no network and no rate limit, and each test
gets exactly the catalog, cart and customer it asks for.

```bash
npm install --save-dev local-storefront
```

```bash
npx local-storefront serve
```

Write `npx local-storefront`, not `npx lsf`: an unrelated package named `lsf` exists on npm.
Inside the `scripts` of `package.json`, `local-storefront serve` is enough.

| Command | What it does |
|---|---|
| `npx local-storefront serve` | Serves the theme at <http://127.0.0.1:9292>. |
| `npx local-storefront validate` | Checks the store data and the checkout scenarios, with or without a theme. |
| `npx local-storefront check --fix` | Fixes what Shopify's strict Liquid parser rejects. |
| `npx local-storefront fmt` | Formats Liquid files as Prettier does. |
| `npx local-storefront profile <path>` | Shows what makes a page slow. |
| `npx local-storefront app dev --app <dir>` | Serves a Shopify app: its checkout, where its Functions run and its UI extensions show, and the Admin API of its backend. |
| `npx local-storefront app check --app <dir>` | Runs the app's checkout scenarios and checks what each must look like, for CI. |

With Playwright:

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';

export default defineConfig({
  // A port of its own, so that a local run never reuses your dev server.
  use: { baseURL: 'http://127.0.0.1:9393' },
  webServer: {
    command: 'npx local-storefront serve --port 9393 --static --quiet --strict',
    url: 'http://127.0.0.1:9393/__lsf/status',
    reuseExistingServer: !process.env.CI,
  },
});
```

This package holds a launcher and the docs. The local-storefront binary comes from a platform
package that npm installs alongside it: macOS, Linux and Windows, on x64 and arm64.

The docs of this version are in the package: `node_modules/local-storefront/docs/`, listed by
`node_modules/local-storefront/llms.txt`. Online: <https://github.com/0xtlt/local-storefront>
