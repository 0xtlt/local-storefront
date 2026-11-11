# Admin API reference

What lsf's Admin API does, family by family: the mutations it simulates, the queries that need
more than reading records, the webhooks they send, and what lsf chose where Shopify does not
say. To point a backend at it, get its tokens, receive its webhooks and test it, read the
[Admin API guide](admin.md) first.

## What the answers say

- Errors are Shopify's, word for word where they are known: parse and validation errors with
  `locations` and `extensions.code`, `you must provide one of first or last`, `first cannot
  exceed 250. …`, `ACCESS_DENIED`, `THROTTLED`, `MAX_COST_EXCEEDED`. A mutation's validation errors
  are its `userErrors`, and a mutation that fails changes nothing.
- `extensions.cost`: `requestedQueryCost`, `actualQueryCost` and `throttleStatus`, as Shopify
  computes them, from a leaky bucket per shop and app (`--admin-throttle`).
- `extensions.search`: Shopify's warnings when a `query:` names a field the connection does not
  have ("Invalid search field for this query."); the field is then ignored, as on Shopify.
- Cursors are opaque, and resume as Shopify's do: right after their item in the connection's
  sort order, even when the item was deleted in between (delete a page of products while paging,
  and `after: endCursor` answers the next ones, not those a page further).
- `extensions.lsf`, only lsf's, only when there is something to say: `defaulted`, the paths of
  the fields answered with a default because the store data does not give them (and
  `defaultedFields`, as `Type.field`), and `notes`, what lsf did not simulate in the request (an
  input it ignored, a search filter it does not read).
- A money input of ten trillion or more in the currency's unit (`Money`, `MoneyInput.amount`, a
  discount's amount, …), or a line or an order whose total would be, answers a `userError`
  ("Price must be less than 10000000000000") and changes nothing. Shopify publishes no limit:
  this one is lsf's, so that no total overflows.
- Nothing answers a silent success for what lsf does not do: an input it ignores is in
  `extensions.lsf.notes`, and a mutation it did not simulate would answer an error with
  `extensions.code: LSF_NOT_SIMULATED` (no mutation of the versions it serves does). The REST
  Admin API (`/admin/api/2026-10/products.json`) answers `404`: lsf serves the GraphQL Admin API
  only.
