//! The store data format: what you write in the data directory.
//!
//! These types are the single source of truth for the format. The JSON Schema used to validate
//! the data (`lsf schema`) and the reference documentation (`lsf docs`) are generated from them,
//! so every doc comment here is user-facing documentation.
//!
//! Conventions shared by all types:
//! - Unknown fields are rejected, to catch typos.
//! - `id` and `handle` are optional everywhere: a handle is derived from the title, an id from
//!   the handle. Both are stable, so URLs and ids do not change between runs.
//! - Money is either an integer number of cents (`1999`, the unit Liquid uses) or a decimal
//!   string in the shop currency (`"19.99"`).
//! - Dates are ISO 8601 strings (`2024-05-01` or `2024-05-01T10:00:00Z`).

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// An amount of money: an integer number of cents (`1999`) or a decimal string in the shop
/// currency (`"19.99"`). Floats such as `19.99` are rejected because they are ambiguous.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum Money {
    /// Cents, e.g. `1999` for 19.99.
    Cents(i64),
    /// Decimal amount, e.g. `"19.99"` or `"20"`.
    Decimal(String),
}

impl JsonSchema for Money {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Money".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "An amount of money: an integer number of cents (`1999`, the unit Liquid uses) or a decimal string in the shop currency (`\"19.99\"`). Floats such as `19.99` are rejected because they are ambiguous.",
            "anyOf": [
                { "type": "integer", "description": "Cents, e.g. 1999 for 19.99." },
                { "type": "string", "pattern": "^-?\\d+(\\.\\d{1,2})?$", "description": "Decimal amount, e.g. \"19.99\" or \"20\"." }
            ]
        })
    }
}

impl Money {
    pub fn cents(&self) -> Option<i64> {
        match self {
            Money::Cents(cents) => Some(*cents),
            Money::Decimal(text) => {
                let (negative, digits) = match text.strip_prefix('-') {
                    Some(rest) => (true, rest),
                    None => (false, text.as_str()),
                };
                let (units, fraction) = digits.split_once('.').unwrap_or((digits, ""));
                if units.is_empty()
                    || fraction.len() > 2
                    || !units
                        .bytes()
                        .chain(fraction.bytes())
                        .all(|b| b.is_ascii_digit())
                {
                    return None;
                }
                let cents = units.parse::<i64>().ok()?.checked_mul(100)?
                    + format!("{fraction:0<2}").parse::<i64>().ok()?;
                Some(if negative { -cents } else { cents })
            }
        }
    }
}

/// An image: either just its source, or an object with more detail.
///
/// The source is a path inside the `files/` directory of the data directory
/// (`"shirt.jpg"` → `files/shirt.jpg`). When no such file exists a placeholder of the declared
/// size is generated, so fixtures work without shipping real images.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ImageInput {
    Src(String),
    Detailed(ImageDetail),
}

/// An image with its metadata.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageDetail {
    /// Path of the image inside `files/`, e.g. `"products/shirt-front.jpg"`.
    pub src: String,
    /// Alternative text. Defaults to an empty string.
    #[serde(default)]
    pub alt: Option<String>,
    /// Width in pixels. Read from the file when it exists, otherwise defaults to 1200.
    #[serde(default)]
    pub width: Option<u32>,
    /// Height in pixels. Read from the file when it exists, otherwise defaults to 1200.
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub id: Option<u64>,
    /// The focal point as percentages from the top-left corner, e.g. `{"x": 50, "y": 20}`.
    #[serde(default)]
    pub focal_point: Option<FocalPoint>,
}

/// The point of an image that should stay visible when it is cropped.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FocalPoint {
    /// Horizontal position, 0 (left) to 100 (right).
    pub x: f64,
    /// Vertical position, 0 (top) to 100 (bottom).
    pub y: f64,
}

impl ImageInput {
    pub fn detail(&self) -> ImageDetail {
        match self {
            ImageInput::Src(src) => ImageDetail {
                src: src.clone(),
                ..ImageDetail::default()
            },
            ImageInput::Detailed(detail) => detail.clone(),
        }
    }
}

/// A piece of product media. Images can be written as a plain string.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum MediaInput {
    Image(ImageInput),
    Other(MediaDetail),
}

