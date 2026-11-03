# Typegen fixtures

What graphql-codegen writes (`generated/api.ts`, the `typescript` and `typescript-operations`
plugins, the config of Shopify's CLI) for the schemas and queries of Functions, to compare with
`crates/checkout/src/javascript/typegen.rs` (the test is in `typegen/tests.rs`).

- `cases.json`: the cases. A case is a schema (`schema`: the name of a Function API of
  `crates/checkout/schemas/2026-10`, or of a file of `schemas/` here), the documents of the
  Function (`documents`, files of `documents/`; each is `src/<its name>` in the Function), and
  what changes in its `codegen` section of `package.json` (`codegen`; by default the one of the
  Shopify templates: both plugins, `omitOperationSuffix`). `"full": true` keeps the whole output.
- `expected.json`: the SHA-256 of the output of graphql-codegen for each case, or its error. The
  whole output of the cases with `"full": true` is in `expected/`.
- `schemas/`, `documents/`: the inputs. The Function APIs have one case each (`api-*`, queries of
  `examples/checkout-playground`), the queries on a schema made for the tests (`features-*`) cover
  what graphql-codegen does with fragments, unions, interfaces, aliases, `@include`, variables,
  names; `config-*` the settings; `invalid-*` what graphql-codegen refuses.

## Regenerate `expected.json`

The oracle is the graphql-codegen that `@shopify/shopify_function@2.0.1` brings to a Function
(`@graphql-codegen/cli` 5.0.6, `typescript` 4.1.6, `typescript-operations` 4.6.1,
`visitor-plugin-common` 5.8.0, `graphql` 16.14.2; the script checks the versions). Install it
once, anywhere:

```sh
mkdir /tmp/typegen-oracle && cd /tmp/typegen-oracle
npm install --save-exact @shopify/shopify_function@2.0.1
```

Then, from this folder (`crates/checkout/tests/fixtures/typegen`):

```sh
node regenerate.mjs /tmp/typegen-oracle            # every case
node regenerate.mjs /tmp/typegen-oracle api-discount config-enums-as-const   # some of them
```

For each case it copies a Function (its `package.json`, the schema, `src/*.graphql`) to a
temporary folder and runs what Shopify's CLI runs there, with the config the CLI writes
(`{ ...J0, ...codegen.config, scalars: { ...J0.scalars, ...codegen.config.scalars } }`, `J0` being
`defaultScalarType: "unknown"` and the scalars of the Function APIs):

```sh
npm exec -- graphql-code-generator --config <tmp>/codegen.json
```

To add a case: a line in `cases.json` (and its documents), then regenerate that case.

`LSF_TYPEGEN_DUMP=<folder> cargo test -p lsf-checkout typegen` writes what the typegen makes for
every case to `<folder>/<case>.ts`, to compare with `expected/` (or with the output of the oracle
of one case) when the SHA-256 of a case differs.

## Random documents

`fuzz.mjs` writes random valid documents on a schema and what graphql-codegen makes of them, and
the ignored test reads them:

```sh
node fuzz.mjs /tmp/typegen-oracle ../../../schemas/2026-10/discount.graphql /tmp/fuzz-discount 300 1
LSF_TYPEGEN_CORPUS=/tmp/fuzz-discount cargo test -p lsf-checkout typegen -- --ignored
```

Run it on each schema when graphql-codegen or the schemas change.
