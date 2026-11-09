# Changelog

What changed in each version of `lsf`, for someone who uses it. The section of a version is
what its [GitHub release](https://github.com/0xtlt/local-storefront/releases) says.

## Unreleased

lsf now serves a local GraphQL Admin API (version 2026-10) and delivers an app's webhooks to its
backend: the backend of a Shopify app runs and is tested offline, against the store data, with
the same data every run.

### Added

- **A local GraphQL Admin API, version 2026-10, with every mutation.** A server with `--app`
  answers `POST /admin/api/2026-10/graphql.json` over the store data, with the ids the storefront, the
  checkout and the Function inputs use. Queries follow Shopify's 2026-10 schema (its
  introspection matches Shopify's), with its validation errors, query costs (`extensions.cost`),
  throttling (`THROTTLED`, `MAX_COST_EXCEEDED`), cursors, the `query:` search syntax, and
  `ACCESS_DENIED` for a scope the token lacks. All 496 mutations of 2026-10 are simulated,
  the deprecated ones included, with their user errors, side effects and webhooks:
  - the catalog: products, variants, media, `productSet`, collections and their sources,
    bundles, combined listings, inventory, transfers and shipments, locations, publications,
    channels, product feeds, selling plans, gift cards;
  - orders: payments and payment terms, fulfillments and fulfillment orders (split, merge,
    reroute), fulfillment services, shipping labels, refunds, returns and exchanges, order
    edits, draft orders, disputes, store credit, risk, tax;
  - customers and B2B companies, segments (their query language is evaluated over the
    customers), saved searches, merges, data erasure, consent, marketing, analytics and a
    ShopifyQL subset;
  - the Subscriptions API: contracts, drafts, billing cycles and attempts (test cards decide the
    outcome), and customers' payment methods;
  - markets, catalogs, price lists and contextual prices, quantity rules, locales, web presences;
  - metafields and their definitions, metaobjects, pages, blogs, menus, redirects (in bulk and by
    CSV import), files, translations, policies;
  - themes and theme files (written over the theme directory in memory, never to disk), script
    tags (loaded by `content_for_header`), pixels, storefront tokens, delegate tokens;
  - the app: billing, discounts and Function settings, bulk operations, staged uploads,
    `appUninstall`, scope revocation, Flow triggers, point of sale.

  A failed mutation changes nothing. `crates/app/tests/coverage.rs` fails if a mutation of a
  version lsf serves is not simulated. Guide: [Admin API](docs/admin.md).
- **API version 2027-01, the release candidate, on demand.** `--admin-versions 2026-10,2027-01`
  serves it next to 2026-10, each at its own `/admin/api/<version>/graphql.json`, with its own
  schema, introspection, access scopes, search filters and webhook topics:
  - a field or a mutation of one version is refused in the other with Shopify's error;
  - a webhook subscription is delivered in the version that made it (`X-Shopify-API-Version`);
  - the records are shared, and 2027-01 reads what it removed the way Shopify says to replace it.

  A version Shopify no longer supports (`2025-10`) is answered by 2026-10, as Shopify falls
  forward, and `X-Shopify-API-Version` says so. Another answers `404` saying which versions are
  served. `lsf app webhook trigger --api-version 2027-01` builds the payload in 2027-01.
  [API versions](docs/admin.md#api-versions).
- **The storefront and the checkout show what the backend changes.** A product it creates is on
  the theme's pages at once, and a draft leaves them. A customer it creates can log in at the
  checkout. A metafield it sets is read by Liquid and by the Functions, and an app discount it
  creates runs its Function at the checkout. Open pages reload. An order placed in the local
  checkout is an order of the Admin API, and its `orders/create` webhook reaches the backend.
- **OAuth, token exchange and session tokens**, answered as Shopify answers them:
  - the authorization code grant, and token exchange from a session token (embedded apps);
  - expiring offline tokens and their refresh, and client credentials;
  - a launch URL that opens the backend as Shopify's admin opens an embedded app.

  Tokens have the scopes of `shopify.app.toml`. The guide says what Shopify's libraries need to
  reach lsf.
- **An admin to preview the embedded app in, with a local App Bridge.**
  `http://127.0.0.1:9292/admin/apps/<handle>` shows the app in a frame, at a new launch URL on
  every load, with the shop's name, the app's navigation and its title bar around it; `serve`
  and `app dev` print its URL, and the home page links to it. The app's pages load lsf's App
  Bridge (`/__lsf/admin/app-bridge.js`) before Shopify's, which then does nothing:
  - `shopify.idToken()` and `fetch()` with the session token, as App Bridge v4 adds it;
  - the title bar (`ui-title-bar`, `s-page`), the navigation (`s-app-nav`, `ui-nav-menu`), the
    save bar (`ui-save-bar`, `data-save-bar` forms), toasts, the loading bar, `shopify.user()`;
  - the resource picker, with the shop's products, variants and collections;
  - modals (`ui-modal`, `s-app-window`), `shopify:admin/…` links.

  What lsf does not simulate (the scanner, POS, Sidekick, …) rejects or warns and says so. The
  admin says when the app runs Shopify's App Bridge instead. In the React Router template, three
  lines behind `APP_BRIDGE_URL` switch to lsf's. [Preview the embedded
  app](docs/admin.md#preview-the-embedded-app).
- **Polaris without the internet.** lsf serves the paths of Shopify's CDN at `/__lsf/cdn/<path>`:
  an app's pages load Polaris from `/__lsf/cdn/shopifycloud/polaris-2.js` (`polarisUrl` of the
  React Router template, in `AppProvider` and `shopifyApp`). `--polaris online`, the default,
  redirects to the CDN; `--polaris offline` (or `PUT /__lsf/polaris` while lsf runs) serves lsf's
  copy of Polaris 2.0.0 and 1.1.0, its icons and its fonts, pointed at lsf, so that the
  components, icons and fonts render with no request to `cdn.shopify.com`. Offline, a page that
  still loads Polaris from the CDN gets lsf's copy from lsf's App Bridge when the CDN's fails.
  [Polaris without the internet](docs/admin.md#polaris-without-the-internet).
- **Webhooks, signed as Shopify signs them.** The subscriptions of `shopify.app.toml` and those
  the backend makes with `webhookSubscriptionCreate` are delivered to the backend:
  - with Shopify's headers and HMAC;
  - retried after 1, 5 and 15 s;
  - filtered by `filter` and `include_fields`;
  - refused by the backend (`401`, `403`): the delivery's `reason`, one line of the server and
    `lsf app webhook trigger` name the secret lsf signed it with, as `client secret:` does.

  Payloads follow Shopify's 2026-10 examples, for every topic Shopify publishes an example of
  (224 of the 227); the other three are logged, never sent with a made-up body. The storefront's
  cart and the local checkout send `carts/*` and `checkouts/*`, and a checkout left without an
  order becomes an abandoned checkout the backend reads. `lsf app webhook trigger` sends one on
  demand, compliance topics included, as `shopify app webhook trigger` does (`--format json`
  for a tool). After an uninstall, nothing is delivered for the shop but `app/uninstalled`.
- **Webhooks stay on this machine.** lsf sends one to this machine (`localhost`,
  `*.localhost`, `127.0.0.0/8`, `::1`), to `--app-url`, or to the `--address` you give, nowhere
  else: a subscription that points at your production backend or a stale tunnel is logged
  `not_sent`, with why, and `lsf app webhook trigger` sends nothing and exits with `2`. Give
  `--app-url http://localhost:<port>`: the start lines say when it is missing. A name under
  `localhost` (`app.localhost`) goes to the loopback whatever your system's resolver says.
- **The webhook log, for tests.** `GET /__lsf/webhooks` lists the deliveries, filtered by
  `topic`, `state`, `shop` (`test-1` or `test-1.myshopify.com`), `resource` (the GID of a
  record: the order a test placed) and `since` (a delivery's id). `?wait=1` waits for one that
  matches every filter. Every answer has `last`, the newest id, to pass as `since` next time:
  a test no longer gets another test's webhook. A wait that could only time out (a shop lsf
  does not have, a topic no subscription covers) answers `422` at once.
  [Control API](docs/control-api.md#webhook-deliveries).
- **The start lines of an app.** With `--app`, the Admin API's URL and token, where the client
  secret came from, where the webhooks go and the app proxy are printed before `ready:`: a tool
  that reads up to `ready:` has the token. Without the Admin API, none of them. An app without
  an `extensions` directory is said to be a backend, in one line.
- **The app proxy.** A storefront request under the prefix of `[app_proxy]` goes to the backend
  with Shopify's parameters and signature. A Liquid answer is rendered in the theme's layout.
- **Billing.** `appSubscriptionCreate` and `appPurchaseOneTimeCreate` lead to a local approval
  page. Approving goes to the `returnUrl`, and the charge is active.
- **Managed pricing (Shopify App Pricing).** Declare the app's plans in a file
  (`--pricing-plans pricing.toml`) or through `PUT /__lsf/admin/pricing-plans`. lsf's admin shows
  them at `/admin/charges/<app handle>/pricing_plans`, where a `shopify:admin/charges/…` link
  and the template's `redirect(…, { target: "_top" })` land. The merchant picks a plan: a test
  subscription with the plan's `planHandle` replaces the current one, `app_subscriptions/update`
  is delivered, and the browser goes to the plan's welcome link with `charge_id` and
  `plan_handle`. The Billing API then refuses to create charges, as Shopify does. With
  `legacy = false` (Shopify App Pricing since April 2026), no change of the subscription sends a
  webhook, `appSubscriptionCancel` from the app included.
  [Managed pricing](docs/admin.md#managed-pricing).
- **A test ends the merchant's plan: `POST /__lsf/admin/pricing-plans/cancel`.** The shop's
  subscription of managed pricing is `CANCELLED` at once, without the app's token, and
  `app_subscriptions/update` is delivered, none with `legacy = false`: the case of an app that has
  to notice by itself that the plan ended. Shopify's plan page has no button for it (the merchant
  chooses another plan or uninstalls), so lsf's page has none either: the route stands for a plan
  that ends while the app stays installed. [Managed pricing](docs/admin.md#managed-pricing).
- **A local Partner API: an app on Shopify App Pricing reads its plan.** Since April 2026 such an
  app learns its plan from the Partner API's `activeSubscription`, not from a webhook. lsf serves
  it at Shopify's path on its own origin, `POST /{partner_id}/api/2026-10/graphql.json` (also
  2026-07 and 2027-01), with Shopify's schema: the app changes only the origin of
  `partners.shopify.com`. `activeSubscription` answers the shop's plan, its trial and its billing
  cycle by the shop's clock; `events` and `app { events }` answer the history: installs,
  uninstalls, plans chosen, changed and ended, and every change of a charge. Both `legacy` modes
  feed it. The Partner ID and the `prtapi_lsf_` token are printed at start (`partner:`, with the
  plans) and in `GET /__lsf/admin/partner`; they are the same every run. A wrong token is
  Shopify's `401`. The rest of the Partner API answers `LSF_NOT_SIMULATED`.
  [Read the plan from the Partner API](docs/admin.md#read-the-plan-from-the-partner-api).
- **`lsf app function run <handle>` says when the checkout of the store data would not run the
  Function.** A named Function runs whatever its owner, as before; when its payment customization
  (or other owner) is disabled in the store data, or there is none, the output says so first
  (`Named: …`, `forced` in JSON), and that it reads the files, not a running lsf.
- **`pricing.toml` is read again when it changes** (unless `--static`, before the next request of
  any kind), and `DELETE
  /__lsf/admin/pricing-plans` reads it again rather than putting back the copy read at start (a
  file that no longer reads is a `422`). Plans a `PUT` set stay until the `DELETE`.
- **The plan selection page takes the app's handle only.** `/admin/charges/<client id>/pricing_plans`
  is a `404` with `X-LSF-Reason` when `shopify.app.toml` has a handle: on Shopify a client id there
  fails, so a link built with it no longer passes locally. An app with no handle keeps its page at
  its client id, and the `pricing:` start line says Shopify's page needs a handle.
- **Control endpoints under `/__lsf/admin`:** reset the shop, make and delete extra shops for
  isolated tests, install and uninstall the app, and make session tokens for a test to call the
  backend's routes. [Control API](docs/control-api.md).
- **The shop's clock and its outbox.** `POST /__lsf/admin/clock` moves a shop's time: payment
  schedules come due, store credit expires, payouts are made, scheduled listings go live,
  checkouts are abandoned, data erasures run, each with its webhooks. What Shopify would send
  the outside world (emails, labels, Flow triggers, point-of-sale messages) is never sent: it is
  in `GET /__lsf/admin/outbox`.
- **`--only theme,checkout,admin` runs some parts of lsf** (also `LSF_ONLY`), on `serve` and
  `app dev`: the storefront of the theme, the checkout of the app (its Functions, UI
  extensions, scenarios), its Admin API (OAuth, webhooks, the app proxy, lsf's admin, managed
  pricing). `serve` runs the theme, all three with `--app`; `app dev` the checkout and the Admin
  API. `serve --app ../my-app --only admin` is the Admin API alone, for a backend's tests, without
  a theme folder. A part left out answers `404` on its routes, with an `X-LSF-Reason` that names
  the `--only` serving it; a line before `ready:` says what runs (`serving: theme, checkout,
  admin`), and `GET /__lsf/status` and `GET /__lsf/admin` list it (`parts`). An option of a part
  left out (`--pricing-plans` without the admin part, `--variant` without the checkout) stops the
  start. `serve` without `--app` serves no Admin API: it was new in this version, and costs
  memory a theme does not use. [What runs](docs/cli.md#what-runs).
- **New options:**
  - `--app-url`: where the backend listens.
  - `--no-functions`: the app's UI extensions and Admin API, and none of its Functions, for a
    backend's tests.
  - `--client-secret`: else `SHOPIFY_API_SECRET`, else the app's `.env`.
  - `--admin-throttle`.
  - `--admin-versions`: the API versions served.
  - `--admin-fetch-local`: sources from a URL (`themeCreate`, `themeFilesUpsert`,
    `urlRedirectImportCreate`) may be on this machine. Off by default, as Shopify cannot reach
    it.
  - `--admin-source <url>=<file>`: a URL of your backend's own answered with a file, never
    downloaded; `PUT /__lsf/admin/sources?url=` stubs one from a test.
- **Expected results in checkout scenarios: `lsf app check` fails when a Function decides
  wrong,** not only when it fails to run. A scenario's `expect` says what its checkout must look
  like once the Functions ran: the payment methods, the express buttons, the delivery options,
  the discounts, the lines and the errors, each with `includes`, `excludes` and `equals` (an
  entry by its handle or name, or by the fields it must have, such as
  `{"handle": "standard", "cost": "0.00"}`), and exact `totals`.
  `"express_methods": {"excludes": ["paypal"]}` with `"payment_methods": {"includes": ["paypal"]}`
  replaces piping `app function run --format json` into `jq`. Each expectation that is not met
  prints the scenario, the assertion, what was expected and what the checkout has, and makes the
  check exit with `1`; `--format json` has them in `expectations`. The Functions run as the
  order is placed (`"step": "interaction"`: the page before it). `validate` warns about a name in
  `excludes` that the checkout does not offer, which would check nothing. The scenario schema has
  `expect`, and two scenarios of the playground use it.
  [Expected results](docs/checkout.md#expected-results).
- **`lsf app check --build`** builds the extensions first, for a clone where `dist/` is not
  committed. `app check` also warns about a Function that ran in no scenario, so that nothing
  checks it (`ranInNoScenario` in JSON), and its summary counts it (`no Function failed, 1
  Function ran in no scenario`); the exit code does not change. `app check --help` describes
  the JSON it prints. A missing build says
  to run `lsf app build <handle>`.
- **The docs ship in the npm package.** `node_modules/local-storefront/` has the `llms.txt`,
  `docs/` and `CHANGELOG.md` of the version you installed, for a tool or an agent to read
  offline.
- **Order metafields on the order status page of the customer account.** A customer account UI
  extension declares them with `owner_type = "ORDER"`, which lsf refused as an unknown owner
  type. `shopify.appMetafields` of the order status targets now has them as `order` entries
  (`target: {type: 'order', id: 'gid://shopify/Order/…'}`), read from the order in the Admin
  API: what the backend sets with `metafieldsSet`, on an order placed in the local checkout or
  on one of the store data, and the cart metafields copied onto an order placed here. As on
  Shopify since 2026-04, a cart metafield is copied when the order is placed if an ORDER
  definition with its namespace and key has `cartToOrderCopyable` enabled, made with
  `metafieldDefinitionCreate` or declared in `shopify.app.toml`
  (`[order.metafields.app.gift_message]` with `capabilities.cart_to_order_copyable = true`,
  which lsf now reads). The checkout and its thank-you page have no `order` entry, as on
  Shopify: an `ORDER` declaration reads nothing there. [Checkout](docs/checkout.md),
  [Admin API](docs/admin.md#what-the-storefront-and-the-checkout-see).
- **`examples/admin-backend`:** a backend written with Shopify's libraries
  (`@shopify/shopify-app-react-router`, `@shopify/shopify-api`), with a script that runs it
  against lsf end to end (`mise run admin:e2e`).
- **`lsf app function schema`** writes a Function's `schema.graphql`, the GraphQL schema of its
  API at its `api_version`, as `shopify app function schema` does, from the schemas lsf
  carries, without the network: in the Function's folder, by handle, or with `--path`;
  `--stdout` prints it. [The schema of a Function](docs/checkout.md#the-schema-of-a-function).
- **Express checkout buttons.** The checkout shows the payment methods of the
  `ACCELERATED_CHECKOUT` placement as express buttons above the contact section (Shop Pay,
  PayPal, Apple Pay, Google Pay… in their colors, then "OR"), once the payment customizations
  ran: a Function that hides a method there (`paymentMethodHide` with `placements`) now has a
  visible effect. A click says lsf does not simulate the wallet. The page's JSON has
  `expressMethods`, and every method its `placements`; `lsf app function run` prints each method
  with its placements. The `payment-methods` scenario of `app init` has PayPal in both
  placements, as Shopify offers it with PayPal Express on.

### Changed

- **A Function runs only through an enabled owner, as on Shopify.** At the shop's checkout (no
  scenario, or a scenario without `extensions`), a payment or delivery customization, validation
  or cart transform Function runs only when the shop has an enabled owner of it: the
  `PaymentCustomization`, `DeliveryCustomization`, `Validation` or `CartTransform` the app
  creates through the Admin API, or one the store data declares
  (`checkout.functions.<handle>.enabled`, new). Disabling it (`paymentCustomizationUpdate`,
  `…Activation`, `validationUpdate(enable:)`) or deleting it takes it out of the next checkout.
  A scenario that names a Function in `extensions`, and `lsf app function run <handle>`, still
  run it alone, without an owner; `app init`'s scenarios name theirs, `all-extensions` every
  extension. The checkout's "Functions" dialog, the home page and `app function run` say which
  Functions did not run and why (`idle` in JSON). Store data that relied on every Function
  running adds `"enabled": true` to them. [What runs](docs/checkout.md#owners-and-scenarios).
- **No owner is created at install.** lsf no longer seeds an enabled payment customization,
  delivery customization, validation and cart transform for every Function: a fresh shop has
  none, so an app that checks whether it is set up sees that it is not. Only those the store data
  declares (`enabled`) are there. Extension names written as a locale key (`name = "t:name"`)
  are read from the extension's default locale (`locales/en.default.json`), for the Functions'
  titles and wherever lsf shows a name.
- **Store data:** products gain `status` (`active`, `draft`, `archived`) and `published`. Only
  an active, published product is on the storefront. Entries of `checkout.discounts` gain an
  `id`. [Data format](docs/data-format.md).
- **Two companies with the same name** are both kept, and reported (`duplicate_company`). The
  second used to replace the first.
- **`POST /checkouts/cn/<token>/complete` answers the order it placed:**
  `{"redirect", "order": {"id", "number", "name"}}`, as the Admin API recorded it. Match a
  webhook by `order.id`. The thank-you page keeps the session's number, which can differ
  (`#1004` there, `#1005` in the Admin API when the shop has a `#1004` already).
- **The dates of the store data are ISO 8601 only:** a day (`2024-05-01`), or a day and a time
  (`2024-05-01T10:00:00`, or a space for the `T`) with `Z`, an offset or neither, for the shop's
  `timezone`; `now` too. The other forms that Liquid's `date` filter reads were accepted as well,
  and are now an `invalid_date` error: `May 1, 2024`, `2024/05/01`, `05/01/2024`, `2024-5-1`,
  RFC 2822 (`Wed, 01 May 2024 10:00:00 +0000`), an offset after a space
  (`2024-05-01 10:00:00 +0200`), a comma before the fraction of a second, and a time alone
  (`10:00`, which took the day 2024-01-01). None of the examples or guides used one. The `date`
  filter of a theme still reads them all.
- **The storefront opens in the shop's country, like the checkout.** A visitor who selected no
  country is in the shop's (`shop.address.country_code`), the one of its primary market, or the
  first country when the store does not sell to it: `localization.country`, `Shopify.country`
  and the localization form say so. The storefront used to show the first country and the
  checkout the shop's: the demo store showed the United States, then checked out in Canada.
- **For contributors, the crates are split along what they need.** `lsf-core` is the store data
  and what the storefront and the app share of it, `lsf-theme` the renderer (the old `lsf-core`),
  `lsf-app` the app (`lsf-checkout` and `lsf-admin` in one crate: `config`, `checkout`, `admin`)
  and `lsf-cli` the command line: `cargo build -p lsf-cli` builds `lsf`. `lsf-app` and
  `lsf-core` build without Liquid or the renderer. Nothing else changes for someone who uses
  `lsf`. [Architecture](docs/architecture.md).
- **An extension must declare the metafields it reads through `shopify.appMetafields`.** As on
  Shopify, `appMetafields` has only what `[[extensions.metafields]]` declares, the cart's too: an
  extension that writes a cart metafield with `applyMetafieldChange` and reads it back declares it
  (`owner_type = "CART"`). Shopify's checkout UI template in Preact reads
  `$app.requestedFreeGift` without declaring it: its checkbox does not stay ticked until the
  declaration is added, as the playground's `ui-preact` now does.

### Fixed

- **Uninstalling the app stops its Functions.** After `POST /__lsf/admin/uninstall` (or
  `appUninstall`), the app's payment customization stayed enabled and its Function kept hiding a
  payment method at the checkout. As Shopify removes an uninstalled app's Functions with their
  owners, the uninstall now deletes every object that runs one of them, with its metafields: payment
  and delivery customizations, validations, cart transforms, fulfillment constraint rules and app
  discounts, those of the store data included (`removedFunctionOwners` counts them). While the app
  is uninstalled, the shop's checkout runs none of its Functions, even one a scenario names in
  `extensions`, and the Functions dialog says "the app is not installed on the shop". A reinstall
  finds none of them, as on Shopify: the app creates them again.
  [Uninstall](docs/admin.md#tokens).
- **An uninstalled app's extensions no longer render.** After an uninstall, the checkout, the
  thank-you page and the customer account still loaded the app's UI extensions, and the
  storefront still loaded its script tags. As Shopify removes an uninstalled app's extensions,
  lsf now renders none of its UI extensions while the app is not installed on the shop, the
  thank-you page of an order placed before included, and an extension's full page answers `404`.
  Each place says why, "the app is not installed on the shop": a banner on the checkout and the
  customer account, the Functions dialog ("Extensions not loaded"), `?targets=all`, the home page
  (a banner, a problem, `app not installed` on each extension, `"installed": false` in its
  JSON) and the terminal, which also says when the app is installed again. The uninstall
  deletes its script tags and web pixel too (`removedScriptTags`, `removedWebPixels`), as
  Shopify does. A reinstall renders the UI extensions again (Shopify restores the active ones
  within 24 hours; lsf has no checkout editor); the app creates its script tags and pixel again.
  Theme app blocks and embeds render nothing in lsf, installed or not.
  [Uninstall](docs/admin.md#tokens).
- **An uninstalled app's proxy no longer forwards.** After an uninstall, `/apps/<subpath>` still
  went to the app's backend. A store gets the app's proxy with the app (shopify.dev: a change of
  `prefix` or `subpath` reaches only new installations, or a store that uninstalls and reinstalls
  the app), so the uninstall removes it: while the app is not installed, the storefront answers
  the path as a page it does not have (the theme's `404` page) and nothing reaches the backend.
  `X-LSF-Reason` and the terminal say why (`app proxy: the app is not installed on the shop:
  uninstalling it removed its app proxy, so /apps/reviews is the storefront's 404 …`). A reinstall
  forwards it again. [App proxy](docs/admin.md#app-proxy).
- **An uninstall deletes the app's own metafield definitions; a reinstall brings them back.**
  shopify.dev: "When a user uninstalls your app, Shopify deletes its app-owned metafield
  definitions and temporarily retains the metafields and their values without a definition", and
  a reinstall "recreates the definitions and associates the existing metafields with them", under
  new ids. lsf now does the same for the definitions of the app's reserved namespaces (`$app`,
  `$app:<name>`), declared in `shopify.app.toml` or made with `metafieldDefinitionCreate`
  (`removedMetafieldDefinitions`): their metafields keep their values, and the reinstall creates
  the definitions again with new ids, so an app that kept a definition id finds it gone, as on
  Shopify. While uninstalled, an order no longer copies the cart metafields of a deleted `$app`
  definition. Definitions of any other namespace (`custom`, or one the app chose) are the
  merchant's and stay, ids included. [Uninstall](docs/admin.md#tokens).
- **An uninstall deletes the app's own metaobject definitions; a reinstall brings them back.**
  shopify.dev says the same of metaobjects ("Manage metaobject definitions", "App uninstall and
  reinstall"). The definitions of the app's reserved types (`$app:<type>`), from the store data
  or `metaobjectDefinitionCreate`, are deleted (`removedMetaobjectDefinitions`), and their
  metaobjects are kept aside (`retainedMetaobjects`). A reinstall creates each definition again
  under a new id and gives it back its metaobjects, ids unchanged. Other types are the merchant's
  and stay. [Uninstall](docs/admin.md#tokens).
- **Adding a checkout scenario no longer throws away what the backend set up.** Without
  `--static`, a file added or changed in `checkouts/` re-seeded the shops of the Admin API,
  dropping the app's subscription, customizations and other writes. Scenarios are checkout
  inputs: lsf now reads them again on top of the same store data and keeps the Admin API's state
  (`checkout scenarios reloaded (checkouts/b2b.json added): the store and what the Admin API
  changed are kept (no re-seed)`). Only a change of the store's own data seeds the shops again,
  and its line names the files (`store data reloaded (products/tee.json changed): the shops of the
  Admin API were seeded again, …`); `schema/` and `README.md` change nothing.
  [When the data changes on disk](docs/cli.md#when-the-data-changes-on-disk).
- **`appSubscriptionCancel` sends no webhook under Shopify App Pricing.** With `legacy = false`
  in the plans file, it still delivered `app_subscriptions/update` (`CANCELLED`), which Shopify
  App Pricing "no longer sends" for any subscription change after April 28, 2026. An uninstall
  sends none in either mode. [Managed pricing](docs/admin.md#managed-pricing).
- **`--watch` no longer rebuilds the Functions when the app's web front end builds.** In the
  app's own folder, `--watch` now follows only what the builds of the Functions read: the
  workspace's manifests and lock files (`Cargo.toml`, `Cargo.lock`, `package.json`, …) and the
  folders that hold a crate or a package (`crates/common/Cargo.toml`). An openshop project, whose
  trunk writes `dist/` at its root on every save of `src/`, built every Function and reloaded the
  pages each time. `dist/`, `build/`, `target/`, `node_modules/`, dot folders (`.git/`,
  `.shopify/`) and the app's own sources are left alone.
- **Polaris offline with the app and lsf on one host.** With the app on `localhost:3000` and lsf
  on `localhost:9393`, Polaris's icons (`/__lsf/cdn/…/icons/<id>.svg`) failed with "blocked by
  CORS policy", and an `s-button slot="secondary-actions"` of an `s-banner` took no room: lsf's
  App Bridge gave its session token to lsf's own origin, and lsf answered the preflight `405`.
  lsf's App Bridge no longer adds the token, `X-Requested-With` or `Accept-Language` to requests
  to lsf's origin (on Shopify the admin and the CDN are never the app's host), and `/__lsf/cdn/`,
  `/__lsf/polaris`, `/__lsf/admin/app-bridge.js` and `/cdn/` answer `OPTIONS` preflights
  (`204`, any origin, the headers asked for, `Max-Age` a day).
- **A control API body that is JSON is read as JSON** without `Content-Type: application/json`
  (`fetch` sends a string as `text/plain`): `PUT /__lsf/session` used to read it as a form.
- **`lsf routes` no longer lists `/account`**, a page of lsf's that `render --strict` refuses:
  a loop of `render --strict` over `routes` passes on a stock theme. `GET /__lsf/status` lists
  the same routes.
- **A large store loads much faster.** The default product recommendations took a time cubic in
  the number of products: 3,000 products took 5 to 6 s to load, and now take 0.3 s. This speeds
  up startup and `--watch` too.
- **No shipping in any total before an address.** Without an address with a country, no
  shipping rate is selected: `cost.shipping` is 0, and `cost.total`, `cost.dueNow` and
  `shopify.cost.totalAmount` leave it out, as the page does ("Enter shipping address").
  `shopify.cost.totalShippingAmount` is undefined, and the Functions read
  `selectedDeliveryOption: null` and a `cart.cost.totalAmount` without shipping, as on Shopify.
- **One country for a buyer with no address.** With no address and no selected country, the
  Functions, the checkout UI extensions and the address form all read the shop's country
  (`shop.address.country_code`), the one of its primary market. The extensions used to read no
  country, and the market `primary`; they now read the market the Functions read.
- **`--build` then no "no Wasm module".** `app function run`, `app dev` and `serve` printed
  the diagnostics of the app as it was before `--build` built it.
- **`app check` says what a trapped Function threw:** `the Function trapped: Error: boom (in
  b2b)`, the last message it logged. Each JSON failure has the `logs` of the first scenario it
  failed in.
- **The hints say `npx local-storefront` when it was installed with npm**, where `lsf` is not
  on the PATH: ``build it with `npx local-storefront app build x` ``, the `next:` lines, the home
  page. The launcher sets `LSF_COMMAND`; the binary of a release says `lsf`.
- **An unknown filter close to one of Shopify has a hint:** `{{ x | uppercas }}` gives
  `hint: did you mean "upcase"?`, in the text and the JSON of `check`.
- **`app init`'s `discount-code` scenario** says that one Function "applies as a code
  discount".
- **One is in the singular, everywhere lsf counts:** `1 Liquid error · 0 warnings` (`render`),
  `wrote 1 file, kept 15 existing ones` (`init`, `app init`), `1 file formatted` (`fmt`),
  `validate`, `check`, the banner, the store data's errors and the status page. No more `(s)`.
- **`shopify.appMetafields` has the metafields an extension declares, of every resource.** As on
  Shopify, it holds the metafields of `[[extensions.metafields]]` (and of
  `[[extensions.targeting.metafields]]` at their target): those of the shop, of the products and
  variants of the lines (and of the components of a bundle), of the customer logged in, of the
  company and location they buy for and of the cart, each with its `target` (`shop`, `product`,
  `variant`, …) and the GID of the resource, read from the store data and what the backend sets
  through the Admin API. An `owner_type` keeps one resource. It held only the cart's, each one
  whether declared or not: an extension now reads a metafield of the cart only if it declares it.
  The order status page of the customer account reads them too (it had none), with the company
  location the order was placed for and the values typed as its `AppMetafield` types them
  (`false`, `2`). A namespace written `app--<id>--<name>` in the toml reads nothing, as on
  Shopify, and the loader warns with the `$app:<name>` to write. `shopify.shop.id` is the id of
  the shop of the store data
  ([#25](https://github.com/0xtlt/local-storefront/issues/25)).
- **A code whose discount Function answers a candidate of 0 is applied, at 0.** A candidate of
  0 % (or a fixed 0.00), which a Function answers to accept a code before the gift it stands for
  reaches the cart, made the code "not applicable". As on Shopify, the code is now accepted and
  its discount shows at 0 on the lines it targets: in the page, `discounts`, `discountCodes`,
  the extensions' `discountAllocations` and the output of `lsf app function run`. An order or a
  shipping candidate of 0 applies the same way; a shipping one gives way to a shipping discount
  that takes something off the option the buyer selected. An automatic discount of 0 shows too,
  at 0 (a Function that reads its percentage from settings that are not set)
  ([#26](https://github.com/0xtlt/local-storefront/issues/26)).

## 0.2.0 - 2026-10-30

lsf now runs the checkout and the customer account of a Shopify app: its Functions and its UI
extensions, on your machine. Classic customer accounts are no longer simulated: see Changed.

### Added

- **A local checkout for Shopify apps.** `lsf app dev --app <dir>` serves the checkout of an
  app without a theme, and `lsf serve --app <dir>` leads the `/checkout` of a theme to it. It
  runs the Shopify Functions of the app (cart transform, discounts, delivery and payment
  customization, validation) from their built Wasm modules, and shows its checkout UI
  extensions in the page, in the standard one-page look or in the Shop app's (`--variant shop`).
  `--extension <handle>` (repeatable) picks the extensions to use, `--build` runs the build
  command of each Function first. A badge in the corner of the page opens a dialog with the
  input and output of each Function run side by side. API versions 2026-10 and 2027-01 are
  supported. Guide: [checkout](docs/checkout.md).
- **Customer account UI extensions, and every checkout target.** With an app, `/account` is the
  customer account of Shopify's new customer accounts: the orders of the customer logged in (from
  the store data, and the last order placed in the local checkout), the status of an order (its
  shipments, what is not shipped, its returns), the profile (addresses, and the company and
  locations of a B2B customer), and the full pages of extensions. The 24 targets of customer
  account UI extensions render there, with the `shopify` global they read (the account logged in,
  the order, navigation, toasts) and the components only customer accounts have (`s-page`,
  `s-customer-account-action`, `s-menu`, …); an order action opens its dialog. The checkout gains
  the 14 targets it lacked: pickup locations and points, the targets of payment apps (with the
  form dialog), the gift card form, bundle lines, the chat, and the address autocomplete, which
  suggests addresses as the buyer types. An order of the store data can list its `returns`. See
  [customer account](docs/checkout.md#customer-account).
- **The commands of a Shopify app are under `lsf app`**, named as Shopify's CLI names them:
  `app dev` serves the app alone (its checkout and its customer account), `app init` writes
  checkout scenarios, `app build` builds its extensions, `app check` runs the scenarios for CI,
  `app function run` and `app function build` its Functions. [CLI reference](docs/cli.md#app).
- **Testing an app's checkout in CI.** `lsf app check` runs the Functions of every scenario
  and exits with 1 when one fails or does not validate, or when there is no scenario. `lsf validate`
  checks the app of the folder too (with the demo store when the app has no store data), and its
  JSON counts the app's errors with the data's. The checkout and the customer account set `data-lsf-ready` on the document once nothing is
  on its way and every extension rendered (`aria-busy` meanwhile). `DELETE /__lsf/sessions` resets
  every session. The orders of a session are numbered after the last one of the store data, one
  after the other whatever the other sessions do, and placed at its `now`: a test reads the same
  number and date every time. [Testing guide](docs/testing.md#a-checkout-in-a-test).
- **Commands of an app need no `--app` in its folder** (`lsf app dev`, `lsf app init`,
  `lsf app function run`, `lsf app check`), and read its `shopify-local/` from anywhere.
  `lsf app init` gives every Function the settings its input query reads
  (`[extensions.input.variables]`): those of the app's own store data, or empty values of the
  right type in `app-settings.json`, so that the scenarios it writes pass `app check`; a
  variable without a value says where to give it. The frame of an extension is in the language
  it renders in (`lang`). `lsf app function run` says why a
  Function asked for did not run (no discount names it, an empty cart, an error of the app) and
  exits with 1; a handle or a scenario close to an existing one is suggested. Its `--session`
  takes `data` as `PUT /__lsf/session` does: a discount or settings for one run.
- **Watch mode for the checkout of an app.** `lsf app dev --app <dir> --watch` (and `serve
  --app <dir> --watch`) builds a Function again when its files (or the files the Functions
  share, such as a Cargo workspace's `Cargo.toml`) change, and reloads the open checkout,
  thank-you and home pages once the builds are over, as `shopify app dev` does. At start it
  builds only the Functions that have no module or changed since the last watch (all with
  `--build`). The checkout runs a copy of the last module that built, never what a build is
  writing: a build that fails keeps it and shows its first error on the page and on the home
  page. A checkout UI extension's bundle, `shopify.app.toml` and the scenarios reload the pages
  too. See [watch mode](docs/checkout.md#watch-mode).
- **Log in as a customer at the checkout.** "Log in" in the Contact section of the local
  checkout opens a menu of the customers of the store data. Picking one logs the checkout (and
  the session) in as them: the Contact section shows the account, Delivery offers their saved
  addresses with the default one chosen, a B2B buyer picks the location they buy for, and the
  Functions and extensions read the new buyer. `/checkout?customer=<email>` opens the checkout
  logged in, and the home page lists the customers. The demo store has a profile of each kind
  (new, returning, B2B, abroad). See [log in as a customer](docs/checkout.md#log-in-as-a-customer).
- **The checkout looks like Shopify's.** The standard look of the local checkout now follows
  Shopify's one-page checkout: its header, the two columns with the grey order summary, floating
  labels, grouped payment and shipping choices, the order summary with its quantity badges, and
  a mobile layout with a folded summary. The checkout UI extensions use the same design tokens
  (`tokens.css`), so they look native in it. See [the look of the page](docs/checkout.md#the-look-of-the-page).
- **`lsf app function run [handle]... --app <dir>`** runs the Functions once, without a server, and
  prints each input, output and log with the checkout that results. `--session` and
  `--checkout` set the cart and what the buyer entered, `--format json` is for tools. It exits
  with `1` when a Function fails to run.
- **A `checkout` section in the store data** sets the shipping rates, the payment methods, the
  discounts that use the Functions of the app, the settings of its other Functions and a tax
  rate. Without it: one "Standard" rate at 5.00, three payment methods and no tax.
- **The checkout in the control API.** `/checkouts/cn/<token>` serves the page, `.json` its
  state, `/update` takes operations and `/complete` places the order, which `/thank-you` shows.
  `GET /__lsf/checkout?session=<id>` reads the state of a session for tests. Without `--app`,
  `/checkout` is the summary of the cart, as before.
- **The Functions of the checkout can read the fields they could not.** An input query that asks
  for `Cart.deliverableLines`, the discount applications and allocations of the cart, its lines
  and its delivery groups (each Function sees the discounts the steps before it applied),
  `PurchasingCompany.contact`, the dates, locale and totals of companies and their locations,
  `MailingAddress.market`, `PaymentCustomizationPaymentMethod.placements` or `Shop.metaobject`
  now runs instead of failing with the field's path. The store data sets them: `created_at`,
  `updated_at`, `locale`, `orders_count` and `total_spent` on companies and locations, a
  `company_contact` on a customer, `localization.markets`, `placements` on a payment method;
  everything has a default. Metaobjects are the ones of the data.
- **Network access for Functions.** The `.fetch` targets of the lines discounts, delivery
  discounts and cart validation Functions run: the checkout sends the HTTP request they ask for
  and gives the response to the `.run` target as `fetchResult`, with Shopify's limits (GET and
  POST, HTTPS, a timeout of 100 to 2000 ms, 100 KB, no redirect; `http` is accepted for
  `localhost`). The request and the response show in the Functions dialog and in
  `lsf app function run`.
- **A deferred payment in the checkout.** A line sold with a selling plan that takes a deposit
  (a pre-order, try before you buy: `checkout_charge` in the store data) is no longer charged in
  full. The order summary shows the total, what is **due today** and what is **due later**; the
  state (`.json`) has `cost.dueNow`, `cost.dueLater` and `cost.checkoutChargeAmount` (the
  `cart.checkout_charge_amount` of the storefront cart) and `dueLater` on the lines; the order
  records `totalReceived` and `totalOutstanding`, and the thank-you page shows the remaining
  balance. `lsf app function run` prints it. Shipping is due today and tax follows the amount it is
  on. The `shopify` global and the Function input do not change: Shopify exposes no deferred
  amount there.
- **A home page for the checkout.** `GET /` of `lsf app dev` (and `/__lsf/checkouts`, which
  `lsf serve --app` prints when it starts, as the theme owns `/`) lists every Function of the
  app (API, targets, build status, network access with a `.fetch` target, what is wrong with
  it) and every checkout UI extension (API version, capabilities, targets, and the place each
  target has on the page, or that it has none), and a card for every checkout scenario, with a
  link that opens a checkout for it. `?format=json` answers what it says.
- **Checkout scenarios.** A file of `checkouts/` in the store data (`checkouts/<name>.json`) is a
  named checkout setup, on top of the store data: the cart (lines, quantities, selling plans,
  note, attributes), the buyer (a guest, a customer, a company location), the country and
  language, what the buyer entered (email, address, discount codes, delivery, payment),
  overrides of the `checkout` section, the look and the extensions that run.
  `/checkout?scenario=<name>` opens a fresh checkout for it, in a new session. Scenarios are
  validated (`validate`, with the codes of the data guide), reloaded when they change, have a
  JSON Schema (`schema checkout-scenario`, and `schema/` written by `init`), and one with errors
  is listed with them, not hidden.
- **`lsf app init [--app <dir>]`** writes the scenarios: for the Functions and checkout UI
  extensions of the app (a cart a cart transform can merge or expand, a cart and a code for a
  discount, several shipping rates or payment methods, a cart a validation refuses, one with
  everything for the extensions), and always a guest, a customer, a B2B buyer, another country
  and language, a subscription, a pre-order with a deposit, the Shop look and a large cart. It
  creates the data folder with the demo store if there is none, uses the products of an existing
  one, and never overwrites without `--force`.
- **`/checkout?targets=all`** opens a checkout where every extension renders at every target it
  declares: the steps of a cart that ships are always there, the thank-you page is shown under the
  checkout, and a panel lists each target as shown, without a place on the page, or waiting for
  something the cart lacks.
- **`lsf app function run --scenario <name>`** runs the Functions for a scenario.
- **`lsf app function build`** builds the Functions of an app. A JavaScript or TypeScript Function
  without a build command, as Shopify's CLI generates it, is built by lsf as
  `shopify app function build` builds it, without Node: its types (`generated/api.ts`, as
  graphql-codegen writes them), `dist/function.js` (bundled as esbuild does, enums inlined and
  unused code left out), then its module, the bytes Javy writes. `--build` and `--watch` build
  it too.
- **Rust Functions run as `cargo build` writes them**: after a Function's build command
  (`--build`, `--watch`, `lsf app function build`), a module that calls the Function Wasm API by name
  is linked to it with Shopify's `shopify-function-trampoline` (2.0.1, as a library), as
  `shopify app build` does, to the byte.
- **`lsf app build`** builds the extensions of an app as `shopify app build` does, without
  Node: its Functions, and its checkout UI extensions whose targets name their sources, bundled
  (JSX with the `jsxImportSource` of their `tsconfig.json`, minified) into `dist/<handle>.js`.
  `--build` and `--watch` build those too. Without a `node_modules`, `preact`,
  `@preact/signals`, `@shopify/ui-extensions` and `@shopify/shopify_function` come from lsf's
  own copy.

### Changed

- **Customer accounts are Shopify's new ones only.** Classic customer accounts are no longer
  simulated: the theme's `templates/customers` are never rendered, every `/account/…` page and
  account form (login, register, password reset, addresses) leads to `/account`, and customers
  have no password. Data that still sets `shop.customer_accounts` or a customer's `password` is
  read as before: those fields are ignored, with a warning. `serve` and `check` say that the
  theme's `templates/customers` are not rendered (a warning of `check`, which does not fail). `/__lsf/status` no longer has
  `customer_accounts`.
- **A field the format does not have no longer drops its file.** It is still an error, and is
  left out: the rest of the file is read, and one typo gives one error. `data.checkout` of
  `PUT /__lsf/session` replaces the store data's field by field, as a scenario's does, and keeps
  the settings of the Functions it does not name.
- **The binaries are larger: they run the checkout.** lsf runs Functions with only what it uses
  of wasmtime (the compiler for the machine it runs on) and Shopify's providers compressed
  (1.6 MB instead of 9.7 MB), and has one copy of each Wasm library; the release binaries leave
  out their symbol table and are optimized as a whole. `lsf` for macOS on Apple silicon is 40 MB
  (18 MB to download), up from 33 MB (12 MB) in 0.1.18. The storefront renders as fast as
  before.

## 0.1.18 - 2026-10-21

### Changed

- **Security update of the dependencies.** 33 crates are updated, among them `brotli` 9,
  `jsonschema` 0.59 and `base64` 0.23. Nothing changes in what `lsf` does.
- **The release binaries are built with Rust 1.99.1**, up from 1.98.1.
- **The npm packages are built and published with Node 24.22.1.** The release and npm workflows
  pin it, and the development toolchain pins Node 26.12.1.

## 0.1.17 - 2026-10-20

### Added

- **The pages of metaobjects are served.** With `templates/metaobject/<type>.json` (or
  `.liquid`) in the theme, `/pages/<type>/<handle>` renders the entry, with the global
  `metaobject`, `?view=` and a per-entry `template_suffix` in the data. `shopify://metaobjects/`
  links lead there, and `metaobject.system.url` is nil for a type without a template, as on a
  store whose definition has no web pages.
- **A 404 page says why.** The server answers it with an `x-lsf-not-found` header (no route,
  unknown resource, no template) and logs the reason, so a test can fail on it.
- **`shopify_asset_url` files are served.** `vendor/qrcode.js` is the MIT qrcode.js library that
  Shopify serves, so a gift card draws its QR code. The other documented files are small local
  stand-ins: `shopify_common.js` formats money, the others only warn in the console.

### Fixed

- **`metaobject` and `metaobject_list` settings are metaobjects**, not the string of the
  template: a handle, `type.handle`, `type/handle` or a `gid://shopify/Metaobject/` id.
- **The search follows `sort_by`**: `relevance`, `price-ascending` and `price-descending`, with
  the other results after the products under a price order, as Shopify does. `search.sort_by`
  says which order applies and `search.sort_options` lists those three.
- **`'now' | date: '%N'` differs between calls.** Each `'now'` reads the clock again, as on
  Shopify, so ids built from it no longer repeat. With a fixed `now` in the data, each read is one
  nanosecond after the previous one: renders stay reproducible.

## 0.1.16 - 2026-10-14

### Added

- **`npx local-storefront check` finds what Shopify's strict Liquid parser rejects, and
  `npx local-storefront check --fix` rewrites it.** Shopify is moving every store to a strict
  parser, and a file it rejects can no longer be uploaded
  (`Expected end_of_string but found pipe`). `npx local-storefront check` reports each tag and
  output it rejects, with Shopify's message and the fix, in `.liquid` files and in the `liquid`
  settings of JSON templates, section groups and `config/settings_data.json`.
  - Where what the code meant is clear, `--fix` does it, and the report says how the output
    changes: a filter in a `render` argument is computed into a variable on the line above, an
    `and` in an argument is evaluated, `('X' | downcase)` is the string and not the variable
    `X`, `reversed` after `limit` reverses, `=` in a condition compares, and the body of
    `{% when 'x' and y %}` goes in `{% if y %}` (a `case` with an `else` becomes
    `if`/`elsif`/`else`).
  - Elsewhere it keeps today's output, as Shopify's own migration does, without the comment it
    leaves in the file.
  - A fix keeps the layout of the markup and is checked before it is written. Markup without a
    fix is reported with the reason.
  - `--format json` prints one JSON document, for tools and AI agents: each problem with its
    code, file, line, markup and fix.

  See [strict
  Liquid](https://github.com/0xtlt/local-storefront/blob/main/docs/strict-liquid.md).

### Changed

- **The documentation is organised by task.** The README is short: what local-storefront does,
  how to start, and a link per task. Each task has its guide in `docs/` (data, testing, strict
  Liquid, formatting, performance, storefront), next to two references: the
  [CLI](https://github.com/0xtlt/local-storefront/blob/main/docs/cli.md), with every option,
  exit code and output format, and the [control
  API](https://github.com/0xtlt/local-storefront/blob/main/docs/control-api.md).
  [docs/agents.md](https://github.com/0xtlt/local-storefront/blob/main/docs/agents.md) tells an
  AI agent how to drive local-storefront, and `llms.txt` lists the documentation for language
  models.
- Messages name the subcommand to run (`the init command`, `run again with --fix`) rather
  than `lsf`, which is only the command of the installs that are not from npm.

## 0.1.15 - 2026-10-14

### Added

- **`lsf fmt`** formats the Liquid files of a theme the way Prettier does with Shopify's
  Liquid plugin, without Node: Liquid, HTML, the JSON of `{% schema %}`, the CSS of
  `<style>`, `{% style %}` and `{% stylesheet %}`, and the JavaScript of `<script>` and
  `{% javascript %}`. `lsf fmt --check` lists the files that are not formatted and fails if
  there are any, for CI. The 283 files of Shopify's Horizon theme are checked in 0.08 s, where
  Prettier takes 1.6 s.
  - It reads the configuration Prettier reads: `.prettierrc`, `.editorconfig` and
    `.prettierignore`. `--option printWidth=100` sets an option from the command line, and
    `--stdin` (with `--stdin-filepath`) formats what an editor sends. Prettier's options for
    JavaScript (`semi`, `trailingComma`, `arrowParens`, ...) apply too.
  - Files named one by one, as a commit hook names them, are formatted when they are `.liquid`
    files that `.prettierignore` does not name. A file is replaced in one step, with its
    permissions. A file that cannot be formatted is reported, and the others are formatted
    all the same.

  See [what differs from Prettier](https://github.com/0xtlt/local-storefront/blob/main/docs/compatibility.md#formatting-lsf-fmt).
- **Feature previews.** `--preview <name>` (or `LSF_PREVIEW`) turns on a feature preview of
  Shopify, as a development store can. Without it, a page renders as on a live store: a theme
  that uses a preview gets the error a live store gives, and `lsf` names the preview to turn
  on. The first one is `liquid-july-26`, Shopify's "Liquid July '26 changes":
  - **`{% block 'name', parameter: value %}content{% endblock %}`** renders
    `blocks/name.liquid` from a template or a layout. The block sees its parameters and its
    body, as `content` or `block.content`; a parameter named after a setting of its schema
    sets `block.settings`; arrays can be written in place. With it, Shopify's Skeleton theme
    renders.
  - **`{% partial 'name' %}`** and **`@shopify/partial-rendering`**: a theme that uses
    partials gets an import map for the module, and `partials.refresh('name')` swaps in the
    region from the server, keeping focus, form values and scroll, in loops and table rows
    too. Shopify has not published its own markup yet: lsf marks a partial with HTML comments.

### Fixed

- A `file_reference` metafield or metaobject field that points to a video (`.mp4`, `.mov`,
  `.webm`, `.m4v`) is a video, as on Shopify: `sources`, `preview_image` and `video_tag`
  work, and `metafield_tag` prints a `<video>` as Shopify does. It was a plain file with only
  a `url`, so themes rendered it empty. The poster is the `.jpg` of the same name. A `.mov` is
  announced as `video/mp4`, as Shopify announces the MP4 it makes of it, so that Chrome and
  Firefox play it; the `video` setting does the same.
- Files of `files/` are served in parts when the browser asks with a `Range` header. Safari
  played no video without it, and no browser could seek in one. Only the part asked for is
  read, and a part is never compressed. `.m4v` files are served as `video/mp4`.
- Metafield values are the objects Shopify gives for four more types. A `color` is a color
  (`.red`, `.rgb`...), not text. An `article_reference` is the article, named `blog/article`
  or by the handle of an article that only one blog has. Every measurement type (`distance`,
  `temperature`, `duration`...) has a `type`, a `value` and a `unit`, and `metafield_tag`
  prints it with two decimals at most, where only `weight`, `volume` and `dimension` were
  printed. A `money` has the `currency` it is written in, and is formatted in it: `€5,00 EUR`
  in a store that sells in dollars.
- `request.locale.direction` (and that of every locale) is `rtl` for Arabic, Hebrew, Persian
  and the other languages written right to left, and follows the script when the locale names
  one (`pa-Arab`). Horizon sets `dir` on `<html>` from it.
- The `applicable_disclosures` filter keeps every disclosure, as Shopify does when it cannot
  tell the buyer's country. It was an unknown filter that printed a warning.

## 0.1.14 - 2026-10-06

### Fixed

- `lsf serve` gives back the memory it used to encode images. A server left running for
  three hours on a large theme held 1 GB. After every image of that theme is asked for,
  277 MB are left where 664 MB were, and pages render as fast as before.

## 0.1.13 - 2026-10-05

### Added

- **`Server-Timing` on every response**, as on a Shopify storefront: `processing` for the
  whole, then `render`, `template`, `layout`, `compress` and `throttle`.
  `lsf serve --timings` also names every section and every theme block.
- **`lsf profile <path>`** renders one page and prints what rendered what: the template, the
  layout, each section, its blocks and their snippets, with the time of each. `--lines` goes
  down to the tag.
- **`lsf profile --points`** counts what the page asks of a store instead of the time it
  takes here: a product, a metafield, a search. A local server reads a metafield as fast as
  a title, Shopify does not. `--cost` changes what each kind of thing costs.
- **Flame graphs.** `/__lsf/profile?path=<path>&html=1` opens the profile in speedscope, the
  viewer that `shopify theme profile` opens. It is embedded: nothing is fetched.
- `/__lsf/timings` adds up the renders so far by template, section and block.

## 0.1.12 - 2026-10-04

The responses of `lsf serve` now weigh what a Shopify storefront sends, so that the sizes a
browser or an audit reports are those of the real store.

### Added

- **Minified stylesheets and scripts**, by the rules Shopify follows, with source maps: the
  tools of a browser still show the files as written. `--no-minify` serves them as they are.
- **Images in the lightest format the browser reads.** AVIF, WebP or the format of the file,
  chosen from the `Accept` header as Shopify's image CDN does.
- **`Cache-Control`** as Shopify sends it for each kind of response: pages, assets, images,
  fonts, the cart.
- **The resource hints Shopify adds.** The `Link` header preloads the stylesheets and the
  scripts that block rendering, and connects to the origins they come from.

### Fixed

- The `Link` header of `preload: true` and `preload_tag` is written as Shopify writes it,
  one header with quoted values. A browser could not read the previous form for an image
  with a `srcset`.
- A font preloaded with `preload_tag` gets `crossorigin`, without which a browser does not
  use it.
- The `?v=` of a `.liquid` asset changes with the theme settings it is rendered with, not
  only with its source.
- A placeholder image served from the cache says that it is one (`x-lsf-placeholder`).

## 0.1.11 - 2026-10-03

### Added

- **Responses are compressed** with Brotli or gzip for the clients that accept it, as on a
  storefront. A 420 KB collection page of Horizon becomes 53 KB. `--no-compression` sends
  responses as they are.

### Fixed

- `{{-` and `{%-` trim whitespace the way storefronts do, which is not quite what the
  language says: when the text before one is nothing but whitespace, its first character
  stays. Line breaks in a `<title>` or between two outputs now match Shopify's.
- The rules of `robots.txt` are on their own lines, those a theme adds with
  `{{ 'Disallow: ...' }}` included.

## 0.1.10 - 2026-10-03

### Changed

- **`--live-reload` goes through a WebSocket.** A page reloads about 100 ms after a save,
  where it asked the server every 700 ms. It still does when the socket cannot open, behind
  a proxy for example.
- A page survives a restart of the server, and reloads if the files changed meanwhile.
- A save or a build that writes several files reloads the pages once.
- Only the directories a theme is made of and the data directory are watched, which is much
  faster for a theme that keeps `node_modules` next to its sources. A data directory outside
  the theme (`--data`) now reloads the pages. A file outside those directories no longer
  does.

## 0.1.9 - 2026-10-03

### Added

- **Selling plans.** `selling_plan_groups` in the data describes subscriptions. Products and
  variants expose their groups and allocations, `?selling_plan=<id>` selects a plan, and
  `/cart/add.js` and `/cart/change.js` take `selling_plan`.
- **Store pickup.** `locations` in the data are the places that stock the products, and
  variants have `store_availabilities`. `/variants/<id>?section_id=` renders a section for a
  variant, which is how Dawn loads pickup availability.
- **Native swatches.** `swatches` in the data gives a color or an image to option values:
  `product_option_value.swatch` and `filter_value.swatch` are filled.
- The demo store has a subscription, two locations and swatches for its colors.

## 0.1.8 - 2026-10-02

### Changed

- Without `--port`, `lsf serve` listens on the next free port when 9292 is taken, up to
  9391, and says which. A port given with `--port` is still that port or an error.

## 0.1.7 - 2026-10-02

The Liquid engine is now checked against
[liquid-spec](https://github.com/Shopify/liquid-spec), Shopify's test suite of the language:
4,608 of the 4,630 specs that apply pass. The others are listed in
[compatibility](https://github.com/0xtlt/local-storefront/blob/main/docs/compatibility.md).

### Added

- `self`, the object that reads the variables of the current scope (`self[name]`).

### Fixed

- `truncate` crashed when given the smallest 64-bit integer.
- Comparing with a float that is not a number raised an error instead of being false.
- `replace`, `replace_first` and `remove` ignored the back-references of the replacement
  (`\'`, `` \` ``, `\+`, `\k<name>`).
- Floats from 1e15 on were printed in fixed notation, and some JSON numbers were read one
  bit off.
- `{% assign x ) = 1 %}` was accepted, and `{% docEXTRA %}` was taken for a nested `doc`.
- An error at the end of a `{% liquid %}` tag was reported one line too far.
- A contact or newsletter form given an `id` posts back to that id
  (`action="/contact#ContactForm"`), as on Shopify.

## 0.1.6 - 2026-10-02

### Added

- **Customer accounts.** `/account` shows who is logged in and lets the visitor become any
  customer of the data. A theme with `templates/customers` keeps its own pages. A customer
  is named by email, `default` or `none`: in the data, in a session, with `/__lsf/login` or
  with `lsf serve --customer`.
- **B2B.** Companies with their locations, the customers that buy for them, and the objects
  that go with them: `customer.b2b?`, `current_company`, `current_location`. Catalogs are
  not simulated.
- **`/robots.txt` and `/sitemap.xml`.** `robots.txt` renders `templates/robots.txt.liquid`,
  or Shopify's default rules.
- **`/password`** shows the password page of the theme and accepts `shop.password`.
- `placeholder_svg_tag` draws Shopify's own illustrations for the three names that are
  published under a free license.

### Changed

- A password in the data no longer locks the storefront: nothing redirects to `/password`.

## 0.1.5 - 2026-10-02

### Added

- **`lsf serve --throttle`** answers requests late, so that loading states can be tested:
  every request (`--throttle 300ms`) or one kind of request
  (`--throttle cart=500ms,cart-add=1s`).
- **Presets.** `simulated` answers as late as a Shopify storefront on a good connection,
  `slow` as on a slow mobile connection. A preset can be adjusted:
  `--throttle simulated,cart-add=2s`.
- A session can have a throttle of its own through `PUT /__lsf/session`: one test can be
  slow without slowing the suite.

### Changed

- The README is written around what a user does, and documents every option of `lsf serve`.

## 0.1.4 - 2026-10-02

### Added

- **The scripts Shopify adds to a page.** `content_for_header` follows Shopify's, with the
  `Shopify` object and `Shopify.loadFeatures`, and pages get `ShopifyAnalytics.meta` and
  `Shopify.analytics`.
- **`Shopify.actions`** (`getCart`, `updateCart`, `openCart`) and the `shopify:cart:*`
  events, written against the local cart.
- **`Shopify.customerPrivacy`**, loaded with `consent-tracking-api`: consent is kept in the
  browser.
- The `standard_event_data` filter, for products, collections and the cart.
- A page can render a section that belongs to another template, which a cart drawer does to
  load recommendations.

### Changed

- The `cart` cookie is set when a cart is created, `/cart.js` bundles sections, and
  `/cart/update.js` removes the attributes set to null.

## 0.1.3 - 2026-10-02

### Fixed

- `image_url` accepts decimal sizes, which themes compute with `width | times: ratio`. They
  raised "invalid integer", and the image was left out.

## 0.1.2 - 2026-10-02

### Added

- **`lsf` is on npm.** `npm install --save-dev local-storefront` gives a project the
  command, with the binary of its platform only. Run it with `npx local-storefront`:
  `npx lsf` can download an unrelated package of that name.

## 0.1.1 - 2026-10-02

### Changed

- **Rendering scales with the number of clients.** On a 14-core machine, the collection page
  of Horizon goes from 70 pages per second to 430 with one client, and from 107 to 3435
  with fourteen. The HTML is the same to the byte.
- With `--static`, an asset edited while the server runs keeps its `?v=`: nothing is read
  from disk twice.

## 0.1.0 - 2026-10-02

First release. `lsf` renders a Shopify theme without calling Shopify, so that end-to-end
tests run against a local server with no rate limit.

- **A port of Shopify's Liquid**, checked against the reference gem by 548 differential
  tests, with Shopify's objects, filters and tags, sections, blocks, JSON templates and
  locales.
- **Store data as JSON files**, with a JSON Schema and a validation that explains mistakes.
- **`lsf serve`**: storefront pages, the Cart Ajax API, the Section Rendering API, forms,
  and images transformed and served locally.
- **A control API** that gives each test session its own cart, customer and data.
- The other commands: `render`, `validate`, `check`, `init`, `routes`, `schema`, `docs`.
- One binary for macOS, Linux (glibc 2.17 or later, and static musl) and Windows, on x86-64
  and ARM64.
