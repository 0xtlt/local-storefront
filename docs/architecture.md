# Architecture

Three crates, each depending only on the one before it.

```text
crates/liquid   lsf-liquid   the Liquid language: no knowledge of Shopify
crates/core     lsf-core     Shopify on top of it: store data, objects, filters, tags, rendering
crates/cli      lsf          the command line and the HTTP server
```

## `lsf-liquid`: the language

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
real gem. `tests/liquid_spec.rs` replays Shopify's
[liquid-spec](https://github.com/Shopify/liquid-spec) suite, downloaded by
`mise run liquid-spec:fetch`: it reports the specs that pass, differ and do not apply for each
suite, and fails when fewer pass than its recorded baselines.

## `lsf-core`: Shopify

| Module | Role |
|---|---|
| `store/model` | The input format, as serde types. Their doc comments are the user documentation: the JSON Schema and the field reference are generated from them. |
| `store/validate` | JSON Schema validation, turned into diagnostics with a code, a JSON pointer and a hint. |
| `store/load`, `store/build` | Read and merge the data files, then resolve them into a `Store`: defaults, ids, handles, references between entities, with semantic diagnostics. |
| `theme/` | Access to a theme directory: cached and revalidated files, parsed Liquid, section schemas, JSON templates, locales. |
| `drops/` | Shopify's Liquid objects, backed by the `Store`, the session and the request. Properties are computed on demand. |
| `filters/`, `tags/` | Shopify's filters and tags (`section`, `sections`, `content_for`, `form`, `paginate`, `render` with blocks, ...). |
| `render/` | URL → template (`routes`), settings resolution, sections and blocks, layouts, the Section Rendering API, the compiled stylesheet and script bundles, and what Shopify injects into pages (`platform`). |
| `assets/platform/` | The JavaScript counterparts of Shopify's own scripts (`Shopify.actions`, `Shopify.loadFeatures`), embedded in the binary and served from the local CDN. Tested with Node's test runner (`mise run test:js`). |
| `images`, `fonts`, `urls` | Image transformations, the format an image is sent in (JPEG, PNG, WebP or AVIF, by what the client accepts) and placeholders, the blank fallback font, local CDN URLs. |
| `site` | What one render sees: theme, store, request, session, clock. |

A render is a pure function of `(theme, store, request, session, now)`: `Renderer::render`
returns the body, the status, the template name and the Liquid errors. It does no I/O beyond
reading theme files, which is what makes it fast and reproducible.

Tests: unit tests next to the code, `tests/render.rs` (a fixture theme rendered against the
demo store, with HTML snapshots), `tests/conformance.rs` (the examples of shopify.dev,
downloaded by `mise run docs:fetch`), `tests/docs.rs` (the generated reference is current).

## `lsf`: commands and server

`commands/` holds one file per subcommand. `server/` is the HTTP side:

| Module | Role |
|---|---|
| `mod` | State (the loaded store, sessions, caches), the request/response types handlers work with, reloading when files change. Handlers are plain functions from a request to a reply, so they are tested without a network (`tests.rs`). |
| `storefront` | Pages, the Section Rendering API, product, search and recommendation JSON. |
| `cart`, `forms` | The Cart Ajax API and form submissions. |
| `account` | The page that stands in for the customer accounts Shopify hosts, and the B2B location switch. |
| `cdn` | Theme assets, files, images and the variants of them kept in memory, bundles, fonts. |
| `minify` | The stylesheets and the scripts of the theme, minified with a source map. |
| `compress` | Brotli or gzip for the responses that gain from it, by what the client accepts. |
| `control` | The `/__lsf` API. |
| `live_reload` | `--live-reload`: the WebSocket open pages listen on, and the task that looks at the theme and data directories while a page listens. The script it injects is `assets/live-reload.js`, tested with Node (`mise run test:js`). |

Rendering is CPU-bound and synchronous; the async server hands each request to a blocking
thread with a large stack, because deeply nested snippets and blocks recurse.

## Development

```bash
mise install               # Rust, and Ruby for the oracle
mise run check             # format check, clippy, tests
mise run horizon:serve     # clone Shopify's Horizon theme and serve it
mise run oracle:install    # install the reference liquid gem
mise run oracle:golden     # regenerate the golden expectations from it
mise run docs:fetch        # download shopify.dev's Liquid reference for the conformance test
mise run liquid-spec:fetch # download Shopify's liquid-spec suite, which the engine's tests replay
```

## Releasing

Set the version in `Cargo.toml` (`workspace.package.version`), commit, then push a tag with
the same version:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

`.github/workflows/release.yml` runs the tests, builds `lsf` for macOS, Linux (glibc and
static musl) and Windows on x86-64 and ARM64, and publishes a GitHub release with one archive
per platform and a `SHA256SUMS` file. A tag that does not match the version of the crate fails
before anything is built. Tags with a suffix (`v0.2.0-rc.1`) are published as prereleases.

To try the builds without releasing, run the workflow by hand from the Actions tab: the
archives are then attached to the run instead of a release.

### npm

`npm/build.mjs` assembles the npm packages from the archives of a release:
`local-storefront`, a launcher (`npm/local-storefront/`), and one package per platform
holding the binary (`local-storefront-<os>-<cpu>`), which the launcher lists as optional
dependencies so that npm installs only the one that matches the machine. Linux gets the
static musl build, which runs on every distribution.

`.github/workflows/npm.yml` publishes them. No token is stored: on npmjs.com each package
names this repository and `npm.yml` as its *trusted publisher*, and npm recognises the
workflow run. When the repository variable `NPM_PUBLISH` is `true`, `release.yml` starts
`npm.yml` once the GitHub release is published. It can also be run by hand, for a release
made before publishing was enabled or after a publication that failed:

```bash
gh workflow run npm.yml -f tag=v0.2.0
```

npm can only trust a workflow for a package that already exists. The first publication is
therefore made from a machine, once, and again whenever a package is added (a new platform):

```bash
npm login
```

```bash
mise run npm:first-publish v0.2.0
```

The script publishes the packages of that release that are not on npm yet, then registers
`npm.yml` as their trusted publisher. It needs two-factor authentication on the npm account.

Every run of `release.yml`, including one started by hand, assembles the packages, installs
them the way a project would and runs `lsf` through `npx`, without publishing.

The Linux targets are cross-compiled with `cargo-zigbuild`, which is what lets the glibc
builds target glibc 2.17 whatever the runner has. The Rust version comes from `mise.toml`.

After changing the data model, regenerate the field reference with
`UPDATE_SNAPSHOTS=1 cargo test -p lsf-core --test docs`; after an intentional change of the
rendered HTML, regenerate the snapshots with `UPDATE_SNAPSHOTS=1 cargo test -p lsf-core --test render`.
