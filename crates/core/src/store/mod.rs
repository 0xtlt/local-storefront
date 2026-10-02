//! The store: products, collections, content and customers, resolved from the data directory
//! into a form that is cheap to query while rendering.

pub mod build;
pub mod demo;
pub mod docs;
pub mod load;
pub mod model;
pub mod reference;
pub mod validate;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use indexmap::IndexMap;
use serde_json::Value as Json;

pub use model::{InventoryPolicy, MediaType, SortOrder, VideoHost};

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

#[derive(Clone, Debug)]
pub struct ProductOption {
    pub name: String,
    pub position: usize,
    pub values: Vec<String>,
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
    pub metafields: Metafields,
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
}

impl Product {
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
}

impl Menu {
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
}

#[derive(Clone, Debug)]
pub struct CompanyLocation {
    pub id: u64,
    pub name: String,
    pub external_id: Option<String>,
    pub shipping_address: Option<Address>,
    pub tax_registration_id: Option<String>,
    pub metafields: Metafields,
}

#[derive(Clone, Debug)]
pub struct Company {
    pub id: u64,
    pub name: String,
    pub external_id: Option<String>,
    pub locations: Vec<CompanyLocation>,
    pub metafields: Metafields,
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
    pub password: Option<String>,
    pub metafields: Metafields,
    /// The company the customer buys for, as an index into the store's companies.
    pub company: Option<usize>,
    /// The locations of that company the customer can buy for, as indexes into its locations.
    pub company_locations: Vec<usize>,
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
    /// Which customer accounts the store uses; decided by the theme when not set.
    pub customer_accounts: Option<model::CustomerAccounts>,
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
}

/// A cart line as stored in a session.
#[derive(Clone, Debug, PartialEq)]
pub struct CartLine {
    pub variant_id: u64,
    pub quantity: u32,
    pub properties: IndexMap<String, String>,
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

pub struct Store {
    pub shop: Shop,
    pub products: Vec<Product>,
    pub collections: Vec<Collection>,
    pub pages: Vec<Page>,
    pub blogs: Vec<Blog>,
    pub menus: Vec<Menu>,
    pub customers: Vec<Customer>,
    pub companies: Vec<Company>,
    pub gift_cards: Vec<GiftCard>,
    pub metaobjects: Vec<Metaobject>,
    pub countries: Vec<Country>,
    pub languages: Vec<Language>,
    pub files: IndexMap<String, FileMeta>,
    pub session_defaults: SessionDefaults,
    /// The frozen clock, if the data sets one.
    pub now: Option<DateTime<Utc>>,
    pub theme_settings: IndexMap<String, Json>,

    product_handles: HashMap<String, usize>,
    product_ids: HashMap<u64, usize>,
    variant_ids: HashMap<u64, (usize, usize)>,
    collection_handles: HashMap<String, usize>,
    page_handles: HashMap<String, usize>,
    blog_handles: HashMap<String, usize>,
    menu_handles: HashMap<String, usize>,
    /// The declared pixel size of every image the data mentions, by path inside `files/`.
    image_sizes: HashMap<String, (u32, u32)>,
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

    pub fn country(&self, iso_code: &str) -> Option<&Country> {
        self.countries
            .iter()
            .find(|country| country.iso_code.eq_ignore_ascii_case(iso_code))
    }

    pub fn metaobject(&self, kind: &str, handle: &str) -> Option<&Metaobject> {
        self.metaobjects
            .iter()
            .find(|entry| entry.kind == kind && entry.handle == handle)
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
