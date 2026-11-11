# Documentation

local-storefront runs a Shopify theme on your machine, without Shopify, from JSON store data.

## Get started

In your theme folder:

```bash
npm install --save-dev local-storefront
npx local-storefront serve        # http://127.0.0.1:9292, with a demo store
npx local-storefront init         # writes shopify-local/, the store data to edit
```

Write `npx local-storefront`, not `npx lsf`: `lsf` is another package on npm. Other ways to
install (mise, a binary, from source):
[the project's README](https://github.com/0xtlt/local-storefront#other-ways-to-install).

## Guides

One task each, from the first command to the details.

| Guide | For |
|---|---|
| [Data format](data-format.md) | Writing the store data: products, customers, menus, metafields. |
| [Testing](testing.md) | End-to-end tests: sessions, per-test data, slow networks, CI. |
| [Strict Liquid](strict-liquid.md) | Finding and fixing what Shopify's strict Liquid parser rejects. |
| [Formatting](formatting.md) | Formatting Liquid files as Prettier does. |
| [Performance](performance.md) | Profiling a page, in time or in what it would cost Shopify. |
| [Checkout](checkout.md) | Running the Functions and the checkout UI extensions of a Shopify app. |
| [Admin API](admin.md) | Pointing the backend of an app at the local Admin API, its OAuth and its webhooks; previewing its embedded pages in a local admin, with a local App Bridge. |
| [Storefront](storefront.md) | The storefront in a browser: accounts, password page, images, previews, the theme app extensions of an app. |
| [Agents](agents.md) | Driving local-storefront from an AI agent or a script. |

## Reference

| Reference | For |
|---|---|
| [CLI](cli.md) | Every command, option, exit code and output format. |
| [Control API](control-api.md) | The `/__lsf` endpoints, sessions and response headers. |
| [Admin API reference](admin-reference.md) | What the local Admin API simulates, family by family: queries, mutations, webhooks, and what it leaves out. |
| [Fields](data-reference.md) | Every field of the store data, generated from its JSON Schema. |
| [Compatibility](compatibility.md) | What matches Shopify, how it is checked, and the known differences. |

## Contributing

[Architecture](architecture.md): how the code is organised, how to develop and how to release.