/// A video, an external (YouTube/Vimeo) video or a 3D model.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaDetail {
    pub media_type: MediaType,
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub alt: Option<String>,
    /// The image shown before the media plays.
    #[serde(default)]
    pub preview_image: Option<ImageInput>,
    /// `video` and `model`: the files, by decreasing preference.
    #[serde(default)]
    pub sources: Vec<MediaSource>,
    /// `video`: length in milliseconds.
    #[serde(default)]
    pub duration: Option<u64>,
    /// `external_video`: `youtube` or `vimeo`.
    #[serde(default)]
    pub host: Option<VideoHost>,
    /// `external_video`: the id of the video on the host, e.g. `dQw4w9WgXcQ`.
    #[serde(default)]
    pub external_id: Option<String>,
    /// Width divided by height. Defaults to the preview image's ratio, or 16:9.
    #[serde(default)]
    pub aspect_ratio: Option<f64>,
}

/// The customer accounts of a store.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CustomerAccounts {
    /// Accounts hosted by Shopify, outside the theme.
    New,
    /// Accounts rendered by the theme's `templates/customers`.
    Legacy,
}

/// The kind of a piece of media that is not an image.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Video,
    ExternalVideo,
    Model,
}

/// Where an external video is hosted.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VideoHost {
    Youtube,
    Vimeo,
}

/// One file of a video or 3D model.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaSource {
    /// Path inside `files/` or an absolute URL.
    pub url: String,
    /// E.g. `video/mp4`, `model/gltf-binary`.
    #[serde(default)]
    pub mime_type: Option<String>,
    /// E.g. `mp4`, `m3u8`, `glb`, `usdz`.
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

/// A metafield value. Write the value directly and its type is inferred (string →
/// `single_line_text_field`, integer → `number_integer`, decimal → `number_decimal`, boolean →
/// `boolean`, anything else → `json`), or spell out `{"type": ..., "value": ...}`.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum MetafieldInput {
    Typed(TypedMetafield),
    Value(Json),
}

/// A metafield with an explicit type.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypedMetafield {
    /// A Shopify metafield type, e.g. `single_line_text_field`, `multi_line_text_field`,
    /// `rich_text_field`, `number_integer`, `number_decimal`, `boolean`, `color`, `date`,
    /// `date_time`, `url`, `json`, `money`, `rating`, `weight`, `volume`, `dimension`,
    /// `product_reference`, `collection_reference`, `page_reference`, `file_reference`,
    /// `metaobject_reference`, or a `list.` variant of those.
    #[serde(rename = "type")]
    pub kind: String,
    /// The value. References are written as handles (`"blue-shirt"`), files as paths inside
    /// `files/`, lists as JSON arrays.
    pub value: Json,
}

/// Metafields grouped by namespace then key: `{"custom": {"care": "Machine wash"}}`.
pub type Metafields = IndexMap<String, IndexMap<String, MetafieldInput>>;

/// A product option. Write just the name (`"Size"`); the values are collected from the variants.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum OptionInput {
    Name(String),
    Detailed(OptionDetail),
}

/// A product option with its values in a chosen order.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptionDetail {
    pub name: String,
    /// The values in display order. Defaults to the order they appear in the variants.
    #[serde(default)]
    pub values: Vec<String>,
}

impl OptionInput {
    pub fn name(&self) -> &str {
        match self {
            OptionInput::Name(name) => name,
            OptionInput::Detailed(detail) => &detail.name,
        }
    }
}

/// The quantities a variant can be bought in (`variant.quantity_rule`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuantityRule {
    /// Minimum quantity. Defaults to 1.
    #[serde(default)]
    pub min: Option<u32>,
    /// Maximum quantity. No maximum when omitted.
    #[serde(default)]
    pub max: Option<u32>,
    /// Quantities must be a multiple of this. Defaults to 1.
    #[serde(default)]
    pub increment: Option<u32>,
}

/// A volume price: the unit price from a given quantity.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuantityPriceBreak {
    pub minimum_quantity: u32,
    pub price: Money,
}

/// What a unit price refers to, e.g. 250 g priced per 100 g.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnitPriceMeasurement {
    /// `weight`, `volume`, `length` or `area`.
    #[serde(default)]
    pub measured_type: Option<String>,
    pub quantity_value: f64,
    /// E.g. `g`, `kg`, `ml`, `l`, `m`.
    pub quantity_unit: String,
    #[serde(default)]
    pub reference_value: Option<f64>,
    #[serde(default)]
    pub reference_unit: Option<String>,
}

/// What happens when a tracked variant is out of stock.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InventoryPolicy {
    /// Stop selling when the stock reaches zero.
    #[default]
    Deny,
    /// Keep selling when out of stock.
    Continue,
}

