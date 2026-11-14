//! The store: products, collections, content and customers, resolved from the data directory
//! into a form that is cheap to query while rendering.

pub mod build;
mod collate;
mod countries;
mod country_names;
mod date;
pub mod demo;
pub mod docs;
pub mod load;
pub mod metafield_references;
pub mod metafield_types;
pub mod model;
mod provinces;
pub mod reference;
pub mod theme_settings;
pub mod validate;

use std::borrow::Cow;
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use indexmap::IndexMap;
use serde_json::Value as Json;

pub use model::{InventoryPolicy, MediaType, ProductStatus, SortOrder, VideoHost};

#[derive(Clone, Debug)]
pub struct Image {
    pub id: u64,
    /// Path inside the data directory's `files/`, e.g. `products/shirt.jpg`.
    pub src: String,
    pub alt: String,
    pub width: u32,
    pub height: u32,
    /// Focal point in percent from the top-left corner.
    pub focal_point: Option<(f64, f64)>,
}

impl Image {
    pub fn aspect_ratio(&self) -> f64 {
        if self.height == 0 {
            1.0
        } else {
            f64::from(self.width) / f64::from(self.height)
        }
    }
}

#[derive(Clone, Debug)]
pub struct MediaSource {
    pub url: String,
    pub mime_type: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub enum MediaKind {
    Image,
    Video {
        sources: Vec<MediaSource>,
        duration: u64,
    },
    ExternalVideo {
        host: VideoHost,
        external_id: String,
    },
    Model {
        sources: Vec<MediaSource>,
    },
}

#[derive(Clone, Debug)]
pub struct Media {
    pub id: u64,
    /// 1-based position among the product's media.
    pub position: usize,
    pub alt: String,
    /// The image itself for image media, the poster for the other kinds.
    pub preview: Option<Image>,
    pub kind: MediaKind,
    pub aspect_ratio: f64,
}

impl Media {
    pub fn media_type(&self) -> &'static str {
        match self.kind {
            MediaKind::Image => "image",
            MediaKind::Video { .. } => "video",
            MediaKind::ExternalVideo { .. } => "external_video",
            MediaKind::Model { .. } => "model",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Metafield {
    pub kind: String,
    pub value: Json,
}

pub type Metafields = IndexMap<String, IndexMap<String, Metafield>>;

/// The app of `--app` as the store has it installed (`app` of the store data).
#[derive(Clone, Debug, Default)]
pub struct AppInstallation {
    /// Its app-data metafields: what `app.metafields` reads in the Liquid of its blocks.
    pub metafields: Metafields,
    /// Its app embed blocks turned on or off, by the name of their file.
    pub embeds: IndexMap<String, AppEmbed>,
}

/// An app embed block the store data turns on or off.
#[derive(Clone, Debug)]
pub struct AppEmbed {
    pub enabled: bool,
    /// The settings it is turned on with; `None` for those of the theme, or the defaults.
    pub settings: Option<serde_json::Map<String, Json>>,
}

/// How an option value is shown instead of its name.
#[derive(Clone, Debug)]
pub struct Swatch {
    /// A CSS color.
    pub color: Option<String>,
    pub image: Option<Image>,
}

#[derive(Clone, Debug)]
pub struct ProductOption {
    pub name: String,
    pub position: usize,
    pub values: Vec<String>,
    /// The swatch of each value, in the order of `values`.
    pub swatches: Vec<Option<Swatch>>,
}

impl ProductOption {
    pub fn swatch(&self, value: &str) -> Option<&Swatch> {
        let index = self.values.iter().position(|name| name == value)?;
        self.swatches.get(index)?.as_ref()
    }
}

/// A place the store stocks products at.
#[derive(Clone, Debug)]
pub struct Location {
    pub id: u64,
    pub name: String,
    pub address: Address,
    pub pick_up_enabled: bool,
    pub pick_up_time: String,
    pub physical_storefront: bool,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub metafields: Metafields,
}

/// How a selling plan changes a price.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adjustment {
    /// Percent off.
    Percentage(i64),
    /// Cents off.
    FixedAmount(i64),
    /// The price, in cents.
    Price(i64),
}

impl Adjustment {
    /// The price of something that costs `price` without the plan.
    pub fn apply(self, price: i64) -> i64 {
        match self {
            Adjustment::Percentage(percent) => (price * (100 - percent) + 50).div_euclid(100),
            Adjustment::FixedAmount(amount) => (price - amount).max(0),
            Adjustment::Price(amount) => amount,
        }
    }

