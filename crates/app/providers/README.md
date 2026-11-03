# The providers

The Wasm modules a Function's module imports, which lsf links when it runs it
(`src/runner/host.rs`), as Shopify does: Shopify's Javy plugins (`shopify_functions_javy_v1` to
`v4`, and the older `javy_quickjs_provider_v1` to `v3`: QuickJS) and the providers of the Function
Wasm API (`shopify_function_v1`, `v2`). lsf also compiles JavaScript to QuickJS bytecode with
`shopify_functions_javy_v4` when it builds a Function (`src/javascript/javy.rs`).

They are the `providers/` of Shopify's function-runner v9.2.2 (Apache-2.0), to the byte:
`providers.txt` lists their names, sizes and SHA-256, in the order `providers.br` holds them, one
after the other, in one Brotli stream (16 MB window). The 9.7 MB of modules, mostly the same
QuickJS, take 1.6 MB. The one `shopify_functions_javy_v4` Shopify's CLI downloads
(`https://cdn.shopify.com/shopifycloud/shopify-functions-javy-plugin/shopify_functions_javy_v4.wasm`)
has the same code; only its `git_hash` custom section differs.

To update them, change the tag in `tools/checkout/providers.sh` and run:

```bash
mise run checkout:providers
```