/// A product variant.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VariantInput {
    /// Generated from the product handle and the variant's position when omitted.
    #[serde(default)]
    pub id: Option<u64>,
    /// Defaults to the option values joined with ` / ` (`"Small / Blue"`).
    #[serde(default)]
    pub title: Option<String>,
    /// One value per product option, in the order of the product's `options`.
    #[serde(default)]
    pub options: Vec<String>,
    /// Defaults to the product's `price`.
    #[serde(default)]
    pub price: Option<Money>,
    /// The price before a sale. Must be higher than `price` to show as a discount.
    #[serde(default)]
    pub compare_at_price: Option<Money>,
    /// Also a convenient way to reference the variant from carts and orders.
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub barcode: Option<String>,
    /// Whether the variant can be bought. Derived from the inventory fields when omitted.
    #[serde(default)]
    pub available: Option<bool>,
    /// Units in stock. Setting it turns inventory tracking on.
    #[serde(default)]
    pub inventory_quantity: Option<i64>,
    /// Set to `false` to disable inventory tracking (the variant is then always available).
    /// Defaults to `true` when `inventory_quantity` is set, `false` otherwise.
    #[serde(default)]
    pub inventory_tracked: Option<bool>,
    #[serde(default)]
    pub inventory_policy: InventoryPolicy,
    /// Weight in grams.
    #[serde(default)]
    pub weight: Option<u64>,
    /// The unit the weight is displayed in: `g`, `kg`, `oz` or `lb`. Defaults to `kg`.
    #[serde(default)]
    pub weight_unit: Option<String>,
    /// Defaults to `true`.
    #[serde(default)]
    pub requires_shipping: Option<bool>,
    /// Defaults to `true`.
    #[serde(default)]
    pub taxable: Option<bool>,
    /// The `src` of the product image that shows this variant.
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub unit_price: Option<Money>,
    #[serde(default)]
    pub unit_price_measurement: Option<UnitPriceMeasurement>,
    #[serde(default)]
    pub quantity_rule: Option<QuantityRule>,
    #[serde(default)]
    pub quantity_price_breaks: Vec<QuantityPriceBreak>,
    #[serde(default)]
    pub metafields: Metafields,
}

/// A product.
///
/// The smallest valid product is `{"title": "Blue shirt", "price": 1999}`: it gets a single
/// default variant, like a product without options in the Shopify admin.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductInput {
    /// Lets editors validate and autocomplete the file, e.g. `"../schema/product.schema.json"`.
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Generated from the handle when omitted.
    #[serde(default)]
    pub id: Option<u64>,
    pub title: String,
    /// The URL slug (`/products/<handle>`). Derived from the title when omitted. In a
    /// `products/<name>.json` file it defaults to the file name.
    #[serde(default)]
    pub handle: Option<String>,
    /// HTML description. Also exposed as `product.content`.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub vendor: Option<String>,
    /// The product type, e.g. `"Shirts"`.
    #[serde(default, rename = "type")]
    pub product_type: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Option names, e.g. `["Size", "Color"]`. At most three. Omit for a product with a
    /// single default variant.
    #[serde(default)]
    pub options: Vec<OptionInput>,
    /// The variants. Each must give one value per option.
    #[serde(default)]
    pub variants: Vec<VariantInput>,
    /// Price of the variants that do not set their own, and of the default variant.
    #[serde(default)]
    pub price: Option<Money>,
    /// Compare-at price of the variants that do not set their own.
    #[serde(default)]
    pub compare_at_price: Option<Money>,
    /// Shorthand for image-only media. Use `media` for videos and 3D models.
    #[serde(default)]
    pub images: Vec<ImageInput>,
    /// Images, videos and models in display order. When set, `images` must be empty.
    #[serde(default)]
    pub media: Vec<MediaInput>,
    /// Shorthand to set availability on every variant that does not say otherwise.
    #[serde(default)]
    pub available: Option<bool>,
    /// Handles of the collections this product belongs to, in addition to the collections
    /// that list it in their own `products`.
    #[serde(default)]
    pub collections: Vec<String>,
    /// Selects `templates/product.<suffix>.json` instead of `templates/product.json`.
    #[serde(default)]
    pub template_suffix: Option<String>,
    /// Whether the product is a gift card (`product.gift_card?`).
    #[serde(default)]
    pub gift_card: bool,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub metafields: Metafields,
    /// Handles of the products returned by product recommendations for this product.
    /// Defaults to other products from the same collections.
    #[serde(default)]
    pub recommendations: Option<Vec<String>>,
}

/// How a collection orders its products by default.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub enum SortOrder {
    #[default]
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "best-selling")]
    BestSelling,
    #[serde(rename = "title-ascending")]
    TitleAscending,
    #[serde(rename = "title-descending")]
    TitleDescending,
    #[serde(rename = "price-ascending")]
    PriceAscending,
    #[serde(rename = "price-descending")]
    PriceDescending,
    #[serde(rename = "created-ascending")]
    CreatedAscending,
    #[serde(rename = "created-descending")]
    CreatedDescending,
}