    /// `(value_type, value)` as Liquid and the product JSON have them.
    pub fn describe(self) -> (&'static str, i64) {
        match self {
            Adjustment::Percentage(value) => ("percentage", value),
            Adjustment::FixedAmount(value) => ("fixed_amount", value),
            Adjustment::Price(value) => ("price", value),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PriceAdjustment {
    pub adjustment: Adjustment,
    /// The number of orders the adjustment applies to; every order when `None`.
    pub order_count: Option<u32>,
}

/// What is due at checkout: a share of the price, or an amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckoutCharge {
    Percentage(i64),
    Price(i64),
}

#[derive(Clone, Debug)]
pub struct SellingPlan {
    pub id: u64,
    pub name: String,
    pub description: Option<String>,
    /// One value per option of the group.
    pub options: Vec<String>,
    pub price_adjustments: Vec<PriceAdjustment>,
    pub recurring_deliveries: bool,
    pub checkout_charge: CheckoutCharge,
}

/// What a variant costs when bought with a selling plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allocation {
    pub price: i64,
    /// The price without the plan, when the plan lowers it; the variant's own compare-at
    /// price otherwise.
    pub compare_at_price: Option<i64>,
    pub per_delivery_price: i64,
    pub checkout_charge_amount: i64,
    pub remaining_balance_charge_amount: i64,
    pub unit_price: Option<i64>,
    /// The price each price adjustment of the plan leads to, in order.
    pub adjusted_prices: Vec<i64>,
}

impl SellingPlan {
    pub fn allocation(&self, variant: &Variant) -> Allocation {
        let adjusted_prices: Vec<i64> = self
            .price_adjustments
            .iter()
            .map(|adjustment| adjustment.adjustment.apply(variant.price))
            .collect();
        let price = adjusted_prices.first().copied().unwrap_or(variant.price);
        let checkout_charge_amount = match self.checkout_charge {
            CheckoutCharge::Percentage(percent) => (price * percent + 50).div_euclid(100),
            CheckoutCharge::Price(amount) => amount.min(price),
        };
        Allocation {
            price,
            compare_at_price: if price < variant.price {
                Some(variant.price)
            } else {
                variant.compare_at_price
            },
            per_delivery_price: price,
            checkout_charge_amount,
            remaining_balance_charge_amount: price - checkout_charge_amount,
            // The unit price follows the price: 10% off the variant is 10% off each unit.
            unit_price: variant.unit_price.map(|unit_price| {
                if variant.price > 0 {
                    (unit_price * price + variant.price / 2).div_euclid(variant.price)
                } else {
                    unit_price
                }
            }),
            adjusted_prices,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SellingPlanGroup {
    /// A hexadecimal string, as on Shopify.
    pub id: String,
    pub name: String,
    pub app_id: Option<String>,
    /// The names of the options the plans differ by.
    pub options: Vec<String>,
    pub plans: Vec<SellingPlan>,
}

#[derive(Clone, Debug)]
pub struct QuantityRule {
    pub min: u32,
    pub max: Option<u32>,
    pub increment: u32,
}

#[derive(Clone, Debug)]
pub struct UnitPriceMeasurement {
    pub measured_type: String,
    pub quantity_value: f64,
    pub quantity_unit: String,
    pub reference_value: f64,
    pub reference_unit: String,
}

#[derive(Clone, Debug)]
pub struct Variant {
    pub id: u64,
    pub title: String,
    pub options: Vec<String>,
    /// Cents.
    pub price: i64,
    pub compare_at_price: Option<i64>,
    pub sku: String,
    pub barcode: String,
    pub available: bool,
    pub inventory_quantity: i64,
    pub inventory_tracked: bool,
    pub inventory_policy: InventoryPolicy,
    /// Grams.
    pub weight: u64,
    pub weight_unit: String,
    pub requires_shipping: bool,
    pub taxable: bool,
    /// Index of the variant's media in the product's media.
    pub media_index: Option<usize>,
    pub unit_price: Option<i64>,
    pub unit_price_measurement: Option<UnitPriceMeasurement>,
    pub quantity_rule: QuantityRule,
    /// `(minimum quantity, price in cents)`.
    pub quantity_price_breaks: Vec<(u32, i64)>,
    /// The locations that stock the variant, as `(index into the store's locations, whether
    /// it is in stock there)`.
    pub store_availabilities: Vec<(usize, bool)>,
    /// The variants a fixed bundle is made of, as `(variant id, quantity per bundle)`: empty
    /// for a variant that is no bundle.
    pub components: Vec<(u64, u32)>,
    pub metafields: Metafields,
}

/// The weight of a variant expressed in its display unit.
pub fn weight_in_unit(grams: u64, unit: &str) -> f64 {
    let grams = grams as f64;
    match unit {
        "g" => grams,
        "oz" => grams / 28.349_523_125,
        "lb" => grams / 453.592_37,
        _ => grams / 1000.0,
    }
}

#[derive(Clone, Debug)]
pub struct Product {
    pub id: u64,
    pub title: String,
    pub handle: String,
    pub description: String,
    pub vendor: String,
    pub product_type: String,
    pub tags: Vec<String>,
    pub options: Vec<ProductOption>,
    pub variants: Vec<Variant>,
    pub media: Vec<Media>,
    pub template_suffix: Option<String>,
    pub gift_card: bool,
    pub created_at: DateTime<Utc>,
    pub published_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metafields: Metafields,
    /// Indexes of the collections that contain the product.
    pub collections: Vec<usize>,
    /// Indexes of the products recommended alongside this one.
    pub recommendations: Vec<usize>,
    /// Indexes of the selling plan groups the product is sold with.
    pub selling_plan_groups: Vec<usize>,
    pub requires_selling_plan: bool,
    pub status: ProductStatus,
    /// Whether the product is published on the Online Store.
    pub published: bool,
}

impl Product {
    /// Whether the product is on the storefront: active and published.
    pub fn listed(&self) -> bool {
        self.status == ProductStatus::Active && self.published
    }

    pub fn has_only_default_variant(&self) -> bool {
        self.options.len() == 1
            && self.options[0].name == "Title"
            && self.variants.len() == 1
            && self.variants[0].title == "Default Title"
    }

    pub fn available(&self) -> bool {
        self.variants.iter().any(|variant| variant.available)
    }

    pub fn price_min(&self) -> i64 {
        self.variants
            .iter()
            .map(|variant| variant.price)
            .min()
            .unwrap_or(0)
    }

    pub fn price_max(&self) -> i64 {
        self.variants
            .iter()
            .map(|variant| variant.price)
            .max()
            .unwrap_or(0)
    }

    pub fn compare_at_price_min(&self) -> i64 {
        self.variants
            .iter()
            .filter_map(|variant| variant.compare_at_price)
            .min()
            .unwrap_or(0)
    }

    pub fn compare_at_price_max(&self) -> i64 {
        self.variants
            .iter()
            .filter_map(|variant| variant.compare_at_price)
            .max()
            .unwrap_or(0)
    }

    /// The image media, in order.
    pub fn images(&self) -> impl Iterator<Item = &Image> {
        self.media
            .iter()
            .filter(|media| matches!(media.kind, MediaKind::Image))
            .filter_map(|media| media.preview.as_ref())
    }

    pub fn first_available_variant(&self) -> Option<&Variant> {
        self.variants.iter().find(|variant| variant.available)
    }
}

#[derive(Clone, Debug)]
pub struct Collection {
    pub id: u64,
    pub title: String,
    pub handle: String,
    pub description: String,
    pub image: Option<Image>,
    /// Indexes into the store's products, in the collection's manual order.
    pub products: Vec<usize>,
    pub sort_order: SortOrder,
    pub template_suffix: Option<String>,
    pub published_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metafields: Metafields,
}

#[derive(Clone, Debug)]
pub struct Page {
    pub id: u64,
    pub title: String,
    pub handle: String,
    pub content: String,
    pub author: String,
    pub template_suffix: Option<String>,
    pub published_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metafields: Metafields,
}

#[derive(Clone, Debug)]
pub struct Comment {
    pub id: u64,
    pub author: String,
    pub email: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct Article {
    pub id: u64,
    pub title: String,
    pub handle: String,
    pub author: String,
    pub content: String,
    pub excerpt: String,
    pub image: Option<Image>,
    pub tags: Vec<String>,
    pub comments: Vec<Comment>,
    pub template_suffix: Option<String>,
    pub created_at: DateTime<Utc>,
    pub published_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metafields: Metafields,
}

#[derive(Clone, Debug)]
pub struct Blog {
    pub id: u64,
    pub title: String,
    pub handle: String,
    pub articles: Vec<Article>,
    pub comments_enabled: bool,
    pub moderated: bool,
    pub template_suffix: Option<String>,
    pub metafields: Metafields,
}

/// What a menu link points to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Http,
    Frontpage,
    Catalog,
    Collections,
    Search,
    Collection(usize),
    Product(usize),
    Page(usize),
    Blog(usize),
    /// `(blog index, article index)`.
    Article(usize, usize),
    Policy(String),
}

impl LinkTarget {
    /// The `link.type` value.
    pub fn type_name(&self) -> &'static str {
        match self {
            LinkTarget::Http => "http_link",
            LinkTarget::Frontpage => "frontpage_link",
            LinkTarget::Catalog => "catalog_link",
            LinkTarget::Collections => "collections_link",
            LinkTarget::Search => "search_link",
            LinkTarget::Collection(_) => "collection_link",
            LinkTarget::Product(_) => "product_link",
            LinkTarget::Page(_) => "page_link",
            LinkTarget::Blog(_) => "blog_link",
            LinkTarget::Article(..) => "article_link",
            LinkTarget::Policy(_) => "policy_link",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Link {
    pub title: String,
    pub handle: String,
    pub url: String,
    pub target: LinkTarget,
    pub links: Vec<Link>,
}

impl Link {
    /// The number of nested levels below this link.
    pub fn levels(&self) -> usize {
        self.links
            .iter()
            .map(|link| 1 + link.levels())
            .max()
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug)]
pub struct Menu {
    pub handle: String,
    pub title: String,
    pub links: Vec<Link>,
    /// Made up by lsf: the data has no menu with this handle, which is one of Shopify's default
    /// menus ([`DEFAULT_MENUS`]). Its links are lsf's, not the store's.
    pub supplied_by_lsf: bool,
}

/// Shopify's default menus: a store cannot delete them, and a `link_list` setting can only
/// default to one of them. lsf makes up the ones the data does not have.
pub const DEFAULT_MENUS: [&str; 2] = ["main-menu", "footer"];

impl Menu {
    /// The titles of its links, for a message: `Home, Catalog`.
    pub fn link_titles(&self) -> String {
        let titles: Vec<&str> = self.links.iter().map(|link| link.title.as_str()).collect();
        if titles.is_empty() {
            "no links".to_string()
        } else {
            titles.join(", ")
        }
    }

    pub fn levels(&self) -> usize {
        self.links
            .iter()
            .map(|link| 1 + link.levels())
            .max()
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Address {
    pub id: u64,
    pub first_name: String,
    pub last_name: String,
    pub company: String,
    pub address1: String,
    pub address2: String,
    pub city: String,
    pub province: String,
    pub province_code: String,
    pub country: String,
    pub country_code: String,
    pub zip: String,
    pub phone: String,
}

#[derive(Clone, Debug)]
pub struct OrderLine {
    /// `(product index, variant index)`.
    pub variant: (usize, usize),
    pub quantity: u32,
}

#[derive(Clone, Debug)]
pub struct Order {
    pub id: u64,
    pub name: String,
    pub order_number: u64,
    pub created_at: DateTime<Utc>,
    pub financial_status: String,
    pub fulfillment_status: String,
    pub line_items: Vec<OrderLine>,
    pub shipping_address: Option<Address>,
    pub billing_address: Option<Address>,
    pub shipping_price: i64,
    pub tax_price: i64,
    pub cancelled: bool,
    pub note: String,
    pub returns: Vec<OrderReturn>,
}

/// A return of an order: what was sent back, as lines of the order.
#[derive(Clone, Debug)]
pub struct OrderReturn {
    pub status: String,
    /// `(index of the line in the order, quantity)`.
    pub lines: Vec<(usize, u32)>,
}

#[derive(Clone, Debug)]
pub struct CompanyLocation {
    pub id: u64,
    pub name: String,
    pub external_id: Option<String>,
    pub shipping_address: Option<Address>,
    pub tax_registration_id: Option<String>,
    pub locale: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub orders_count: u32,
    /// In minor units of the store currency.
    pub total_spent: i64,
    pub metafields: Metafields,
}

#[derive(Clone, Debug)]
pub struct Company {
    pub id: u64,
    pub name: String,
    pub external_id: Option<String>,
    pub locations: Vec<CompanyLocation>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metafields: Metafields,
}

/// A customer as a contact of their company.
#[derive(Clone, Debug)]
pub struct CompanyContact {
    pub title: Option<String>,
    pub locale: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct Customer {
    pub id: u64,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub phone: String,
    pub tags: Vec<String>,
    pub accepts_marketing: bool,
    pub has_account: bool,
    pub tax_exempt: bool,
    pub addresses: Vec<Address>,
    pub orders: Vec<Order>,
    pub metafields: Metafields,
    /// The company the customer buys for, as an index into the store's companies.
    pub company: Option<usize>,
    /// The locations of that company the customer can buy for, as indexes into its locations.
    pub company_locations: Vec<usize>,
    /// The customer as a contact of that company.
    pub company_contact: Option<CompanyContact>,
}

impl Customer {
    /// The location a B2B customer buys for: the one the session selected when they have
    /// access to it, the first one otherwise.
    pub fn current_location(&self, selected: Option<u64>, store: &Store) -> Option<usize> {
        let company = &store.companies[self.company?];
        self.company_locations
            .iter()
            .copied()
            .find(|index| Some(company.locations[*index].id) == selected)
            .or_else(|| self.company_locations.first().copied())
    }
}

/// A URL redirect: a visit to `path` that finds nothing goes to `target`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    /// With a leading `/`.
    pub path: String,
    pub target: String,
}

/// A path as a URL redirect keeps it: trimmed, with a leading `/`, as the Admin API keeps it.
pub fn redirect_path(path: &str) -> String {
    let path = path.trim();
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

#[derive(Clone, Debug)]
pub struct GiftCard {
    pub id: u64,
    pub code: String,
    /// The secret part of the card's URL.
    pub token: String,
    pub initial_value: i64,
    pub balance: i64,
    pub currency: String,
    pub expires_on: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub customer: Option<u64>,
    pub message: String,
    pub recipient: Option<GiftCardRecipient>,
    pub send_on: Option<DateTime<Utc>>,
    pub product: Option<usize>,
    pub properties: IndexMap<String, String>,
    pub template_suffix: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct GiftCardRecipient {
    pub name: String,
    pub email: String,
    pub nickname: String,
}

impl GiftCard {
    /// The path of the page showing the card.
    pub fn path(&self, shop_id: u64) -> String {
        format!("/gift_cards/{shop_id}/{}", self.token)
    }
}

#[derive(Clone, Debug)]
pub struct Policy {
    /// `refund-policy`, ...
    pub handle: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, Default)]
pub struct Brand {
    pub slogan: String,
    pub short_description: String,
    pub logo: Option<Image>,
    pub square_logo: Option<Image>,
    pub cover_image: Option<Image>,
    pub favicon: Option<Image>,
    pub colors: Json,
}

#[derive(Clone, Debug)]
pub struct Shop {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub email: String,
    pub phone: String,
    /// The configured domain, if any. The request host is used otherwise.
    pub domain: Option<String>,
    pub permanent_domain: String,
    pub currency: String,
    pub money_format: String,
    pub money_with_currency_format: String,
    pub timezone: Tz,
    pub address: Address,
    pub policies: Vec<Policy>,
    pub enabled_payment_types: Vec<String>,
    pub customer_accounts_enabled: bool,
    pub customer_accounts_optional: bool,
    pub taxes_included: bool,
    pub password_message: String,
    /// The password the `/password` page accepts.
    pub password: String,
    pub brand: Option<Brand>,
    pub metafields: Metafields,
}

#[derive(Clone, Debug)]
pub struct Country {
    pub iso_code: String,
    pub name: String,
    pub currency: String,
    pub unit_system: String,
    pub popular: bool,
    /// The provinces the store ships to, sorted by name: empty for a country without.
    pub provinces: Vec<Province>,
}

/// A province of a country: a state, a region, a prefecture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Province {
    /// Shopify's code: `ON`, `JP-13`.
    pub code: String,
    /// The name an address holds and a form sends: `Tōkyō`.
    pub name: String,
    /// The name a selector shows: `Tokyo`.
    pub label: String,
}

impl Province {
    /// Every province Shopify lists for a country, sorted by name.
    pub fn all_of(country: &str) -> Vec<Province> {
        reference::provinces(country)
            .iter()
            .map(|(code, name, label)| Province {
                code: (*code).to_string(),
                name: (*name).to_string(),
                label: (*label).to_string(),
            })
            .collect()
    }
}

/// A group of countries sold as one market.
#[derive(Clone, Debug)]
pub struct Market {
    pub handle: String,
    /// ISO 3166-1 alpha-2 codes, upper case.
    pub countries: Vec<String>,
    /// The ISO codes of the languages it offers, as `localization.languages` spells them, its
    /// default first. Empty when it offers every language of the store.
    pub languages: Vec<String>,
    pub metafields: Metafields,
    /// The suffix of its subfolders (`ca` for `/en-ca/`), lower case: `None` for a market
    /// served at `/`.
    pub subfolder_suffix: Option<String>,
    /// The currency it declares (ISO 4217, upper case), which its countries pay in unless they
    /// have their own.
    pub currency: Option<String>,
    /// From the store currency to the currency of its countries.
    pub rate: Option<Rate>,
    /// Its fixed prices, in the currency of its countries, by variant id.
    pub prices: HashMap<u64, FixedPrice>,
}

/// The market a country is sold in ([`Store::market_for`]).
#[derive(Clone, Debug)]
pub struct CountryMarket<'s> {
    /// The number of its GID: `gid://shopify/Market/<id>`.
    pub id: u64,
    pub handle: String,
    /// The market of the data, when the country is in one: `None` for a market of its own.
    pub declared: Option<&'s Market>,
}

impl CountryMarket<'_> {
    /// `gid://shopify/Market/<id>`.
    pub fn gid(&self) -> String {
        format!("gid://shopify/Market/{}", self.id)
    }
}

/// A fixed price of a market's price list, in cents of its currency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedPrice {
    pub price: i64,
    pub compare_at_price: Option<i64>,
}

/// An exchange rate: what one unit of the store currency is worth in another, kept exact as the
/// decimal it was written as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rate {
    /// The digits, without the decimal point: `92` for `0.92`.
    digits: i128,
    /// The number of decimals: `2` for `0.92`.
    scale: u32,
}

impl Rate {
    /// One for one.
    pub const ONE: Rate = Rate {
        digits: 1,
        scale: 0,
    };

    /// A positive decimal (`"0.92"`, `"150"`), up to 12 decimals.
    pub fn parse(text: &str) -> Option<Rate> {
        let text = text.trim();
        let (units, fraction) = text.split_once('.').unwrap_or((text, ""));
        if units.is_empty()
            || fraction.len() > 12
            || !units
                .bytes()
                .chain(fraction.bytes())
                .all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let digits: i128 = format!("{units}{fraction}").parse().ok()?;
        let mut rate = Rate {
            digits,
            scale: fraction.len() as u32,
        };
        while rate.scale > 0 && rate.digits % 10 == 0 {
            rate.digits /= 10;
            rate.scale -= 1;
        }
        Some(rate)
    }

    pub fn is_zero(&self) -> bool {
        self.digits == 0
    }

    pub fn is_one(&self) -> bool {
        *self == Rate::ONE
    }

    /// An amount in cents of the store currency, in cents of the other, rounded half away from
    /// zero; to whole units for a currency without decimals.
    pub fn convert(&self, cents: i64, currency: &str) -> i64 {
        let unit: i128 = if reference::has_no_decimals(currency) {
            100
        } else {
            1
        };
        let denominator = 10_i128.pow(self.scale) * unit;
        let numerator = i128::from(cents) * self.digits;
        let rounded = (numerator + numerator.signum() * denominator / 2) / denominator * unit;
        i64::try_from(rounded).unwrap_or(if rounded < 0 { i64::MIN } else { i64::MAX })
    }
}

impl std::fmt::Display for Rate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let digits = format!("{:0>width$}", self.digits, width = self.scale as usize + 1);
        let (units, fraction) = digits.split_at(digits.len() - self.scale as usize);
        if fraction.is_empty() {
            write!(f, "{units}.0")
        } else {
            write!(f, "{units}.{fraction}")
        }
    }
}

/// What a store presented to the buyers of a country is in: their currency, the rate from the
/// store currency, and the market whose fixed prices it has.
#[derive(Clone, Debug)]
pub struct Presentment {
    pub currency: String,
    pub rate: Rate,
    pub market: Option<String>,
}

/// The stores presented in other currencies, made once each from the store they convert.
#[derive(Default)]
pub struct PresentedStores(std::sync::Mutex<HashMap<String, std::sync::Arc<Store>>>);

impl Clone for PresentedStores {
    /// A copy of a store converts again what it needs.
    fn clone(&self) -> Self {
        PresentedStores::default()
    }
}

#[derive(Clone, Debug)]
pub struct Language {
    pub iso_code: String,
    pub name: String,
    pub endonym_name: String,
    /// `/` for the primary language, `/<iso_code>` for the others.
    pub root_url: String,
    pub primary: bool,
}

#[derive(Clone, Debug)]
pub struct FileMeta {
    pub alt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub focal_point: Option<(f64, f64)>,
}

#[derive(Clone, Debug)]
pub struct Metaobject {
    pub kind: String,
    pub handle: String,
    pub fields: IndexMap<String, Metafield>,
    /// The values of its fields in the store's other languages, by language (its `iso_code` in
    /// `localization.languages`) then field key. Only fields of a translatable type have one.
    pub translations: IndexMap<String, IndexMap<String, Json>>,
    pub template_suffix: Option<String>,
}

impl Metaobject {
    /// Whether the entry is of the type `kind` names: [`same_metaobject_type`].
    pub fn is_of(&self, kind: &str) -> bool {
        same_metaobject_type(&self.kind, kind)
    }

