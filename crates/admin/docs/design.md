# Design of the local Admin API

## Goal

The backend of a Shopify app (a React Router or Remix app, Rails, Laravel, Express, Django, Go,
…) talks to Shopify through the **GraphQL Admin API** and receives **webhooks**. `lsf` serves
a local Admin API, **API version 2026-10 only**, backed by the store data, and delivers the
webhooks of the app to its backend, signed as Shopify signs them. A developer runs and tests
their backend offline: no development store, no tunnel, no network, the same data every run,
and the failures that are hard to provoke on a real store (throttling, a missing access scope, a
webhook delivered twice) on demand.

It is to the backend what `lsf app dev` is to the checkout: the same store data, the same app
directory (`shopify.app.toml`), the same server. A product the backend creates through the Admin
API is a product of the store; an order placed in the local checkout is an order the Admin API
lists and the `orders/create` webhook reports.

Not a goal: the REST Admin API (deprecated by Shopify; `/admin/api/<v>/*.json` other than
`graphql.json` answers that it is not served), the Storefront API, the Customer Account API,
other API versions (2027-01 comes later, the way the checkout added it), the admin web UI.

## User-facing surface

```bash
lsf app dev --app ../my-app --app-url http://localhost:3000
#   serves the checkout, the customer account, and now the Admin API of the store:
#   POST http://127.0.0.1:9292/admin/api/2026-10/graphql.json
#   and delivers the app's webhooks to http://localhost:3000/<uri of each subscription>
lsf serve --app ../my-app            # the same, with a theme
lsf app webhook trigger --topic orders/create --app ../my-app [--address URL] [--api-version 2026-10]
#   as `shopify app webhook trigger`: one webhook, built from the store data, signed, sent
```

The server prints, when it starts, the Admin API URL and an access token that works at once:

```text
Admin API  http://127.0.0.1:9292/admin/api/2026-10/graphql.json
           X-Shopify-Access-Token: shpat_…   (shop: my-shop.myshopify.com)
Webhooks   3 subscriptions of shopify.app.toml → http://localhost:3000
```

| Option | What it does |
|---|---|
| `--app-url <url>` (`LSF_APP_URL`) | Where the backend of the app listens. Relative webhook `uri`s are joined to it, and an absolute one on the origin of `application_url` is moved to it. Without it, `application_url` is used. |
| `--client-secret <secret>` (`LSF_CLIENT_SECRET`) | The secret webhooks, OAuth and session tokens are signed with. Default: `SHOPIFY_API_SECRET` of the environment, else of the app's `.env`, else a fixed local secret printed at start. |

The client id is `client_id` of `shopify.app.toml` (a fixed local one without an app). The
Admin API is served whenever the server runs; the webhooks, the scopes and the client id come
from the app when there is one (`--app`).

## The HTTP protocol, as Shopify's

- `POST /admin/api/2026-10/graphql.json` with `{"query", "variables", "operationName"}`
  (`application/json`), or the query alone (`application/graphql`). `GET` with `?query=` too.
- Authentication: `X-Shopify-Access-Token`. Missing or unknown: `401`
  `{"errors":"[API] Invalid API key or access token (unrecognized login or wrong password)"}`.
- Another version (`2026-07`, `2027-01`, `unstable`): `404` with a JSON message saying lsf serves
  2026-10 only. Unlike Shopify, which serves the oldest supported version instead: a local tool
  says what it does not do.
- Response headers: `X-Shopify-API-Version: 2026-10`, `X-Request-Id`, JSON content type.
- Errors in the format of Shopify (graphql-ruby): parse errors, validation errors with
  `locations`, `path` and `extensions.code` (`undefinedField`, `argumentNotAccepted`,
  `missingRequiredArguments`, …), variable errors with `extensions.problems`, field errors with
  `path`, `ACCESS_DENIED`, `THROTTLED`, `MAX_COST_EXCEEDED`, `NOT_FOUND`. The messages are
  graphql-ruby's and Shopify's, word for word where we can know them.
- `extensions.cost` on every answer: `requestedQueryCost`, `actualQueryCost`, `throttleStatus`
  (`maximumAvailable`, `currentlyAvailable`, `restoreRate`), computed as Shopify documents it
  (scalars and enums 0, objects 1, connections 2 + `first`/`last` × the cost of a node, a
  mutation 10), a leaky bucket per shop and app. A query over 1000 points is refused
  (`MAX_COST_EXCEEDED`), an empty bucket answers `THROTTLED`. `--admin-throttle off` (or the store
  data) turns it off; the plan sets the size of the bucket.
- `extensions.lsf` (only lsf adds it, and only when there is something to say): the fields that
  were answered with a default because lsf does not simulate them (`defaulted`: their paths), so
  that a developer never mistakes a default for data.

## Shops, apps and tokens

- **One shop by default**: the store data's (`shop.permanent_domain`, e.g.
  `my-shop.myshopify.com`), on which the app is installed when the server starts, with the
  scopes of `shopify.app.toml` (`[access_scopes] scopes`; every scope when there is no app).