impl SortOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            SortOrder::Manual => "manual",
            SortOrder::BestSelling => "best-selling",
            SortOrder::TitleAscending => "title-ascending",
            SortOrder::TitleDescending => "title-descending",
            SortOrder::PriceAscending => "price-ascending",
            SortOrder::PriceDescending => "price-descending",
            SortOrder::CreatedAscending => "created-ascending",
            SortOrder::CreatedDescending => "created-descending",
        }
    }

    pub fn parse(value: &str) -> Option<SortOrder> {
        Some(match value {
            "manual" => SortOrder::Manual,
            "best-selling" => SortOrder::BestSelling,
            "title-ascending" => SortOrder::TitleAscending,
            "title-descending" => SortOrder::TitleDescending,
            "price-ascending" => SortOrder::PriceAscending,
            "price-descending" => SortOrder::PriceDescending,
            "created-ascending" => SortOrder::CreatedAscending,
            "created-descending" => SortOrder::CreatedDescending,
            _ => return None,
        })
    }
}

/// A rule of an automated collection: products matching it are included.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CollectionRule {
    /// What to look at: `tag`, `type`, `vendor`, `title`, `price` (cents) or
    /// `compare_at_price` (cents).
    pub column: RuleColumn,
    /// Defaults to `equals`.
    #[serde(default)]
    pub relation: RuleRelation,
    /// The value compared with, e.g. `"sale"` for a tag or `"5000"` for a price in cents.
    pub condition: String,
}

/// The product property an automated collection rule looks at.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleColumn {
    Tag,
    Type,
    Vendor,
    Title,
    Price,
    CompareAtPrice,
}

/// How an automated collection rule compares the property to its condition.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleRelation {
    #[default]
    Equals,
    NotEquals,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    GreaterThan,
    LessThan,
}

/// A collection of products.
///
/// A collection named `all` containing every product always exists; define it yourself only
/// to change its title or order.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CollectionInput {
    /// Lets editors validate and autocomplete the file, e.g. `"../schema/collection.schema.json"`.
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default)]
    pub id: Option<u64>,
    pub title: String,
    #[serde(default)]
    pub handle: Option<String>,
    /// HTML description.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub image: Option<ImageInput>,
    /// Handles of the products in the collection, in manual order.
    #[serde(default)]
    pub products: Vec<String>,
    /// Makes the collection automated: products matching the rules are added after the ones
    /// listed in `products`.
    #[serde(default)]
    pub rules: Vec<CollectionRule>,
    /// With several rules: `true` to match any of them, `false` (default) to match all.
    #[serde(default)]
    pub disjunctive: bool,
    /// The default order of the products. Defaults to `manual`.
    #[serde(default)]
    pub sort_order: SortOrder,
    /// Selects `templates/collection.<suffix>.json`.
    #[serde(default)]
    pub template_suffix: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub metafields: Metafields,
}

/// A page (`/pages/<handle>`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageInput {
    /// Lets editors validate and autocomplete the file, e.g. `"../schema/page.schema.json"`.
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default)]
    pub id: Option<u64>,
    pub title: String,
    #[serde(default)]
    pub handle: Option<String>,
    /// HTML content.
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub author: Option<String>,
    /// Selects `templates/page.<suffix>.json`, e.g. `"contact"`.
    #[serde(default)]
    pub template_suffix: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub metafields: Metafields,
}

/// A comment on an article.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentInput {
    #[serde(default)]
    pub id: Option<u64>,
    pub author: String,
    #[serde(default)]
    pub email: Option<String>,
    /// HTML content.
    pub content: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// A blog post.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArticleInput {
    #[serde(default)]
    pub id: Option<u64>,
    pub title: String,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    /// HTML content.
    #[serde(default)]
    pub content: String,
    /// HTML excerpt. `article.excerpt_or_content` falls back to the content.
    #[serde(default)]
    pub excerpt: Option<String>,
    #[serde(default)]
    pub image: Option<ImageInput>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub comments: Vec<CommentInput>,
    #[serde(default)]
    pub template_suffix: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub metafields: Metafields,
}

