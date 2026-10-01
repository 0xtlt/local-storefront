# Architecture

Three crates, each depending only on the one before it.

```text
crates/liquid   slt-liquid   the Liquid language: no knowledge of Shopify
crates/core     slt-core     Shopify on top of it: store data, objects, filters, tags, rendering
crates/cli      slt          the command line and the HTTP server
```

## `slt-liquid`: the language

A port of Shopify's `liquid` gem, kept faithful to it down to the error messages. Nothing in
it is specific to a storefront.

| Module | Role |
|---|---|
| `tokenizer`, `lexer`, `lax` | Source → tokens. `lexer` is the strict expression parser, `lax` the regex-like scanners the gem falls back to; a tag is parsed strictly first and leniently if that fails, as the gem does in its default mode. |
| `parser`, `template`, `tags/` | Tokens → a tree of `Tag`s. Each tag implements `render` and says whether it is `blank`. |
| `expr`, `variable`, `condition` | Expressions, filter chains, and the comparison rules of `if`/`case`. |
| `value`, `number`, `time` | The value model and Ruby's arithmetic, string conversion and `strftime`. |
| `context` | Scopes, counters, registers, the globals object, and the partial loader `render`/`include` use. |
| `environment` | The registry of tags and filters. Embedders add their own. |
| `filters/` | The standard filters. |

Values that are not plain data implement the `Object` trait (`get`, `index`, `items`, `size`,
`render`, `to_json`, ...): this is how the Shopify layer exposes lazy objects ("drops").

Tests: `tests/golden.rs` renders `tests/cases/*.txt` and compares with `tests/golden/*.json`,
which `tools/oracle/generate_golden.rb` produces by running the same templates through the
real gem.

## `slt-core`: Shopify

| Module | Role |
|---|---|
| `store/model` | The input format, as serde types. Their doc comments are the user documentation: the JSON Schema and the field reference are generated from them. |
| `store/validate` | JSON Schema validation, turned into diagnostics with a code, a JSON pointer and a hint. |
| `store/load`, `store/build` | Read and merge the data files, then resolve them into a `Store`: defaults, ids, handles, references between entities, with semantic diagnostics. |
| `theme/` | Access to a theme directory: cached and revalidated files, parsed Liquid, section schemas, JSON templates, locales. |
| `drops/` | Shopify's Liquid objects, backed by the `Store`, the session and the request. Properties are computed on demand. |
| `filters/`, `tags/` | Shopify's filters and tags (`section`, `sections`, `content_for`, `form`, `paginate`, `render` with blocks, ...). |
| `render/` | URL → template (`routes`), settings resolution, sections and blocks, layouts, the Section Rendering API, the compiled stylesheet and script bundles. |
| `images`, `fonts`, `urls` | Image transformations and placeholders, the blank fallback font, local CDN URLs. |
| `site` | What one render sees: theme, store, request, session, clock. |

A render is a pure function of `(theme, store, request, session, now)`: `Renderer::render`
returns the body, the status, the template name and the Liquid errors. It does no I/O beyond
reading theme files, which is what makes it fast and reproducible.

Tests: unit tests next to the code, `tests/render.rs` (a fixture theme rendered against the
demo store, with HTML snapshots), `tests/conformance.rs` (the examples of shopify.dev,
downloaded by `mise run docs:fetch`), `tests/docs.rs` (the generated reference is current).

## `slt`: commands and server

`commands/` holds one file per subcommand. `server/` is the HTTP side:

| Module | Role |
|---|---|
| `mod` | State (the loaded store, sessions, caches), the request/response types handlers work with, reloading when files change. Handlers are plain functions from a request to a reply, so they are tested without a network (`tests.rs`). |
| `storefront` | Pages, the Section Rendering API, product, search and recommendation JSON. |
| `cart`, `forms` | The Cart Ajax API and form submissions. |
| `cdn` | Theme assets, files, image transformations, bundles, fonts. |
| `control` | The `/__slt` API. |

Rendering is CPU-bound and synchronous; the async server hands each request to a blocking
thread with a large stack, because deeply nested snippets and blocks recurse.

## Development

```bash
mise install            # Rust, and Ruby for the oracle
mise run check          # format check, clippy, tests
mise run horizon:serve  # clone Shopify's Horizon theme and serve it
mise run oracle:install # install the reference liquid gem
mise run oracle:golden  # regenerate the golden expectations from it
mise run docs:fetch     # download shopify.dev's Liquid reference for the conformance test
```

After changing the data model, regenerate the field reference with
`UPDATE_SNAPSHOTS=1 cargo test -p slt-core --test docs`; after an intentional change of the
rendered HTML, regenerate the snapshots with `UPDATE_SNAPSHOTS=1 cargo test -p slt-core --test render`.
