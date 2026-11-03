# npm packages lsf builds with

The packages a Function's entry and a Preact checkout UI extension import, at the versions
Shopify's templates install, for the apps that have no `node_modules`: lsf embeds them
(`src/javascript/packages.rs`) and resolves them after the app's own `node_modules`.

| Package | Version | License |
|---|---|---|
| `@shopify/shopify_function` | 2.0.1 | Apache-2.0 |
| `preact` | 10.29.8 | MIT |
| `@preact/signals` | 2.11.4 | MIT |
| `@preact/signals-core` | 1.14.5 | MIT |
| `@shopify/ui-extensions` | 2026.10.0 | MIT |

Only the files those entries reach are kept, with each package's `package.json` and license. To
change a version, edit `tools/checkout/javascript_packages.mjs`, then `mise run checkout:packages`.
