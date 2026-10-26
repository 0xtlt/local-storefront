# Architecture

Three crates, each depending only on the one before it, and two that stand beside them.

```text
crates/liquid    lsf-liquid     the Liquid language: no knowledge of Shopify
crates/core      lsf-core       Shopify on top of it: store data, objects, filters, tags, rendering
crates/checkout  lsf-checkout   the Functions and checkout UI extensions of a Shopify app, on the store data
crates/cli       lsf            the command line and the HTTP server
crates/fmt       lsf-fmt        the formatter of `lsf fmt`: a source in, a source out
```

`lsf-checkout` uses `lsf-core` for the store and the session; `lsf` uses both.

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
| `profiler` | What a render spends its time in, when it is asked to record it: every named template as a frame inside the one that rendered it, and on demand every tag and output by line. Each event also carries the points charged so far: one kind for a tag or an output, the others charged by the host through the profiler of the thread, since the objects that load things do not see the context. A render without a profiler pays one test per block body. |
| `filters/` | The standard filters. |
| `strict2` | Shopify's strict parser (`error_mode: :strict2`): the rules each tag registers with `Environment::register_syntax`, an audit the parser runs as it parses (`audit`), and the fixes of `lsf check --fix` (`apply`, `apply_intents`, `intent`). A fix that keeps the output is printed from what the engine parses and kept only when both parsers read it back the same. |

Values that are not plain data implement the `Object` trait (`get`, `index`, `items`, `size`,
`render`, `to_json`, ...): this is how the Shopify layer exposes lazy objects ("drops").

Tests: `tests/golden.rs` renders `tests/cases/*.txt` and compares with `tests/golden/*.json`,
which `tools/oracle/generate_golden.rb` produces by running the same templates through the
real gem. `tests/liquid_spec.rs` replays Shopify's
[liquid-spec](https://github.com/Shopify/liquid-spec) suite, downloaded by
`mise run liquid-spec:fetch`: it reports the specs that pass, differ and do not apply for each
suite, and fails when fewer pass than its recorded baselines. `tests/strict2_spec.rs` checks
the strict parser against the liquid-spec specs that run in `strict2` mode, and
`mise run oracle:strict2-fixes` renders the fixes with the gem.

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
| `assets/platform/` | The JavaScript counterparts of Shopify's own scripts (`Shopify.actions`, `Shopify.loadFeatures`), embedded in the binary and served from the local CDN, with Shopify's global assets (`themes_support/`, see its README). Tested with Node's test runner (`mise run test:js`). |
| `images`, `fonts`, `urls` | Image transformations, the format an image is sent in (JPEG, PNG, WebP or AVIF, by what the client accepts) and placeholders, the blank fallback font, local CDN URLs. |
| `site` | What one render sees: theme, store, request, session, clock. |

A render is a pure function of `(theme, store, request, session, now)`: `Renderer::render`
returns the body, the status, the template name and the Liquid errors. It does no I/O beyond
reading theme files, which is what makes it fast and reproducible.

Tests: unit tests next to the code, `tests/render.rs` (a fixture theme rendered against the
demo store, with HTML snapshots), `tests/conformance.rs` (the examples of shopify.dev,
downloaded by `mise run docs:fetch`; it also fails when Shopify documents a filter or a tag lsf
lacks), `tests/docs.rs` (the generated reference is current).

## `lsf-checkout`: the checkout of an app

Reads a Shopify app, runs its Functions against a cart and describes the checkout the buyer
sees. It does not serve anything: `lsf` does, with `lsf checkout`, `lsf serve --app` and
`lsf function run`.

| Module | Role |
|---|---|
| `app` | Reads `shopify.app.toml` and `extensions/*/shopify.extension.toml`: Functions, checkout UI extensions, the others listed and ignored. What is wrong with the app (a missing build, an unknown target) is a diagnostic, not an error. `Selection` is the set of extensions in use (`--extension`). |
| `input` | Builds the input of a Function: runs its GraphQL input query against the cart, the buyer and the store (a small executor over a resolver of the Function input graph). |
| `runner` | Runs a Wasm module with Shopify's own `function-runner` as a library, caches the compiled modules by path, time and size, stops a Function after 10 seconds, and reports logs, instructions and errors. |
| `pipeline` | `compute`: the Functions in the order Shopify runs them, applying each output, to a `CheckoutResult`. |
| `types` | The state the buyer builds, the configuration (`CheckoutConfig`, which `lsf` maps from the `checkout` section of the store data) and the result, which is the JSON the page reads. |
| `assets` | The page and the sandbox of its extensions, embedded in the binary: the files of `assets/`, except `tests/` and `preview/`. |

`assets/` holds the checkout page and `extension-host.js`, which runs each extension in a
sandboxed frame (`sandbox.html`; its scripts are classic scripts, since the frame has an opaque
origin and cannot load modules). They need no build step. Tests: the unit tests, `tests/` against
the fixture app (`tests/fixtures/app`: Rust Functions whose `dist/function.wasm` is committed and
two built UI extensions; `mise run fixtures:functions` rebuilds the Functions), and Node's test
runner for the scripts (`mise run test:js`).

## `lsf-fmt`: the formatter

A port of Prettier with Shopify's Liquid plugin (`@shopify/prettier-plugin-liquid`), step for
step, so that the same source gives the same output. It shares nothing with `lsf-liquid`: a
formatter needs a tree that keeps HTML, comments and where everything was written, which a
renderer does not.

| Module | Role |
|---|---|
| `tokenizer`, `parser`, `markup`, `liquid_doc` | Source → tree: Shopify's `liquid-html-parser`, in the tolerant mode the plugin runs it in. Liquid and HTML are one tree; what cannot be parsed is printed as Prettier prints it (an unclosed `{% for %}` gets its `{% endfor %}`), so a source is never refused. |
| `ast` | The tree, in an arena, with what the plugin adds to each node before printing. |
| `preprocess` | Parents and siblings, the CSS `display` of each node, and whether the whitespace around it may change without changing the page. |
| `printer/` | Tree → document: the printer of the plugin. `children` decides what separates two nodes, `tag` and `element` print HTML, `liquid` and `expression` print Liquid. |
| `doc` | The document (groups, lines that break or not, indentation) and the algorithm that lays it out in a width: Prettier's `printDocToString`. |
| `embed/json` | The JSON of `{% schema %}` and of JSON scripts, as Prettier's `json` parser prints it. |
| `embed/css` | CSS, as Prettier's PostCSS printer prints it: `parser` cuts a style sheet the way PostCSS does, `value`, `selector` and `media` are ports of the three parsers Prettier reads values, selectors and media queries with. |
| `embed/js` | JavaScript, formatted by Oxc's formatter (`oxc_formatter`, a port of Prettier's printer) from the indentation the script is printed at, which `doc` gives it through `Doc::AtIndent`. |