/// A blog (`/blogs/<handle>`) and its articles.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BlogInput {
    /// Lets editors validate and autocomplete the file, e.g. `"../schema/blog.schema.json"`.
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default)]
    pub id: Option<u64>,
    pub title: String,
    #[serde(default)]
    pub handle: Option<String>,
    /// Articles, newest first unless they carry `published_at` dates.
    #[serde(default)]
    pub articles: Vec<ArticleInput>,
    /// Whether readers can comment. Defaults to `false`.
    #[serde(default)]
    pub comments_enabled: bool,
    /// Whether comments wait for approval. Defaults to `false`.
    #[serde(default)]
    pub moderated: bool,
    #[serde(default)]
    pub template_suffix: Option<String>,
    #[serde(default)]
    pub metafields: Metafields,
}

/// A link in a navigation menu.
///
/// Point it at a resource with exactly one of `url`, `collection`, `product`, `page`, `blog`,
/// `article` or `policy`; the URL and the link type follow from it. A link with none of them
/// points to `#`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkInput {
    pub title: String,
    /// A literal URL: `/`, `/collections/all`, `/search`, `https://example.com`.
    #[serde(default)]
    pub url: Option<String>,
    /// Handle of a collection.
    #[serde(default)]
    pub collection: Option<String>,
    /// Handle of a product.
    #[serde(default)]
    pub product: Option<String>,
    /// Handle of a page.
    #[serde(default)]
    pub page: Option<String>,
    /// Handle of a blog.
    #[serde(default)]
    pub blog: Option<String>,
    /// An article as `<blog-handle>/<article-handle>`.
    #[serde(default)]
    pub article: Option<String>,
    /// A policy: `privacy-policy`, `refund-policy`, `shipping-policy`, `terms-of-service`.
    #[serde(default)]
    pub policy: Option<String>,
    /// Nested links (up to three levels, like the Shopify admin).
    #[serde(default)]
    pub links: Vec<LinkInput>,
}

/// A navigation menu (`linklists.<handle>`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MenuInput {
    /// Lets editors validate and autocomplete the file, e.g. `"../schema/menu.schema.json"`.
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Defaults to the handle, title-cased.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub links: Vec<LinkInput>,
}

/// A postal address. Every field is optional.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddressInput {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
    #[serde(default)]
    pub address1: Option<String>,
    #[serde(default)]
    pub address2: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    /// Province or state name, e.g. `"Ontario"`.
    #[serde(default)]
    pub province: Option<String>,
    /// E.g. `"ON"`.
    #[serde(default)]
    pub province_code: Option<String>,
    /// Country name, e.g. `"Canada"`.
    #[serde(default)]
    pub country: Option<String>,
    /// ISO 3166-1 alpha-2 code, e.g. `"CA"`.
    #[serde(default)]
    pub country_code: Option<String>,
    #[serde(default)]
    pub zip: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
}

/// A line of an order.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrderLineInput {
    /// A variant, by id or SKU, or a product handle (its first variant is used).
    pub variant: VariantRef,
    /// Defaults to 1.
    #[serde(default = "one")]
    pub quantity: u32,
}

fn one() -> u32 {
    1
}

/// An order of a customer, shown on account pages.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrderInput {
    #[serde(default)]
    pub id: Option<u64>,
    /// The order name, e.g. `"#1001"`. Generated when omitted.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// `paid`, `pending`, `refunded`, ... Defaults to `paid`.
    #[serde(default)]
    pub financial_status: Option<String>,
    /// `fulfilled`, `unfulfilled`, `partial`. Defaults to `unfulfilled`.
    #[serde(default)]
    pub fulfillment_status: Option<String>,
    #[serde(default)]
    pub line_items: Vec<OrderLineInput>,
    #[serde(default)]
    pub shipping_address: Option<AddressInput>,
    #[serde(default)]
    pub billing_address: Option<AddressInput>,
    #[serde(default)]
    pub shipping_price: Option<Money>,
    #[serde(default)]
    pub tax_price: Option<Money>,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub note: Option<String>,
}

/// A customer. Sessions log in as a customer by email (see `session.customer`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomerInput {
    /// Lets editors validate and autocomplete the file, e.g. `"../schema/customer.schema.json"`.
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default)]
    pub id: Option<u64>,
    pub email: String,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub accepts_marketing: bool,
    /// Defaults to `true`.
    #[serde(default)]
    pub has_account: Option<bool>,
    #[serde(default)]
    pub tax_exempt: bool,
    /// The first address is the default one (`customer.default_address`).
    #[serde(default)]
    pub addresses: Vec<AddressInput>,
    #[serde(default)]
    pub orders: Vec<OrderInput>,
    /// The password accepted by the local login form. Any password works when omitted.
    #[serde(default)]
    pub password: Option<String>,
    /// The name of a company in `companies`: the customer buys for it (B2B).
    #[serde(default)]
    pub company: Option<String>,
    /// The names of the locations of the company the customer can buy for. Defaults to all
    /// of them.
    #[serde(default)]
    pub company_locations: Vec<String>,
    #[serde(default)]
    pub metafields: Metafields,
}