    /// A field as a language shows it: its translation into `locale` when the entry has one,
    /// else into the base language of a regional one (`fr` for `fr-CA`), else its own value
    /// (that of the primary language). Shopify falls back so, field by field
    /// (https://shopify.dev/docs/apps/build/markets).
    pub fn field_in(&self, key: &str, locale: &str) -> Option<Cow<'_, Metafield>> {
        let field = self.fields.get(key)?;
        let translation = |code: &str| {
            self.translations
                .iter()
                .find(|(language, _)| language.eq_ignore_ascii_case(code))
                .and_then(|(_, values)| values.get(key))
        };
        let base = locale.split_once('-').map(|(base, _)| base);
        let translated = translation(locale).or_else(|| base.and_then(translation));
        Some(match translated {
            Some(value) => Cow::Owned(Metafield {
                kind: field.kind.clone(),
                value: value.clone(),
            }),
            None => Cow::Borrowed(field),
        })
    }
}

/// Whether two metaobject types name the same one. An app's own types are `$app:<type>` in the
/// store data and in the app's code (its Function input queries, its theme app extension), and
/// `app--<app id>--<type>` where Shopify writes them in full (a theme's Liquid, the Admin API):
/// either names the other, whatever the id, which the store data does not know.
pub fn same_metaobject_type(a: &str, b: &str) -> bool {
    a == b || app_metaobject_type(a).is_some_and(|name| app_metaobject_type(b) == Some(name))
}

/// The name of an app's own metaobject type: `author` for `$app:author` and
/// `app--1234--author`.
fn app_metaobject_type(kind: &str) -> Option<&str> {
    if let Some(name) = kind.strip_prefix("$app:") {
        return (!name.is_empty()).then_some(name);
    }
    let (id, name) = kind.strip_prefix("app--")?.split_once("--")?;
    (!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()) && !name.is_empty())
        .then_some(name)
}

/// A cart line as stored in a session.
#[derive(Clone, Debug, PartialEq)]
pub struct CartLine {
    pub variant_id: u64,
    pub quantity: u32,
    pub properties: IndexMap<String, String>,
    /// The id of the selling plan the line is bought with.
    pub selling_plan: Option<u64>,
}

/// The state a session starts with.
#[derive(Clone, Debug, Default)]
pub struct SessionDefaults {
    pub customer_email: Option<String>,
    pub cart_lines: Vec<CartLine>,
    pub cart_note: String,
    pub cart_attributes: IndexMap<String, String>,
    pub country: Option<String>,
    /// The id of the company location a B2B customer starts with.
    pub company_location: Option<u64>,
}

/// A metafield of the checkout settings: its value is the string form (JSON for `json`).
#[derive(Clone, Debug, PartialEq)]
pub struct CheckoutMetafield {
    pub namespace: String,
    pub key: String,
    pub kind: String,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct CheckoutShippingRate {
    pub handle: String,
    pub title: String,
    pub price: i64,
    pub description: Option<String>,
    pub kind: model::CheckoutDeliveryKind,
}

#[derive(Clone, Debug)]
pub struct CheckoutPaymentMethod {
    pub handle: String,
    pub name: String,
    /// Never empty.
    pub placements: Vec<model::CheckoutPaymentPlacement>,
}

#[derive(Clone, Debug)]
pub struct CheckoutDiscount {
    /// The number of its GID: `gid://shopify/DiscountAutomaticNode/<id>`.
    pub id: u64,
    pub function: String,
    pub title: String,
    pub code: Option<String>,
    pub classes: Vec<model::CheckoutDiscountClass>,
    pub metafields: Vec<CheckoutMetafield>,
}

/// How the local checkout is set up, with the defaults applied.
#[derive(Clone, Debug)]
pub struct CheckoutSettings {
    pub shipping_rates: Vec<CheckoutShippingRate>,
    pub payment_methods: Vec<CheckoutPaymentMethod>,
    pub discounts: Vec<CheckoutDiscount>,
    /// Function handle → the metafields of its owner.
    pub functions: IndexMap<String, Vec<CheckoutMetafield>>,
    /// The Functions of `functions` the shop has an owner of (`"enabled"`), by handle: whether
    /// it is enabled. Only those that are run at the checkout, unless a scenario names them.
    pub owners: IndexMap<String, bool>,
    /// The Functions of `functions` whose owner says whether a failed run blocks the checkout
    /// (`"block_on_failure"`), by handle.
    pub block_on_failure: IndexMap<String, bool>,
    /// Basis points: 2000 is 20 %.
    pub tax_rate_bps: i64,
    /// UI extension handle → the values of its settings, by key (`shopify.settings`).
    pub extensions: IndexMap<String, IndexMap<String, Json>>,
}

impl Default for CheckoutSettings {
    fn default() -> Self {
        CheckoutSettings {
            shipping_rates: vec![CheckoutShippingRate {
                handle: "standard".to_string(),
                title: "Standard".to_string(),
                price: 500,
                description: None,
                kind: model::CheckoutDeliveryKind::Shipping,
            }],
            payment_methods: [
                ("card", "Credit card"),
                ("cod", "Cash on Delivery"),
                ("bank", "Bank Deposit"),
            ]
            .into_iter()
            .map(|(handle, name)| CheckoutPaymentMethod {
                handle: handle.to_string(),
                name: name.to_string(),
                placements: vec![model::CheckoutPaymentPlacement::PaymentMethod],
            })
            .collect(),
            discounts: Vec::new(),
            functions: IndexMap::new(),
            owners: IndexMap::new(),
            block_on_failure: IndexMap::new(),
            tax_rate_bps: 0,
            extensions: IndexMap::new(),
        }
    }
}

/// A named setup of the local checkout, read from `checkouts/<name>.json`. A scenario with
/// `problems` cannot be opened: the rest of its fields hold what could be read.
#[derive(Clone, Debug)]
pub struct CheckoutScenario {
    /// The name of the file without its extension.
    pub name: String,
    /// The file, relative to the data directory.
    pub file: String,
    /// What the file says, or the name.
    pub title: String,
    pub description: Option<String>,
    /// Who buys and what is in the cart, as written. Applied with
    /// [`build::apply_session_input`] on the session a checkout starts from.
    pub session: model::SessionInput,
    pub entered: model::ScenarioEnteredInput,
    /// The ISO code of a language of the store.
    pub language: Option<String>,
    pub look: Option<model::ScenarioLook>,
    /// The handles of the extensions that run, when the scenario names some.
    pub extensions: Option<Vec<String>>,
    /// The settings of the store with this scenario's overrides on top.
    pub checkout: CheckoutSettings,
    /// What the scenario overrides of the `checkout` section: `shipping_rates`,
    /// `payment_methods`, `discounts`, `functions`, `extensions`, `tax_rate`.
    pub overrides: Vec<&'static str>,
    /// What the checkout must look like once the Functions ran, which `lsf app check` checks.
    /// Its amounts are valid: those that are not are in `problems`.
    pub expect: Option<model::ScenarioExpectInput>,
    /// What stops the scenario from opening, as the diagnostics `validate` prints them.
    pub problems: Vec<String>,
}

#[derive(Clone)]
pub struct Store {
    pub shop: Shop,
    /// For a store presented to the buyers of a country in another currency than the shop's
    /// ([`Store::presented_for`]): that currency and how its prices were made. `None` for the
    /// store itself, in the shop's currency.
    pub presentment: Option<Presentment>,
    /// The products on the storefront: active and published.
    pub products: Vec<Product>,
    /// The products of the data that are not on the storefront: drafts, archived products and
    /// those not published on the Online Store. Neither the storefront nor the checkout sees
    /// them; the Admin API lists them.
    pub unlisted_products: Vec<Product>,
    pub collections: Vec<Collection>,
    pub pages: Vec<Page>,
    pub blogs: Vec<Blog>,
    pub menus: Vec<Menu>,
    pub customers: Vec<Customer>,
    pub companies: Vec<Company>,
    pub gift_cards: Vec<GiftCard>,
    /// The URL redirects, by their path: [`Store::redirect`].
    pub redirects: Vec<Redirect>,
    pub locations: Vec<Location>,
    pub selling_plan_groups: Vec<SellingPlanGroup>,
    pub metaobjects: Vec<Metaobject>,
    /// The handle of the web pages of each metaobject type whose definition sets one
    /// (`metaobject_definitions.<type>.url_handle`): [`Store::metaobject_url_handle`].
    pub metaobject_url_handles: HashMap<String, String>,
    pub countries: Vec<Country>,
    /// The markets the data declares; a country in none of them is a market of its own.
    pub markets: Vec<Market>,
    pub languages: Vec<Language>,
    pub files: IndexMap<String, FileMeta>,
    /// The pixel size of each image of `files/`, read from the file: what an image known only by
    /// its path (an `image_picker` setting, `images[...]`, a file metafield) measures.
    pub file_sizes: HashMap<String, (u32, u32)>,
    pub session_defaults: SessionDefaults,
    /// How the local checkout is set up.
    pub checkout: CheckoutSettings,
    /// The named setups of the checkout (`checkouts/*.json`), by name.
    pub scenarios: Vec<CheckoutScenario>,
    /// The frozen clock, if the data sets one.
    pub now: Option<DateTime<Utc>>,
    pub theme_settings: IndexMap<String, Json>,
    /// The app of `--app` as the store has it installed: its app-data metafields and its app
    /// embed blocks turned on or off.
    pub app: AppInstallation,