- A bug of lsf while a mutation runs (a panic, in the mutation or in what follows it: segments,
  listings, product feeds) answers Shopify's internal error: `Internal error. Looks like
  something went wrong on our end.` with `extensions.code: INTERNAL_SERVER_ERROR`, a
  `requestId`, and what went wrong in `extensions.lsf.panic`. The shop is put back as it was
  before the mutation, so the next request finds it intact; the terminal shows the panic.

## What lsf simulates

Every mutation of the versions lsf serves (2026-10, and [2027-01](admin.md#api-versions) when
asked) is simulated, and every query answered (`GET /__lsf/admin` lists the mutations,
`mutations`; for contributors, `crates/app/tests/coverage.rs` fails when a version brings one
that is not). Simulated means:

- **Checked as Shopify checks**: the input is validated, and what is wrong answers the
  mutation's `userErrors` (Shopify's field, message and code where shopify.dev documents them,
  lsf's words where it does not). A mutation that fails changes nothing.
- **Done**: the records of the shop change, and every query that follows, the storefront and the
  checkout see it ([what they see](admin.md#what-the-storefront-and-the-checkout-see)). Work Shopify
  does in the background is done at once: its `Job` answers `done: true`.
- **Delivered**: the mutation's webhooks reach the backend, with Shopify's payloads (every topic
  but `returns/process`, `shipping_addresses/create` and `/update`, which Shopify publishes no
  example of, and which lsf logs as not sent).
- **Kept local**: what Shopify would send or buy outside (an email, a label, a Flow trigger) is
  recorded in the [outbox](#the-outbox); what waits on time happens by the [shop's
  clock](#the-shops-clock). Where lsf cannot do what Shopify does, the answer says so
  (`extensions.lsf.notes`), and so does the family below.

A query reads the records of the shop: a field the store data does not give is a default,
reported in `extensions.lsf.defaulted`. The families below list their mutations, the queries
that need more than reading records, and what lsf chose where Shopify does not say.

### Products, collections and inventory

A backend that manages a catalog through the Admin API works against lsf as against a store:
what it changes is read back by the queries, delivered as webhooks, and shown by the storefront
and the checkout (see "Store sync" below).

| Area | Mutations lsf simulates |
|---|---|
| Products | `productCreate`, `productUpdate` (with `media` and `metafields`), `productDelete`, `productSet` (synchronous or not, upsert by `id`, `handle` or `customId`), `productDuplicate`, `productChangeStatus` |
| Bundles and combined listings | `productBundleCreate`, `productBundleUpdate`, `productVariantRelationshipBulkUpdate`, `combinedListingUpdate` |
| Variants | `productVariantsBulkCreate` (its `strategy` included), `productVariantsBulkUpdate` (`allowPartialUpdates` included), `productVariantsBulkDelete`, `productVariantsBulkReorder` |
| Options | `productOptionsCreate` (`LEAVE_AS_IS`, `CREATE`), `productOptionUpdate` (`LEAVE_AS_IS`, `MANAGE`), `productOptionsDelete` (`DEFAULT`, `NON_DESTRUCTIVE`, `POSITION`), `productOptionsReorder` |
| Media | `productCreateMedia`, `productUpdateMedia`, `productDeleteMedia`, `productReorderMedia`, `productVariantAppendMedia`, `productVariantDetachMedia` |
| Collections | `collectionCreate`, `collectionUpdate` (with the `sources` of 2026-10), `collectionDelete`, `collectionDuplicate`, `collectionAddProducts`, `collectionAddProductsV2`, `collectionRemoveProducts`, `collectionReorderProducts`, `collectionConditionsSourceCreate`, `collectionConditionsSourceUpdate`, `collectionConditionsSourceDelete` |
| Inventory | `inventoryAdjustQuantities`, `inventorySetQuantities`, `inventoryMoveQuantities`, `inventorySetOnHandQuantities`, `inventoryActivate`, `inventoryDeactivate`, `inventoryBulkToggleActivation`, `inventoryItemUpdate` |
| Locations | `locationAdd`, `locationEdit`, `locationActivate`, `locationDeactivate`, `locationDelete`, `locationLocalPickupEnable`, `locationLocalPickupDisable` |
| Publications | `publishablePublish`, `publishableUnpublish`; the deprecated `productPublish`, `productUnpublish`, `collectionPublish`, `collectionUnpublish` |
| Product feeds | `productFeedCreate`, `productFeedDelete`, `productFullSync` |
| Selling plans | `sellingPlanGroupCreate`, `sellingPlanGroupUpdate`, `sellingPlanGroupDelete`, `sellingPlanGroupAddProducts`, `sellingPlanGroupRemoveProducts`, `sellingPlanGroupAddProductVariants`, `sellingPlanGroupRemoveProductVariants`, `productJoinSellingPlanGroups`, `productLeaveSellingPlanGroups`, `productVariantJoinSellingPlanGroups`, `productVariantLeaveSellingPlanGroups` |
| Gift cards | `giftCardCreate`, `giftCardUpdate`, `giftCardCredit`, `giftCardDebit`, `giftCardDeactivate`, `giftCardProductSet` (`productSet` for a gift card product: its variants untaxed, unshipped, untracked), `giftCardSendNotificationToCustomer`, `giftCardSendNotificationToRecipient` (the email in the shop's [outbox](#the-outbox)) |

How they behave, as shopify.dev documents it:

- **Validation** answers Shopify's `userErrors` (`field`, `message`, `code`) and changes nothing:
  "The variant 'Red / S' already exists.", "Option 'Color' already exists.", "Can't manually
  remove products from a smart collection", "The location cannot be deleted while it is
  active.", … Where Shopify documents an error by its code only, the message is the code's
  description in the 2026-10 schema.
- **Metafields** given to `productCreate`, `productUpdate`, `productSet`, the variant
  mutations, collections, locations and selling plans (and to customers, app discounts and the
  owners of Functions) are checked as `metafieldsSet` checks them
  (see "Metafields, metaobjects and content"): the type and the value, a definition's type and
  validations, `$app` and another app's namespaces. The errors are the mutation's, at the path of
  the input (`["metafields", "0", "value"]`; `productSet`'s with the code `INVALID_METAFIELD`).
- **Derived values follow**: price ranges, `totalInventory`, `hasOnlyDefaultVariant`,
  `featuredMedia`, a variant's `title`, `displayName`, `position` and `inventoryQuantity`, an
  option value's `hasVariants`, a variant's `availableForSale` after an inventory change.
- **Inventory**: `on_hand` stays the sum of `available`, `committed`, `reserved`, `damaged`,
  `safety_stock` and `quality_control`; adjusting `available` changes `on_hand` with it, moving
  between two of them does not. `changeFromQuantity` must be given (`null` skips the check) and a
  stale one is `CHANGE_FROM_QUANTITY_STALE`. Reasons and ledger documents are checked. The answer
  is an `InventoryAdjustmentGroup` (`reason: "Inventory correction"`, `quantityAfterChange:
  null`, as the Admin API answers right after the change).
- **`@idempotent`**: the inventory mutations, `locationActivate` and `locationDeactivate` require
  it, as Shopify does since 2026-04. The same key with the same arguments within 24 hours answers
  the first answer again without doing anything; with other arguments,
  `IDEMPOTENCY_KEY_PARAMETER_MISMATCH`.
- **Smart collections** (`ruleSet`) list the products their rules match (`TAG`, `TITLE`, `TYPE`,
  `VENDOR`, `VARIANT_TITLE`, `VARIANT_PRICE`, `VARIANT_COMPARE_AT_PRICE`, `VARIANT_WEIGHT`,
  `VARIANT_INVENTORY`, `IS_PRICE_REDUCED`; all of them, or any with `appliedDisjunctively`), and
  follow the products every mutation changes. Custom collections keep their manual order.
- **Publishing**: a product created through the Admin API is unpublished; `publishablePublish`
  on the Online Store sets its `publishedAt` and `onlineStoreUrl`, `publishableUnpublish` clears
  them.
- **Asynchronous work is done at once**: `productSet(synchronous: false)` answers a
  `ProductSetOperation` already `COMPLETE`, `productReorderMedia`, `collectionAddProductsV2`,
  `collectionRemoveProducts`, `collectionReorderProducts` a `Job` already `done`.
- **Bundles**: a fixed bundle's variants have components (`productVariantComponents`, each a
  variant and a quantity; `requiresComponents` is then `true`). `productVariantRelationshipBulkUpdate`
  gives a variant its components and prices it (`COMPONENTS_SUM`, `FIXED`, `NONE`);
  `productBundleCreate` makes a product whose options are the components' option selections
  (or their `consolidatedOptions`, and a component's `quantityOption`) and whose variants are
  their combinations, priced at the sum of their components *(lsf's choice)*, and answers a
  `ProductBundleOperation` already `COMPLETE` (`productOperation` reads it). A bundle's inventory
  is the bundles its components make ("the minimum of the component's inventory quantities",
  each divided by the quantity it takes), and it sells while they do: it follows every change of
  their stock. Selling a bundle takes its components' units: an order that claims stock
  (`orderCreate` with `DECREMENT_…`, a completed draft, an edit, a subscription's billing)
  commits each component's quantity times the bundle's, `DECREMENT_OBEYING_POLICY` and billing
  attempts check the components, and fulfilling, cancelling with a restock or refunding with a
  restock move the components' units; the bundle's quantity, `availableForSale` and
  `variants/in_stock` / `variants/out_of_stock` follow. A component deleted (its variant or its
  product) leaves its bundles (`products/update`); one with none left no longer
  `requiresComponents`. `Product.bundleComponents`, `productComponents`, `productParents` and
  `ProductVariant.productParents` read the relation both ways. Nested bundles, more than 30
  components or 3 options are refused, as Shopify documents.
- **Combined listings**: a product created with `combinedListingRole: PARENT` takes children
  with `combinedListingUpdate` (`productsAdded`, `productsEdited`, `productsRemovedIds`,
  `optionsAndValues` for the order of its options and values): a parent variant per child, the
  child `CHILD`. `Product.combinedListing` answers the listing for the parent and its children;
  `products(query: "combined_listing_role:PARENT")` finds the parents.
- **Collection sources** (2026-10): a collection is the union of its sources: conditions
  sources (typed conditions on tags, titles, types, vendors, statuses, the variants' prices,
  compare-at prices, inventory, titles and weights, and metafields of a definition; manual
  selections; exclusions by tag, type, vendor, collection or selection), sub-collection sources
  (the members of other collections, one level deep) and an app's shareable sources
  (`collectionConditionsSourceCreate`; changing one changes every collection that links it,
  deleting it unlinks it). A collection of the store data reads as the one source it amounts to
  (`Collection.sources`), and keeps it when it gets more. `collectionDuplicate` copies a
  collection with its products, sources, metafields and, unless `copyPublications: false`, its
  publications.
- **Product feeds**: a feed is a country and a language of the app's channel (`channelCreate`),
  `ACTIVE` while its market is active and its language published. `productFullSync` (and
  `channelFullSync`) sends `product_feeds/full_sync` for each product published on the channel,
  then `product_feeds/full_sync_finish`; a product listed, changed (its stock included) or
  unlisted on the channel sends `product_feeds/incremental_sync` (`CREATE`, `UPDATE`,
  `DELETE`); a feed whose status changes `product_feeds/update`. The payloads are in the shop's
  currency, without delivery estimates.
- **Gift card products and notifications**: `giftCardProductSet` runs `productSet` for a gift
  card product (with its `issuanceCurrency`, set at creation only). The notifications are not
  sent: each is an email in the shop's [outbox](#the-outbox) (`gift_card_created` to the card's
  customer, `gift_card_recipient` with the recipient's message); a card disabled, expired or
  spent is refused (`INVALID`).
- **Media given by URL** is recorded, not downloaded: `image.url` is the URL, the size unknown,
  and the medium is `READY` at once (Shopify answers `UPLOADED` and processes it later).
- **Queries**: `products` and `productVariants` take their documented sort keys
  (`INVENTORY_TOTAL`, `FULL_TITLE`, `SKU`, …) and search filters (`sku`, `barcode`, `price`,
  `inventory_total`, `is_price_reduced`, `out_of_stock_somewhere`, `published_status`,
  `option1`, `location_id`, …); `productByIdentifier` and `productVariantByIdentifier` find a
  `customId`; `productTags`, `productTypes`, `productVendors`, `inCollection`,
  `InventoryItem.inventoryLevel(locationId:)`, `publishedOnPublication`,
  `resourcePublications`, the `appliesTo…` fields of selling plan groups are answered.

**Webhooks**: `products/create`, `products/update`, `products/delete`, `collections/*`,
`inventory_items/*`, `inventory_levels/update`, `…/connect`, `…/disconnect`, `locations/*`
(`activate` and `deactivate` included), `variants/in_stock` and `variants/out_of_stock` (a
variant whose stock crosses zero), `product_publications/create` and `…/delete` (the same for
`collection_publications`), `selling_plan_groups/*`, `product_feeds/*` (`create`, `update`,
`full_sync`, `full_sync_finish`, `incremental_sync`). A variant, option or media change is a `products/update`; an inventory
change is an `inventory_levels/update` only.

**Store sync**: products (with their status and Online Store publication), variants (price,
SKU, barcode, stock), options, images of `files/`, collections and their products, locations,
selling plan groups and the gift cards of the store data are written back to the store the
storefront and the checkout see. Not synced: smart collection rules (a changed smart collection
becomes a manual one with the products it has), collection publications (an unpublished
collection stays on the storefront), gift cards the Admin API creates (the store data keys a
card by its code, which Shopify answers once), media from a URL outside `files/`, SEO, the
category.

Inputs a simulated mutation ignores (`category`, `linkedMetafield`, a
future `publishDate`, …) are said in `extensions.lsf.notes`; a product category condition of a
collection source matches no product (lsf keeps no category), and a variant-scoped source
(`targetType: VARIANTS`) puts in the products one of whose variants matches. The collections of the store data are custom collections
for the Admin API (lsf resolves their rules into products). `productTags`,
`productTypes` and `productVendors` page 250 at most, as every connection of lsf (Shopify allows
1000 to 5000).

### Orders, fulfillments and draft orders

An order placed in the local checkout is an order of the Admin API at once (`orders/create`,
`order_transactions/create` for its payment, `orders/paid`), and the backend processes it as on
Shopify: it fulfills, refunds, edits, cancels and returns it; the order's totals, statuses,
fulfillment orders, transactions and stock follow, and each change fires its webhooks (the
specific topic first, then `orders/updated`, which Shopify sends "in addition to the more
specific webhooks").

- **Scopes**: reading an order takes `read_orders`; fulfilling it takes
  `write_merchant_managed_fulfillment_orders` (`write_orders` is not enough: its
  `fulfillmentOrders` answer `ACCESS_DENIED` with the scopes that would do).
- **The storefront does not show it**: orders do not [sync](admin.md#what-the-storefront-and-the-checkout-see),
  so the account pages show no fulfillment, refund or edit the backend makes. Read them back
  with the Admin API (`displayFulfillmentStatus`, `fulfillments { trackingInfo }`).

- **Queries**: `orders` and `ordersCount` with the filters of the reference (`financial_status`,
  `fulfillment_status` with `shipped`, `partial`, `unshipped`, `unfulfilled`, `on_hold`, `status`
  with `open`, `closed`, `cancelled`, `not_closed`, `name` with or without `#`, `email`,
  `customer_id`, `sku`, `tag`, `created_at`, `processed_at`, `updated_at`, `gateway`,
  `current_total_price`, `return_status`, `delivery_method`, `location_id`, `test`,
  `source_name`, …) and every `OrderSortKeys`; `fulfillmentOrders` (open ones unless
  `includeClosed`), `assignedFulfillmentOrders`, `manualHoldsFulfillmentOrders`,
  `FulfillmentOrder.supportedActions` and `locationsForMove`, `returnableFulfillments`,
  `Order.suggestedRefund`, `draftOrders`.
- **Orders**: `orderCreate` (imported orders: lines by variant or custom, customer, addresses,
  shipping lines, tax lines, a discount code, transactions, `financialStatus`,
  `fulfillmentStatus`, `inventoryBehaviour`), `orderUpdate`, `orderCancel` (cancelled at once,
  its `job` done and its `jobResult` readable with `node`; authorizations voided, payments
  refunded with `refundMethod`, units restocked with `restock`), `orderClose`, `orderOpen`,
  `orderMarkAsPaid`, `orderCapture`, `transactionVoid`, `orderCustomerSet`,
  `orderCustomerRemove`, `orderInvoiceSend` (the invoice in the shop's [outbox](#the-outbox)),
  `orderDelete` (a cancelled or closed order).
- **Fulfillment**: the deprecated `fulfillmentCreateV2` and `fulfillmentTrackingInfoUpdateV2` run
  their successors below. `fulfillmentCreate` (with `lineItemsByFulfillmentOrder`, partial or whole;
  fulfillment orders close or stay in progress, the order is fulfilled or partially fulfilled),
  `fulfillmentTrackingInfoUpdate`, `fulfillmentCancel`, `fulfillmentEventCreate`;
  `fulfillmentOrderHold` and `ReleaseHold` (several holds, a hold of some items splits the
  fulfillment order), `Move`, `Cancel`, `Open`, `Reschedule`.
- **Fulfillment services**: `fulfillmentServiceCreate`, `Update`, `Delete` (with their
  location), and the requests between merchant and service:
  `fulfillmentOrderSubmitFulfillmentRequest`, `AcceptFulfillmentRequest`,
  `RejectFulfillmentRequest`, `SubmitCancellationRequest`, `AcceptCancellationRequest`,
  `RejectCancellationRequest`, `fulfillmentOrderClose`. lsf does not call the service's
  `callbackUrl`: the service reads `assignedFulfillmentOrders`.
- **Refunds**: `refundCreate` (line items with `CANCEL`, `RETURN` or `NO_RESTOCK`, shipping,
  transactions against the order's payments, `refundMethods.storeCreditRefund` to the
  customer's store credit; a refund without transactions moves no money, as on Shopify).
- **Returns**: `returnRequest`, `returnApproveRequest`, `returnDeclineRequest`, `returnCreate`
  (with exchange line items, restocking fees and a return shipping fee), `returnProcess` (with
  its refund, its exchanges, its dispositions), `returnClose`, `returnReopen`, `returnCancel`,
  `removeFromReturn`, `returnLineItemRemoveFromReturn`, `returnRefund`; reverse fulfillment
  orders: `reverseDeliveryCreateWithShipping`, `reverseDeliveryShippingUpdate`,
  `reverseFulfillmentOrderDispose`. `returnCalculate`, `Return.suggestedFinancialOutcome` and
  `Return.suggestedRefund` compute what a return gives back.
- **Order edits**: `orderEditBegin`, `orderEditAddVariant`, `orderEditAddCustomItem`,
  `orderEditSetQuantity`, `orderEditAddLineItemDiscount`, `orderEditUpdateDiscount`,
  `orderEditRemoveDiscount`, `orderEditRemoveLineItemDiscount`, `orderEditAddShippingLine`,
  `orderEditUpdateShippingLine`, `orderEditRemoveShippingLine`, `orderEditCommit`
  (`orders/edited`, with the lines added and removed); requested edits:
  `requestedOrderEditCreate`, `requestedOrderEditResolve`, `requestedOrderEditDecline`,
  `requestedOrderEditCalculate`.
- **Draft orders**: `draftOrderCreate`, `draftOrderUpdate`, `draftOrderDelete`,
  `draftOrderCalculate`, `draftOrderComplete` (an order, paid unless `paymentPending` or payment
  terms), `draftOrderDuplicate`, `draftOrderInvoiceSend` (the invoice in the shop's outbox),
  `draftOrderInvoicePreview`, `draftOrderCreateFromOrder`,
  `draftOrderBulkAddTags`, `draftOrderBulkRemoveTags`, `draftOrderBulkDelete`; B2B drafts
  (`purchasingEntity.purchasingCompany`: the location's addresses and payment terms). Taxes are
  at the store data's `checkout.tax_rate`; `draftOrderAvailableDeliveryOptions` offers its
  `checkout.shipping_rates` and the locations with local pickup.
- **Payments**: `paymentTermsCreate`, `paymentTermsUpdate`, `paymentTermsDelete` and a draft's
  `paymentTerms` (the nine templates of `paymentTermsTemplates`; one schedule, due `dueInDays`
  after it is issued, on a fixed date, on receipt or on fulfillment), `orderCreateManualPayment`,
  `orderCreateMandatePayment` (idempotent by its key), `paymentScheduleCapture` (a due schedule,
  through the order's vaulted payment method: the card of a checkout that left a balance, the
  customer's payment methods), `paymentReminderSend` (in the outbox). `Order.paymentTerms`,
  `paymentCollectionDetails`, `PaymentTerms.due` and `overdue` follow the money and the clock.
  Every sale, capture and refund is a `TenderTransaction` (`tenderTransactions`).
- **Store credit**: `storeCreditAccountCredit`, `storeCreditAccountDebit` (one account per owner
  and currency, credits that expire, Shopify's messages); `orderCreate` takes a payment through
  the gateway `shopify_store_credit` from the customer's account; refunds, returns and
  cancellations give money back as store credit.
- **Risk and attribution**: `orderRiskAssessmentCreate` (`Order.risk` with its recommendation),
  `orderAttributionDefinitionUpsert`, `orderAttributionDefinitionDelete`
  (`orderAttributionDefinitions`, `Order.attribution` from the order's `sourceName`).
- **Shopify Payments**: `shopifyPaymentsAccount` (balance transactions of the money it processed,
  daily payouts, its balance), disputes (`disputes`, `dispute`, `Order.disputes`,
  `disputeEvidenceUpdate`), `shopifyPaymentsPayoutAlternateCurrencyCreate`.
- **Tax apps**: `taxAppConfigure`, `taxSummaryCreate`.
- **Webhooks**: `orders/create`, `/updated`, `/paid`, `/cancelled`, `/fulfilled`,
  `/partially_fulfilled`, `/edited`, `/delete`, `/risk_assessment_changed`,
  `/shopify_protect_eligibility_changed`, `/link_requested`; `order_transactions/create`;
  `tender_transactions/create`; `payment_terms/create`, `/update`, `/delete`;
  `payment_schedules/due`; `fulfillments/create`, `/update`; `fulfillment_events/create`;
  `fulfillment_holds/added`, `/released`; `fulfillment_orders/*` (holds, moves, splits,
  cancellations, requests to a service); `refunds/create`; `returns/request`, `/approve`,
  `/decline`, `/close`, `/reopen`, `/cancel`, `/update` (`returns/process` fires, but neither
  the webhook reference nor dev-mcp has an example payload to build it from: it is logged as
  not sent); `reverse_deliveries/attach_deliverable`; `reverse_fulfillment_orders/dispose`;
  `requested_order_edit/created`, `/declined`, `/resolved`; `draft_orders/create`, `/update`,
  `/delete`; `disputes/create`, `/update`; `tax_services/create`, `/update`.

**Time**: a payment schedule that came due fires `payment_schedules/due` (once), store credit
that expired leaves its account, the days Shopify Payments processed are paid out, by the
[shop's clock](#the-shops-clock).

**Emails**: lsf sends none. What Shopify would email (payment reminders, invoices, gift cards,
store credit, return labels, a requested edit declined, a new order link) is in the shop's
[outbox](#the-outbox).

**What buyers, banks and Shopify do**, which no mutation makes, is a control endpoint: `POST
/__lsf/admin/disputes` (`{"order", "amount"?, "type"?, "reason"?}`) opens a chargeback or an
inquiry (`disputes/create`), `POST /__lsf/admin/disputes/<n>/won`, `/lost` or `/accepted`
decides it; `POST /__lsf/admin/orders/protect` (`{"order", "status"?, "eligibility"?}`) changes
Shopify Protect's view of an order; `POST /__lsf/admin/orders/link-request` (`{"order"}`) is a
buyer asking for a new order link; `POST /__lsf/admin/tax-app/activate` is the merchant turning
on the tax app. See [the control API](control-api.md).

What lsf chose where Shopify does not say, or where a local tool must differ:

- The local checkout keeps the stock of the store data: its orders commit nothing, so test
  purchases never sell a product out. Orders the backend makes do: `orderCreate` with a
  `DECREMENT_…` `inventoryBehaviour`, a completed draft order, units an edit adds. Shipping
  takes the units the order committed; a restock puts back what the order took. An order that
  took nothing (of the store data or the checkout) ships and restocks without moving the stock. A variant an order
  sells out is out of stock as after an inventory mutation: `availableForSale` is `false`,
  `variants/out_of_stock` is sent, and the storefront shows it sold out.
- Where an order that names no location fulfills from (`orderCreate` without
  `fulfillment.locationId`, a completed draft order, a subscription's billing): Shopify does not
  document it for these orders. lsf takes the shop's primary location (`location` without an id)
  when it can give every line, else the first active location that can, else the primary one,
  where `DECREMENT_OBEYING_POLICY` and billing then find the stock short.
- `orderCreate`'s `discountCode` makes a `DiscountCodeApplication` (its `value` as the
  reference's examples answer it) and the allocations on the lines (an amount off the order is
  spread over them by their totals) or on the shipping line. Its `allocationMethod`,
  `targetSelection` and `targetType` are lsf's: Shopify's examples do not show them.
- `orderCancel` is synchronous (Shopify's runs as a job). `orderCreate` with a
  `financialStatus` and no `transactions` records the transaction the status implies (a sale of
  the total for `PAID`). `fulfillmentCancel` does not put the shipped units back in stock.
- Messages Shopify does not publish are lsf's, in Shopify's form; those it does publish (in the
  reference's examples, or quoted from real answers) are word for word. For contributors, the
  sources are in the doc comments of `crates/app/src/admin/domains/orders/`.
- The `returns/*` payloads list no return line items (Shopify's examples have none, and lsf does
  not know their shape): read them through the API. `returns/update` lists the return line
  items a change removed, as its example does.
- Money: Shopify Payments processes every payment that is not through a manual gateway (the
  checkout's cards, mandates, the orders of the store data), at 2.9 % + 0.30 a charge and 15.00
  a chargeback; it pays out each day once the shop's clock left it behind. Every amount is in
  the shop's currency: an alternate currency payout of another currency finds no balance.
- Payment terms have one schedule; a schedule is `due` once its date passed and `overdue` from
  the day after, by the shop's clock. A mandate is the card of a checkout that left a balance to
  pay later, or one of the customer's payment methods; the payments through it are Shopify
  Payments' and succeed.
- An exchange is processed as Shopify's returns do it: its items become line items of the order
  and the returned units are taken back by a refund, so that the order owes (or is owed) the
  difference. Restocking and return shipping fees are taken off what a return suggests to
  refund.
- Risk recommendations are lsf's rule (cancel for a high risk, investigate for a medium one,
  accept for a low one); lsf makes no fraud analysis of its own, nor any Shopify Protect
  decision. A store credit balance stays below 100,000 of its currency (Shopify does not publish
  its limit).

lsf's orders have no duties (`currentTotalDutiesSet` is `null`, as for an order without). Not
built: the `returns/process` payload (no example to build it from), `tax_summaries/create` (not a
topic of the 2026-10 enum).

### Inventory transfers, fulfillment and delivery

A backend that moves stock between locations, routes and prepares fulfillment orders, buys
labels or manages shipping settings works against lsf as against a store, the stock following
end to end.

| Area | Mutations lsf simulates |
|---|---|
| Inventory transfers | `inventoryTransferCreate`, `inventoryTransferCreateAsReadyToShip`, `inventoryTransferDuplicate`, `inventoryTransferEdit`, `inventoryTransferSetItems`, `inventoryTransferRemoveItems`, `inventoryTransferMarkAsReadyToShip`, `inventoryTransferCancel`, `inventoryTransferDelete` |
| Inventory shipments | `inventoryShipmentCreate`, `inventoryShipmentCreateInTransit`, `inventoryShipmentAddItems`, `inventoryShipmentRemoveItems`, `inventoryShipmentUpdateItemQuantities`, `inventoryShipmentMarkInTransit`, `inventoryShipmentReceive`, `inventoryShipmentSetTracking`, `inventoryShipmentSetBarcode`, `inventoryShipmentDelete` |
| Fulfillment orders | `fulfillmentOrderSplit`, `fulfillmentOrderMerge`, `fulfillmentOrderReportProgress`, `fulfillmentOrderLineItemsPreparedForPickup`, `fulfillmentOrdersReroute`, `fulfillmentOrdersSetFulfillmentDeadline` (and `fulfillmentOrderOpen` stops reported progress, from 2026-01) |
| Labels and rules | `shippingLabelPurchase`, `fulfillmentConstraintRuleCreate`, `…Update`, `…Delete` (a fulfillment constraints Function of the app, with its metafields; `fulfillmentConstraintRules` lists them) |
| Carriers and delivery | `carrierServiceCreate`, `…Update`, `…Delete`, `deliveryProfileCreate`, `…Update`, `…Remove`, `deliveryPromiseProviderUpsert`, `deliveryPromiseParticipantsUpdate`, `deliverySettingUpdate`, `deliveryShippingOriginAssign`, `shippingPackageUpdate`, `shippingPackageMakeDefault`, `shippingPackageDelete` |

How the stock moves, as the Help Center's "Creating and processing inventory transfers"
describes it:

- **A draft** commits nothing. **Ready to ship** commits the units at the origin (`available` to
  `committed`). **A shipment in transit** takes them out of the origin (`committed`, else
  `available`, and `on_hand`) and puts them in `incoming` at the destination, which is stocked
  with the item if it was not (`inventory_levels/connect`). **Receiving** makes the accepted
  units `available` there; rejected and cancelled ones only leave `incoming`. **Cancelling** a
  transfer that is ready to ship gives its units back to `available` at the origin (a transfer
  with a shipment cannot be cancelled: `SHIPMENT_EXISTS`). Each change is followed as an
  inventory mutation is: `inventory_levels/update`, `variants/in_stock` or `variants/out_of_stock`,
  and the storefront's stock (store sync).
- The transfer follows its shipments: lines `shippedQuantity`, `pickedForShipmentQuantity` (draft
  shipments), `processableQuantity`; `IN_PROGRESS` once a shipment is in transit, `TRANSFERRED`
  once every unit shipped and was received (`inventory_transfers/complete`).
- **Deactivating a location** with a `destinationLocationId` moves its stock there, and the open
  transfers that start or end at it too: a shipment in transit to it is received at the
  destination, and the inactive location stocks nothing.
- **Rerouting** and **merging** keep each order line's committed stock right: a fulfillment
  order rerouted takes what the order committed to its new location; a split or a merge stays at
  one location, so nothing moves.

What lsf chose where Shopify does not say, or where a local tool must differ:

- `@idempotent` is required from 2026-04 on the transfer and shipment mutations whose reference
  says so (create, create as ready to ship, duplicate, set items; shipment create, create in
  transit, add items, receive), optional before; a key reused with other arguments is
  `IDEMPOTENCY_KEY_PARAMETER_MISMATCH`.
- A shipment's items must be on its transfer, within what the transfer has left to ship.
  Transfers are named `#T1`, their shipments `T1-1`.
- **Nothing leaves the machine.** A pickup's "Ready for pickup" notice (`ready_for_pickup`,
  `out_for_delivery` for a local delivery), a label bought (`shipping_label`), a shipping
  confirmation (`shipping_confirmation`) are recorded in the shop's [outbox](#the-outbox), never
  sent. `shippingLabelPurchase` answers a
  result already `PURCHASED` with a label (its tracking number made from its id, the same every
  run, from the carrier asked, else USPS, Canada Post or DHL Express by the origin's country), and
  fulfills the fulfillment order with that tracking, as a label bought in the admin does.
- Carrier services are recorded, never called: `availableServicesForCountries` is empty (with a
  note) and the checkout does not ask them for rates.
- Rerouting follows Shopify's default rule: the first active location that fulfills online
  orders and has every unit available, within `includedLocationIds`, outside
  `excludedLocationIds`.
- `fulfillmentOrderLineItemsPreparedForPickup` prepares local delivery fulfillment orders too
  (`fulfillment_orders/line_items_prepared_for_local_delivery`): 2026-10 has no other mutation
  for them.
- Every shop has the general delivery profile, made from the store data: its locations, the rest
  of the world, the `checkout.shipping_rates` that are shipping rates (pickup and local delivery
  are the locations' on Shopify). Shipping packages have no GraphQL type: every shop has
  Shopify's "Sample box" (`gid://shopify/ShippingPackage/1`), which `GET /__lsf/admin` shows
  (`shippingPackages`); a label bought without a package uses the default one.
- A delivery promise provider is a location of the app's fulfillment service
  (`MUST_BELONG_TO_APP`). `deliveryShippingOriginAssign` answers that single origin shipping is no
  longer supported, as Shopify retired it. `deliverySettingUpdate` takes no input in 2026-10 and
  changes nothing.

**Queries**: `inventoryTransfer(s)` (sort keys `CREATED_AT`, `NAME`, `STATUS`, `ORIGIN_NAME`,
`DESTINATION_NAME`, `EXPECTED_SHIPMENT_ARRIVAL`; filters `status`, `tag`, `origin_id`,
`destination_id`, `product_id`, `product_variant_id`, `created_at`), `inventoryShipment(s)`
(`status`, `barcode`, `tracking_number`, `destination_id`), `deliveryProfile(s)` with their
items, counts and unassigned locations, `ProductVariant.deliveryProfile`,
`locationsAvailableForDeliveryProfiles(Connection)`, `carrierService(s)`,
`availableCarrierServices`, `deliveryPromiseProvider`, `deliveryPromiseParticipants`,
`deliveryPromiseSettings`, `fulfillmentConstraintRules`,
`FulfillmentOrder.fulfillmentOrdersForMerge` and the `MERGE`, `REPORT_PROGRESS`, `MARK_AS_OPEN`
actions, `shippingLabel` and the label's `ShippingLabelPurchaseResult` (`node`).

**Webhooks**: `inventory_transfers/add_items`, `/update_item_quantities`, `/remove_items`,
`/ready_to_ship`, `/cancel`, `/complete`, `/updated`; `inventory_shipments/create`, `/add_items`,
`/remove_items`, `/update_item_quantities`, `/mark_in_transit`, `/receive_items`,
`/update_tracking`, `/delete`; `fulfillment_orders/split`, `/merged`, `/moved`,
`/order_routing_complete`, `/progress_reported`, `/manually_reported_progress_stopped`,
`/line_items_prepared_for_pickup`, `/line_items_prepared_for_local_delivery`; `profiles/create`,
`/update`, `/delete`. `fulfillment_events/delete` and `delivery_promise_settings/update` are built
for `lsf app webhook trigger` (no mutation of 2026-10 deletes a fulfillment event or changes the
promise settings). `shipping_addresses/create` and `/update` have no example payload in
Shopify's reference: lsf does not build them.

**Store sync**: the general profile's method definitions that ship to the rest of the world,
with a flat price and no condition, are the local checkout's shipping rates: a price, a name, a
description changed, a rate added or deactivated shows at the checkout. The other zones and
profiles, conditional rates and carrier services' rates have nowhere to go in the store data and
do not reach it; transfers, shipments, fulfillment orders, rules, labels, promises and packages
do not sync (the stock they move does).

### Customers and companies

The customers and companies of the store data, and those the backend creates. What the backend
changes reaches the storefront and the checkout: a customer it creates logs in at the local
checkout by email, with the addresses, consent, tags and metafields it gave, and buys for the
company locations its roles give it.

**Queries**: `customers` (sort keys `NAME`, `LOCATION`, `CREATED_AT`, `UPDATED_AT`, `ID`; the
search filters of 2026-10: `email`, `phone`, `first_name`, `last_name`, `state`, `tag`,
`tag_not`, `country`, `accepts_marketing`, `orders_count`, `total_spent`, `customer_date`,
`order_date`, `updated_at`, `id`, and a free term on the names, email, phone and tags),
`customersCount` (which documents `id` only: another filter is ignored, with Shopify's warning),
`customer(id:)`, `customerByIdentifier` (`id`, `emailAddress`, `phoneNumber`, `customId`),
`companies` (sort keys `NAME`, `ORDER_COUNT`, `TOTAL_SPENT`, `SINCE_DATE`, …), `companiesCount`,
`company`, `companyLocations`, `companyLocation`, `companyContact`, `companyContactRole`, and the
contacts, locations, roles and role assignments of a company.

**Mutations**, as Shopify validates them (`userErrors` with Shopify's messages where they are
published, Rails' wording otherwise), with their webhooks:

| Mutations | What lsf does | Webhooks |
|---|---|---|
| `customerCreate`, `customerUpdate`, `customerSet` | A name, an email or a phone is required; email and phone are unique ("Email has already been taken") and checked ("Email is invalid", "Phone is invalid"). A new customer is `DISABLED`, not subscribed, in the shop's language. `tags` replace the tags; `metafields` set the customer's, checked as `metafieldsSet` checks them; the deprecated `addresses` replace the address book. `customerSet` creates, updates by `id`, or upserts by `email`, `phone` or `customId`. | `customers/create` or `customers/update`, `customer.tags_added` / `customer.tags_removed` for the tags that changed, the consent topics for a consent that changed |
| `customerDelete` | Refused for a customer with orders. Its company contacts go with it. | `customers/delete` |
| `customerAddressCreate`, `customerAddressUpdate`, `customerAddressDelete`, `customerUpdateDefaultAddress` | The first address is the default one; `setAsDefault` changes it; the default address cannot be deleted. `countryCode` gives the country's name; a province keeps its code when its name is not given. | `customers/update` |
| `customerEmailMarketingConsentUpdate`, `customerSmsMarketingConsentUpdate` | `SUBSCRIBED`, `UNSUBSCRIBED` or `PENDING`, for a customer with an email (a phone); `consentUpdatedAt` is now unless given. | `customers/update`, then `customers_email_marketing_consent/update` or `customers_marketing_consent/update` |
| `customerSendAccountInviteEmail` | The customer is `INVITED`. The email is not sent: `extensions.lsf.notes` says so. | `customers/update` |
| `customerGenerateAccountActivationUrl` | `https://<shop>/account/activate/<id>/<token>`; refused for an enabled account. | none |
| `customerAddTaxExemptions`, `customerRemoveTaxExemptions`, `customerReplaceTaxExemptions` | | `customers/update` |
| `tagsAdd`, `tagsRemove` on a customer | The webhook names only the tags added (removed). | `customers/update`, `customer.tags_added` / `customer.tags_removed` |
| `companyCreate` | The company with Shopify's two roles ("Location admin", "Ordering only"), a location (named after the company when none is given), and the contact given, a new customer, main contact, "Ordering only" at the location. | `companies/create`, `company_locations/create`, `customers/create`, `company_contacts/create`, `company_contact_roles/assign` |
| `companyUpdate`, `companyDelete`, `companiesDelete` | Deleting a company deletes its locations, contacts and roles; its customers stay. | `companies/update`; `company_contacts/delete`, `company_locations/delete`, `companies/delete` |
| `companyLocationCreate`, `companyLocationUpdate`, `companyLocationDelete`, `companyLocationsDelete`, `companyLocationAssignAddress`, `companyAddressDelete`, `companyLocationTaxSettingsUpdate`, `companyLocationAssignTaxExemptions`, `companyLocationRevokeTaxExemptions` | `billingSameAsShipping` (or both address types) gives one address for both. | `company_locations/create`, `/update`, `/delete` |
| `companyContactCreate`, `companyContactUpdate`, `companyContactDelete`, `companyContactsDelete`, `companyContactRemoveFromCompany`, `companyAssignCustomerAsContact`, `companyAssignMainContact`, `companyRevokeMainContact` | A contact's id is its customer's (the checkout's `purchasingCompany.contact.id`). Deleting a contact keeps its customer. | `company_contacts/create`, `/update`, `/delete`; `companies/update` for the main contact |
| `companyContactAssignRole(s)`, `companyLocationAssignRoles`, `companyContactRevokeRole(s)`, `companyLocationRevokeRoles` | One role per contact and location; the role and the location must be the contact's company's. | `company_contact_roles/assign`, `/revoke` |

A company location's `ordersCount` and `totalSpent` (and its company's) count the orders placed
for it at the local checkout by a B2B buyer, on top of what the store data says, as a customer's
`numberOfOrders` and `amountSpent` count theirs; a cancelled order costs nothing.

Every id that names nothing answers a `userError` (`Customer does not exist`, `Resource requested
does not exist.`), and a mutation that fails changes nothing. The order of the webhooks of one
mutation is lsf's: Shopify does not document it.

Merges, data erasure, the WhatsApp consent, segments and the staff and tax registrations of a
company location are in "Segments, marketing, analytics and privacy" below.

The WhatsApp consent of `customerCreate` is ignored, with a note.

### Segments, marketing, analytics and privacy

A backend that segments customers, runs campaigns, reports on sales or handles privacy requests
works against lsf as against a store: the segment query language is parsed and evaluated over the
customers of the shop, the marketing activities and analytics the app records are read back,
what Shopify would send to the outside world is recorded in the [outbox](#the-outbox), never
sent.

**Segments** (`segmentCreate`, `segmentUpdate`, `segmentDelete`; `segments`, `segment`,
`segmentsCount`, `customerSegmentMembers`, `customerSegmentMembership`,
`customerSegmentMembersQueryCreate` and `customerSegmentMembersQuery`, `segmentFilters`,
`segmentFilterSuggestions`, `segmentValueSuggestions`):

- The query language of shopify.dev's "Segment query language reference": clauses joined by
  `AND` (tighter) and `OR`, parentheses (100 deep at most, lsf's bound), `=`, `!=`, `>`, `>=`, `<`, `<=`, `BETWEEN … AND …`,
  `CONTAINS`, `NOT CONTAINS`, `IS NULL`, `IS NOT NULL`, `MATCHES (…)` and `NOT MATCHES (…)`;
  quoted strings, numbers, dates, date-times, offsets (`-7d`, `-2w`, `-1m`, `+1y`, at most
  10,000 years either way: lsf's bound, past it the value is not valid), `today`, `yesterday`,
  the deprecated `30_days_ago`; at most 10 clauses. Dates are whole days in the
  shop's time zone (`customerSegmentMembers(timezone:)` for that query, UTC by default).
- The filters: `amount_spent`, `number_of_orders`, `customer_added_date`, `first_order_date`,
  `last_order_date`, `abandoned_checkout_date` (the local checkout's), `customer_account_status`,
  `email_subscription_status`, `sms_subscription_status`, `customer_tags` (case ignored),
  `customer_email_domain`, `customer_language` (`en` matches `en-GB`), `customer_countries`,
  `customer_regions` (`US-NY`), `customer_cities` (`US-NY-NewYorkCity`), `companies`,
  `created_by_app_id` (the app, for the customers it created), `product_subscription_status`,
  `orders_placed MATCHES (app_id, location_id, count, amount, sum_amount, date)`,
  `products_purchased MATCHES (id, tag, quantity, sum_quantity, date)`,
  `store_credit_accounts MATCHES (balance, currency, next_expiry_date, last_credit_date)`,
  `customer_within_distance MATCHES (coordinates, distance_km | distance_mi)`, and
  `anniversary('metafields.<namespace>.<key>')`. What lsf has no data for matches no customer:
  `predicted_spend_tier` and `rfm_group` (Shopify's models: `IS NULL` holds), `shopify_email.*`
  (lsf sends no email), `storefront_event.*` (the storefront records no views).
- A bad query is a user error on `query`, in Shopify's form: `Query Line 1 Column 0:
  'total_spent' filter cannot be found.` (the form and `'MATCHES' is unexpected.` are Shopify's,
  quoted from real answers; the other messages are lsf's).
- Members: `customerSegmentMembers(segmentId: | query: | queryId:)` with `sortKey`
  (`created_at`, `first_order_date`, `last_abandoned_order_date`, `last_order_date`,
  `number_of_orders`, `amount_spent`), `reverse`, `totalCount` and
  `statistics.attributeStatistics` (`amount_spent`, `number_of_orders`). A member's id is
  `gid://shopify/CustomerSegmentMember/<customer id>`. `customerSegmentMembersQueryCreate` runs
  at once: the query is `done`, and `queryId` reads the members it found then.
- **Webhooks**: `segments/create`, `/update`, `/delete`; `customer.joined_segment` and
  `customer.left_segment` when a change of a customer (their record, an order, a company
  contact, a checkout) moves them into or out of a segment. Creating or editing a segment moves
  nobody, and a member whose dates move them out as time passes leaves at their next change.

**Saved searches** (`savedSearchCreate`, `savedSearchUpdate`, `savedSearchDelete`; the
`…SavedSearches` queries of products, collections, customers, orders, draft orders, files, URL
redirects, discounts and redeem codes; `segmentMigrations`): the query is split into
`searchTerms` and `filters` as Shopify's example shows; a connection given `savedSearchId`
(`orders`, `products`, `abandonedCheckouts`, …) lists what the saved search's query keeps.
Customer saved searches fire `customer_groups/create`, `/update`, `/delete`.

**Customers**:

| Mutation | What lsf does | Webhooks |
|---|---|---|
| `customerMerge` (with `customerMergePreview`, `customerMergeJobStatus`, `Customer.mergeable`) | Shopify's rules of the customer kept (the override's email, the only email, an enabled account, the consent, else `customerTwoId`); refused for a company contact, gift cards, a subscription, store credit, payment methods, a multipass identifier, a pending erasure, merged tags over 250 or a note over 5000 characters. The kept customer gets the other's orders, draft orders, addresses, metafields, tags and note (or the overrides); the job is done at once. | `customers/update`, `customers/delete`, `customers/merge` |
| `customerRequestDataErasure`, `customerCancelDataErasure` | The erasure waits 10 days, cancellable meanwhile; then (by the [shop's clock](#the-shops-clock)) the names, email, phone, note and addresses are redacted, consents `REDACTED`, and `customers/redact` is sent to the app. Each step is in the outbox. | `customers/update`, `customers/redact` |
| `customerWhatsAppMarketingConsentUpdate` | As the SMS consent: a phone is required, `SUBSCRIBED`, `UNSUBSCRIBED` or `PENDING`; it shows in `defaultPhoneNumber.whatsAppMarketingConsent`. | `customers/update`, `customers_whats_app_marketing_consent/update` |
| `dataSaleOptOut` | The customer of the email opts out (`dataSaleOptOut: true`); an email no customer has gets a customer of its own *(chosen)*. | `customers/update` or `customers/create` |
| `companyLocationAssignStaffMembers`, `companyLocationRemoveStaffMembers` | The shop's owner is a `StaffMember` (`Shop.accountOwner`); assignments show in `CompanyLocation.staffMemberAssignments`. | none |
| `companyLocationCreateTaxRegistration`, `companyLocationRevokeTaxRegistration` (deprecated) | As `companyLocationTaxSettingsUpdate`: the location's `taxSettings.taxRegistrationId`. | `company_locations/update` |

`customers/purchasing_summary` follows every change of a customer's sales (an order created,
paid, cancelled, edited, refunded): sent when the app subscribes to it, its numbers those of the
customer after the change.

**Marketing** (`marketingActivityUpsertExternal`, `…CreateExternal` (deprecated),
`…UpdateExternal`, `…DeleteExternal`, `marketingActivitiesDeleteAllExternal`,
`marketingActivityCreate` and `marketingActivityUpdate` (the deprecated extension activities),
`marketingEngagementCreate`, `marketingEngagementsDelete`; `marketingActivities` with
`marketingActivityIds`, `remoteIds` and `utm`, `marketingActivity`, `marketingEvent`,
`marketingEvents`): each external activity has its `MarketingEvent` (remote id, URLs, UTM
parameters, dates); `remoteId` names it in every mutation; the errors are
`MarketingActivityUserErrorCode`'s (a UTM campaign or remote id taken, UTM parameters that cannot
change, an activity outside a hierarchy without UTM or URL parameter, …); engagements add up to
the activity's `adSpend`; deleting all is a `Job` done at once. Nothing is sent to the channels.

**Abandoned checkouts**: `abandonedCheckouts` (filters `status`, `recovery_state`,
`email_state`, `created_at`, `updated_at`; sort keys `CUSTOMER_NAME`, `TOTAL_PRICE`, …),
`abandonedCheckoutsCount`, `abandonment`, `abandonmentByAbandonedCheckoutId`,
`abandonmentUpdateActivitiesDeliveryStatuses` and the deprecated `abandonmentEmailStateUpdate`
(the email state an app's marketing activity reports). See "Webhooks" for how the storefront makes
them.

**Analytics**: `analyticsAnnotationCreate`, `…Update`, `…Delete` (the app's annotations, of the
types apps may create: `campaign`, `product_launch`, …; `Shop.analyticsAnnotations`);
`analyticsTargetCreate`, `…Update`, `analyticsTargetsDelete` (a metric of the `sales` schema,
unique by metric, dates and filters; `analyticsTargets`, `presentmentExpectedValue` in the shop's
currency). `shopifyqlQuery` answers `FROM sales` over the shop's orders and refunds: `SHOW`,
`WHERE`, `GROUP BY`, `TIMESERIES` (periods filled), `SINCE`/`UNTIL`/`DURING`, `ORDER BY`,
`LIMIT`, `WITH TOTALS`; the metrics `gross_sales`, `discounts`, `returns`, `net_sales`,
`shipping_charges`, `taxes`, `total_sales`, `orders`, `average_order_value`, `quantity_ordered`,
`net_items_sold`, `customers`, `new_customers`, `returning_customers`, … and the dimensions of
time, order, customer, product and address; a column it does not know is the reference's
`Column Not Found: Column 'total_sale' not found`. `VISUALIZE` and `COMPARE TO` are ignored, with
a note; another schema than `sales` is a parse error, and so are an offset past 10,000 years
(`Syntax Error: '-99999999999d' is not a date`) and parentheses nested more than 100 deep in a
condition (lsf's bounds).

**Privacy**: `consentPolicyUpdate` (once `privacyFeaturesDisable([COOKIE_BANNER])` turned
Shopify's banner off, as Shopify requires), `consentPolicy`, `consentPolicyRegions`;
`privacyFeaturesDisable` and `privacySettings`.

**Webhooks**: also `customer_account_settings/update` (built from the shop's customer account
setting, sent on demand: no mutation changes it) and `audit_events/admin_api_activity` (each
GraphQL request, when the app subscribes to it; Shopify delivers it through Pub/Sub or
EventBridge only, which lsf logs as not sent).

**Store sync**: none of these records reach the storefront or the checkout (segments, saved
searches, marketing, analytics, consent policies, abandoned checkouts, staff assignments). A
merge, an erasure and a data sale opt-out change customers, which sync as customers do.

### Subscriptions and customer payment methods

A subscription app runs against lsf from the purchase to the last renewal: a customer buys with a
selling plan at the local checkout (or the backend creates the contract), the backend bills each
cycle, the order is created, and the card declines or asks for 3D Secure when the test card says
so. Every change is delivered as its webhook.

| Area | Mutations lsf simulates | Webhooks |
|---|---|---|
| Payment methods | `customerPaymentMethodCreditCardCreate`, `…CreditCardUpdate` (a session of lsf's [card vault](control-api.md)), `…PaypalBillingAgreementCreate`, `…PaypalBillingAgreementUpdate`, `…RemoteCreate` (Stripe, Braintree, Authorize.Net: imported at once), `…Revoke` (refused while an active contract uses it), `…GetUpdateUrl` (Shop Pay only, as on Shopify), `…SendUpdateEmail`, `paymentInstrumentSendAddEmail` | `customer_payment_methods/create`, `/update`, `/revoke` |
| Contracts (draft API) | `subscriptionContractCreate`, `subscriptionContractUpdate`, `subscriptionDraftUpdate`, `subscriptionDraftLineAdd`, `…LineUpdate`, `…LineRemove`, `subscriptionDraftDiscountAdd`, `…DiscountUpdate`, `…DiscountRemove`, `…DiscountCodeApply` (the shop's code discounts), `subscriptionDraftFreeShippingDiscountAdd`, `…Update`, `subscriptionDraftCommit`, `subscriptionContractAtomicCreate` | `subscription_contracts/create`, `/update` |
| Contracts (calculation API of 2026-10) | `subscriptionContractCreateCalculate`, `subscriptionContractUpdateCalculate`, `subscriptionBillingCycleContractEditCalculate`, `subscriptionContractCalculationCommit`; the `subscriptionContractCalculation` query | the contract's or the cycle's |
| Status and changes | `subscriptionContractActivate`, `…Pause`, `…Cancel`, `…Expire`, `…Fail`, `subscriptionContractProductChange`, `subscriptionContractSetNextBillingDate` | `subscription_contracts/activate`, `/pause`, `/cancel`, `/expire`, `/fail`, `/update` |
| Billing cycles | `subscriptionBillingCycleSkip`, `…Unskip`, `…ScheduleEdit`, `…EditDelete`, `…EditsDelete`, `subscriptionBillingCycleContractEdit`, `…ContractDraftCommit`, `…ContractDraftConcatenate` | `subscription_billing_cycles/skip`, `/unskip`, `subscription_billing_cycle_edits/create`, `/update`, `/delete` |
| Billing | `subscriptionBillingAttemptCreate`, `subscriptionBillingCycleCharge`, `subscriptionBillingCycleBulkCharge`, `subscriptionBillingCycleBulkSearch` | `subscription_billing_attempts/success`, `/failure`, `/challenged`; the order's |

**Queries**: `subscriptionContract`, `subscriptionContracts` (filters `status`, `customer_id`,
`product_id`, `product_variant_id`, `selling_plan_id`, `last_billing_attempt_error_type`, the
dates; sort keys `CREATED_AT`, `ID`, `STATUS`, `UPDATED_AT`), `subscriptionBillingAttempt`,
`subscriptionBillingAttempts` (`error_code`, `error_message`, `created_at`), `subscriptionDraft`
(with `linesAdded`, `discountsUpdated`, … and `deliveryOptions`, the store data's shipping rates),
`subscriptionBillingCycle`, `subscriptionBillingCycles` (by index or by dates),
`subscriptionBillingCycleBulkResults`, `customerPaymentMethod(showRevoked:)`,
`Customer.paymentMethods`, `Customer.subscriptionContracts`.

How they behave, as shopify.dev documents it:

- **Bought at the checkout**: an order placed in the local checkout with a recurring selling plan
  makes one contract per plan (`subscription_contracts/create`): the plan's policies, the lines at
  their checkout price with the plan's pricing policies, the order's address and delivery,
  `originOrder`, and the card vaulted from the payment (`customer_payment_methods/create`). The
  order's line items name their selling plan. A guest order makes none: Shopify requires a
  customer account for a subscription.
- **Billing cycles** are computed from the billing policy and the shop's clock: cycle 1 starts
  when the contract is created, each is billed at its end (`billingAttemptExpectedDate`, unless a
  schedule edit moves it within the cycle), anchors (`MONTHDAY`, `WEEKDAY`, `YEARDAY`) end the
  cycles at midnight in the shop's time zone, `maxCycles` ends them, and so does the year 9999
  (lsf's bound: a cycle past it is `CYCLE_INDEX_OUT_OF_RANGE`); a new billing interval keeps
  the current cycle's start and never ends it in the past. Edits are per cycle: a skip, a billing
  date, a contract edit (one cycle bills other lines without changing the contract), a
  concatenation (one order bills the cycles of several contracts of the customer).
- **A billing attempt** answers pending (`ready: false`), as Shopify's does, and is processed
  before the answer: the next query reads its outcome. A success creates the order as the orders
  family does (the cycle's lines, discounts, delivery, tax at the store data's rate, a sale of the
  total, `sourceName: subscription_contract`) and commits its stock; the contract lists it and is
  `lastPaymentStatus: SUCCEEDED`. A contract in a presentment currency bills an order presented
  in it (`presentmentCurrencyCode`, every `presentmentMoney` in the contract's currency), its
  `shopMoney` the contract's amounts at a market's manual rate (`baseCurrencyManualRate`), else
  at 1.0: lsf has no exchange rates. A failure has Shopify's codes (`CARD_DECLINED`,
  `INSUFFICIENT_FUNDS`, `INSUFFICIENT_INVENTORY` with the variants short of stock unless
  `ALLOW_OVERSELLING`, `PAYMENT_METHOD_NOT_SPECIFIED`, `EXPIRED_CARD`, …) in its `state` and the
  deprecated `errorCode`; the contract's status is the app's to change (dunning). The same
  `idempotencyKey` answers the first attempt again.
- **Statuses**: a cancelled or expired contract is terminated ("Subscription contract status
  cannot be changed once terminated."). Each committed change is a new `revisionId`; a draft or a
  calculation of an older revision is `STALE_CONTRACT`.
- **The calculation API** answers a `SubscriptionContractCalculationPending`, and the first poll
  reads the `…Success` (calculated contract, projected order totals, warnings) or `…Failure`.
  Lines are restated whole: without `priceOverride` a line is priced from its variant.
- **Emails** are not sent: they are in the shop's [outbox](#the-outbox)
  (`customer_payment_method_update`, `payment_instrument_add`, the link in `data.url`), and
  `GET /__lsf/admin/subscriptions` lists them with the charges.

#### Test cards

What charging a payment method does is decided when it is made, from Shopify Payments' test
cards (and the Bogus gateway's numbers) for a card, deterministic for the same input:

| Card number (a vault session's) | A billing attempt |
|---|---|
| `4242 4242 4242 4242`, `5555 5555 5555 4444`, `1`, any other | succeeds |
| `4000 0000 0000 0002`, `2` | fails: `CARD_DECLINED` |
| `4000 0000 0000 9995` | fails: `INSUFFICIENT_FUNDS` |
| `4000 0000 0000 0069` | fails: `EXPIRED_CARD` (and so does any card past its expiry, by the shop's clock) |
| `4000 0000 0000 0127` | fails: `INCORRECT_CVC` |
| `4000 0000 0000 0119`, `3` | fails: `PROCESSING_ERROR` |
| `4242 4242 4242 4241` | fails: `INCORRECT_NUMBER` |
| `4000 0000 0000 3220`, `4000 0025 0000 3155`, `4000 0000 0000 3063` | is challenged (3D Secure): `subscription_billing_attempts/challenged`, its `nextActionUrl` lsf's page, where **Complete** bills it and **Fail** fails it (`AUTHENTICATION_FAILED`) |

A remote method (`customerPaymentMethodRemoteCreate`) reads Stripe's test payment method ids and
the words they are made of, in any gateway's id: `insufficient` (`INSUFFICIENT_FUNDS`), `expired`,
`incorrectCvc`, `processingError`, `declined` (`CARD_DECLINED`), `authentication`,
`threeDSecure` or `3ds` (challenged); `pm_card_visa` and any other id succeed. An `inactive`
PayPal agreement fails with `INVALID_CUSTOMER_BILLING_AGREEMENT`. To make a contract's next
attempt fail, give its method a new card: `customerPaymentMethodCreditCardUpdate` with a session
of a declining card, or the update page.

```bash
curl -s -X POST http://127.0.0.1:9292/__lsf/admin/card-sessions \
  -H 'content-type: application/json' \
  -d '{"credit_card": {"number": "4000000000000002", "first_name": "Jane", "last_name": "Doe", "month": 8, "year": 2030}}'
# {"id":"lsf-card-…"}: the sessionId of customerPaymentMethodCreditCardCreate
```

What lsf chose where Shopify does not say, or where a local tool must differ:

- Billing is synchronous: Shopify processes attempts in the background, lsf before it answers
  (the answer still says pending). A charge moves no money; the order records a sale through the
  method's gateway (`shopify_payments`, `paypal`, `stripe`, …).
- A billing attempt without a selector bills the earliest cycle that is due and not billed, else
  the one the shop's clock is in (Shopify says "the current billing cycle"). Unanchored cycles
  keep the time of day of the contract's creation in the shop's time zone, across changes of
  summer time; anchored ones end at midnight there.
- The webhooks `subscription_contract_calculations/succeed` and `/fail` the guides name are not
  topics of 2026-10's `WebhookSubscriptionTopic`: none is sent. The payloads of
  `subscription_billing_attempts/*` are those of Shopify's subscriptions reference app (the
  reference has no example of them); `contract_edit` of the cycle payloads is `null`, as in every
  example. A PayPal agreement's `payment_instrument` has its fields in snake_case (the examples
  show only a card's).
- Bundles (`productVariantParentLine`) are kept as a line of their parent variant: lsf runs no cart
  transform on a contract. Delivery options are the store data's shipping rates, for any address.
- Contracts of an app that is uninstalled are not cancelled 48 hours later.
- Store sync: contracts, drafts, billing attempts and payment methods do not reach the storefront
  nor the checkout (the store data has none); the stock a billing order commits does.

### Metafields, metaobjects and content

**Metafields.** `metafieldsSet` and `metafieldsDelete` work on every owner the schema allows
(products, variants, collections, customers, companies and their locations, orders, draft
orders, pages, blogs, articles, locations, markets, discounts, the owners of Functions, the
shop, the app's installation).

- Every type of 2026-10 is checked as Shopify checks it (`number_integer` `"330\n"` is
  "Value must be an integer."), and kept as the Admin API returns it: JSON compact, `money` with
  a string amount in the shop's currency, measurements as `{"value", "unit"}`, references as
  GIDs of records that exist. `metafieldDefinitionTypes` lists the types with their categories and
  validations.
- 25 inputs at most, all or nothing: one error and nothing is set. Errors have Shopify's `field`
  (`["metafields", "0", "value"]`) and `code` (`INVALID_VALUE`, `INVALID_TYPE`, `BLANK`,
  `APP_NOT_AUTHORIZED`, `STALE_OBJECT`, …).
- `namespace` omitted, `$app` and `$app:x` are the app's reserved namespaces (`app--<id>`,
  `app--<id>--x`), in mutations and queries alike; another app's is refused.
- `compareDigest`: `Metafield.compareDigest` gives it; given, the value must not have changed;
  `null`, the metafield must not exist yet.
- With a definition of the owner type, namespace and key, `type` may be left out, must be the
  definition's when given, and the value must pass its validations (`min`, `max`, `regex`,
  `choices`, `list.min`, …). Setting a metafield does not create a definition, through
  `metafieldsSet` or the `metafields` input of a product, a customer or an app discount. The
  store data's metafields come with definitions lsf infers (one per owner type, namespace and
  key).
- The token needs the access of the owner (`write_products` for a product's).
- A product's metafields (or a variant's) are written to the store: the theme's Liquid and the
  Function inputs read them, as every metafield of an owner the store data has.

**Metafield definitions.** `metafieldDefinitionCreate`, `metafieldDefinitionUpdate`,
`metafieldDefinitionDelete` (`deleteAllAssociatedMetafields`, required for the app's
namespaces and for reference types with metafields), `metafieldDefinitionPin`,
`metafieldDefinitionUnpin`; `metafieldDefinitions(ownerType:, namespace:, key:, pinnedStatus:,
sortKey:)` and `metafieldDefinition(identifier:)`. A definition created where metafields exist
takes them and checks them (`validationStatus`). `capabilities` are kept as given; of their
effects, only `cartToOrderCopyable` of an ORDER definition is simulated: it copies a cart
metafield onto the order placed from the cart ([admin](admin.md#what-the-storefront-and-the-checkout-see)).
The order definitions `shopify.app.toml` declares (`[order.metafields.app.<key>]`) are
definitions of the shop from the start; those of other owner types are not read. Unlike
Shopify's, which are read-only through the Admin API, the mutations may change them.
Uninstalling the app deletes the definitions of its reserved namespaces, declared or created,
and keeps their metafields; a reinstall creates them again under new ids, with those metafields,
as Shopify does. The others stay ([uninstall](admin.md#tokens)).

**Metaobjects.** `metaobjectDefinitionCreate`, `metaobjectDefinitionUpdate` (field definitions
created, updated and deleted; the entries follow), `metaobjectDefinitionDelete` (with its
entries), `metaobjectCreate`, `metaobjectUpdate`, `metaobjectUpsert`, `metaobjectDelete`,
`metaobjectBulkDelete` (done at once: its `job` is `done`); `metaobjects(type:)` with `sortKey`
and `query` (`fields.<key>:value`), `metaobjectByHandle`, `metaobjectDefinitionByType`,
`Metaobject.field(key:)` and `values`.

- `$app:author` is the app's type (`app--<app id>--author` in the answers, as on Shopify). Its
  entries reach the storefront: the theme reads them as `metaobjects['app--<app id>--author']`
  (with its real app id, which lsf does not need to know) or, in the app's extension,
  `metaobjects['$app:author']`; a Function as `shop.metaobject(handle: {type: "$app:author", …})`.
  The store data writes the type `$app:author`.
- Field values are checked against the field definitions as
  metafield values are; required fields must have one; `fields` patches, `values` replaces.
- A handle left out is made from the display name (`Jane Austen` → `jane-austen`, then
  `jane-austen-1`).
- With the `publishable` capability, an entry is a `DRAFT` until it is `ACTIVE`; only active
  entries are on the storefront.

**Online store.** `pageCreate`, `pageUpdate`, `pageDelete`; `blogCreate`, `blogUpdate`,
`blogDelete`; `articleCreate` (in a blog by `blogId`, or a new one from `blog: {title}`),
`articleUpdate`, `articleDelete`; `commentApprove`, `commentSpam`, `commentNotSpam`,
`commentDelete`; `menuCreate`, `menuUpdate`, `menuDelete`; `urlRedirectCreate`,
`urlRedirectUpdate`, `urlRedirectDelete`. Handles are made from titles, `redirectNewHandle`
creates the redirect, `metafields` inputs are checked as `metafieldsSet` checks them. Published
pages and articles, menus, and active metaobjects are written to the store the theme renders.

**Files.** `fileCreate` (up to 250, from a URL or a staged upload), `fileUpdate` (alt, filename,
content, the products a file is a medium of), `fileDelete`; `files` searched by name and alt,
sorted by `FILENAME`. A file is `READY` as soon as it is created: lsf has nothing to process, so
a backend that polls `fileStatus` sees it at once. Its URL is the source's. From a staged upload
(`originalSource` the target's `resourceUrl`, once the file is uploaded), the file has the name
the target was made for, the MIME type it was uploaded with, and its size (`originalFileSize`,
`originalSource.fileSize`); its URL serves the bytes.

**Translations.** `translationsRegister` and `translationsRemove`; `translatableResource`,
`translatableResources(resourceType:)`, `translatableResourcesByIds`, and `translations(locale:)`
of the resources. lsf translates products, collections, pages, blogs, articles, metafields and
metaobject fields of translatable types, policies, product options and their values, and menus.
The digest is the SHA-256 of the content, as Shopify's; the locale must be one of the shop's
other than its primary one. A translation is `outdated` once its resource changed. Translations
are not shown on the storefront.

**Webhooks.** `metafield_definitions/create`, `/update` (also pin and unpin), `/delete`;
`metaobjects/create`, `/update`, `/delete`; `products/update` when a product's or a variant's
metafields change. Shopify has no topic for pages, blogs, articles, comments, menus, redirects,
files, or the metafields of other owners.

Redirects in bulk, standard definitions, `fileAcknowledgeUpdateFailed` and themes are in
"[Themes, script tags, redirects in bulk and
policies](#themes-script-tags-redirects-in-bulk-and-policies)" below. Not simulated:
metafield definition `constraints`, the effects of `capabilities` and `access` (kept as given),
and staff authors of articles (`author.userId`).

### Markets, catalogs, price lists and channels

The markets of the store data (those it declares, and a market of its own for each country in
none) are the shop's markets, with the primary domain's web presence in every locale; a backend
that manages markets, B2B catalogs or a sales channel works against lsf as against a store.

| Area | Mutations lsf simulates |
|---|---|
| Markets | `marketCreate`, `marketUpdate` (conditions to add and delete, currency settings, price inclusions, catalogs, web presences, discounts), `marketDelete`, `backupRegionUpdate`; the deprecated `marketCurrencySettingsUpdate`, `marketRegionsCreate`, `marketRegionDelete`, `marketRegionsDelete` |
| Localization | `marketLocalizationsRegister`, `marketLocalizationsRemove`, `shopLocaleEnable`, `shopLocaleUpdate`, `shopLocaleDisable`, `webPresenceCreate`, `webPresenceUpdate`, `webPresenceDelete`; the deprecated `marketWebPresenceCreate`, `…Update`, `…Delete` |
| Catalogs | `catalogCreate`, `catalogUpdate`, `catalogDelete` (`deleteDependentResources` included), `catalogContextUpdate` |
| Price lists | `priceListCreate`, `priceListUpdate`, `priceListDelete`, `priceListFixedPricesAdd`, `priceListFixedPricesUpdate`, `priceListFixedPricesDelete`, `priceListFixedPricesByProductUpdate`, `quantityRulesAdd`, `quantityRulesDelete`, `quantityPricingByVariantUpdate` |
| Publications and channels | `publicationCreate`, `publicationUpdate`, `publicationDelete`, `channelCreate`, `channelUpdate`, `channelDelete`, `channelFullSync`, `bulkProductResourceFeedbackCreate`, `shopResourceFeedbackCreate`; the deprecated `publishablePublishToCurrentChannel`, `publishableUnpublishToCurrentChannel` |

How they behave, as shopify.dev documents it:

- **Markets** match buyers by their conditions: countries (`REGION`), company locations, retail
  locations. A country is in one active region market at most: a second answers
  `DUPLICATE_UNIQUE_MARKET`, unless `makeDuplicateUniqueMarketsDraft` puts the other in draft.
  The handle is made from the name, and written as a handle (`"NA"` is `na`). A market's
  currency is the local one of its countries when they share one, else the shop's;
  `localCurrencies` says whether each country keeps its own. The primary market (the shop's
  country's) cannot be deleted or put in draft.
- **Web presences** are a domain or a subfolder of the primary domain (`/fr-ca`), with a
  default locale and alternate ones, which must be the shop's (`DISABLED_LANGUAGE`). Their
  `rootUrls` follow (`https://shop/fr`, `https://shop/fr-ca`). A domain whose presence or
  locales change sends `domains/update`, its `localization` in the payload.
- **Locales**: `shopLocaleEnable` adds one unpublished (Shopify's 137 locales,
  `availableLocales`), `shopLocaleUpdate` publishes it, `shopLocaleDisable` deletes it with its
  translations. Market localizations are of metafields (`value`) and metaobjects (their
  fields), checked by the content's digest as translations are.
- **Catalogs** have markets or company locations as context ("exactly one context type"), a
  price list and a publication, both optional. `gid://shopify/Catalog/<n>` names any catalog,
  as Shopify's examples do. The Online Store's catalog and a channel's are app catalogs, which
  cannot be changed.
- **Price lists** adjust every variant's price by a percentage (`RELATIVE` prices), with fixed
  prices in the list's currency (`FIXED`; "The specified currency does not match the price
  list's currency."). Changing the currency removes the fixed prices. Quantity rules and price
  breaks are B2B: a company location catalog's price list takes them, with the checks and the
  messages of the reference's examples (the increment at most the minimum, a break at least the
  rule's minimum, …). `quantityPricingByVariantUpdate` deletes, then adds, all or nothing.
- **Contextual pricing**: `Product.contextualPricing` and `ProductVariant.contextualPricing`
  answer the price of a context (`country`, `companyLocationId`, `locationId`) from the price
  list of its active catalog (the lowest of several, as B2B does), with the variant's quantity
  rule and price breaks; without one, the variant's own price in the market's currency.
  `marketsResolvedValues` answers what a buyer of a country gets.
- **Publications** of catalogs: empty or with every product (`ALL_PRODUCTS`), and the products
  created later with `autoPublish`. `publicationUpdate` adds and removes products and
  collections, 50 at most, with the events of `publishablePublish`.
- **Channels**: `channelCreate` makes the app's channel, with a publication and an app catalog
  of its own; it covers every country of the shop's active region markets (lsf has no channel
  specification to read). Publishing on it (`publishablePublish` with its publication, or
  `publishablePublishToCurrentChannel`) is `product_listings/add` (`collection_listings/add`),
  unpublishing or deleting the product `…/remove`, changing it while listed `…/update`. A future
  `publishDate` on a channel schedules the listing: `scheduled_product_listings/add`, listed once
  the [shop's clock](#the-shops-clock) passes the date. `channelFullSync` answers a trace per
  country and published language.
- **Resource feedback**: `bulkProductResourceFeedbackCreate` (read by
  `productResourceFeedback`) and `shopResourceFeedbackCreate` (read by `App.feedback` and
  `Channel.resourceFeedback`; `ACCEPTED` clears it). Older feedback answers "Feedback for a
  later version of this resource was already accepted."
- **Queries**: `markets(type:)` (by name), `marketByGeography`, `primaryMarket`,
  `backupRegion`, `marketLocalizableResource(s)`, `catalogs(type:)`, `catalogsCount`,
  `priceList(s)`, `publications(catalogType:)`, `publicationsCount`, `channels`,
  `channelByHandle`, `shopLocales(published:)`, `webPresences`, and what they are read through:
  `Market.catalogs`, `Market.webPresences`, `CompanyLocation.catalogs`, `inCatalog`, `market`,
  `Customer.market`, `publishedOnChannel`, `publishedOnCurrentChannel`, `unpublishedChannels`.

**Webhooks**: `markets/create`, `markets/update`, `markets/delete`,
`markets_backup_region/update`, `locales/create`, `locales/update`, `locales/destroy`,
`domains/update` (and `domains/create`, `domains/destroy` on demand: Shopify makes domains, not
the Admin API), `channels/delete`, `product_listings/*`, `collection_listings/*`,
`scheduled_product_listings/*`. Shopify has no topic for catalogs, price lists, publications,
web presences or market localizations.

**Store sync**: a market's countries and the currency each sells in reach the store (a market
created, renamed, put in draft, given countries or another currency), and so do the published
languages (served under `/<locale>/`). Not synced: price lists and their fixed prices (the
storefront has one price per variant, and no exchange rates), B2B catalogs and quantity rules
(the local checkout gives a company location buyer the prices and rules of the data, as it
gives everyone), web presences (the storefront has one address), channels and their listings,
market localizations.

**Differences**: lsf has no exchange rates: a price converted to another currency uses the
market's `baseCurrencyManualRate`, else 1.0, and `extensions.lsf.notes` says so.
`channelFullSync` syncs the channel's product feeds (see "Products, collections and inventory"). Where shopify.dev documents a user
error by its code only, the message is the code's description in the schema or lsf's (the source
of each is in the code).

### Point of sale, cash drawers and payment sessions

Shopify POS runs on phones and tablets the Admin API cannot make: the seed gives each active
location one `PointOfSaleDevice`, and the shop its owner (a `StaffMember`) when nothing else
seeded staff, so that a backend that manages cash works against lsf as against a store with POS.

| Area | Mutations lsf simulates |
|---|---|
| Cash drawers | `cashDrawerCreate`, `cashDrawerFindOrCreate`, `cashDrawerUpdate`, `pointOfSaleDeviceAssignToCashDrawer` |
| Reason codes | `cashManagementReasonCodeCreate`, `cashManagementReasonCodeDelete` |
| Payment sessions | `pointOfSaleDevicePaymentSessionOpen`, `…Count`, `…Adjust`, `…Close` |

- **Drawers** have a name unique at their location and a `balance` in the shop's currency; a
  device is assigned to one drawer of its location ("must be in the same location").
- **Sessions**: a device with a drawer opens one session at a time (`ALREADY_OPEN`). Counts
  (opening, mid-session, closing) set the drawer's balance to what was counted and record the
  discrepancy; adjustments add or take cash. `totalDiscrepancy`, `totalAdjustments`,
  `closingAdjustment`, `cashCountedAtClose` and the activities (`cashActivities`) follow, as do
  the drawer's `totalAdjustments(dateRange:)` and `totalDiscrepancies`, and
  `cashManagementLocationSummary`, `cashManagementShopSummary` (sessions opened and closed,
  balances at the start and the end, net cash). lsf has no POS sales: sales and refunds are 0.
- **Reason codes**: Shopify's default and system codes are seeded; only custom ones can be
  deleted.
- **Queries**: `cashDrawer(s)`, `pointOfSaleDevice`, `pointOfSaleDevicePaymentSession(s)`
  (`is_open`, `location_id`, `point_of_sale_device_id`, the times), `cashManagementReasonCodes`,
  the deprecated `cashTrackingSession(s)`, read from the same sessions.
- **Nothing reaches a device**: what Shopify would push to the POS app (a drawer assigned, a
  session opened or closed) is in the shop's [outbox](#the-outbox) (channel `pos`, `to` the
  device) and said in `extensions.lsf.notes`. Shopify has no webhook topic for any of it.
  Nothing syncs to the storefront.

### Billing, discounts, Functions and bulk operations

**Billing.** The billing helpers of the React Router template (`billing.require`,
`billing.request`, `billing.check`, `billing.cancel`, `createUsageRecord`,
`updateUsageCappedAmount` of `@shopify/shopify-api`) work unchanged, in the loader of a page and
in a route the front end fetches (["Billing"](admin.md#javascript-the-react-router-app-and-shopifyshopify-api)):

- `appSubscriptionCreate` (recurring and usage lines, a discount, `trialDays`, `test`) and
  `appPurchaseOneTimeCreate` make a `PENDING` charge and answer a `confirmationUrl` as
  Shopify's, on the shop's admin:
  `https://<shop>/admin/charges/<app id>/<n>/RecurringApplicationCharge/confirm_recurring_application_charge?signature=…`
  (`ApplicationCharge/confirm_application_charge` for a one-time purchase). lsf's admin serves
  that path on its origin and redirects it to lsf's charge page,
  `/__lsf/admin/charges/<n>?shop=…`. The page shows the charge; **Approve** (or **Decline**)
  sends the merchant back to `returnUrl` with `charge_id=<n>`, as Shopify does; a `returnUrl`
  on Shopify's admin or on the app is the app in lsf's admin. In a test,
  `POST /__lsf/admin/charges/<n>/approve?shop=<shop>` (or `/decline`) does it without the page.
  Charges count from 1 in each shop, on the shop of the URL (see
  [approve a charge](admin.md#approve-a-charge)).
- Approved, a subscription is `ACTIVE` and the app's other active subscription `CANCELLED`;
  `currentAppInstallation { activeSubscriptions oneTimePurchases allSubscriptions }` list them. A
  charge left pending two days (by the shop's clock) is `EXPIRED`. Uninstalling the app
  (`POST /__lsf/admin/uninstall`) cancels its subscription, without a webhook.
- `appUsageRecordCreate` charges a usage line of an active subscription, up to its capped amount
  (`"Failed to create usage charge"` past it); the same `idempotencyKey` gives back the same
  record. `appSubscriptionLineItemUpdate` asks for a new capped amount, which the merchant
  approves on the same page. `appSubscriptionTrialExtend` and `appSubscriptionCancel` too.
- Webhooks: `app_subscriptions/update` (a status or a capped amount changed),
  `app_purchases_one_time/update`, `app_subscriptions/approaching_capped_amount` (the usage
  balance crossed 90%). Under managed pricing with `legacy = false` (Shopify App Pricing), no
  change of a subscription sends one, `appSubscriptionCancel` included
  ([managed pricing](admin.md#managed-pricing)).
- No money moves: `prorate` credits nothing. `currentPeriodEnd` is the end of the trial, else 30
  days (365 for an annual plan) after the approval; once [the shop's clock](#the-shops-clock)
  passes it, the next request starts the next cycle: `currentPeriodEnd` moves on by the
  interval, the usage balance starts again from nothing, a limited discount has an interval
  less (`remainingDurationInIntervals`), and no webhook is sent. A replacement Shopify defers to
  the next cycle (`APPLY_ON_NEXT_BILLING_CYCLE`, and some `STANDARD` ones) applies at once.
- [Managed pricing](admin.md#managed-pricing): with plans declared (`--pricing-plans`,
  `PUT /__lsf/admin/pricing-plans`), the merchant picks one at
  `/admin/charges/<app handle>/pricing_plans` of lsf's admin; the subscription is the same record,
  its line's `planHandle` set, and the two creating mutations above answer "Managed Pricing Apps
  cannot use the Billing API (to create charges)." `POST /__lsf/admin/pricing-plans/cancel` ends
  the shop's plan without the app: `CANCELLED`, with the webhook unless `legacy = false`. The app
  reads its plan and its history from lsf's [Partner API](admin.md#read-the-plan-from-the-partner-api)
  (`activeSubscription`, `events`, `app { events }`), which every change of a charge, install and
  uninstall feeds.

**Discounts.** `discountAutomaticAppCreate`/`Update` and `discountCodeAppCreate`/`Update` take the
Function by `functionHandle` (or the deprecated `functionId`), which must be a discount Function
of the app; their `metafields` are the Function's configuration, checked as `metafieldsSet`
checks them (a definition of the `DISCOUNT` owner type applies). The store sync writes an active
app discount back to the store data's `checkout.discounts` (its Function, title, first code,
classes and metafields), so the local checkout runs it. Shopify's own types (basic, buy X get Y,
free shipping, automatic and code) are kept as records: the local checkout runs Functions only.
Also: `discountAutomaticActivate`/`Deactivate`/`Delete` and the code ones (a discount's `status`
follows its dates), `discountAutomaticBulkDelete`, `discountCodeBulkActivate`/`Deactivate`/
`Delete`, `discountRedeemCodeBulkAdd`, `discountCodeRedeemCodeBulkDelete`,
`discountBulkTagsAdd`/`Remove` (5 tags at most each): the bulk ones take exactly one of `ids`,
`search` and `savedSearchId` (a saved search of discounts, `PRICE_RULE`, or of redeem codes,
`DISCOUNT_REDEEM_CODE`), and are done at once (their `job` is `done`); the queries
`discountNodes` (ids `DiscountNode/<n>`),
`discountNodesCount`, `discountNode`, `codeDiscountNodeByCode`, `discountCodesCount`,
`automaticDiscountNodes`, `codeDiscountNodes`, with their sort keys and search filters (`status`,
`type`, `method`, `title`, `code`, `tag`, the dates, …). Webhooks: `discounts/create`,
`discounts/update`, `discounts/delete`, `discounts/redeemcode_added`,
`discounts/redeemcode_removed`.

**Functions.** `shopifyFunctions(apiType:)` lists the app's Functions. Payment and delivery
customizations (`…Create`, `…Update`, `…Delete`, `…Activation`), validations (`validationCreate`,
`Update`, `Delete`) and cart transforms (`cartTransformCreate`, `Delete`) take the Function by
handle or id, check it implements their API, and keep its configuration in their metafields,
which the store sync writes back as the Function's settings in `checkout.functions`, with
`enabled`: the local checkout runs the Function only through an enabled owner, as Shopify does,
so creating, disabling (`…Update`, `…Activation`, `validationUpdate(enable:)`) or deleting one
changes the next checkout. A shop the app was just installed on has none: lsf creates no owner
at install (the store data may declare some, `checkout.functions.<handle>.enabled`). Uninstalling
the app deletes them all, with the app discounts and the fulfillment constraint rules, and a
reinstall brings none back ([uninstall](admin.md#tokens)). Titles are the Functions' names, a
`t:` key of `shopify.extension.toml` read from its default locale.

**Bulk operations.** `bulkOperationRunQuery` runs the query against the shop, every page of every
connection, and writes JSON Lines as Shopify does (one line per node, nested connections on lines
of their own with `__parentId`); Shopify's restrictions are checked (a connection, five at most,
two levels deep, no top-level `node`). `bulkOperationRunMutation` runs the mutation once per line
of a staged upload. Both finish within the request: the payload says `CREATED`, as Shopify's
does, the next query reads `COMPLETED` with its `url`, `objectCount` and `fileSize`, and
`bulk_operations/finish` is delivered. The file is at
`http://127.0.0.1:9292/__lsf/admin/bulk/<shop>/<n>.jsonl`. `bulkOperation`, `bulkOperations`
(filtered by `status`, `operation_type`, `created_at`), `currentBulkOperation`.

```js
// In a loader or an action of the React Router template: `const { admin } = await authenticate.admin(request);`
const started = await admin.graphql(`#graphql
  mutation {
    bulkOperationRunQuery(query: "{ products { edges { node { id title variants { edges { node { id sku } } } } } } }") {
      bulkOperation { id status }
      userErrors { field message }
    }
  }`);
const { bulkOperationRunQuery } = (await started.json()).data;
const { id } = bulkOperationRunQuery.bulkOperation; // status CREATED

const polled = await admin.graphql(
  `#graphql
  query($id: ID!) { node(id: $id) { ... on BulkOperation { status url } } }`,
  { variables: { id } },
);
const { node } = (await polled.json()).data;
// COMPLETED at the first poll on lsf; on Shopify, poll until it is, or wait for bulk_operations/finish.
const lines = (await (await fetch(node.url)).text()).trim().split("\n").map((line) => JSON.parse(line));
// { id: "gid://shopify/Product/…", title }, then its variants, each with the product's id in __parentId
```

`bulk_operations/finish` reaches the backend only through a subscription:
`topics = ["bulk_operations/finish"]` in `shopify.app.toml`, or `webhookSubscriptionCreate`.

**Staged uploads.** `stagedUploadsCreate` answers targets on lsf, in the form of Shopify's Google
Cloud Storage ones: a `POST` target (`url` and form `parameters`, the `file` last) or a `PUT`
target (the default). The bytes stay in the shop until it is reset, for
`bulkOperationRunMutation` (`stagedUploadPath`) and `fileCreate` (`originalSource`, the
`resourceUrl`). A target takes up to 1 GiB (Shopify's largest staged upload, a video; an image
is 20 MB there, a 3D model 500 MB), and a MiB for the form around it; any other request of lsf
takes 32 MiB. A larger body is answered `413`, never read as an empty one. The deprecated
`stagedUploadTargetGenerate` (one target) and `stagedUploadTargetsGenerate` (`urls`) answer the
same targets.

Credits, revenue attribution records and frozen subscriptions have no data behind them. The
app's access, delegate tokens and Flow triggers are in "[The app's installation and Shopify's
other services](#the-apps-installation-and-shopifys-other-services)" below.

### Themes, script tags, redirects in bulk and policies

**Themes.** The theme lsf serves (`lsf serve --theme`, the scratch theme of `lsf app dev`) is
the shop's **main theme**, `gid://shopify/OnlineStoreTheme/1` (the id `content_for_header`
gives `Shopify.theme`), named as its `config/settings_schema.json` says. `theme`, `themes(roles:,
names:)` and `OnlineStoreTheme.files(filenames:, first:)` read its files from the directory,
with their `checksumMd5` (hex MD5), `size`, `contentType` (`text/x-liquid` for Liquid) and
body (text, or Base64 for the others); `filenames` takes `*`, and a name no file has is one of
the connection's `userErrors` (`NOT_FOUND`).

- **The directory is never written to.** `themeFilesUpsert`, `themeFilesCopy` and
  `themeFilesDelete` on the main theme are kept in memory and served by the storefront on top of
  the directory: a backend that writes `templates/index.json` sees its page on the storefront,
  and your files stay yours. `DELETE /__lsf/admin` drops what it wrote.
- `themeCreate` from a ZIP (a staged upload of `stagedUploadsCreate`, or a URL lsf downloads:
  see "Sources from a URL"; 50 MB at most; `INVALID_ZIP`, `ZIP_IS_EMPTY`, `ZIP_TOO_LARGE`; roles `UNPUBLISHED` and
  `DEVELOPMENT`), `themeDuplicate` (all the files, "Copy of …"), `themeUpdate` (the name, 50
  characters), `themeDelete` (not the main theme, and never the local theme, even once another
  is published: it is the directory, which would be gone until a reset), `themePublish`. A
  published theme is what the storefront serves, alone, and what the page names: `Shopify.theme`
  and the `theme` object have its id and name, and asset URLs are under `/cdn/shop/t/<id>/`;
  publishing the local theme again serves the directory, id 1.
- `themeFilesUpsert`: 50 files at most, one input per filename, files in the theme's directories
  (no `..`, and no `\`, `:`, NUL or control character: what Windows would read as another
  path is refused on every system, and no file outside the directory is ever read), JSON files
  valid JSON (`FILE_VALIDATION_ERROR`, `DUPLICATE_FILE_INPUT`,
  `LESS_THAN_OR_EQUAL_TO`); `TEXT`, `BASE64` and `URL` bodies (a `URL` file comes through the
  `job`, done, as in Shopify's example; one over 50 MB is refused, `FILE_VALIDATION_ERROR`, never
  cut). Nothing is written when a file is refused.
  `layout/theme.liquid` cannot be deleted.
- Webhooks: `themes/create`, `themes/update`, `themes/publish`, `themes/delete` (none for files,
  as on Shopify).

**Sources from a URL.** The `source` of `themeCreate`, the `URL` bodies of `themeFilesUpsert`
and the `url` of `urlRedirectImportCreate` are read from lsf's staged uploads when they are
one (`/__lsf/admin/uploads/…`, never over HTTP), and otherwise downloaded before the request
locks the shop: a slow source holds up no other request of the shop (the storefront's carts and
checkouts included). 8 are fetched at a time, each within 10 s, all of a request's within 30 s;
one over 50 MB is refused. A URL that only a line of a bulk operation names is not fetched, and
the line says so.

Only public addresses are fetched from: Shopify downloads a source from the internet and can
never reach your machine, and lsf refuses the same, or a backend (yours under development, or
a third party's) could read what your machine serves locally and get it back as a theme file.
The loopback (`localhost`, `127.0.0.1`, `::1`), private networks (`10/8`, `172.16/12`,
`192.168/16`, `100.64/10`), link-local (`169.254/16`, `fe80::/10`, cloud metadata endpoints),
unique-local (`fc00::/7`), unspecified, multicast and reserved addresses are refused, IPv6 ones
that embed an IPv4 address (`::ffff:127.0.0.1`) as that address. The check is made on the
addresses a host resolves to, on every redirect, and proxies of the environment are not used.
A refused source answers what an unreachable one does (`INVALID_ZIP` for `themeCreate`,
`FILE_VALIDATION_ERROR` for a file, `NOT_FOUND` for an import), its message saying why. To serve
a theme ZIP from your machine on purpose, start lsf with `--admin-fetch-local`. `file://` and
the schemes other than `http` and `https` are never fetched.

<a id="stubbed-urls"></a>
**Stubbed URLs.** A URL your backend has of its own (a theme ZIP on its CDN, a CSV a service of yours makes) has
nothing behind it in a test. Stub it: lsf answers it with the bytes you give, and never
downloads it, wherever it points (`localhost` too, without `--admin-fetch-local`).

- **From a request**, in a test's setup: the body is the file.

  ```js
  const url = "https://cdn.example.com/themes/comfort.zip";
  await fetch(`${LSF}/__lsf/admin/sources?url=${encodeURIComponent(url)}`, {
    method: "PUT",
    body: await readFile("fixtures/comfort.zip"),
  });
  // themeCreate(source: "https://cdn.example.com/themes/comfort.zip") reads it.
  ```

- **From a file**, for the whole run:
  `lsf serve --admin-source https://cdn.example.com/themes/comfort.zip=./fixtures/comfort.zip`.
  Repeatable. The file must exist when lsf starts, and is read at each use: an edit is seen at
  once.

A URL is matched as written, its query included; the scheme and host in any case, the fragment
left out. A stub stands for the internet, not for a shop: every shop reads it, and a reset
(`DELETE /__lsf/admin`) keeps it. `GET /__lsf/admin/sources` lists the stubs,
`GET /__lsf/admin/sources?url=` gives one's bytes back, and `DELETE /__lsf/admin/sources`
forgets those requests made (`?url=` one of them; an `--admin-source` file answers its URL
again). A stub over 50 MB answers what a source that large does (`ZIP_TOO_LARGE`), and an
`--admin-source` file gone since lsf started answers what an unreachable source does.

A request never names a file of the machine: only `--admin-source` does, or any client of
lsf could read a file of yours back as a theme file. A backend that takes the URL as a
parameter needs no stub: give it a staged upload of lsf
(`PUT /__lsf/admin/uploads/<shop>/<key>`, then
`http://127.0.0.1:9292/__lsf/admin/uploads/<shop>/<key>`), which is read from the shop.

**Script tags.** `scriptTagCreate`, `scriptTagUpdate`, `scriptTagDelete`; `scriptTag`,
`scriptTags(src:, query:)`. A script tag shown on the online store (`ONLINE_STORE`, or the
deprecated `ALL`) is loaded by the storefront's `content_for_header` as Shopify loads it, once
the page has loaded, with `shop=<domain>` added to its URL. `cache` is kept (lsf serves the
script from its `src` either way). Uninstalling the app deletes its script tags, as Shopify does,
and the storefront stops loading them ([uninstall](admin.md#tokens)).

**Pixels.** `webPixelCreate`, `webPixelUpdate`, `webPixelDelete`, `webPixel`: one per app, its
`settings` checked against the `[settings]` of the app's web pixel extension
(`type = "web_pixel_extension"` in `extensions/*/shopify.extension.toml`): every field given
("Missing keys: accountID"), no other, each value as its type and validations say. Without such
an extension: `NO_EXTENSION`. The storefront does not run web pixels. Uninstalling the app
deletes its web pixel: a reinstalled app creates it again. `serverPixelCreate`,
`serverPixelDelete`, `eventBridgeServerPixelUpdate`, `pubSubServerPixelUpdate`, `serverPixel`:
one per app, `CONNECTED` once it has an endpoint; nothing is sent to it.

**Storefront access tokens.** `storefrontAccessTokenCreate` (100 per shop),
`storefrontAccessTokenDelete`, `shop.storefrontAccessTokens`: 32 hex digits, with the app's
`unauthenticated_*` scopes. lsf serves no Storefront API, so they authenticate nothing.

**Redirects in bulk.** `urlRedirectBulkDeleteAll`, `urlRedirectBulkDeleteByIds`,
`urlRedirectBulkDeleteBySearch` (the `query` of `urlRedirects`; a field it does not have is
refused rather than deleting every redirect), `urlRedirectBulkDeleteBySavedSearch` (a [saved
search](#segments-marketing-analytics-and-privacy) of redirects), each done at once (its `job`
done). `urlRedirectImportCreate` reads a redirect CSV (`Redirect from,Redirect to`, RFC 4180:
quoted cells may hold commas and line breaks) from a staged upload or a URL (`count`,
`previewRedirects`); `urlRedirectImportSubmit`
creates and updates the redirects (`createdCount`, `updatedCount`, `failedCount`, `finished`;
twice is `ALREADY_IMPORTED`); `urlRedirectImport`.

**Policies.** `shopPolicyUpdate`: the policy of its type replaced, or created with Shopify's title
and URL; the five policies of the store data (privacy, refund, shipping, terms of service,
subscription) reach the storefront's `shop.policies`.

**Standard definitions.** `standardMetafieldDefinitionTemplates` (shopify.dev's 16, `excludeActivated`),
`standardMetafieldDefinitionEnable` (by `id`, or `namespace` and `key`; the owner type must be
one of the template's) and `standardMetaobjectDefinitionEnable` (`product_review`) create the
definition as `metafieldDefinitionCreate` and `metaobjectDefinitionCreate` do (same checks, same
webhooks), with its `standardTemplate`.

**Files.** `fileAcknowledgeUpdateFailed` puts a `FAILED` file back to `READY` and clears its
errors (lsf's files are `READY` at once, so only a file made `FAILED` otherwise is one).

### The app's installation and Shopify's other services

**Access.** `appUninstall` uninstalls the app as `POST /__lsf/admin/uninstall` does
([Uninstall](admin.md#tokens)): its Functions go with every object that runs them, its script
tags, web pixel and the metafield definitions of its reserved namespaces are deleted (their
metafields stay), and its UI extensions and app proxy stop until it is installed again.
`appRevokeAccessScopes` revokes optional scopes the installation was granted (not the
`scopes` of `shopify.app.toml`: `CANNOT_REVOKE_REQUIRED_SCOPES`; not an undeclared one; not a
`read_x` its `write_x` implies): every token of the installation loses them at once, and
`app/scopes_update` is delivered. `appDowngradeAccessScopes` (2027-01) turns optional `write_x`
scopes into `read_x`. To grant optional scopes first: `POST /__lsf/admin/install {"scopes": …}`.

**Delegate access tokens.** `delegateAccessTokenCreate` makes a `shppa_…` token with a subset of
the parent's scopes (`UNKNOWN_SCOPES` for one the app was not granted), expiring with its parent
or sooner (`expiresIn`, `EXPIRES_AFTER_PARENT`); it authenticates the Admin API with those
scopes only, and cannot make delegates of its own (`DELEGATE_ACCESS_TOKEN`), nor be exchanged
at `/admin/oauth/access_token` as an offline token (`400` `invalid_subject_token`: what it would
become carries every scope of the installation). `delegateAccessTokenDestroy` revokes one. A scope the installation loses goes from its delegates
too.

**Flow triggers.** `flowTriggerReceive(handle:, payload:)` is checked against the app's
`flow_trigger` extensions: the handle, every field of the trigger (a reference field by its key,
`customer_id`, the `legacyResourceId` of a record of the shop), 50,000 bytes at most. lsf runs
no workflow: the trigger goes to the shop's [outbox](#the-outbox) (channel `flow`, the handle as
`to`, the payload as `data`).

**Mobile apps.** `mobilePlatformApplicationCreate`, `Update`, `Delete`,
`mobilePlatformApplication(s)`: Android and Apple apps. The storefront serves their association
files, as Shopify does on the shop's domain: `/.well-known/assetlinks.json` (App Links) and
`/.well-known/apple-app-site-association` (Universal Links, shared web credentials, app clips).

**Checkout configuration.** `checkoutAndAccountsConfigurationUpdate` and the deprecated
`checkoutBrandingUpsert` merge the branding they are given into the shop's default
configuration and checkout profile (a field given `null` is reset); `checkoutProfile(s)`,
`checkoutBranding`, `checkoutAndAccountsConfiguration(s)` read it back. The local checkout keeps
its own look.

**Webhook subscriptions to EventBridge and Pub/Sub.** The deprecated
`eventBridgeWebhookSubscriptionCreate`/`Update` (an `arn`) and `pubSubWebhookSubscriptionCreate`/`Update`
(a `pubSubProject` and a `pubSubTopic`) make the same subscriptions as `webhookSubscriptionCreate`.
lsf delivers nothing to them: their deliveries are in `GET /__lsf/webhooks`, `not_sent`, with
their payload.

**Queries.** `app`, `appByHandle`, `appByKey`, `appInstallation(s)`, `currentAppInstallation`,
`staffMember(s)` (the shop owner is `StaffMember/1`) and `currentStaffMember` (the user of an
online token, `null` for an offline one) answer every field; what an App Store listing would give
(an icon, a category) is lsf's, an app in development having none.

**Webhooks** without a mutation behind them, sent by `POST /__lsf/webhooks/trigger` and
`lsf app webhook trigger`: `rollouts/*` (from a `Rollout`, else Shopify's example),
`finance_app_staff_member/*` (from the staff member) and `finance_kyc_information/update`, which
Shopify documents with an empty payload: lsf sends the record in REST's shape.

## The shop's clock

A shop's time is its clock: the system's, or the store data's `now` (then the same run gives the
same answers). What waits on time happens when the clock passes it, before every Admin API
request and when a test moves the clock, with its webhooks:

- a payment schedule that came due fires `payment_schedules/due`, once;
- store credit that expired leaves its account;
- Shopify Payments pays out the days the shop's day left behind;
- a charge of the app left pending two days is `EXPIRED`;
- a scheduled channel listing whose date passed is listed (`product_listings/add`);
- a data erasure ten days old is processed (`customers/redact`);
- an abandoned checkout three months old is deleted (`checkouts/delete`).

What is worked out when it is read follows the clock without waiting: a checkout left ten
minutes is abandoned, a discount runs between its dates (the checkout too), `PaymentTerms.due`
and `overdue`, a gift card's expiry. A webhook `filter` counts `now` by the shop's clock.

`POST /__lsf/admin/clock` moves it: `{"advance": "31d"}` (seconds, or `s`, `m`, `h`, `d`) or
`{"now": "2026-12-01T00:00:00Z"}`, with `"shop"?`. The clock then stays at that instant, frozen,
and what the move passed happens at once; the answer says which customers were erased and how
many checkouts were deleted (`erased`, `deletedCheckouts`). `GET /__lsf/admin/clock` reads it;
`DELETE /__lsf/admin` puts the store data's back.

## The outbox

lsf sends no email, buys no label and pushes nothing to a device: what Shopify would have sent,
or bought, is recorded in the shop's outbox instead, for a test to read. (Webhooks go to the
backend lsf is pointed at: [where they go](admin.md#webhooks).)

| Channel | What lands there (`what`) |
|---|---|
| `email` | The emails to a buyer or a customer: `payment_reminder`, `order_invoice`, `draft_order_invoice`, `gift_card_created`, `gift_card_recipient`, `store_credit_issued`, `return_label`, `requested_order_edit_declined`, `order_link_requested`, `ready_for_pickup`, `out_for_delivery`, `shipping_confirmation`, `customer_payment_method_update`, `payment_instrument_add` |
| `carrier` | A shipping label bought (`shipping_label`): the carrier as `to`, the service, the tracking number and the package in `data` |
| `flow` | A Flow trigger (`flow_trigger`): its handle as `to`, its payload as `data` |
| `pos` | What Shopify would push to a POS device: `cash_drawer_assigned`, `payment_session_opened`, `payment_session_closed` |
| `data_erasure` | A customer's data erasure: `data_erasure_requested`, `data_erasure_cancelled`, `customer_data_erased` |

```json
{
  "id": 1,
  "at": "2026-11-10T09:00:00Z",
  "channel": "email",
  "to": "buyer@example.com",
  "what": "payment_reminder",
  "about": "gid://shopify/PaymentSchedule/1",
  "subject": "Payment reminder for order #1001",
  "body": "120.0 USD is due on 2026-12-10 for order #1001."
}
```

`id` counts per shop, `at` is the shop's clock, `to` an address, a carrier, a trigger or a device,
`about` the record it is about. An email has its `subject` and `body` (lsf's words: Shopify does
not publish the text of its notifications); another channel says what happened in `body`, and
`data` holds what else would have gone out.

- `GET /__lsf/admin/outbox?shop=&channel=email` lists them, oldest first, the last 1000 (`{shop,
  count, outbox}`); `DELETE /__lsf/admin/outbox` forgets them (`?channel=` those of one channel).
- `GET /__lsf/admin/emails` and `DELETE /__lsf/admin/emails` are the same for `channel=email`
  (`{shop, count, emails}`).
- `GET /__lsf/admin` shows each shop's outbox (`shops[].outbox`).
- A mutation that fails records nothing; `DELETE /__lsf/admin` empties every outbox.
