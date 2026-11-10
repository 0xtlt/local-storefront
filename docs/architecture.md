# Architecture

The store data is one crate, and two sides are built on it: the storefront, which renders a
theme with Liquid, and the app, which runs the Functions, checkout UI extensions and backend of
a Shopify app. The command line puts them together.

```text
crates/core      lsf-core       the store data, and what the storefront and the app both need of it
crates/liquid    lsf-liquid     the Liquid language: no knowledge of Shopify
crates/theme     lsf-theme      Shopify's storefront on Liquid: objects, filters, tags, rendering
crates/app       lsf-app        a Shopify app: its checkout, its Admin API (GraphQL, 2026-10) and webhooks
crates/fmt       lsf-fmt        the formatter of `lsf fmt`: a source in, a source out
crates/cli       lsf-cli        the command line and the HTTP server: the `lsf` binary
```

`lsf-theme` uses `lsf-liquid` and `lsf-core`; `lsf-app` uses `lsf-core` alone, so that neither
it nor `lsf-core` builds Liquid or the renderer, and `lsf-theme` builds no Wasm runtime.
`lsf-cli` uses them all.

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

## `lsf-core`: the store data

What the storefront and the app both read, and nothing that renders Liquid.

| Module | Role |
|---|---|
| `store/model` | The input format, as serde types. Their doc comments are the user documentation: the JSON Schema and the field reference are generated from them. |
| `store/validate` | JSON Schema validation, turned into diagnostics with a code, a JSON pointer and a hint. |
| `store/load`, `store/build` | Read and merge the data files, then resolve them into a `Store`: defaults, ids, handles, references between entities, dates (ISO 8601, `store/date`), with semantic diagnostics. The files of `checkouts/` are not merged: each is a checkout scenario, resolved against the store (`Store::scenarios`) and kept with its errors when it has some. |
| `session`, `cart` | The visitor's state (cart, customer, country), and its cart lines resolved against the store: what a line costs, which the storefront cart and the checkout both show. |
| `theme` | A theme as files on disk: the directories it is made of, cached and revalidated reads, the files the Admin API writes over them (`FileOverlay`), and which theme of the shop it is. |
| `color`, `money`, `urls` | Values both sides print: CSS colors, amounts in a shop's money format, percent-encoding and the pieces of CDN URLs. |
| `util`, `json`, `diagnostics`, `error` | Handles and stable ids, lenient JSON, the diagnostics of the store data, errors. |

Tests: unit tests next to the code, `tests/docs.rs` (the generated reference is current).

## `lsf-theme`: Shopify's storefront

| Module | Role |
|---|---|
| `theme/` | A theme directory read through `lsf-core`'s files: parsed Liquid, section schemas, JSON templates, locales. |
| `drops/` | Shopify's Liquid objects, backed by the `Store`, the session and the request. Properties are computed on demand. |
| `filters/`, `tags/` | Shopify's filters and tags (`section`, `sections`, `content_for`, `form`, `paginate`, `render` with blocks, ...). |
| `render/` | URL → template (`routes`), settings resolution, sections and blocks, layouts, the Section Rendering API, the compiled stylesheet and script bundles, and what Shopify injects into pages (`platform`). |
| `assets/platform/` | The JavaScript counterparts of Shopify's own scripts (`Shopify.actions`, `Shopify.loadFeatures`), embedded in the binary and served from the local CDN, with Shopify's global assets (`themes_support/`, see its README). Tested with Node's test runner (`mise run test:js`). |
| `images`, `fonts`, `urls` | Image transformations, the format an image is sent in (JPEG, PNG, WebP or AVIF, by what the client accepts) and placeholders, the blank fallback font, the local CDN URLs of a site. |
| `previews` | Shopify's feature previews (`--preview liquid-july-26`): what a render adds when one is on. |
| `site` | What one render sees: theme, store, request, session, clock. |

A render is a pure function of `(theme, store, request, session, now)`: `Renderer::render`
returns the body, the status, the template name and the Liquid errors. It does no I/O beyond
reading theme files, which is what makes it fast and reproducible.

Tests: unit tests next to the code, `tests/render.rs` (a fixture theme rendered against the
demo store, with HTML snapshots), `tests/conformance.rs` (the examples of shopify.dev,
downloaded by `mise run docs:fetch`; it also fails when Shopify documents a filter or a tag lsf
lacks), `tests/overlay.rs` (the files the Admin API writes over the theme).

## `lsf-app`: a Shopify app

Reads a Shopify app, runs its Functions against a cart and describes the checkout the buyer
sees, and serves the Admin API and the webhooks its backend talks to. It does not serve anything
itself: `lsf` does, with `lsf app dev`, `lsf serve --app` and `lsf app function run`.

| Module | Role |
|---|---|
| `config` | Reads `shopify.app.toml` and `extensions/*/shopify.extension.toml`: Functions, checkout UI extensions, the others listed and ignored. What is wrong with the app (a missing build, an unknown target) is a diagnostic, not an error. `Selection` is the set of extensions in use (`--extension`). `config::version` holds the API releases and the Function schemas of each (`schemas/functions/`). |
| `checkout` | The checkout, below. |
| `admin` | The Admin API, below. |