    product_handles: HashMap<String, usize>,
    product_ids: HashMap<u64, usize>,
    variant_ids: HashMap<u64, (usize, usize)>,
    collection_handles: HashMap<String, usize>,
    page_handles: HashMap<String, usize>,
    blog_handles: HashMap<String, usize>,
    menu_handles: HashMap<String, usize>,
    /// The declared pixel size of every image the data mentions, by path inside `files/`.
    image_sizes: HashMap<String, (u32, u32)>,
    presented: PresentedStores,
}

impl Store {
    /// The size of an image known to the store, used to draw a placeholder of the right
    /// proportions when the file itself does not exist.
    pub fn image_size(&self, src: &str) -> Option<(u32, u32)> {
        self.image_sizes.get(src).copied().or_else(|| {
            self.files
                .get(src)
                .and_then(|file| Some((file.width?, file.height?)))
        })
    }

    pub fn product_index(&self, handle: &str) -> Option<usize> {
        self.product_handles.get(handle).copied()
    }

    /// The product a URL names: its handle in any case, as a store answers
    /// `/products/TEE` with the product `tee`. Collection URLs keep their case.
    pub fn product_index_for_url(&self, handle: &str) -> Option<usize> {
        self.product_index(handle)
            .or_else(|| self.product_index(&handle.to_lowercase()))
    }