/// A place a company buys for (`customer.current_location`): a branch, a shop, a warehouse.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompanyLocationInput {
    #[serde(default)]
    pub id: Option<u64>,
    pub name: String,
    /// The id the merchant gives the location in its own systems.
    #[serde(default)]
    pub external_id: Option<String>,
    /// Where orders of this location ship to.
    #[serde(default)]
    pub shipping_address: Option<AddressInput>,
    /// The tax number of the location, e.g. a VAT number.
    #[serde(default)]
    pub tax_registration_id: Option<String>,
    #[serde(default)]
    pub metafields: Metafields,
}

/// A company that buys from the store (B2B). A customer whose `company` names it is a B2B
/// customer: `customer.b2b?` is true, and `customer.current_company` is this company.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompanyInput {
    #[serde(default)]
    pub id: Option<u64>,
    /// The name customers refer to the company by.
    pub name: String,
    /// The id the merchant gives the company in its own systems.
    #[serde(default)]
    pub external_id: Option<String>,
    /// The places the company buys for. The first one is selected when a customer logs in.
    #[serde(default)]
    pub locations: Vec<CompanyLocationInput>,
    #[serde(default)]
    pub metafields: Metafields,
}

/// A store policy (`shop.refund_policy`, `/policies/refund-policy`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyInput {
    /// Defaults to the standard title of the policy.
    #[serde(default)]
    pub title: Option<String>,
    /// HTML body.
    pub body: String,
}

/// The store policies. Each one that is set gets a page under `/policies/`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoliciesInput {
    #[serde(default)]
    pub privacy_policy: Option<PolicyInput>,
    #[serde(default)]
    pub refund_policy: Option<PolicyInput>,
    #[serde(default)]
    pub shipping_policy: Option<PolicyInput>,
    #[serde(default)]
    pub terms_of_service: Option<PolicyInput>,
    #[serde(default)]
    pub subscription_policy: Option<PolicyInput>,
}

/// The brand assets of the store (`shop.brand`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrandInput {
    #[serde(default)]
    pub slogan: Option<String>,
    #[serde(default)]
    pub short_description: Option<String>,
    #[serde(default)]
    pub logo: Option<ImageInput>,
    #[serde(default)]
    pub square_logo: Option<ImageInput>,
    #[serde(default)]
    pub cover_image: Option<ImageInput>,
    #[serde(default)]
    pub favicon: Option<ImageInput>,
    /// Brand colors as CSS hex strings, e.g. `{"primary": ["#112233"], "secondary": []}`.
    #[serde(default)]
    pub colors: Option<Json>,
}

/// General store information (the `shop` object). Every field has a sensible default.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShopInput {
    #[serde(default)]
    pub id: Option<u64>,
    /// Defaults to `"Local Store"`.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    /// The primary domain, without scheme. Defaults to the host the server is reached on.
    #[serde(default)]
    pub domain: Option<String>,
    /// The `*.myshopify.com` domain. Defaults to a value derived from the name.
    #[serde(default)]
    pub permanent_domain: Option<String>,
    /// ISO 4217 code of the store currency. Defaults to `USD`.
    #[serde(default)]
    pub currency: Option<String>,
    /// How `money` formats amounts, e.g. `"${{amount}}"` or `"{{amount_with_comma_separator}} €"`.
    /// Defaults to the usual format of the currency.
    #[serde(default)]
    pub money_format: Option<String>,
    /// How `money_with_currency` formats amounts, e.g. `"${{amount}} USD"`.
    #[serde(default)]
    pub money_with_currency_format: Option<String>,
    /// IANA time zone used to display dates, e.g. `"Europe/Paris"`. Defaults to `UTC`.
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub address: Option<AddressInput>,
    #[serde(default)]
    pub policies: PoliciesInput,
    /// Payment icons to show, e.g. `["visa", "master", "american_express", "paypal"]`.
    #[serde(default)]
    pub enabled_payment_types: Option<Vec<String>>,
    /// Whether customer accounts exist at all. Defaults to `true`.
    #[serde(default)]
    pub customer_accounts_enabled: Option<bool>,
    /// Whether checking out as a guest is possible. Defaults to `true`.
    #[serde(default)]
    pub customer_accounts_optional: Option<bool>,
    /// Which customer accounts the store uses. `new`: accounts are hosted by Shopify, and
    /// `/account` is a page of lsf where you choose who is logged in. `legacy`: the theme's
    /// `templates/customers` are rendered. Defaults to `legacy` when the theme has those
    /// templates, and to `new` otherwise.
    #[serde(default)]
    pub customer_accounts: Option<CustomerAccounts>,
    /// Whether prices include taxes. Defaults to `false`.
    #[serde(default)]
    pub taxes_included: bool,
    /// The message shown on the password page.
    #[serde(default)]
    pub password_message: Option<String>,
    /// The password the `/password` page accepts. Defaults to `"password"`. The storefront is
    /// never locked: the page is there to be worked on, and nothing leads to it.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub brand: Option<BrandInput>,
    #[serde(default)]
    pub metafields: Metafields,
}