`crates/cli/src/commands/fmt/` finds the files and reads the configuration Prettier reads
(`.prettierrc`, its `overrides`, `.editorconfig`, `.prettierignore`).

Tests: unit tests next to the code; `tests/golden.rs` formats the cases of
`tests/cases/*.liquid` and compares with `tests/golden/*.json`, which
`tools/prettier/generate_golden.mjs` produces by running the same cases through Prettier.
`tests/plugin_fixtures.rs` replays the tests of the plugin itself, downloaded by
`mise run prettier:fixtures`, and fails when fewer pass than its recorded baseline.

## `lsf`: commands and server

`commands/` holds one file per subcommand (`checkout` is `serve` without a theme: `App::open_without_theme`; `function run` runs the pipeline once). `server/` is the HTTP side:

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
| `checkout` | `--app`: the pages at `/checkouts/cn/<token>`, the operations the page sends, the files of the page and of the extensions. What the buyer chose is in the session (`SessionEntry::checkout`), next to the cart they share with the theme. The options, the mapping from the store data to `lsf_checkout::CheckoutConfig` and the build of the Functions are in `src/checkout.rs`. |
| `../profile` | Shared by `lsf profile` and `/__lsf/profile`: renders a page several times with the profiler, keeps the render in the middle, and writes it as a tree of text or in the format of speedscope, in time or in points. `lsf-core` adds the frames Liquid does not know (`render`, a JSON template, `section <id>`, `block <key>`) and the costs (`render/cost`): the objects charge for what they load where they are created. |
| `speedscope` | The viewer of flame graphs, served from the binary for `/__lsf/profile?...&html=1`. Its files are in `assets/speedscope/`, with their licenses. |
| `timing` | The `Server-Timing` header of each response, and the render times `/__lsf/timings` adds up: a histogram per template, per section and per theme block, so that they take the same room after a million pages. |
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
mise run docs:coverage     # what Shopify documents and lsf lacks: filters, tags, object properties
mise run liquid-spec:fetch # download Shopify's liquid-spec suite, which the engine's tests replay
mise run prettier:install  # install Prettier and Shopify's Liquid plugin, to compare `lsf fmt` with
mise run prettier:golden   # regenerate the golden expectations of the formatter from them
mise run prettier:fixtures # download the tests of the plugin, which the formatter's tests replay
mise run prettier:compare -- path/to/theme   # list the files `lsf fmt` and Prettier disagree on
mise run oracle:strict2-fixes                # check the fixes of `lsf check --fix` with the gem
```

Documentation: user-facing changes go into the guide of their task in `docs/` and into
`CHANGELOG.md`; the README only lists the tasks. Tables that repeat values of the code are
checked by tests (`throttle.rs` reads `docs/testing.md`, `fmt` reads `docs/formatting.md`,
`profile.rs` reads `docs/performance.md`), `docs/data-reference.md` is generated from the JSON
Schema (`UPDATE_SNAPSHOTS=1 cargo test -p lsf-core --test docs` rewrites it after a change to
`store/model.rs`), and `docs/data-format.md` is embedded in the binary by `lsf docs` and
`lsf init`.

## Releasing

Set the version in `Cargo.toml` (`workspace.package.version`), add a section for it at the top
of `CHANGELOG.md` (`## 0.2.0 - 2026-10-06`), commit, then push a tag with the same version:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

`.github/workflows/release.yml` runs the tests, builds `lsf` for macOS, Linux (glibc and
static musl) and Windows on x86-64 and ARM64, and publishes a GitHub release with one archive
per platform and a `SHA256SUMS` file. A tag that does not match the version of the crate, or
that `CHANGELOG.md` has no section for, fails before anything is built. Tags with a suffix
(`v0.2.0-rc.1`) are published as prereleases.

The release says what the changelog says of the version. Write that section for someone who
uses `lsf`: what they can do now, what behaves differently, what was wrong. Links are
absolute, as a release does not resolve them from the repository. To read it as the release
will show it:

```bash
mise run release:notes 0.2.0
```

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