### `checkout`

Its design, and what each supported Shopify API release changes, are in
`crates/app/docs/checkout/` (`design.md`, `api-versions.md`, `ui-api-versions.md`).

| Module | Role |
|---|---|
| `input` | Builds the input of a Function: runs its GraphQL input query against the cart, the buyer and the store (a small executor over a resolver of the Function input graph). |
| `runner` | Runs a Wasm module as Shopify's own `function-runner` runs it (`runner::host`, its engine ported, with the providers of `providers/`), runs every Function on one engine (a module is compiled once, whatever the number of runs at once; at most one run per CPU), caches the compiled modules by path, time and size, stops a Function after 10 seconds, and reports logs, instructions and errors. |
| `pipeline` | `compute`: the Functions in the order Shopify runs them, applying each output, to a `CheckoutResult`. |
| `schema` | A Function API's schema: the input query only asks for what it has, the output fits its result type. |
| `fetch` | The HTTP request of a `.fetch` target, checked against Shopify's limits, and the answer its `.run` target reads. |
| `javascript/`, `trampoline`, `providers` | Building a Function as Shopify's CLI does, without Node: a JavaScript one bundled and compiled with Javy, a Rust one linked by Shopify's trampoline; and the providers a module imports when it runs. `javascript/` also bundles the UI extensions that name their sources. |
| `expect` | The `expect` of a checkout scenario, checked against what its Functions computed: what `lsf app check` fails on when a Function decides wrong. |
| `placement`, `app_metafields` | Where the pages place each target of the checkout and customer account UI extensions, and the metafields `appMetafields` gives them. |
| `types` | The state the buyer builds, the configuration (`CheckoutConfig`, which `lsf` maps from the `checkout` section of the store data) and the result, which is the JSON the page reads. |
| `assets` | The page and the sandbox of its extensions, embedded in the binary: the files of `assets/`, except `tests/` and `preview/`. |

`assets/` holds the checkout page and `extension-host.js`, which runs each extension in a
sandboxed frame (`sandbox.html`; its scripts are classic scripts, since the frame has an opaque
origin and cannot load modules). They need no build step.

### `admin`

The GraphQL Admin API of a shop seeded from the store data, in the versions of `admin::version`
(their schemas in `schemas/admin/`), the OAuth endpoints, the webhooks of the app, and the sync
that writes what the backend changed back into the store data. It reads the app and the
checkout's types directly. Its design, and what each module owns, are in
`crates/app/docs/admin/design.md`.

Tests: the unit tests, and `tests/` against the fixture apps (`tests/fixtures/app`: Rust
Functions whose `dist/function.wasm` is committed and two built UI extensions,
`mise run fixtures:functions` rebuilds the Functions; `tests/fixtures/admin-app` and
`tests/fixtures/platform-app` for the Admin API) and the `rich` store. `lsf-app` does not render pages, not
even in its tests: that what the backend changes shows in the storefront is tested by lsf-cli
(`crates/cli/src/server/tests/store_sync.rs`). Node's test runner tests the scripts (`mise run test:js`).

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

## `lsf-cli`: commands and server

`commands/` holds one file per subcommand (`app.rs` is `lsf app`: `app dev` is `serve` without a theme, `App::open_without_theme`, and `app init` (`app_init.rs`) writes the checkout scenarios for an app; `function.rs` is `app function run`, which runs the pipeline once, and `app function build`, with `function/schema.rs` for `app function schema`; `webhook.rs` is `app webhook trigger`). `server/` is the HTTP side:

| Module | Role |
|---|---|
| `mod` | State (the loaded store, sessions, caches), the request/response types handlers work with (`reply`, `params`), reloading when files change (`data_files`: a change of checkout scenarios alone keeps the shops of the Admin API). Handlers are plain functions from a request to a reply, so they are tested without a network (`tests.rs`, `tests/`). |
| `storefront` | Pages, the Section Rendering API, product, search and recommendation JSON. |
| `cart`, `forms` | The Cart Ajax API and form submissions. |
| `account` | The page that stands in for the customer accounts Shopify hosts, and the B2B location switch. |
| `throttle` | `--throttle` and the `throttle` of a session: answering some requests late. |
| `cdn` | Theme assets, files, images and the variants of them kept in memory, bundles, fonts. |
| `minify` | The stylesheets and the scripts of the theme, minified with a source map. |
| `compress` | Brotli or gzip for the responses that gain from it, by what the client accepts. |
| `control` | The `/__lsf` API. |
| `parts` | The parts the server runs (`--only`): the theme when it was opened with one, the checkout and the Admin API when it was given them (`ServerState::parts`). `dispatch` answers the routes of a part that is off with a `404` and an `X-LSF-Reason` naming the `--only` that serves it; what nothing else answers is the theme's. `commands/serve.rs` picks the parts (the defaults, the errors of an option whose part is off). |
| `home` | `--app`: the home page (`/` of `lsf app dev`, `/__lsf/checkouts`) and the notices that say why a scenario did not open: HTML in the look of the checkout, drawn from the JSON of `?format=json`. `home_check` runs the Functions of every scenario for it (`/__lsf/checkouts/check`), as `lsf app check` does. |
| `checkout` | `--app`: the pages at `/checkouts/cn/<token>`, `/checkout?scenario=<name>`, the operations the page sends, the files of the page and of the extensions. What the buyer chose is in the session (`SessionEntry::checkout`), next to the cart they share with the theme. The options, the mapping from the store data to `lsf_app::checkout::CheckoutConfig` and the build of the Functions are in `src/checkout.rs`. `checkout_account` logs the checkout in as a customer of the data. |
| `customer_account` | `--app`: the customer account where the app's customer account UI extensions render (`/account`, its orders, its profile, the full pages of extensions), in place of `account`. |
| `admin` | `--app`: `lsf_app::admin::AdminApi`, built from the store data and the app, routed before the storefront and the checkout: the GraphQL Admin API, OAuth, lsf's admin, `/__lsf/admin`, `/__lsf/webhooks`, the Partner API. It prints the `admin:` lines at start. |
| `store_sync` | What the backend changes in the default shop of the Admin API shows in the storefront and the checkout: the store is built again from the files with those changes on top, on the next request that reads it. |
| `polaris` | `/__lsf/cdn/<path>` and `/__lsf/polaris`: Polaris for the app's pages in lsf's admin, a redirect to Shopify's CDN (`online`) or the copy in the binary (`offline`, `assets/shopify-cdn/`). |
| `app_proxy` | `--app` with an `[app_proxy]`: `/<prefix>/<subpath>` and below (a locale prefix too), routed after `/__lsf` and `/cdn` and before the theme's pages, forwarded with `ureq` to the backend from the blocking thread of the request, the query signed with `lsf_app::admin::auth::app_proxy_signature` and the client secret of the Admin API, the headers as Shopify forwards and strips them. An answer in `application/liquid` is rendered by `Renderer::render_source` of `lsf-theme`, as a Liquid template inside the theme's layout. While the default shop has the app uninstalled (`ServerState::app_uninstalled`), nothing is forwarded: the storefront answers the path (the theme's `404` page) with `X-LSF-Reason`. |
| `../profile` | Shared by `lsf profile` and `/__lsf/profile`: renders a page several times with the profiler, keeps the render in the middle, and writes it as a tree of text or in the format of speedscope, in time or in points. `lsf-theme` adds the frames Liquid does not know (`render`, a JSON template, `section <id>`, `block <key>`) and the costs (`render/cost`): the objects charge for what they load where they are created. |
| `speedscope` | The viewer of flame graphs, served from the binary for `/__lsf/profile?...&html=1`. Its files are in `assets/speedscope/`, with their licenses. |
| `timing` | The `Server-Timing` header of each response, and the render times `/__lsf/timings` adds up: a histogram per template, per section and per theme block, so that they take the same room after a million pages. |
| `live_reload` | `--live-reload`: the WebSocket open pages listen on, and the task that looks at the theme and data directories while a page listens. The script it injects is `assets/live-reload.js`, tested with Node (`mise run test:js`). |

Rendering is CPU-bound and synchronous; the async server hands each request to a blocking
thread with a large stack, because deeply nested snippets and blocks recurse.

## Development

```bash
mise install               # Rust, Node (the tests of the scripts, Prettier, the docs site), and Ruby for the oracle
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
mise run admin:schema                        # regenerate the Admin API schema of lsf-app from the pinned @shopify/dev-mcp
mise run partner:schema                      # regenerate the schema of its Partner API
mise run admin:access-scopes                 # regenerate its access scopes and search filters
mise run admin:topics                        # regenerate its webhook topics
mise run admin:webhook-examples              # download Shopify's example webhook payloads the payload tests compare with
mise run admin:version -- 2027-01            # everything a new Admin API version needs, then what is left to do by hand
mise run admin:e2e                           # run examples/admin-backend (Shopify's libraries) end to end against lsf
mise run admin:bench -- admin --bin target/release/lsf   # RAM and CPU of a release build, suite by suite (tools/admin/bench.mjs)
mise run docs:site                           # the guides of docs/ as a site (VitePress), at http://localhost:5173
mise run docs:site:build                     # build it; fails on a link of llms.txt to a page the site does not have
```

Documentation: user-facing changes go into the guide of their task in `docs/` and into
`CHANGELOG.md`; the README only lists the tasks. `docs/README.md` is the index of the guides,
and the sidebar of the docs site (`tools/docs-site`): a new guide goes in its tables, and in
`llms.txt`, the index an AI agent reads first. Tables that repeat values of the code are
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
rendered HTML, regenerate the snapshots with
`UPDATE_SNAPSHOTS=1 cargo test -p lsf-theme --test render`; after a change of the guide or of the
schemas, write the copies of `examples/checkout-playground/shopify-local` again with
`UPDATE_SNAPSHOTS=1 cargo test -p lsf-cli the_example_data`.
