# local-storefront

A local storefront for Shopify themes. Its command, `lsf`, serves a theme from your machine,
rendered from JSON fixtures instead of the Shopify API: no network, no rate limit, and each
test gets exactly the catalog, cart and customer it asks for.

```bash
npm install --save-dev local-storefront
```

```bash
npx lsf serve
```

With Playwright:

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';

export default defineConfig({
  use: { baseURL: 'http://127.0.0.1:9292' },
  webServer: {
    command: 'npx lsf serve --port 9292 --static --quiet --strict',
    url: 'http://127.0.0.1:9292/__lsf/status',
    reuseExistingServer: !process.env.CI,
  },
});
```

This package only contains a launcher. The `lsf` binary comes from a platform package that
npm installs alongside it: macOS, Linux and Windows, on x64 and arm64.

Documentation: <https://github.com/0xtlt/local-storefront>