- **More shops for isolation**: `POST /__lsf/admin/shops {"shop": "test-1"}` creates
  `test-1.myshopify.com`, seeded from the same store data, with the app installed, and answers its
  token. Parallel tests each get a shop of their own, as storefront tests each get a session.
  A token belongs to one shop: the token, not the host, picks the shop of a request (the backend
  sends every shop to the same `http://127.0.0.1:9292`).
- **Access tokens**: the one printed at start (deterministic for a shop and an app, so a test can
  hard-code it), and those the OAuth endpoints issue. Offline tokens `shpat_…`, online tokens
  `shpua_…`.
- **OAuth**, as Shopify: `GET /admin/oauth/authorize` approves at once and redirects to the
  `redirect_uri` (checked against `[auth] redirect_urls`) with `code`, `hmac`, `host`, `shop`,
  `state`, `timestamp`; `POST /admin/oauth/access_token` exchanges the code, or a session token
  (token exchange, `urn:ietf:params:oauth:grant-type:token-exchange`, offline or online access
  token, expiring or not), or the client credentials. Every answer is Shopify's JSON.
- **Session tokens** (App Bridge's `id_token`): `POST /__lsf/admin/session-token` mints one
  (HS256 with the client secret, Shopify's claims), so that a test can call the authenticated
  routes of the backend; `GET /__lsf/admin/launch` redirects to the app URL with the query an
  embedded app is opened with (`embedded`, `hmac`, `host`, `id_token`, `locale`, `session`,
  `shop`, `timestamp`).
- **Access scopes are enforced**: a field or a mutation the token's scopes do not cover answers
  `ACCESS_DENIED`, with the scope it requires (`write_*` implies `read_*`). A table maps the root
  fields and mutations to their scope.

## The state of a shop (`state`)

A shop's state is a set of **records**: JSON objects in the shape of their GraphQL type, keyed by
their GID (`gid://shopify/Product/1`). It is seeded from the `lsf_core::Store` the server
loaded, with the ids lsf-core gave each entity, so that the GIDs of the Admin API are those the
storefront, the checkout and the Function inputs already use.

```json
{
  "__typename": "Product",
  "id": "gid://shopify/Product/1",
  "title": "Organic cotton t-shirt",
  "handle": "organic-cotton-t-shirt",
  "status": "ACTIVE",
  "tags": ["cotton"],
  "createdAt": "2026-01-01T00:00:00Z",
  "variants": ["gid://shopify/ProductVariant/11", "gid://shopify/ProductVariant/12"],
  "priceRangeV2": { "minVariantPrice": { "amount": "19.0", "currencyCode": "EUR" }, "…": "…" }
}
```

- A field holds its GraphQL value: scalars as Shopify writes them (`Money` as `"19.00"`, which
  `ProductVariant.price` reads, `Decimal` as `"19.0"`, which `MoneyV2.amount` reads,
  `UnsignedInt64` as a string of digits, `DateTime` in RFC 3339 with `Z` and no fraction, `Date`
  as `2026-01-31`), enums as their name, embedded objects (`MoneyV2`, `MoneyBag`,
  `MailingAddress`, `SEO`, `Image`, …) as nested objects, references to other records as their
  key (their GID), a connection or a list of records as an array of keys.
- An array of a connection or of a list holds the keys of records, or the objects inline when
  their type is not one the state keeps as records (`Market.regions` of `MarketRegionCountry`,
  `Customer.addressesV2` of `MailingAddress`, `ProductVariant.barcodes`, `Product.images`,
  `DiscountCodeApp.codes`, `InventoryLevel.quantities`): the resolver pages over either, and an
  inline object's cursor is made of its position and its `id` when it has one.
- **Every record has `__typename`** (all of them implement `Node` or another interface).
  Nested objects take their type from the field, or from their own `__typename` when the field
  type is abstract (`SellingPlan.pricingPolicies`, `DiscountAutomaticNode.automaticDiscount`).
- **The key of a record is its `id`**, a GID. Two types have no GID and are kept under another
  key: `ShopifyFunction` (its `id` is a UUID string, the key; `PaymentCustomization
  .shopifyFunction` holds it), `ShopLocale` (no `id`; kept under its locale, `fr`). A field of an
  object type whose value is a string is always the key of a record.
- Records are kept in **creation order**: the order of the store data for the seeded ones, then
  the order the Admin API created the others in (`ShopState::all`, `all_of` for the members of an
  interface or a union). Shopify's root connections sort by `ID` by default (`locations` and
  `markets` by `NAME`, `orders` by `PROCESSED_AT`, `appInstallations` by `INSTALLED_AT`): the
  resolver applies the `sortKey` and its default on top. The arrays of a record are in the order
  Shopify shows them (variants and media by position, the products of a collection in its manual
  order).
- New records get ids from a counter per type that starts above every seeded id
  (`ShopState::new_gid`; the discount nodes share one counter, as they share ids on Shopify).
  `ShopState::touch` sets `updatedAt` (on the records whose type has it) from the clock of the
  shop: the store data's `now` when it has one, frozen there (a test reads the same dates every
  run), the system clock to the second otherwise. Dates the store data does not give are fixed
  (noon on 2024-01-01, the date lsf-core gives undated entities), never the clock, so that the
  same data gives the same records.