    pub fn product_by_handle(&self, handle: &str) -> Option<&Product> {
        self.product_index(handle)
            .map(|index| &self.products[index])
    }

    pub fn product_index_by_id(&self, id: u64) -> Option<usize> {
        self.product_ids.get(&id).copied()
    }

    /// `(product index, variant index)` of a variant id.
    pub fn variant_location(&self, id: u64) -> Option<(usize, usize)> {
        self.variant_ids.get(&id).copied()
    }

    pub fn variant(&self, id: u64) -> Option<(&Product, &Variant)> {
        let (product, variant) = self.variant_location(id)?;
        Some((
            &self.products[product],
            &self.products[product].variants[variant],
        ))
    }

    /// `(group index, plan index)` of a selling plan id.
    pub fn selling_plan_location(&self, id: u64) -> Option<(usize, usize)> {
        self.selling_plan_groups
            .iter()
            .enumerate()
            .find_map(|(group_index, group)| {
                let plan = group.plans.iter().position(|plan| plan.id == id)?;
                Some((group_index, plan))
            })
    }

    /// The selling plan with this id, when the product is sold with it.
    pub fn selling_plan_of(
        &self,
        product: &Product,
        id: u64,
    ) -> Option<(&SellingPlanGroup, &SellingPlan)> {
        let (group, plan) = self.selling_plan_location(id)?;
        product.selling_plan_groups.contains(&group).then(|| {
            let group = &self.selling_plan_groups[group];
            (group, &group.plans[plan])
        })
    }

    pub fn collection_index(&self, handle: &str) -> Option<usize> {
        self.collection_handles.get(handle).copied()
    }

    pub fn collection_by_handle(&self, handle: &str) -> Option<&Collection> {
        self.collection_index(handle)
            .map(|index| &self.collections[index])
    }

    pub fn page_index(&self, handle: &str) -> Option<usize> {
        self.page_handles.get(handle).copied()
    }

    pub fn blog_index(&self, handle: &str) -> Option<usize> {
        self.blog_handles.get(handle).copied()
    }

    pub fn article_index(&self, blog: usize, handle: &str) -> Option<usize> {
        self.blogs
            .get(blog)?
            .articles
            .iter()
            .position(|article| article.handle == handle)
    }

    pub fn menu(&self, handle: &str) -> Option<&Menu> {
        self.menu_handles
            .get(handle)
            .map(|index| &self.menus[*index])
    }