/// A country the store sells to.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CountryInput {
    /// ISO 3166-1 alpha-2 code, e.g. `"FR"`.
    pub iso_code: String,
    /// Defaults to the English name of the country.
    #[serde(default)]
    pub name: Option<String>,
    /// ISO 4217 currency code. Defaults to the store currency.
    #[serde(default)]
    pub currency: Option<String>,
    /// `metric` or `imperial`. Defaults to `metric`.
    #[serde(default)]
    pub unit_system: Option<String>,
    /// Whether to list the country among the popular ones.
    #[serde(default)]
    pub popular: bool,
}

/// A language the store is published in.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanguageInput {
    /// Locale code, e.g. `"en"`, `"fr"`, `"pt-BR"`.
    pub iso_code: String,
    /// The language's name in English. Defaults to a built-in name.
    #[serde(default)]
    pub name: Option<String>,
    /// The language's name in that language. Defaults to a built-in name.
    #[serde(default)]
    pub endonym_name: Option<String>,
}

/// The countries and languages the store sells in (the `localization` object).
///
/// The first language is the primary one and is served at `/`; the others are served under
/// `/<iso_code>/`. Defaults to one country (`US`) and the theme's default language.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalizationInput {
    #[serde(default)]
    pub countries: Vec<CountryInput>,
    #[serde(default)]
    pub languages: Vec<LanguageInput>,
}

/// A file in `files/` that needs metadata. Files without an entry work too.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileInput {
    #[serde(default)]
    pub alt: Option<String>,
    /// Size to assume when the file itself is absent (a placeholder is generated).
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub focal_point: Option<FocalPoint>,
}

/// A metaobject entry (`metaobjects.<type>.<handle>`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MetaobjectInput {
    pub handle: String,
    /// Field values, with the same shorthand as metafields.
    #[serde(default)]
    pub fields: IndexMap<String, MetafieldInput>,
}

/// A reference to a variant: its numeric id, its SKU, or a product handle (meaning that
/// product's first variant).
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum VariantRef {
    Id(u64),
    SkuOrHandle(String),
}

/// A line of the cart a session starts with.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CartLineInput {
    /// A variant, by id or SKU, or a product handle (its first variant is used).
    pub variant: VariantRef,
    /// Defaults to 1.
    #[serde(default = "one")]
    pub quantity: u32,
    /// Line item properties, e.g. `{"Engraving": "Hello"}`.
    #[serde(default)]
    pub properties: IndexMap<String, String>,
}

/// The cart a session starts with.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CartInput {
    #[serde(default)]
    pub items: Vec<CartLineInput>,
    #[serde(default)]
    pub note: Option<String>,
    /// Cart attributes.
    #[serde(default)]
    pub attributes: IndexMap<String, String>,
}

/// The state a browser session starts with. Tests can replace it per session through the
/// control API (`PUT /__lsf/session`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionInput {
    /// Lets editors validate and autocomplete the file, e.g. `"../schema/session.schema.json"`.
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Who is logged in: the email of a customer, `"default"` for the first customer of the
    /// data, or `"none"`. Nobody is logged in when omitted.
    #[serde(default)]
    pub customer: Option<String>,
    /// The content of the cart. Empty when omitted.
    #[serde(default)]
    pub cart: Option<CartInput>,
    /// ISO code of the selected country. Defaults to the first country.
    #[serde(default)]
    pub country: Option<String>,
    /// For a B2B customer: the name of the company location they buy for. Defaults to the
    /// first one they have access to.
    #[serde(default)]
    pub company_location: Option<String>,
}

/// Who a gift card was sent to.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GiftCardRecipientInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub nickname: Option<String>,
}