- Not stored, derived by the resolver from the record: `legacyResourceId` (the number of the
  GID, a string), `defaultCursor`, and a `Count` field `<x>Count` whose record has the array
  `<x>` (`{count, precision: EXACT}`). A `Count` the data gives without the list behind it is
  stored (`CompanyLocation.ordersCount`).
- Derived values that are stored are recomputed by one function of their domain, which the seed
  and the mutations share: `catalog::refresh_product` (price ranges, `totalInventory`,
  `tracksInventory`, `hasOutOfStockVariants`, `hasOnlyDefaultVariant`, `featuredMedia`, the
  variants' `inventoryQuantity`), `customers::refresh_customer` (`orders`, `numberOfOrders`,
  `amountSpent`, `lastOrder`, `canDelete`), `content::attach_definition` (a metafield's
  `definition`, the definition's `metafields` and `metafieldsCount`).
- URLs are on the shop's primary domain (`https://<shop.domain or permanent_domain>`):
  `onlineStoreUrl` is `https://my-shop.myshopify.com/products/<handle>`, files are
  `https://<domain>/cdn/shop/files/<path>?v=<version>`, the path the storefront serves them at.
- Events: `ShopState::emit(topic, gid)` records `{topic, shop, gid, at, id}` (a `…/delete` event
  keeps a copy of the record: emit before removing, or use `ShopState::delete`);
  `take_events` hands them over. Event ids are UUIDs made of the shop and a sequence that a reset
  does not restart, so a receiver never sees one twice.
- **Shops** (`Shops`): the default shop is the store data's (`shop.permanent_domain`); `create`
  makes another from the same seed (cloned, not seeded again) and moves it to its domain: its own
  `Shop` id (`stable_id("shop", domain)`), its `myshopifyDomain`, and every value naming the old
  domain or `Shop` GID rewritten (a custom primary domain stays). `reset` puts every shop back to
  the seed and keeps the shops; `reseed` seeds again from new store data, then resets.
- Store data edited while the server runs (`--watch`) reseeds every shop: what the Admin API
  changed is dropped, and the terminal says so. `DELETE /__lsf/admin` reseeds on demand.

### How a field is resolved (`resolve`)

The executor asks the resolver for each field. The resolver of the domain owning a type
handles what needs code (arguments, computed values, filters); everything else is generic:

1. A custom resolver registered for `(type, field)` answers it.
2. Otherwise the record's own value, converted by the schema type: a GID for an object type is the
   record it names (`null` when the record is gone), an array for a connection type is paginated
   (`first`, `last`, `after`, `before`, `reverse`, `sortKey`, `query`), an array for a list is
   mapped.
3. Otherwise a default: `null` for a nullable field; for a non-null one, an empty list or
   connection, `""`, `false`, `0`, `"0.0"` for `Money`/`Decimal`, the first enum value, or the
   defaults of an object type, recursively. The path goes into `extensions.lsf.defaulted`, except
   for fields that are empty by nature (an empty connection of a record that has none).

Root fields follow conventions before custom code: `products` (a connection of `Product`) lists
every record of the type; `product(id:)` reads one; `node(id:)` and `nodes(ids:)` read any;
`productsCount` counts with the same `query`. Cursors are opaque (base64 of the position and the
GID), stable across requests while the list does not change.

### Search syntax (`search`)

The `query` argument of connections, and the `filter` of webhook subscriptions, use Shopify's
search syntax: terms, `field:value`, comparators (`:<`, `:<=`, `:>`, `:>=`), `AND`, `OR`, `NOT`,
`-`, parentheses, quoted phrases, `*` prefix and existence (`field:*`), dates (`created_at:>2026-01-01`,
`created_at:>-7d`?). `search` parses it into a tree and matches a record through a per-type table
of the fields each connection documents (`title`, `tag`, `status`, `vendor`, `sku`, `email`, …);
an unknown field matches nothing, as Shopify. A free term matches the type's default text fields.

### Mutations (`mutations`)

A mutation is a function from the state and its arguments to its payload, run while the shop is
locked. It validates as Shopify does and answers `userErrors` (`field`, `message`, `code`) when
the input is wrong, changes nothing when it fails, and returns the **events** of what it
changed. A mutation lsf does not simulate answers a GraphQL error with
`extensions.code = "LSF_NOT_SIMULATED"`, never a silent success.

## Records

What the seed (`state::seed`, one module of `domains/` per family) makes of the store data and
the app. `stable_id(kind, key)` is lsf-core's (`lsf_core::util::stable_id`, the hash that gives
the store data its ids); "lsf-core's id" is the id lsf-core gave the entity, the one the
storefront and the checkout use. A field not listed is not set: the resolver gives it a default
and reports it in `extensions.lsf.defaulted`. The values chosen where Shopify has an obvious one
are marked *(chosen)*.