    pub fn customer_by_email(&self, email: &str) -> Option<&Customer> {
        self.customers
            .iter()
            .find(|customer| customer.email.eq_ignore_ascii_case(email))
    }

    /// Who a session is logged in as, from the way the data, a flag or a test names them:
    /// the email of a customer, `default` (the first customer) or `none`.
    pub fn customer_named(&self, who: &str) -> std::result::Result<Option<&Customer>, String> {
        match who.trim() {
            "" | "none" => Ok(None),
            "default" => Ok(self.customers.first()),
            email => self
                .customer_by_email(email)
                .map(Some)
                .ok_or_else(|| format!("there is no customer with the email \"{email}\"")),
        }
    }

    pub fn customer_by_id(&self, id: u64) -> Option<&Customer> {
        self.customers.iter().find(|customer| customer.id == id)
    }

    pub fn policy(&self, handle: &str) -> Option<&Policy> {
        self.shop
            .policies
            .iter()
            .find(|policy| policy.handle == handle)
    }

    pub fn primary_language(&self) -> &Language {
        self.languages
            .iter()
            .find(|language| language.primary)
            .unwrap_or(&self.languages[0])
    }

    pub fn language(&self, iso_code: &str) -> Option<&Language> {
        self.languages
            .iter()
            .find(|language| language.iso_code.eq_ignore_ascii_case(iso_code))
    }

    /// The checkout scenario named so, by the name of its file.
    pub fn scenario(&self, name: &str) -> Option<&CheckoutScenario> {
        self.scenarios.iter().find(|scenario| scenario.name == name)
    }

    /// The country of a visitor who selected none, on the storefront and, until they enter an
    /// address, at the checkout: the shop's (`shop.address.country_code`), the country of its
    /// primary market, when the store sells to it; otherwise the first country served at `/`
    /// (in no market with a subfolder), else the first country.
    pub fn default_country(&self) -> &Country {
        self.country(&self.shop.address.country_code)
            .or_else(|| {
                self.countries
                    .iter()
                    .find(|country| self.served_at_root(&country.iso_code))
            })
            .unwrap_or(&self.countries[0])
    }

    /// The currency prices are in: the buyer's, for a store presented to a country in another
    /// currency ([`Store::presented_for`]), else the shop's.
    pub fn currency(&self) -> &str {
        self.presentment
            .as_ref()
            .map_or(&self.shop.currency, |presentment| &presentment.currency)
    }

    /// What the buyers of a country pay in, when it is not the store as it is: their currency,
    /// the rate of their market and its fixed prices.
    fn presentment_of(&self, country: &str) -> Option<Presentment> {
        let currency = self.country(country)?.currency.to_ascii_uppercase();
        let market = self.market_of(country);
        let rate = market
            .and_then(|market| market.rate.clone())
            .unwrap_or(Rate::ONE);
        let fixed = market.is_some_and(|market| !market.prices.is_empty());
        (currency != self.shop.currency.to_ascii_uppercase() || !rate.is_one() || fixed).then(
            || Presentment {
                currency,
                rate,
                market: market
                    .filter(|market| !market.prices.is_empty() || market.rate.is_some())
                    .map(|market| market.handle.clone()),
            },
        )
    }

    /// The store as the buyers of a country see it: priced in their currency, at their
    /// market's rate or at its fixed prices, as Shopify converts a storefront for a market
    /// ("About catalogs in Markets"). The store itself when they pay in the shop's currency
    /// at its prices. Each conversion is made once, and shared by the countries that have it.
    pub fn presented_for(self: &std::sync::Arc<Self>, country: &str) -> std::sync::Arc<Store> {
        let Some(presentment) = self
            .presentment
            .is_none()
            .then(|| self.presentment_of(country))
            .flatten()
        else {
            return self.clone();
        };
        let key = format!(
            "{}|{}|{}",
            presentment.currency,
            presentment.rate,
            presentment.market.as_deref().unwrap_or_default()
        );
        let mut presented = self.presented.0.lock().unwrap_or_else(|e| e.into_inner());
        presented
            .entry(key)
            .or_insert_with(|| std::sync::Arc::new(self.converted(presentment)))
            .clone()
    }

    /// A copy of the store with its prices in another currency.
    fn converted(&self, presentment: Presentment) -> Store {
        let mut store = self.clone();
        let rate = &presentment.rate;
        let currency = presentment.currency.as_str();
        let convert = |cents: i64| rate.convert(cents, currency);
        let fixed = presentment
            .market
            .as_deref()
            .and_then(|handle| self.market_named(handle))
            .map(|market| market.prices.clone())
            .unwrap_or_default();
        for product in store
            .products
            .iter_mut()
            .chain(store.unlisted_products.iter_mut())
        {
            for variant in &mut product.variants {
                let original = variant.price;
                match fixed.get(&variant.id) {
                    Some(price) => {
                        variant.price = price.price;
                        variant.compare_at_price = price.compare_at_price;
                    }
                    None => {
                        variant.price = convert(variant.price);
                        variant.compare_at_price = variant.compare_at_price.map(convert);
                    }
                }
                // A unit price follows the price it divides.
                variant.unit_price = variant.unit_price.map(|unit| match original {
                    0 => convert(unit),
                    _ => {
                        (i128::from(unit) * i128::from(variant.price) / i128::from(original)) as i64
                    }
                });
                for (_, price) in &mut variant.quantity_price_breaks {
                    *price = convert(*price);
                }
            }
        }
        for group in &mut store.selling_plan_groups {
            for plan in &mut group.plans {
                for adjustment in &mut plan.price_adjustments {
                    adjustment.adjustment = match adjustment.adjustment {
                        Adjustment::FixedAmount(amount) => Adjustment::FixedAmount(convert(amount)),
                        Adjustment::Price(amount) => Adjustment::Price(convert(amount)),
                        percentage => percentage,
                    };
                }
                if let CheckoutCharge::Price(amount) = plan.checkout_charge {
                    plan.checkout_charge = CheckoutCharge::Price(convert(amount));
                }
            }
        }
        for rate in &mut store.checkout.shipping_rates {
            rate.price = convert(rate.price);
        }
        store.presentment = Some(presentment);
        store
    }

    /// The market with this handle.
    pub fn market_named(&self, handle: &str) -> Option<&Market> {
        self.markets.iter().find(|market| market.handle == handle)
    }

    /// The market served under the subfolders of this suffix (`ca` for `/en-ca/`).
    pub fn subfolder_market(&self, suffix: &str) -> Option<&Market> {
        self.markets.iter().find(|market| {
            market
                .subfolder_suffix
                .as_deref()
                .is_some_and(|own| own.eq_ignore_ascii_case(suffix))
        })
    }

    /// Whether a country is served at `/`: its market has no subfolder of its own.
    pub fn served_at_root(&self, iso_code: &str) -> bool {
        self.market_of(iso_code)
            .is_none_or(|market| market.subfolder_suffix.is_none())
    }

    /// The country of a visitor on a web presence: under the subfolders of `market`, or at `/`
    /// (`None`). The one they selected when that presence serves it, else the presence's
    /// default: the first country of the market, or [`Store::default_country`] at `/`. A
    /// Shopify storefront does the same: the country a visitor picked elsewhere does not follow
    /// them to a presence that does not serve it.
    pub fn country_at(&self, selected: Option<&str>, market: Option<&Market>) -> &Country {
        let selected = selected.and_then(|code| self.country(code));
        match market {
            Some(market) => selected
                .filter(|country| {
                    market
                        .countries
                        .iter()
                        .any(|code| code.eq_ignore_ascii_case(&country.iso_code))
                })
                .or_else(|| market.countries.iter().find_map(|code| self.country(code)))
                .unwrap_or_else(|| self.default_country()),
            None => selected
                .filter(|country| self.served_at_root(&country.iso_code))
                .unwrap_or_else(|| self.default_country()),
        }
    }