/// An issued gift card, shown by `templates/gift_card.liquid` at the URL `lsf routes` lists
/// (`/gift_cards/<shop id>/<token>`).
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GiftCardInput {
    /// The code customers redeem at checkout, e.g. `"WCGX7X97G74JGDGC"`.
    pub code: String,
    /// The value the card was issued with.
    pub initial_value: Money,
    /// What is left on the card. Defaults to `initial_value`.
    #[serde(default)]
    pub balance: Option<Money>,
    /// ISO 4217 code. Defaults to the shop's currency.
    #[serde(default)]
    pub currency: Option<String>,
    /// The day the card stops working, e.g. `"2026-12-31"`. A date before `now` makes the card
    /// `expired`.
    #[serde(default)]
    pub expires_on: Option<String>,
    /// `false` for a card the merchant disabled.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// The email of the customer (from `customers`) the card belongs to.
    #[serde(default)]
    pub customer: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub recipient: Option<GiftCardRecipientInput>,
    /// When the card is scheduled to be sent to its recipient.
    #[serde(default)]
    pub send_on: Option<String>,
    /// The handle of the gift card product that was bought.
    #[serde(default)]
    pub product: Option<String>,
    /// Line item properties of the purchase.
    #[serde(default)]
    pub properties: IndexMap<String, String>,
    /// Selects `templates/gift_card.<suffix>.liquid`.
    #[serde(default)]
    pub template_suffix: Option<String>,
}

/// A data file: any combination of the sections below. Every `*.json` file at the root of the
/// data directory is one of these, and they are merged, so you can split the data as you like
/// (`shop.json`, `catalog.json`, ...).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreInput {
    /// Lets editors validate and autocomplete the file, e.g. `"./schema/store.schema.json"`.
    #[serde(default, rename = "$schema")]
    pub schema: Option<String>,
    /// General store information. At most one data file may define it.
    #[serde(default)]
    pub shop: Option<ShopInput>,
    /// Products. They can also live one per file in `products/`.
    #[serde(default)]
    pub products: Vec<ProductInput>,
    /// Collections. They can also live one per file in `collections/`.
    #[serde(default)]
    pub collections: Vec<CollectionInput>,
    /// Pages. They can also live one per file in `pages/`.
    #[serde(default)]
    pub pages: Vec<PageInput>,
    /// Blogs with their articles. They can also live one per file in `blogs/`.
    #[serde(default)]
    pub blogs: Vec<BlogInput>,
    /// Navigation menus by handle, e.g. `main-menu` and `footer`.
    #[serde(default)]
    pub menus: IndexMap<String, MenuInput>,
    /// Customers. They can also live one per file in `customers/`.
    #[serde(default)]
    pub customers: Vec<CustomerInput>,
    /// Companies that buy from the store (B2B). Customers join one with their `company`.
    #[serde(default)]
    pub companies: Vec<CompanyInput>,
    /// Issued gift cards, each with its own page.
    #[serde(default)]
    pub gift_cards: Vec<GiftCardInput>,
    /// Metaobject entries by type.
    #[serde(default)]
    pub metaobjects: IndexMap<String, Vec<MetaobjectInput>>,
    /// The countries and languages of the store. At most one data file may define it.
    #[serde(default)]
    pub localization: Option<LocalizationInput>,
    /// Metadata for files in `files/`, by path.
    #[serde(default)]
    pub files: IndexMap<String, FileInput>,
    /// What every new browser session starts with: a logged-in customer, a filled cart...
    #[serde(default)]
    pub session: Option<SessionInput>,
    /// Freezes the clock: the instant `'now'` resolves to, e.g. `"2025-01-15T10:00:00Z"`.
    /// Set it to make renders reproducible. Defaults to the real time.
    #[serde(default)]
    pub now: Option<String>,
    /// Overrides for theme settings (`config/settings_data.json`), by setting id.
    #[serde(default)]
    pub theme_settings: IndexMap<String, Json>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_money() {
        assert_eq!(Money::Cents(1999).cents(), Some(1999));
        assert_eq!(Money::Decimal("19.99".into()).cents(), Some(1999));
        assert_eq!(Money::Decimal("20".into()).cents(), Some(2000));
        assert_eq!(Money::Decimal("0.5".into()).cents(), Some(50));
        assert_eq!(Money::Decimal("-1.25".into()).cents(), Some(-125));
        assert_eq!(Money::Decimal("1.234".into()).cents(), None);
        assert_eq!(Money::Decimal("abc".into()).cents(), None);
    }

    #[test]
    fn minimal_product_parses() {
        let product: ProductInput =
            serde_json::from_str(r#"{"title": "Blue shirt", "price": "19.99"}"#).unwrap();
        assert_eq!(product.price.unwrap().cents(), Some(1999));
        assert!(serde_json::from_str::<ProductInput>(r#"{"title": "x", "prise": 1}"#).is_err());
    }
}
