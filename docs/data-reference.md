# Store data reference

Every type of the store data format. This file is generated from the JSON Schema the
validator uses (`lsf docs --reference`); the guide is in [data-format.md](data-format.md).

## Data file

A data file: any combination of the sections below. Every `*.json` file at the root of the
data directory is one of these, and they are merged, so you can split the data as you like
(`shop.json`, `catalog.json`, ...).

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"./schema/store.schema.json"`. |
| `shop` | [Shop](#shop) | General store information. At most one data file may define it. |
| `products` | array of [Product](#product) | Products. They can also live one per file in `products/`. |
| `collections` | array of [Collection](#collection) | Collections. They can also live one per file in `collections/`. |
| `pages` | array of [Page](#page) | Pages. They can also live one per file in `pages/`. |
| `blogs` | array of [Blog](#blog) | Blogs with their articles. They can also live one per file in `blogs/`. |
| `menus` | map of [Menu](#menu) | Navigation menus by handle, e.g. `main-menu` and `footer`. |
| `customers` | array of [Customer](#customer) | Customers. They can also live one per file in `customers/`. |
| `gift_cards` | array of [GiftCard](#giftcard) | Issued gift cards, each with its own page. |
| `metaobjects` | map of array of [Metaobject](#metaobject) | Metaobject entries by type. |
| `localization` | [Localization](#localization) | The countries and languages of the store. At most one data file may define it. |
| `files` | map of [File](#file) | Metadata for files in `files/`, by path. |
| `session` | [Session](#session) | What every new browser session starts with: a logged-in customer, a filled cart... |
| `now` | string | Freezes the clock: the instant `'now'` resolves to, e.g. `"2025-01-15T10:00:00Z"`. Set it to make renders reproducible. Defaults to the real time. |
| `theme_settings` | map of any JSON value | Overrides for theme settings (`config/settings_data.json`), by setting id. |

## Shop

General store information (the `shop` object). Every field has a sensible default.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `name` | string | Defaults to `"Local Store"`. |
| `description` | string |  |
| `email` | string |  |
| `phone` | string |  |
| `domain` | string | The primary domain, without scheme. Defaults to the host the server is reached on. |
| `permanent_domain` | string | The `*.myshopify.com` domain. Defaults to a value derived from the name. |
| `currency` | string | ISO 4217 code of the store currency. Defaults to `USD`. |
| `money_format` | string | How `money` formats amounts, e.g. `"${{amount}}"` or `"{{amount_with_comma_separator}} €"`. Defaults to the usual format of the currency. |
| `money_with_currency_format` | string | How `money_with_currency` formats amounts, e.g. `"${{amount}} USD"`. |
| `timezone` | string | IANA time zone used to display dates, e.g. `"Europe/Paris"`. Defaults to `UTC`. |
| `address` | [Address](#address) |  |
| `policies` | [Policies](#policies) |  |
| `enabled_payment_types` | array of string | Payment icons to show, e.g. `["visa", "master", "american_express", "paypal"]`. |
| `customer_accounts_enabled` | boolean | Whether customer accounts exist at all. Defaults to `true`. |
| `customer_accounts_optional` | boolean | Whether checking out as a guest is possible. Defaults to `true`. |
| `taxes_included` | boolean | Whether prices include taxes. Defaults to `false`. |
| `password_message` | string | The message shown on the password page. |
| `password` | string | When set, the storefront is locked and every page shows the password page until this password is entered. |
| `brand` | [Brand](#brand) |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

## Address

A postal address. Every field is optional.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `first_name` | string |  |
| `last_name` | string |  |
| `company` | string |  |
| `address1` | string |  |
| `address2` | string |  |
| `city` | string |  |
| `province` | string | Province or state name, e.g. `"Ontario"`. |
| `province_code` | string | E.g. `"ON"`. |
| `country` | string | Country name, e.g. `"Canada"`. |
| `country_code` | string | ISO 3166-1 alpha-2 code, e.g. `"CA"`. |
| `zip` | string |  |
| `phone` | string |  |

## Policies

The store policies. Each one that is set gets a page under `/policies/`.

| Field | Type | Description |
|---|---|---|
| `privacy_policy` | [Policy](#policy) |  |
| `refund_policy` | [Policy](#policy) |  |
| `shipping_policy` | [Policy](#policy) |  |
| `terms_of_service` | [Policy](#policy) |  |
| `subscription_policy` | [Policy](#policy) |  |

## Policy

A store policy (`shop.refund_policy`, `/policies/refund-policy`).

| Field | Type | Description |
|---|---|---|
| `title` | string | Defaults to the standard title of the policy. |
| `body` | string | **Required.** HTML body. |

## Brand

The brand assets of the store (`shop.brand`).

| Field | Type | Description |
|---|---|---|
| `slogan` | string |  |
| `short_description` | string |  |
| `logo` | [Image](#image) |  |
| `square_logo` | [Image](#image) |  |
| `cover_image` | [Image](#image) |  |
| `favicon` | [Image](#image) |  |
| `colors` | any JSON value | Brand colors as CSS hex strings, e.g. `{"primary": ["#112233"], "secondary": []}`. |

## Image

An image: either just its source, or an object with more detail.

The source is a path inside the `files/` directory of the data directory
(`"shirt.jpg"` → `files/shirt.jpg`). When no such file exists a placeholder of the declared
size is generated, so fixtures work without shipping real images.

One of:

- string
- [ImageDetail](#imagedetail)

## ImageDetail

An image with its metadata.

| Field | Type | Description |
|---|---|---|
| `src` | string | **Required.** Path of the image inside `files/`, e.g. `"products/shirt-front.jpg"`. |
| `alt` | string | Alternative text. Defaults to an empty string. |
| `width` | integer | Width in pixels. Read from the file when it exists, otherwise defaults to 1200. |
| `height` | integer | Height in pixels. Read from the file when it exists, otherwise defaults to 1200. |
| `id` | integer |  |
| `focal_point` | [FocalPoint](#focalpoint) | The focal point as percentages from the top-left corner, e.g. `{"x": 50, "y": 20}`. |

## FocalPoint

The point of an image that should stay visible when it is cropped.

| Field | Type | Description |
|---|---|---|
| `x` | number | **Required.** Horizontal position, 0 (left) to 100 (right). |
| `y` | number | **Required.** Vertical position, 0 (top) to 100 (bottom). |

## Metafield

A metafield value. Write the value directly and its type is inferred (string →
`single_line_text_field`, integer → `number_integer`, decimal → `number_decimal`, boolean →
`boolean`, anything else → `json`), or spell out `{"type": ..., "value": ...}`.

One of:

- [TypedMetafield](#typedmetafield)
- any JSON value

## TypedMetafield

A metafield with an explicit type.

| Field | Type | Description |
|---|---|---|
| `type` | string | **Required.** A Shopify metafield type, e.g. `single_line_text_field`, `multi_line_text_field`, `rich_text_field`, `number_integer`, `number_decimal`, `boolean`, `color`, `date`, `date_time`, `url`, `json`, `money`, `rating`, `weight`, `volume`, `dimension`, `product_reference`, `collection_reference`, `page_reference`, `file_reference`, `metaobject_reference`, or a `list.` variant of those. |
| `value` | any JSON value | **Required.** The value. References are written as handles (`"blue-shirt"`), files as paths inside `files/`, lists as JSON arrays. |

## Product

A product.

The smallest valid product is `{"title": "Blue shirt", "price": 1999}`: it gets a single
default variant, like a product without options in the Shopify admin.

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/product.schema.json"`. |
| `id` | integer | Generated from the handle when omitted. |
| `title` | string | **Required.** |
| `handle` | string | The URL slug (`/products/<handle>`). Derived from the title when omitted. In a `products/<name>.json` file it defaults to the file name. |
| `description` | string | HTML description. Also exposed as `product.content`. |
| `vendor` | string |  |
| `type` | string | The product type, e.g. `"Shirts"`. |
| `tags` | array of string |  |
| `options` | array of [Option](#option) | Option names, e.g. `["Size", "Color"]`. At most three. Omit for a product with a single default variant. |
| `variants` | array of [Variant](#variant) | The variants. Each must give one value per option. |
| `price` | [Money](#money) | Price of the variants that do not set their own, and of the default variant. |
| `compare_at_price` | [Money](#money) | Compare-at price of the variants that do not set their own. |
| `images` | array of [Image](#image) | Shorthand for image-only media. Use `media` for videos and 3D models. |
| `media` | array of [Media](#media) | Images, videos and models in display order. When set, `images` must be empty. |
| `available` | boolean | Shorthand to set availability on every variant that does not say otherwise. |
| `collections` | array of string | Handles of the collections this product belongs to, in addition to the collections that list it in their own `products`. |
| `template_suffix` | string | Selects `templates/product.<suffix>.json` instead of `templates/product.json`. |
| `gift_card` | boolean | Whether the product is a gift card (`product.gift_card?`). |
| `created_at` | string |  |
| `published_at` | string |  |
| `updated_at` | string |  |
| `metafields` | map of map of [Metafield](#metafield) |  |
| `recommendations` | array of string | Handles of the products returned by product recommendations for this product. Defaults to other products from the same collections. |

## Option

A product option. Write just the name (`"Size"`); the values are collected from the variants.

One of:

- string
- [OptionDetail](#optiondetail)

## OptionDetail

A product option with its values in a chosen order.

| Field | Type | Description |
|---|---|---|
| `name` | string | **Required.** |
| `values` | array of string | The values in display order. Defaults to the order they appear in the variants. |

## Variant

A product variant.

| Field | Type | Description |
|---|---|---|
| `id` | integer | Generated from the product handle and the variant's position when omitted. |
| `title` | string | Defaults to the option values joined with ` / ` (`"Small / Blue"`). |
| `options` | array of string | One value per product option, in the order of the product's `options`. |
| `price` | [Money](#money) | Defaults to the product's `price`. |
| `compare_at_price` | [Money](#money) | The price before a sale. Must be higher than `price` to show as a discount. |
| `sku` | string | Also a convenient way to reference the variant from carts and orders. |
| `barcode` | string |  |
| `available` | boolean | Whether the variant can be bought. Derived from the inventory fields when omitted. |
| `inventory_quantity` | integer | Units in stock. Setting it turns inventory tracking on. |
| `inventory_tracked` | boolean | Set to `false` to disable inventory tracking (the variant is then always available). Defaults to `true` when `inventory_quantity` is set, `false` otherwise. |
| `inventory_policy` | [InventoryPolicy](#inventorypolicy) | Defaults to `"deny"`. |
| `weight` | integer | Weight in grams. |
| `weight_unit` | string | The unit the weight is displayed in: `g`, `kg`, `oz` or `lb`. Defaults to `kg`. |
| `requires_shipping` | boolean | Defaults to `true`. |
| `taxable` | boolean | Defaults to `true`. |
| `image` | string | The `src` of the product image that shows this variant. |
| `unit_price` | [Money](#money) |  |
| `unit_price_measurement` | [UnitPriceMeasurement](#unitpricemeasurement) |  |
| `quantity_rule` | [QuantityRule](#quantityrule) |  |
| `quantity_price_breaks` | array of [QuantityPriceBreak](#quantitypricebreak) |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

## Money

An amount of money: an integer number of cents (`1999`, the unit Liquid uses) or a decimal string in the shop currency (`"19.99"`). Floats such as `19.99` are rejected because they are ambiguous.

One of:

- integer: Cents, e.g. 1999 for 19.99.
- string: Decimal amount, e.g. "19.99" or "20".

## InventoryPolicy

What happens when a tracked variant is out of stock.

One of:

- `"deny"`: Stop selling when the stock reaches zero.
- `"continue"`: Keep selling when out of stock.

## UnitPriceMeasurement

What a unit price refers to, e.g. 250 g priced per 100 g.

| Field | Type | Description |
|---|---|---|
| `measured_type` | string | `weight`, `volume`, `length` or `area`. |
| `quantity_value` | number | **Required.** |
| `quantity_unit` | string | **Required.** E.g. `g`, `kg`, `ml`, `l`, `m`. |
| `reference_value` | number |  |
| `reference_unit` | string |  |

## QuantityRule

The quantities a variant can be bought in (`variant.quantity_rule`).

| Field | Type | Description |
|---|---|---|
| `min` | integer | Minimum quantity. Defaults to 1. |
| `max` | integer | Maximum quantity. No maximum when omitted. |
| `increment` | integer | Quantities must be a multiple of this. Defaults to 1. |

## QuantityPriceBreak

A volume price: the unit price from a given quantity.

| Field | Type | Description |
|---|---|---|
| `minimum_quantity` | integer | **Required.** |
| `price` | [Money](#money) | **Required.** |

## Media

A piece of product media. Images can be written as a plain string.

One of:

- [Image](#image)
- [MediaDetail](#mediadetail)

## MediaDetail

A video, an external (YouTube/Vimeo) video or a 3D model.

| Field | Type | Description |
|---|---|---|
| `media_type` | [MediaType](#mediatype) | **Required.** |
| `id` | integer |  |
| `alt` | string |  |
| `preview_image` | [Image](#image) | The image shown before the media plays. |
| `sources` | array of [MediaSource](#mediasource) | `video` and `model`: the files, by decreasing preference. |
| `duration` | integer | `video`: length in milliseconds. |
| `host` | [VideoHost](#videohost) | `external_video`: `youtube` or `vimeo`. |
| `external_id` | string | `external_video`: the id of the video on the host, e.g. `dQw4w9WgXcQ`. |
| `aspect_ratio` | number | Width divided by height. Defaults to the preview image's ratio, or 16:9. |

## MediaType

The kind of a piece of media that is not an image.

One of: `"video"`, `"external_video"`, `"model"`.

## MediaSource

One file of a video or 3D model.

| Field | Type | Description |
|---|---|---|
| `url` | string | **Required.** Path inside `files/` or an absolute URL. |
| `mime_type` | string | E.g. `video/mp4`, `model/gltf-binary`. |
| `format` | string | E.g. `mp4`, `m3u8`, `glb`, `usdz`. |
| `width` | integer |  |
| `height` | integer |  |

## VideoHost

Where an external video is hosted.

One of: `"youtube"`, `"vimeo"`.

## Collection

A collection of products.

A collection named `all` containing every product always exists; define it yourself only
to change its title or order.

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/collection.schema.json"`. |
| `id` | integer |  |
| `title` | string | **Required.** |
| `handle` | string |  |
| `description` | string | HTML description. |
| `image` | [Image](#image) |  |
| `products` | array of string | Handles of the products in the collection, in manual order. |
| `rules` | array of [CollectionRule](#collectionrule) | Makes the collection automated: products matching the rules are added after the ones listed in `products`. |
| `disjunctive` | boolean | With several rules: `true` to match any of them, `false` (default) to match all. |
| `sort_order` | [SortOrder](#sortorder) | The default order of the products. Defaults to `manual`. |
| `template_suffix` | string | Selects `templates/collection.<suffix>.json`. |
| `published_at` | string |  |
| `updated_at` | string |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

## CollectionRule

A rule of an automated collection: products matching it are included.

| Field | Type | Description |
|---|---|---|
| `column` | [RuleColumn](#rulecolumn) | **Required.** What to look at: `tag`, `type`, `vendor`, `title`, `price` (cents) or `compare_at_price` (cents). |
| `relation` | [RuleRelation](#rulerelation) | Defaults to `equals`. |
| `condition` | string | **Required.** The value compared with, e.g. `"sale"` for a tag or `"5000"` for a price in cents. |

## RuleColumn

The product property an automated collection rule looks at.

One of: `"tag"`, `"type"`, `"vendor"`, `"title"`, `"price"`, `"compare_at_price"`.

## RuleRelation

How an automated collection rule compares the property to its condition.

One of: `"equals"`, `"not_equals"`, `"contains"`, `"not_contains"`, `"starts_with"`, `"ends_with"`, `"greater_than"`, `"less_than"`.

## SortOrder

How a collection orders its products by default.

One of: `"manual"`, `"best-selling"`, `"title-ascending"`, `"title-descending"`, `"price-ascending"`, `"price-descending"`, `"created-ascending"`, `"created-descending"`.

## Page

A page (`/pages/<handle>`).

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/page.schema.json"`. |
| `id` | integer |  |
| `title` | string | **Required.** |
| `handle` | string |  |
| `content` | string | HTML content. |
| `author` | string |  |
| `template_suffix` | string | Selects `templates/page.<suffix>.json`, e.g. `"contact"`. |
| `published_at` | string |  |
| `updated_at` | string |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

## Blog

A blog (`/blogs/<handle>`) and its articles.

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/blog.schema.json"`. |
| `id` | integer |  |
| `title` | string | **Required.** |
| `handle` | string |  |
| `articles` | array of [Article](#article) | Articles, newest first unless they carry `published_at` dates. |
| `comments_enabled` | boolean | Whether readers can comment. Defaults to `false`. |
| `moderated` | boolean | Whether comments wait for approval. Defaults to `false`. |
| `template_suffix` | string |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

## Article

A blog post.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `title` | string | **Required.** |
| `handle` | string |  |
| `author` | string |  |
| `content` | string | HTML content. |
| `excerpt` | string | HTML excerpt. `article.excerpt_or_content` falls back to the content. |
| `image` | [Image](#image) |  |
| `tags` | array of string |  |
| `comments` | array of [Comment](#comment) |  |
| `template_suffix` | string |  |
| `created_at` | string |  |
| `published_at` | string |  |
| `updated_at` | string |  |
| `metafields` | map of map of [Metafield](#metafield) |  |

## Comment

A comment on an article.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `author` | string | **Required.** |
| `email` | string |  |
| `content` | string | **Required.** HTML content. |
| `created_at` | string |  |

## Menu

A navigation menu (`linklists.<handle>`).

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/menu.schema.json"`. |
| `title` | string | Defaults to the handle, title-cased. |
| `links` | array of [Link](#link) |  |

## Link

A link in a navigation menu.

Point it at a resource with exactly one of `url`, `collection`, `product`, `page`, `blog`,
`article` or `policy`; the URL and the link type follow from it. A link with none of them
points to `#`.

| Field | Type | Description |
|---|---|---|
| `title` | string | **Required.** |
| `url` | string | A literal URL: `/`, `/collections/all`, `/search`, `https://example.com`. |
| `collection` | string | Handle of a collection. |
| `product` | string | Handle of a product. |
| `page` | string | Handle of a page. |
| `blog` | string | Handle of a blog. |
| `article` | string | An article as `<blog-handle>/<article-handle>`. |
| `policy` | string | A policy: `privacy-policy`, `refund-policy`, `shipping-policy`, `terms-of-service`. |
| `links` | array of [Link](#link) | Nested links (up to three levels, like the Shopify admin). |

## Customer

A customer. Sessions log in as a customer by email (see `session.customer`).

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/customer.schema.json"`. |
| `id` | integer |  |
| `email` | string | **Required.** |
| `first_name` | string |  |
| `last_name` | string |  |
| `phone` | string |  |
| `tags` | array of string |  |
| `accepts_marketing` | boolean |  |
| `has_account` | boolean | Defaults to `true`. |
| `tax_exempt` | boolean |  |
| `addresses` | array of [Address](#address) | The first address is the default one (`customer.default_address`). |
| `orders` | array of [Order](#order) |  |
| `password` | string | The password accepted by the local login form. Any password works when omitted. |
| `metafields` | map of map of [Metafield](#metafield) |  |

## Order

An order of a customer, shown on account pages.

| Field | Type | Description |
|---|---|---|
| `id` | integer |  |
| `name` | string | The order name, e.g. `"#1001"`. Generated when omitted. |
| `created_at` | string |  |
| `financial_status` | string | `paid`, `pending`, `refunded`, ... Defaults to `paid`. |
| `fulfillment_status` | string | `fulfilled`, `unfulfilled`, `partial`. Defaults to `unfulfilled`. |
| `line_items` | array of [OrderLine](#orderline) |  |
| `shipping_address` | [Address](#address) |  |
| `billing_address` | [Address](#address) |  |
| `shipping_price` | [Money](#money) |  |
| `tax_price` | [Money](#money) |  |
| `cancelled` | boolean |  |
| `note` | string |  |

## OrderLine

A line of an order.

| Field | Type | Description |
|---|---|---|
| `variant` | [VariantRef](#variantref) | **Required.** A variant, by id or SKU, or a product handle (its first variant is used). |
| `quantity` | integer | Defaults to 1. |

## VariantRef

A reference to a variant: its numeric id, its SKU, or a product handle (meaning that
product's first variant).

One of:

- integer
- string

## GiftCard

An issued gift card, shown by `templates/gift_card.liquid` at the URL `lsf routes` lists
(`/gift_cards/<shop id>/<token>`).

| Field | Type | Description |
|---|---|---|
| `code` | string | **Required.** The code customers redeem at checkout, e.g. `"WCGX7X97G74JGDGC"`. |
| `initial_value` | [Money](#money) | **Required.** The value the card was issued with. |
| `balance` | [Money](#money) | What is left on the card. Defaults to `initial_value`. |
| `currency` | string | ISO 4217 code. Defaults to the shop's currency. |
| `expires_on` | string | The day the card stops working, e.g. `"2026-12-31"`. A date before `now` makes the card `expired`. |
| `enabled` | boolean | `false` for a card the merchant disabled. |
| `customer` | string | The email of the customer (from `customers`) the card belongs to. |
| `message` | string |  |
| `recipient` | [GiftCardRecipient](#giftcardrecipient) |  |
| `send_on` | string | When the card is scheduled to be sent to its recipient. |
| `product` | string | The handle of the gift card product that was bought. |
| `properties` | map of string | Line item properties of the purchase. |
| `template_suffix` | string | Selects `templates/gift_card.<suffix>.liquid`. |

## GiftCardRecipient

Who a gift card was sent to.

| Field | Type | Description |
|---|---|---|
| `name` | string |  |
| `email` | string |  |
| `nickname` | string |  |

## Metaobject

A metaobject entry (`metaobjects.<type>.<handle>`).

| Field | Type | Description |
|---|---|---|
| `handle` | string | **Required.** |
| `fields` | map of [Metafield](#metafield) | Field values, with the same shorthand as metafields. |

## Localization

The countries and languages the store sells in (the `localization` object).

The first language is the primary one and is served at `/`; the others are served under
`/<iso_code>/`. Defaults to one country (`US`) and the theme's default language.

| Field | Type | Description |
|---|---|---|
| `countries` | array of [Country](#country) |  |
| `languages` | array of [Language](#language) |  |

## Country

A country the store sells to.

| Field | Type | Description |
|---|---|---|
| `iso_code` | string | **Required.** ISO 3166-1 alpha-2 code, e.g. `"FR"`. |
| `name` | string | Defaults to the English name of the country. |
| `currency` | string | ISO 4217 currency code. Defaults to the store currency. |
| `unit_system` | string | `metric` or `imperial`. Defaults to `metric`. |
| `popular` | boolean | Whether to list the country among the popular ones. |

## Language

A language the store is published in.

| Field | Type | Description |
|---|---|---|
| `iso_code` | string | **Required.** Locale code, e.g. `"en"`, `"fr"`, `"pt-BR"`. |
| `name` | string | The language's name in English. Defaults to a built-in name. |
| `endonym_name` | string | The language's name in that language. Defaults to a built-in name. |

## File

A file in `files/` that needs metadata. Files without an entry work too.

| Field | Type | Description |
|---|---|---|
| `alt` | string |  |
| `width` | integer | Size to assume when the file itself is absent (a placeholder is generated). |
| `height` | integer |  |
| `focal_point` | [FocalPoint](#focalpoint) |  |

## Session

The state a browser session starts with. Tests can replace it per session through the
control API (`PUT /__lsf/session`).

| Field | Type | Description |
|---|---|---|
| `$schema` | string | Lets editors validate and autocomplete the file, e.g. `"../schema/session.schema.json"`. |
| `customer` | string | Email of the logged-in customer. Nobody is logged in when omitted. |
| `cart` | [Cart](#cart) | The content of the cart. Empty when omitted. |
| `country` | string | ISO code of the selected country. Defaults to the first country. |

## Cart

The cart a session starts with.

| Field | Type | Description |
|---|---|---|
| `items` | array of [CartLine](#cartline) |  |
| `note` | string |  |
| `attributes` | map of string | Cart attributes. |

## CartLine

A line of the cart a session starts with.

| Field | Type | Description |
|---|---|---|
| `variant` | [VariantRef](#variantref) | **Required.** A variant, by id or SKU, or a product handle (its first variant is used). |
| `quantity` | integer | Defaults to 1. |
| `properties` | map of string | Line item properties, e.g. `{"Engraving": "Hello"}`. |