Fidelity: field names, types and enum values were checked against the 2026-10 introspection of
`@shopify/dev-mcp@1.17.0` (SHA-256 f651c221…), and every seeded record of the demo store and of
`tests/fixtures/rich` is checked against the SDL by the shape checker (`tests/support/shape.rs`).
Checked on shopify.dev: `Money` reads `"20.00"` and `MoneyV2.amount` `"10.0"`
(`productVariant` examples), the categories of the metafield types
(`metafieldDefinitionTypes` example), `"discounts"` as the `apiType` of a Function of the
Discount API (`shopifyFunctions` example). Not verified, chosen by analogy: the other `apiType`s
(the Function API names in the plural for the legacy discount APIs: `product_discounts`, …),
`Fulfillment.name` (`#1001-F1`), `Return.name` (`#1001-R1`), the opt-in level of consents.

### Ids

| Record | GID |
|---|---|
| `Shop`, `Product`, `ProductVariant`, `Collection`, `Page`, `Blog`, `Article`, `Comment`, `Customer`, `Company`, `CompanyLocation`, `Location`, `Order`, `GiftCard`, `SellingPlan` | lsf-core's id |
| `MediaImage`, `Video`, `ExternalVideo`, `Model3d` of a product | lsf-core's media id, the type by kind |
| `MediaImage`, `Video`, `GenericFile` of `files/` | `stable_id("image" \| "video" \| "file", path)`, by extension (the ids Liquid gives a `file_reference`) |
| `CompanyContact` | the customer's id (the checkout's `purchasingCompany.contact.id`) |
| `Market` | `stable_id("market", handle)` of a declared market, `stable_id("market", "FR")` (upper-case code) of a country in none (the checkout's) |
| `Metaobject` | `stable_id("metaobject", "<type>/<handle>")` (the checkout's, lsf-core's `metaobject_by_reference`) |
| `DiscountAutomaticNode`, `DiscountCodeNode` | position in `checkout.discounts` from 1 (the checkout's `discount.id`; `DiscountNode/<n>` of the legacy APIs is the same discount) |
| `CartTransform`, `DeliveryCustomization`, `PaymentCustomization`, `Validation` | position of the Function in the app from 1 (the checkout's owner ids) |
| `ShopifyFunction` | no GID: a UUID made of the client id and the handle (`app::function_id`), also its key |
| `App` | `stable_id("app", client_id)`; `AppInstallation`: `stable_id("app_installation", client_id)` |
| `ProductOption` | `stable_id("product_option", "<product id>/<option name>")` |
| `ProductOptionValue` | `stable_id("product_option_value", "<product id>/<option>/<value>")` |
| `InventoryItem` | `stable_id("inventory_item", "<variant id>")` |
| `InventoryLevel` | `stable_id("inventory_level", "<variant id>/<location id>")`, with `?inventory_item_id=<item id>` as Shopify writes it |
| `LineItem` | `stable_id("line_item", "<order id>/<position>")` |
| `FulfillmentOrder`, `Fulfillment`, `OrderTransaction`, `Return`, their line items | `stable_id(<kind>, "<order GID>/<n>")`: `fulfillment_order`, `fulfillment`, `order_transaction` (n from 1), `fulfillment_order_line_item` and `fulfillment_line_item` (n the line's position), `return` (n from 0), `return_line_item` (`"<return GID>/<n>"`) |
| `Metafield` | `stable_id("metafield", "<owner GID>/<namespace>.<key>")`, the namespace as Admin writes it |
| `MetafieldDefinition` | `stable_id("metafield_definition", "<OWNER_TYPE>/<namespace>.<key>")` |
| `MetaobjectDefinition` | `stable_id("metaobject_definition", type)` |
| `Publication` (Online Store) | `stable_id("publication", "online-store")` |
| `SellingPlanGroup` | `stable_id("selling_plan_group", lower-case name)` (lsf-core's id is a hexadecimal string, Liquid's) |
| `Domain` | `stable_id("domain", host)`; `ShopPolicy`: `stable_id("shop_policy", handle)`; `Menu`: `stable_id("menu", handle)` |
| `CompanyContactRole` | `stable_id("company_contact_role", "<company id>/location-admin" \| "ordering-only")`; assignment: `stable_id("company_contact_role_assignment", "<customer id>/<location id>")` |
| inline objects with an id | `MailingAddress/<lsf-core address id>?model_name=CustomerAddress` (address book) or `Address` (order); `ImageSource`/`CollectionImage`/`ArticleImage` + lsf-core's image id; `MenuItem`: `stable_id("menu_item", "<menu>/<i>/<j>…")`; `MarketRegionCountry`: `stable_id("market_region", code)`; others `stable_id(<snake type>, …)` |

Every id is a function of the data: the same data gives the same records every run.

### Catalog (`domains/catalog.rs`)

- **`Location`**: those of the data; when there are none, one named "Shop location" at the shop's
  address *(chosen: every Shopify shop has one)*, with the id lsf-core would give it. `name`,
  `address` (`LocationAddress` with `formatted`, `latitude`, `longitude`), `isActive`,
  `fulfillsOnlineOrders`, `shipsInventory`, `hasActiveInventory` *(chosen: true)*,
  `isFulfillmentService: false`, `localPickupSettingsV2` when pickup is on and its time is one of
  Shopify's (`Usually ready in 2 hours` → `TWO_HOURS`), `inventoryLevels`, `metafields`.
- **`Product`**: `title`, `handle`, `descriptionHtml` and `bodyHtml`, `description` (the text of
  the HTML), `vendor`, `productType`, `tags`, `status: ACTIVE` *(chosen: a product of the data is
  live)*, `createdAt`, `updatedAt`, `publishedAt`, `templateSuffix`, `giftCardTemplateSuffix:
  null`, `isGiftCard`, `requiresSellingPlan`, `options`, `variants`, `media`, `images` (inline),
  `seo` (`title` and `description` null: no override), `onlineStoreUrl`,
  `onlineStorePreviewUrl`, `collections` (without `all`), `sellingPlanGroups`, `metafields`, and
  what `refresh_product` derives.
- **`ProductOption`** (`name`, `position`, `values`, `optionValues`, `createdAt`) and
  **`ProductOptionValue`** (`name`, `hasVariants`, `swatch` with its `color` and the file of its
  image).
- **`ProductVariant`**: `title`, `displayName` (`<product> - <variant>`), `sku`, `barcode` and
  `barcodes`, `price`, `compareAtPrice`, `position`, `selectedOptions` (with `optionValue`),
  `inventoryItem`, `inventoryPolicy`, `inventoryQuantity` (the `available` of its levels),
  `availableForSale` (lsf-core's), `taxable`, `requiresComponents: false`, `product`, `media`,
  `image`, `unitPrice`, `unitPriceMeasurement`, `showUnitPrice`, `sellingPlanGroups`,
  `createdAt`, `updatedAt`, `metafields`.
- **`InventoryItem`**: `sku`, `tracked`, `requiresShipping`, `measurement.weight` (in the
  variant's unit), `inventoryLevels`, `variant`, `variants`, dates.
- **`InventoryLevel`** per location that stocks the variant (`store_availabilities`): `item`,
  `location`, `isActive`, `canDeactivate`, and `quantities`, the eight names of Shopify
  (`available`, `committed`, `damaged`, `incoming`, `on_hand`, `quality_control`, `reserved`,
  `safety_stock`; `quantities(names:)` filters them). The variant's stock is spread evenly over
  the locations where it is in stock, the first ones taking the remainder; a stock below zero,
  or that no location has in stock, is at the first location *(chosen: the data gives one
  number)*. `available` and `on_hand` hold it, the others are 0.
- **`Collection`**: `title`, `handle`, `descriptionHtml`, `description`, `image`, `products` (in
  the collection's order), `sortOrder` (`price-descending` → `PRICE_DESC`, …), `ruleSet: null`
  (lsf-core resolves automated collections into their products: they are manual here),
  `templateSuffix`, `seo`, `createdAt` (= `publishedAt` of the data), `updatedAt`, `metafields`.
  The `all` collection lsf-core adds is left out: `/collections/all` is not a collection on
  Shopify.
- **`Publication`**: the Online Store's (`name`), with every product and collection.
- **`SellingPlanGroup`**: `name`, `appId`, `options`, `products`, `sellingPlans`, `createdAt`.
  **`SellingPlan`**: `name`, `description`, `options`, `position`, `pricingPolicies` (the first
  adjustment a `SellingPlanFixedPricingPolicy`, the second a `SellingPlanRecurringPricingPolicy`
  with `afterCycle` = the first one's `order_count`), `billingPolicy`, `deliveryPolicy`,
  `category`, `metafields: []`. A recurring plan bills and delivers by the interval its name or
  options say (`Every 2 weeks` → `WEEK`, 2; `monthly`), a month otherwise *(chosen: the data has
  no interval)*, and is a `SUBSCRIPTION`; a plan that does not recur has a fixed billing policy
  with its checkout charge and is a `PRE_ORDER` when it takes a deposit, `OTHER` otherwise.
- **`GiftCard`**: `balance`, `initialValue` (in the card's currency), `enabled`, `expiresOn`,
  `isRedeemable` (enabled, not expired by the shop's clock, a balance left), `lastCharacters`,
  `maskedCode`, `customer`, `recipientAttributes` when the recipient is a customer (`recipient`
  is a `Customer!`: the message and nickname of a card for someone else are not kept),
  `templateSuffix`, dates.

### Customers (`domains/customers.rs`)

- **`Customer`**: `firstName`, `lastName`, `displayName` (the name, else the email), `email`,
  `phone`, `defaultEmailAddress`, `defaultPhoneNumber`, `emailMarketingConsent`,
  `smsMarketingConsent` (subscribed when `accepts_marketing`; opt-in level `SINGLE_OPT_IN`
  *(chosen: Shopify's default setting)*), `addresses` and `addressesV2` (inline
  `MailingAddress`es), `defaultAddress` (the first), `state` (`ENABLED` with an account,
  `DISABLED` otherwise), `verifiedEmail`, `taxExempt`, `taxExemptions: []`, `dataSaleOptOut:
  false`, `tags`, `locale` (the primary language of the store *(chosen)*),
  `companyContactProfiles`, `createdAt` (the first order's, else the fixed date: as the
  checkout's `Customer.createdAt` when the data freezes the clock), `metafields`, and what
  `refresh_customer` derives: `amountSpent` is what the orders that are not cancelled cost, as
  the checkout counts it.
- **`Company`**: `name`, `externalId`, `note: null`, `customerSince`, dates, `locations`,
  `contacts` (every buyer of the company, as in the checkout), `mainContact` (the first),
  `contactRoles` (Shopify's two: "Location admin", "Ordering only"), `defaultRole` (Ordering
  only), `totalSpent` (the sum of its locations'), `metafields`.
- **`CompanyLocation`**: `name`, `externalId`, `company`, `locale`, `currency`,
  `shippingAddress` (an inline `CompanyAddress` with its id), `billingAddress: null` (the data
  has none), `taxSettings` (`taxRegistrationId`), `ordersCount`, `totalSpent`,
  `roleAssignments`, dates, `metafields`.
- **`CompanyContact`**: `company`, `customer`, `title`, `locale`, `isMainContact`,
  `roleAssignments`, dates. **`CompanyContactRoleAssignment`**: one per location the buyer can
  order for, with the default role *(chosen)*.

### Orders (`domains/orders.rs`)

An order of the data or of the checkout is first an `OrderDraft`; `insert_order` makes its
records, so both are the same shape.

- **`Order`**: `name`, `number` (1001 for `#1001`), `createdAt`, `processedAt`, `updatedAt`,
  `cancelledAt` (the creation date of a cancelled order, the only date the data has) with
  `cancelReason: OTHER` *(chosen)*, `confirmed`, `test: false`, `email`, `phone`, `customer`,
  `purchasingEntity`, `customerAcceptsMarketing`, `currencyCode`, `presentmentCurrencyCode`,
  `taxesIncluded`, `taxExempt`, `note`, `tags: []`, `customAttributes`, `discountCode(s)`,
  `sourceName` (`web` for the checkout's), `lineItems`, `shippingAddress`, `billingAddress`,
  `displayAddress`, `billingAddressMatchesShippingAddress`, `shippingLine`/`shippingLines` (the
  title of the checkout rate that costs what the order paid for shipping, else "Shipping"),
  `taxLines` (one, "Tax", when there is tax *(chosen)*), the `MoneyBag`s: subtotal, shipping,
  tax, discounts, total (subtotal + shipping + tax, as the storefront's `order.total_price`),
  received, refunded, outstanding, capturable; `totalWeight`, `displayFinancialStatus`,
  `displayFulfillmentStatus`, `fullyPaid`, `unpaid`, `capturable`, `canMarkAsPaid`,
  `refundable`, `restockable`, `fulfillable`, `requiresShipping`, `returnStatus`, `returns`,
  `fulfillments`, `fulfillmentOrders`, `transactions`, `refunds: []`, `metafields: []`.
- **`LineItem`**: `name`, `title`, `variantTitle` (null for the default variant), `sku`,
  `vendor`, `quantity`, `currentQuantity`, `unfulfilledQuantity`, the unit and total
  `MoneyBag`s before and after discounts, `requiresShipping`, `taxable`, `isGiftCard`,
  `product`, `variant`, `image`, `weight`, `customAttributes`. The price is the variant's in the
  data (it has no price of its own for an order).
- **`FulfillmentOrder`** (every order has one, at the first location): `status` (`OPEN`,
  `IN_PROGRESS` when partly fulfilled, `CLOSED`, `CANCELLED`), `requestStatus: UNSUBMITTED`,
  `assignedLocation`, `destination` (the shipping address), `deliveryMethod`, `lineItems`
  (`FulfillmentOrderLineItem`: `totalQuantity`, `remainingQuantity`, …), `fulfillments`.
- **`Fulfillment`** of a fulfilled or partly fulfilled order: `status: SUCCESS`, `displayStatus:
  FULFILLED`, `fulfillmentLineItems`, `location`, `totalQuantity`. Which lines a `partial` order
  shipped the data does not say: the first line, or half the units of an order of one line
  *(chosen)*.
- **`OrderTransaction`**: a `SALE` of what was paid for a paid order (and a `REFUND` of it for a
  refunded one), an `AUTHORIZATION` for an authorized one, none for a pending one.
- **`Return`** per return of the data: `name`, `status`, `totalQuantity`, `returnLineItems`: a
  `ReturnLineItem` of the line's fulfillment when it was fulfilled, an
  `UnverifiedReturnLineItem` (with `unitPrice`) otherwise.
- **Orders of the checkout**: `orders::record_placed_order(state, &PlacedOrder)` takes what the
  server's `place_order` has (the `CheckoutResult`, the order number and id it gave, the time,
  the customer, the contact, the address, the note, the attributes, the payment method's name as
  the gateway), keeps the checkout's id when it is free (sessions number their orders on their
  own: a taken id gets a new one), refreshes the customer, and emits `orders/create` and, when
  paid, `orders/paid`. Not wired yet.

### Content (`domains/content.rs`)

- **`Shop`**: `name`, `email`, `contactEmail`, `description`, `myshopifyDomain`, `primaryDomain`
  and `domains` (`Domain`: the custom domain of the data, else the myshopify one, which is in
  `domains` either way), `url`, `currencyCode`, `enabledPresentmentCurrencies` (the shop's and
  its countries'), `currencyFormats`, `ianaTimezone`, `timezoneAbbreviation`, `timezoneOffset`,
  `timezoneOffsetMinutes` (at the shop's clock), `shopAddress` and `billingAddress`,
  `unitSystem` and `weightUnit` (the unit system of the shop's country), `taxesIncluded`,
  `customerAccounts`, `plan` (`partnerDevelopment: true`, "Development", `shopifyPlus` when the
  data has companies, B2B being a Plus feature *(chosen)*), `shopPolicies` (`ShopPolicy`:
  `type`, `title`, `body`, `url`), `shipsToCountries`, `orderNumberFormatPrefix: "#"`,
  `metafields`.
- **`Metafield`** for every owner (`Shop`, `Product`, `ProductVariant`, `Collection`,
  `Customer`, `Company`, `CompanyLocation`, `Location`, `Market`, `Page`, `Blog`, `Article`, the
  discounts and the owners of Functions): `namespace` (`$app:config` of the data is
  `app--<app id>--config`), `key`, `type`, `value` (the string Admin returns: references are
  GIDs, lists and objects compact JSON, `money` with a string amount and its currency),
  `jsonValue`, `reference` or `references` for the reference types, `owner`, `ownerType`,
  `definition`, `sizeInBytes`, dates (the owner's creation date). References of the data
  (handles, paths of `files/`, `type/handle`, SKUs, emails) become the GIDs they name; a file
  something names gets a file record.
- **`MetafieldDefinition`**, inferred: one per owner type, namespace and key that has
  metafields, with the type of the first (`validationStatus: SOME_INVALID` when another differs),
  `name` (the key, humanized), `type` (`name`, `category`), `validations: []`, `metafields`,
  `metafieldsCount`. None for the app's reserved namespaces: their definitions come from the
  app's configuration, which lsf does not read.
- **`Metaobject`**: `type`, `handle`, `displayName` (its `name` or `title` field, else the
  handle humanized), `definition`, `fields` (inline `MetaobjectField`s: `key`, `type`, `value`,
  `jsonValue`, `reference(s)`, `definition`), `capabilities` (published, its template suffix),
  dates. **`MetaobjectDefinition`**, inferred from the entries of a type: `type`, `name`,
  `displayNameKey`, `fieldDefinitions` (every key the entries have, in order, not required),
  `metaobjects`, `metaobjectsCount`, `capabilities` (publishable, online store).
- **`Page`** (`body`, `isPublished`, `publishedAt`, `templateSuffix`, …), **`Blog`**
  (`commentPolicy` from `comments_enabled` and `moderated`, `articles`, `tags` of its articles),
  **`Article`** (`author.name`, `body`, `summary` = the excerpt, `image`, `tags`, `comments`, …),
  **`Comment`** (`author`, `body`, `bodyHtml`, `status: PUBLISHED`, …).
- **`Menu`**: `handle`, `title`, `isDefault` (`main-menu`, `footer`), `items`: inline
  `MenuItem`s (`type`, `url`, `resourceId`, nested `items`); a link to `/collections/all` is a
  `CATALOG` item.
- **`Market`**: the declared ones, then a market of its own per country in none, as the checkout
  sees them: `name`, `handle`, `type: REGION`, `status: ACTIVE`, `enabled`, `primary` (the
  shop's country), `conditions.regionsCondition.regions` and `regions` (inline
  `MarketRegionCountry`), `currencySettings`, `metafields`.
- **`ShopLocale`** per language: `locale`, `name`, `primary`, `published: true`.
- **Files**: `MediaImage`, `Video` or `GenericFile` for each path of `files` and each path
  something refers to, by extension: `alt`, `image` or `sources` or `url`, `mimeType`, statuses
  `READY`, dates.

### App (`domains/app.rs`)

- **`App`**: `title` (`name` of `shopify.app.toml`, "lsf" without an app), `handle`, `apiKey`
  (the client id, `lsf-local-client-id` without one), `embedded`, `developerType: PARTNER`,
  `requestedAccessScopes`, `availableAccessScopes`, `optionalAccessScopes`, `installation`,
  `webhookApiVersion`. **`AppInstallation`**: `app`, `accessScopes` (`[access_scopes] scopes`;
  without an app every scope the 2026-10 schema names, `app::ALL_SCOPES`), `launchUrl`.
- **`ShopifyFunction`** per Function of the app: `id`, `title`, `handle`, `apiType`,
  `apiVersion`, `app`, `appKey`, `inputQuery`.
- **Discounts** of `checkout.discounts`: a `DiscountAutomaticNode` (`automaticDiscount`: a
  `DiscountAutomaticApp`) or, with a code, a `DiscountCodeNode` (`codeDiscount`: a
  `DiscountCodeApp` with its `codes`): `title`, `discountId`, `appDiscountType` (the Function),
  `discountClasses` (all three when the data names none), `combinesWith` all false and
  `appliesOnOneTimePurchase: true`, `appliesOnSubscription: false` *(chosen: Shopify's defaults
  on creation)*, `status: ACTIVE`, `startsAt`, `context` (all buyers), and the node's
  `metafields`.
- **`CartTransform`**, **`DeliveryCustomization`**, **`PaymentCustomization`**,
  **`Validation`** for each Function of the app with such a target (whether or not
  `checkout.functions` names it): `title`, `enabled`, `functionId`, `shopifyFunction`,
  `blockOnFailure: false`, and the metafields `checkout.functions` gives it. Without an app,
  none: there is no Function to own.

## Events and webhooks (`webhooks`)

A change emits events: `{topic, shop, gid, at}` (`products/update`, `orders/create`, …), after
the mutation succeeded. Events become deliveries for each subscription of the shop whose topic
matches and whose `filter` matches the payload:

- **Subscriptions**: those of `shopify.app.toml` (`[[webhooks.subscriptions]]`, app-wide,
  `api_version` of `[webhooks]`), and those the backend creates through the Admin API
  (`webhookSubscriptionCreate`, per shop). Compliance topics (`customers/data_request`,
  `customers/redact`, `shop/redact`) are only triggered on demand.
- **Payload**: the JSON of Shopify's webhooks, which is the REST representation of the resource
  (`id` as a number, `admin_graphql_api_id`, snake_case, prices as strings), built from the
  record when the event is delivered; `include_fields` keeps only those fields. Topics whose
  payload lsf does not build are listed, not sent.
- **Headers**: `X-Shopify-Topic`, `X-Shopify-Hmac-Sha256` (base64 HMAC-SHA256 of the body with
  the client secret), `X-Shopify-Shop-Domain`, `X-Shopify-API-Version`, `X-Shopify-Webhook-Id`,
  `X-Shopify-Event-Id`, `X-Shopify-Triggered-At`, `X-Shopify-Name` for a named subscription,
  `Content-Type: application/json`, `User-Agent: Shopify-Captain-Hook`.
- **Delivery**: from a background worker, never from the thread of a request (the backend's
  handler may call the Admin API back). A `2xx` within 5 seconds is a success; anything else is
  retried a few times with a short back-off (Shopify retries 8 times over 4 hours), then marked
  failed. Each attempt is logged: the request, the status, the time, the start of the answer.
- **Control**: `GET /__lsf/webhooks` lists the deliveries (`?topic=`, `?wait=<n>` waits until there
  are n, for tests), `DELETE /__lsf/webhooks` forgets them, `POST /__lsf/webhooks/trigger`
  sends one (`{"topic", "id"?}`), as `lsf app webhook trigger` does from the command line.

## Store sync

Admin API changes that the store data can express are written back into the store the
storefront and the checkout see: a product created by the backend is on the theme's collection
page, an app discount created with `discountAutomaticAppCreate` runs its Function at the
checkout with the metafields it was given, a customer created is one the checkout can log in as.
The state projects its changed records to a store-data overlay, applied with
`lsf_core::store::load::load_with_overlay` (which learns to remove entities). The checkout
reports placed orders to the state, which records them and emits `orders/create`.

## Crate layout

`lsf-admin` (`crates/admin`) depends on `lsf-core` (the store) and `lsf-checkout` (the app:
`app::App`, whose `config` is the backend's part of `shopify.app.toml`, and the Functions). It
serves nothing itself: `lsf` routes the requests to [`api`], as it does for the checkout.

| Module | Role |
|---|---|
| `version` | The API versions lsf serves (2026-10), from the path of a request. |
| `schema` | The schema of the Admin API, parsed once from `schemas/2026-10/admin.graphql` (generated by `tools/admin/admin_schema.mjs` from the pinned `@shopify/dev-mcp`, with its descriptions, for introspection). |
| `graphql` | Parsing, validation, execution, introspection, cost: a GraphQL server over a `Resolver`. Knows nothing of Shopify's data. |
| `state` | Shops, records, ids, the clock, events; the seed from `lsf_core::Store` (`state::seed`, which runs the seed of each domain). |
| `resolve` | The generic resolution of records, connections and root fields, and the registry of custom resolvers. |
| `domains/` | One module per family of resources: its seed, its custom resolvers, its mutations, its webhook payloads (`catalog`, `orders`, `customers`, `content`, `app`). |
| `search` | Shopify's search syntax. |
| `auth` | Tokens, OAuth, token exchange, session tokens, access scopes, HMAC. |
| `webhooks` | Topics, subscriptions, payloads, signing, the delivery worker and its log. |
| `throttle` | The cost bucket of each shop and app. |
| `api` | One HTTP request in, one answer out: the Admin API endpoints, OAuth, and the `/__lsf/admin` and `/__lsf/webhooks` control endpoints. |
