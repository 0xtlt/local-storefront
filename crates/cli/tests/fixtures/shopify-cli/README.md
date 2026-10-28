# Builds with Shopify's CLI

`app/` is a Shopify app as Shopify's CLI takes it, with the checkout playground's extensions
written in JavaScript and TypeScript: two Functions (`discount-typescript`,
`validation-javascript`) and two checkout UI extensions built from their sources (`ui-preact`,
`ui-preact-typescript`).

`expected/` holds what Shopify's CLI 5.0.0 writes for them on its own, run from the root of the
same app (`shopify app function build`, `shopify app build`): the modules of the Functions and
the bundles of the checkout UI extensions.

`lsf_s_builds_are_shopify_s_cli_s_to_the_byte` (`crates/cli/src/shopify_cli.rs`) copies the app,
installs the dependencies of each extension, builds it with lsf (`lsf function build`, then
`lsf checkout build`), and compares what it wrote with `expected/`, byte for byte. It needs Node,
npm and the network, so it is not part of `mise run test`:

```bash
mise run shopify-cli:test
```

The packages of the checkout UI extensions are ranges (`preact ^10.10.x`): a new release of one
changes Shopify's bundle as much as lsf's, and `expected/` is then rebuilt with the CLI.