    /// The prefix of every storefront URL in a language on a web presence: `/fr-ca` under the
    /// subfolders of a market (every language has one there, the primary too), and at `/` empty
    /// for the primary language, `/fr` for the others.
    /// A language the market's subfolders do not serve has its root at `/`.
    pub fn locale_root(&self, language: &Language, market: Option<&Market>) -> String {
        let market = market.filter(|market| self.serves_language(market, language));
        match market.and_then(|market| market.subfolder_suffix.as_deref()) {
            Some(suffix) => format!("/{}-{suffix}", language.iso_code.to_ascii_lowercase()),
            None => language.root_url.trim_end_matches('/').to_string(),
        }
    }

    /// The web presence a URL prefix (`/fr`, `/en-ca`, empty) is: the market of its
    /// subfolder, or `None` at `/`.
    pub fn market_of_root(&self, root: &str) -> Option<&Market> {
        let segment = root.trim_start_matches('/');
        let (_, suffix) = segment.rsplit_once('-')?;
        self.subfolder_market(suffix)
            .filter(|market| self.locale_of_root(segment, market).is_some())
    }

    /// The language of a subfolder of a market: `en` for `en-ca`.
    fn locale_of_root(&self, segment: &str, market: &Market) -> Option<&Language> {
        let suffix = market.subfolder_suffix.as_deref()?;
        let language = segment
            .len()
            .checked_sub(suffix.len() + 1)
            .and_then(|end| segment.get(..end))
            .filter(|_| segment[segment.len() - suffix.len()..].eq_ignore_ascii_case(suffix))?;
        self.language(language)
            .filter(|language| self.serves_language(market, language))
    }

    /// Whether a market's subfolders have a language: one of its `languages`, or any language
    /// of the store when it lists none. On Shopify, a subfolder in a language the market does
    /// not offer (`/es-be` for Belgium in English and French) is not found.
    pub fn serves_language(&self, market: &Market, language: &Language) -> bool {
        market.languages.is_empty()
            || market
                .languages
                .iter()
                .any(|code| code.eq_ignore_ascii_case(&language.iso_code))
    }

    /// The languages a web presence serves: those of the market whose subfolders it is (every
    /// language of the store when it lists none), or every language at `/`.
    pub fn presence_languages(&self, market: Option<&Market>) -> Vec<&Language> {
        self.languages
            .iter()
            .filter(|language| market.is_none_or(|market| self.serves_language(market, language)))
            .collect()
    }

    /// The language a visitor in a country reads: `wanted` when the country's market offers
    /// it, else the market's default, as Shopify moves a visitor to a country whose market does
    /// not offer their language ("If the currently selected language is not supported by the
    /// selected country, it will be updated to the default language for that country").
    pub fn language_for<'s>(&'s self, wanted: &'s Language, country: &str) -> &'s Language {
        if self
            .languages_of(country)
            .iter()
            .any(|offered| offered.iso_code == wanted.iso_code)
        {
            wanted
        } else {
            self.default_language_of(country)
        }
    }

    /// Splits the locale prefix off a storefront path: `/fr/cart` → (`fr`, `/fr`, `/cart`),
    /// `/fr-ca/cart` → (`fr`, `/fr-ca`, `/cart`) for a market served under `ca`, `/cart` →
    /// (the primary language, empty, `/cart`).
    pub fn split_locale(&self, path: &str) -> (&Language, String, String) {
        let trimmed = path.trim_start_matches('/');
        let (first, rest) = trimmed.split_once('/').unwrap_or((trimmed, ""));
        if let Some(language) = self
            .languages
            .iter()
            .find(|language| !language.primary && language.iso_code.eq_ignore_ascii_case(first))
        {
            return (language, language.root_url.clone(), format!("/{rest}"));
        }
        if let Some((_, suffix)) = first.rsplit_once('-')
            && let Some(market) = self.subfolder_market(suffix)
            && let Some(language) = self.locale_of_root(first, market)
        {
            return (
                language,
                self.locale_root(language, Some(market)),
                format!("/{rest}"),
            );
        }
        (self.primary_language(), String::new(), path.to_string())
    }

    pub fn country(&self, iso_code: &str) -> Option<&Country> {
        self.countries
            .iter()
            .find(|country| country.iso_code.eq_ignore_ascii_case(iso_code))
    }

    /// The market the data groups a country in, if it does.
    pub fn market_of(&self, iso_code: &str) -> Option<&Market> {
        self.markets.iter().find(|market| {
            market
                .countries
                .iter()
                .any(|country| country.eq_ignore_ascii_case(iso_code))
        })
    }

    /// The market that sells to a country, as Shopify names it: the one the data groups it in,
    /// else a market of its own, named after the country (`france`). The storefront
    /// (`localization.market`, `country.market`), the Functions and the checkout UI extensions
    /// read the same, and its `id` is the one of the Admin API (`gid://shopify/Market/<id>`).
    pub fn market_for(&self, iso_code: &str) -> CountryMarket<'_> {
        let declared = self.market_of(iso_code);
        let code = iso_code.to_ascii_uppercase();
        let id = crate::util::stable_id("market", declared.map_or(&code, |m| m.handle.as_str()));
        let handle = match (declared, self.country(&code)) {
            (Some(market), _) => market.handle.clone(),
            (None, Some(country)) => crate::util::handleize(&country.name),
            (None, None) => code.to_ascii_lowercase(),
        };
        CountryMarket {
            id,
            handle,
            declared,
        }
    }

    /// The languages a country is offered (`country.available_languages`): those of its market,
    /// in the market's order (its default first), when the market lists some; otherwise every
    /// language of the store, the primary first.
    pub fn languages_of(&self, iso_code: &str) -> Vec<&Language> {
        match self
            .market_of(iso_code)
            .filter(|market| !market.languages.is_empty())
        {
            Some(market) => market
                .languages
                .iter()
                .filter_map(|code| self.language(code))
                .collect(),
            None => self.languages.iter().collect(),
        }
    }

    /// The language a country defaults to: the first its market lists, else the store's
    /// primary language.
    pub fn default_language_of(&self, iso_code: &str) -> &Language {
        self.market_of(iso_code)
            .and_then(|market| market.languages.first())
            .and_then(|code| self.language(code))
            .unwrap_or_else(|| self.primary_language())
    }

    /// The entry of a type (an app's in either spelling: [`same_metaobject_type`]) and a handle.
    /// The handle of the web pages of a metaobject type: its definition's `url_handle`, else
    /// the type. Shopify serves the entries at `/pages/<url handle>/<handle>`.
    /// <https://shopify.dev/docs/apps/build/metaobjects/use-metaobject-capabilities#online-store>
    pub fn metaobject_url_handle<'a>(&'a self, kind: &'a str) -> &'a str {
        self.metaobject_url_handles
            .get(kind)
            .map_or(kind, String::as_str)
    }

    /// The path of the web page of a metaobject entry: `/pages/<url handle>/<handle>`.
    pub fn metaobject_path(&self, entry: &Metaobject) -> String {
        format!(
            "/pages/{}/{}",
            self.metaobject_url_handle(&entry.kind),
            entry.handle
        )
    }

    /// The URL redirect of a path, which matches it whatever its case, as on Shopify.
    /// <https://help.shopify.com/en/manual/online-store/menus-and-links/url-redirect>
    pub fn redirect(&self, path: &str) -> Option<&Redirect> {
        self.redirects
            .iter()
            .find(|redirect| redirect.path.eq_ignore_ascii_case(path))
    }

    pub fn metaobject(&self, kind: &str, handle: &str) -> Option<&Metaobject> {
        self.metaobjects
            .iter()
            .find(|entry| entry.is_of(kind) && entry.handle == handle)
    }

    /// The metaobject a setting value points to: `<type>/<handle>`, `<type>.<handle>`, a
    /// `gid://shopify/Metaobject/<id>`, or a bare handle of the `kind` the setting picks from.
    pub fn metaobject_by_reference(
        &self,
        reference: &str,
        kind: Option<&str>,
    ) -> Option<&Metaobject> {
        let reference = reference.trim();
        if let Some(id) = reference.strip_prefix("gid://shopify/Metaobject/") {
            let id = id.parse::<u64>().ok()?;
            return self.metaobjects.iter().find(|entry| entry.id() == id);
        }
        if let Some(entry) = kind.and_then(|kind| self.metaobject(kind, reference)) {
            return Some(entry);
        }
        let (kind, handle) = reference
            .split_once('/')
            .or_else(|| reference.split_once('.'))?;
        self.metaobject(kind, handle)
    }

    pub(crate) fn index(&mut self) {
        self.product_handles = self
            .products
            .iter()
            .enumerate()
            .map(|(i, p)| (p.handle.clone(), i))
            .collect();
        self.product_ids = self
            .products
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id, i))
            .collect();
        self.variant_ids = self
            .products
            .iter()
            .enumerate()
            .flat_map(|(i, product)| {
                product
                    .variants
                    .iter()
                    .enumerate()
                    .map(move |(j, variant)| (variant.id, (i, j)))
            })
            .collect();
        self.collection_handles = self
            .collections
            .iter()
            .enumerate()
            .map(|(i, c)| (c.handle.clone(), i))
            .collect();
        self.page_handles = self
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.handle.clone(), i))
            .collect();
        self.blog_handles = self
            .blogs
            .iter()
            .enumerate()
            .map(|(i, b)| (b.handle.clone(), i))
            .collect();
        self.menu_handles = self
            .menus
            .iter()
            .enumerate()
            .map(|(i, m)| (m.handle.clone(), i))
            .collect();

        let mut sizes = HashMap::new();
        let mut remember = |image: &Image| {
            sizes.insert(image.src.clone(), (image.width, image.height));
        };
        for product in &self.products {
            product
                .media
                .iter()
                .filter_map(|media| media.preview.as_ref())
                .for_each(&mut remember);
            product
                .options
                .iter()
                .flat_map(|option| option.swatches.iter().flatten())
                .filter_map(|swatch| swatch.image.as_ref())
                .for_each(&mut remember);
        }
        self.collections
            .iter()
            .filter_map(|collection| collection.image.as_ref())
            .for_each(&mut remember);
        for blog in &self.blogs {
            blog.articles
                .iter()
                .filter_map(|article| article.image.as_ref())
                .for_each(&mut remember);
        }
        if let Some(brand) = &self.shop.brand {
            [
                &brand.logo,
                &brand.square_logo,
                &brand.cover_image,
                &brand.favicon,
            ]
            .into_iter()
            .flatten()
            .for_each(&mut remember);
        }
        self.image_sizes = sizes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_regional_language_falls_back_to_its_base_language_then_the_field() {
        // https://shopify.dev/docs/apps/build/markets: content in "Dutch (Belgium)" falls back
        // to base Dutch, then to the shop's primary language.
        let text = |value: &str| Metafield {
            kind: "single_line_text_field".to_string(),
            value: Json::from(value),
        };
        let entry = Metaobject {
            kind: "faq".to_string(),
            handle: "shipping".to_string(),
            fields: [
                ("question".to_string(), text("How long?")),
                ("answer".to_string(), text("Two days.")),
            ]
            .into_iter()
            .collect(),
            translations: [
                (
                    "fr".to_string(),
                    [
                        ("question".to_string(), Json::from("Combien de temps ?")),
                        ("answer".to_string(), Json::from("Deux jours.")),
                    ]
                    .into_iter()
                    .collect(),
                ),
                (
                    "fr-CA".to_string(),
                    [("answer".to_string(), Json::from("Deux jours ouvrables."))]
                        .into_iter()
                        .collect(),
                ),
            ]
            .into_iter()
            .collect(),
            template_suffix: None,
        };
        let value = |key: &str, locale: &str| {
            entry
                .field_in(key, locale)
                .and_then(|field| field.value.as_str().map(str::to_string))
        };
        assert_eq!(
            value("answer", "fr-CA").as_deref(),
            Some("Deux jours ouvrables.")
        );
        assert_eq!(
            value("question", "fr-ca").as_deref(),
            Some("Combien de temps ?")
        );
        assert_eq!(value("answer", "fr").as_deref(), Some("Deux jours."));
        assert_eq!(value("question", "de-CH").as_deref(), Some("How long?"));
        assert_eq!(value("question", "en").as_deref(), Some("How long?"));
    }

    #[test]
    fn an_app_metaobject_type_is_named_either_way() {
        assert!(same_metaobject_type("$app:author", "app--1234--author"));
        assert!(same_metaobject_type("app--99--author", "$app:author"));
        assert!(same_metaobject_type("app--1--author", "app--2--author"));
        assert!(same_metaobject_type("designer", "designer"));
        assert!(!same_metaobject_type("$app:author", "author"));
        assert!(!same_metaobject_type("$app:author", "app--x1--author"));
        assert!(!same_metaobject_type("$app:author", "$app:book"));
        assert!(!same_metaobject_type("app--12", "$app:"));
    }

    fn variant(price: i64, compare_at_price: Option<i64>, unit_price: Option<i64>) -> Variant {
        Variant {
            id: 1,
            title: "Default Title".to_string(),
            options: vec!["Default Title".to_string()],
            price,
            compare_at_price,
            sku: String::new(),
            barcode: String::new(),
            available: true,
            inventory_quantity: 0,
            inventory_tracked: false,
            inventory_policy: InventoryPolicy::Deny,
            weight: 0,
            weight_unit: "kg".to_string(),
            requires_shipping: true,
            taxable: true,
            media_index: None,
            unit_price,
            unit_price_measurement: None,
            quantity_rule: QuantityRule {
                min: 1,
                max: None,
                increment: 1,
            },
            quantity_price_breaks: Vec::new(),
            store_availabilities: Vec::new(),
            components: Vec::new(),
            metafields: Metafields::new(),
        }
    }

    fn plan(adjustments: &[Adjustment], checkout_charge: CheckoutCharge) -> SellingPlan {
        SellingPlan {
            id: 1,
            name: "Plan".to_string(),
            description: None,
            options: Vec::new(),
            price_adjustments: adjustments
                .iter()
                .map(|adjustment| PriceAdjustment {
                    adjustment: *adjustment,
                    order_count: None,
                })
                .collect(),
            recurring_deliveries: true,
            checkout_charge,
        }
    }

    #[test]
    fn selling_plans_adjust_the_price() {
        let full = CheckoutCharge::Percentage(100);
        // 10% off 19.99 is 17.991, rounded to the cent.
        let allocation =
            plan(&[Adjustment::Percentage(10)], full).allocation(&variant(1999, None, Some(400)));
        assert_eq!(allocation.price, 1799);
        assert_eq!(allocation.compare_at_price, Some(1999));
        assert_eq!(allocation.per_delivery_price, 1799);
        assert_eq!(allocation.checkout_charge_amount, 1799);
        assert_eq!(allocation.remaining_balance_charge_amount, 0);
        assert_eq!(allocation.unit_price, Some(360));
        assert_eq!(allocation.adjusted_prices, vec![1799]);

        // The first adjustment is the price; the second one is what later orders cost.
        let allocation = plan(
            &[Adjustment::FixedAmount(500), Adjustment::Price(1700)],
            full,
        )
        .allocation(&variant(2000, None, None));
        assert_eq!(allocation.price, 1500);
        assert_eq!(allocation.adjusted_prices, vec![1500, 1700]);
        // An amount off never goes below zero.
        assert_eq!(
            plan(&[Adjustment::FixedAmount(5000)], full)
                .allocation(&variant(2000, None, None))
                .price,
            0
        );
    }

    #[test]
    fn a_plan_without_adjustment_keeps_the_price() {
        let plan = plan(&[], CheckoutCharge::Percentage(25));
        let allocation = plan.allocation(&variant(2000, Some(2500), None));
        assert_eq!(allocation.price, 2000);
        // Nothing is taken off: the compare-at price is the variant's own.
        assert_eq!(allocation.compare_at_price, Some(2500));
        assert!(allocation.adjusted_prices.is_empty());
        // A deposit: a quarter now, the rest later.
        assert_eq!(allocation.checkout_charge_amount, 500);
        assert_eq!(allocation.remaining_balance_charge_amount, 1500);
        assert_eq!(
            plan.allocation(&variant(2000, None, None)).compare_at_price,
            None
        );
    }
}
