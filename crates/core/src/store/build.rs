//! Turns the data as written (`model`) into the resolved [`Store`]: fills in defaults, links
//! entities together and reports everything that does not add up.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, TimeZone, Utc};
use chrono_tz::Tz;
use indexmap::IndexMap;
use serde_json::Value as Json;

use super::model::{
    self, ImageInput, MediaInput, MetafieldInput, Money, RuleColumn, RuleRelation, VariantRef,
};
use super::reference;
use super::*;
use crate::diagnostics::Diagnostics;
use crate::util::{closest_match, handleize, humanize, stable_id};

/// Where a piece of data was written, for diagnostics.
#[derive(Clone, Debug, Default)]
pub struct Origin {
    pub file: String,
    /// JSON pointer inside the file, e.g. `/products/2`.
    pub pointer: String,
}

impl Origin {
    pub fn new(file: impl Into<String>, pointer: impl Into<String>) -> Self {
        Origin {
            file: file.into(),
            pointer: pointer.into(),
        }
    }

    pub fn child(&self, segment: impl std::fmt::Display) -> Origin {
        Origin {
            file: self.file.clone(),
            pointer: format!("{}/{segment}", self.pointer),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sourced<T> {
    pub value: T,
    pub origin: Origin,
}

/// Every data file merged together, each entity remembering where it came from.
#[derive(Default)]
pub struct MergedInput {
    pub shop: Option<Sourced<model::ShopInput>>,
    pub products: Vec<Sourced<model::ProductInput>>,
    pub collections: Vec<Sourced<model::CollectionInput>>,
    pub pages: Vec<Sourced<model::PageInput>>,
    pub blogs: Vec<Sourced<model::BlogInput>>,
    pub menus: IndexMap<String, Sourced<model::MenuInput>>,
    pub customers: Vec<Sourced<model::CustomerInput>>,
    pub gift_cards: Vec<Sourced<model::GiftCardInput>>,
    pub metaobjects: Vec<(String, Sourced<model::MetaobjectInput>)>,
    pub localization: Option<Sourced<model::LocalizationInput>>,
    pub files: IndexMap<String, model::FileInput>,
    pub session: Option<Sourced<model::SessionInput>>,
    pub now: Option<Sourced<String>>,
    pub theme_settings: IndexMap<String, Json>,
}

pub struct BuildOptions<'a> {
    /// The locales the theme has translations for, default first.
    pub theme_locales: Vec<String>,
    /// Reads the pixel size of a file in `files/`, when it exists.
    pub probe_image: &'a dyn Fn(&str) -> Option<(u32, u32)>,
}

/// The instant entities without dates are considered created. Fixed so that renders are
/// reproducible.
fn epoch() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0)
        .single()
        .expect("valid date")
}

struct Builder<'a> {
    options: &'a BuildOptions<'a>,
    diagnostics: Diagnostics,
    timezone: Tz,
    files: IndexMap<String, FileMeta>,
}

impl Builder<'_> {
    fn error(
        &mut self,
        code: &'static str,
        origin: &Origin,
        message: impl Into<String>,
    ) -> &mut crate::diagnostics::Diagnostic {
        self.diagnostics
            .error(code, &origin.file, &origin.pointer, message)
    }

    fn money(&mut self, money: &Money, origin: &Origin) -> i64 {
        match money.cents() {
            Some(cents) => cents,
            None => {
                self.error(
                    "invalid_money",
                    origin,
                    format!("{money:?} is not a valid amount"),
                )
                .hint("use an integer number of cents (1999) or a decimal string (\"19.99\")");
                0
            }
        }
    }

    fn date(
        &mut self,
        input: &Option<String>,
        origin: &Origin,
        default: DateTime<Utc>,
    ) -> DateTime<Utc> {
        let Some(text) = input else {
            return default;
        };
        match slt_liquid::time::parse_time(&text.to_lowercase(), self.timezone, epoch()) {
            Some(date) => date.with_timezone(&Utc),
            None => {
                self.error("invalid_date", origin, format!("\"{text}\" is not a date"))
                    .hint("use ISO 8601, e.g. \"2024-05-01\" or \"2024-05-01T10:00:00Z\"");
                default
            }
        }
    }

    fn image(&mut self, input: &ImageInput, scope: &str) -> Image {
        let detail = input.detail();
        let meta = self.files.get(&detail.src).cloned();
        let probed = (self.options.probe_image)(&detail.src);
        let width = detail
            .width
            .or(probed.map(|(w, _)| w))
            .or(meta.as_ref().and_then(|m| m.width))
            .unwrap_or(1200);
        let height = detail
            .height
            .or(probed.map(|(_, h)| h))
            .or(meta.as_ref().and_then(|m| m.height))
            .unwrap_or(1200);
        Image {
            id: detail
                .id
                .unwrap_or_else(|| stable_id("image", &format!("{scope}/{}", detail.src))),
            alt: detail
                .alt
                .or(meta.as_ref().map(|m| m.alt.clone()))
                .unwrap_or_default(),
            focal_point: detail
                .focal_point
                .map(|point| (point.x, point.y))
                .or(meta.and_then(|m| m.focal_point)),
            src: detail.src,
            width,
            height,
        }
    }

    fn metafields(&mut self, input: &model::Metafields) -> Metafields {
        input
            .iter()
            .map(|(namespace, fields)| {
                (
                    namespace.clone(),
                    fields
                        .iter()
                        .map(|(key, field)| (key.clone(), metafield(field)))
                        .collect(),
                )
            })
            .collect()
    }

    fn handle(
        &mut self,
        kind: &str,
        handle: &Option<String>,
        title: &str,
        origin: &Origin,
        seen: &mut HashSet<String>,
    ) -> String {
        let handle = match handle {
            Some(handle) => {
                if handle.is_empty() || handleize(handle) != *handle {
                    self.error(
                        "invalid_handle",
                        &origin.child("handle"),
                        format!("\"{handle}\" is not a valid handle"),
                    )
                    .hint(format!(
                        "handles are lowercase letters, digits and hyphens, e.g. \"{}\"",
                        handleize(if handle.is_empty() { title } else { handle })
                    ));
                }
                handle.clone()
            }
            None => handleize(title),
        };
        if handle.is_empty() {
            self.error(
                "invalid_handle",
                origin,
                format!("cannot derive a handle for this {kind} from its title \"{title}\""),
            )
            .hint("add an explicit \"handle\"");
        } else if !seen.insert(handle.clone()) {
            self.error(
                "duplicate_handle",
                origin,
                format!("another {kind} already uses the handle \"{handle}\""),
            )
            .hint("handles must be unique: change the title or set a different \"handle\"");
        }
        handle
    }
}

fn metafield(input: &MetafieldInput) -> Metafield {
    match input {
        MetafieldInput::Typed(typed) => Metafield {
            kind: typed.kind.clone(),
            value: typed.value.clone(),
        },
        MetafieldInput::Value(value) => Metafield {
            kind: match value {
                Json::String(text) if text.contains('\n') => "multi_line_text_field",
                Json::String(_) => "single_line_text_field",
                Json::Number(number) if number.is_i64() || number.is_u64() => "number_integer",
                Json::Number(_) => "number_decimal",
                Json::Bool(_) => "boolean",
                _ => "json",
            }
            .to_string(),
            value: value.clone(),
        },
    }
}

fn address(input: &model::AddressInput, scope: &str) -> Address {
    let country_code = input
        .country_code
        .clone()
        .or_else(|| {
            input
                .country
                .as_deref()
                .and_then(reference::country_code_from_name)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let country = input
        .country
        .clone()
        .or_else(|| reference::country_name(&country_code).map(str::to_string))
        .unwrap_or_default();
    Address {
        id: input.id.unwrap_or_else(|| stable_id("address", scope)),
        first_name: input.first_name.clone().unwrap_or_default(),
        last_name: input.last_name.clone().unwrap_or_default(),
        company: input.company.clone().unwrap_or_default(),
        address1: input.address1.clone().unwrap_or_default(),
        address2: input.address2.clone().unwrap_or_default(),
        city: input.city.clone().unwrap_or_default(),
        province: input.province.clone().unwrap_or_default(),
        province_code: input.province_code.clone().unwrap_or_default(),
        country,
        country_code,
        zip: input.zip.clone().unwrap_or_default(),
        phone: input.phone.clone().unwrap_or_default(),
    }
}

/// Builds the store. The store is always returned, so that the server can keep running and show
/// the problems; callers decide what to do when the diagnostics contain errors.
pub fn build(input: MergedInput, options: &BuildOptions<'_>) -> (Store, Diagnostics) {
    let shop_origin = input
        .shop
        .as_ref()
        .map(|shop| shop.origin.clone())
        .unwrap_or_default();
    let shop_input = input.shop.map(|shop| shop.value).unwrap_or_default();

    let mut builder = Builder {
        options,
        diagnostics: Diagnostics::new(),
        timezone: Tz::UTC,
        files: input
            .files
            .iter()
            .map(|(path, file)| {
                (
                    path.clone(),
                    FileMeta {
                        alt: file.alt.clone().unwrap_or_default(),
                        width: file.width,
                        height: file.height,
                        focal_point: file.focal_point.map(|point| (point.x, point.y)),
                    },
                )
            })
            .collect(),
    };

    if let Some(timezone) = &shop_input.timezone {
        match timezone.parse::<Tz>() {
            Ok(tz) => builder.timezone = tz,
            Err(_) => {
                builder
                    .error(
                        "invalid_timezone",
                        &shop_origin.child("timezone"),
                        format!("\"{timezone}\" is not a time zone"),
                    )
                    .hint("use an IANA name such as \"Europe/Paris\" or \"America/New_York\"");
            }
        }
    }

    let shop = build_shop(&mut builder, &shop_input);
    let products = build_products(&mut builder, &input.products, shop_input.name.as_deref());
    let mut store = Store {
        shop,
        products,
        collections: Vec::new(),
        pages: Vec::new(),
        blogs: Vec::new(),
        menus: Vec::new(),
        customers: Vec::new(),
        gift_cards: Vec::new(),
        metaobjects: Vec::new(),
        countries: Vec::new(),
        languages: Vec::new(),
        files: IndexMap::new(),
        session_defaults: SessionDefaults::default(),
        now: None,
        theme_settings: input.theme_settings,
        product_handles: HashMap::new(),
        product_ids: HashMap::new(),
        variant_ids: HashMap::new(),
        collection_handles: HashMap::new(),
        page_handles: HashMap::new(),
        blog_handles: HashMap::new(),
        menu_handles: HashMap::new(),
        image_sizes: HashMap::new(),
    };
    store.index();

    build_collections(
        &mut builder,
        &mut store,
        &input.collections,
        &input.products,
    );
    build_recommendations(&mut builder, &mut store, &input.products);
    build_pages(&mut builder, &mut store, &input.pages);
    build_blogs(&mut builder, &mut store, &input.blogs);
    store.index();
    build_menus(&mut builder, &mut store, &input.menus);
    build_customers(&mut builder, &mut store, &input.customers);
    build_gift_cards(&mut builder, &mut store, &input.gift_cards);
    build_localization(&mut builder, &mut store, input.localization.as_ref());
    for (kind, entry) in &input.metaobjects {
        store.metaobjects.push(Metaobject {
            kind: kind.clone(),
            handle: entry.value.handle.clone(),
            fields: entry
                .value
                .fields
                .iter()
                .map(|(key, field)| (key.clone(), metafield(field)))
                .collect(),
        });
    }
    if let Some(session) = &input.session {
        build_session(&mut builder, &mut store, session);
    }
    if let Some(now) = &input.now {
        let parsed = builder.date(&Some(now.value.clone()), &now.origin, epoch());
        store.now = Some(parsed);
    }
    store.files = builder.files;
    store.index();
    (store, builder.diagnostics)
}

fn build_shop(builder: &mut Builder<'_>, input: &model::ShopInput) -> Shop {
    let name = input
        .name
        .clone()
        .unwrap_or_else(|| "Local Store".to_string());
    let currency = input
        .currency
        .clone()
        .unwrap_or_else(|| "USD".to_string())
        .to_uppercase();
    let policy_inputs = [
        ("privacy-policy", &input.policies.privacy_policy),
        ("refund-policy", &input.policies.refund_policy),
        ("shipping-policy", &input.policies.shipping_policy),
        ("terms-of-service", &input.policies.terms_of_service),
        ("subscription-policy", &input.policies.subscription_policy),
    ];
    let policies = policy_inputs
        .iter()
        .filter_map(|(handle, policy)| {
            policy.as_ref().map(|policy| Policy {
                handle: (*handle).to_string(),
                title: policy
                    .title
                    .clone()
                    .unwrap_or_else(|| reference::policy_title(handle).to_string()),
                body: policy.body.clone(),
            })
        })
        .collect();
    let brand = input.brand.as_ref().map(|brand| Brand {
        slogan: brand.slogan.clone().unwrap_or_default(),
        short_description: brand.short_description.clone().unwrap_or_default(),
        logo: brand
            .logo
            .as_ref()
            .map(|image| builder.image(image, "brand")),
        square_logo: brand
            .square_logo
            .as_ref()
            .map(|image| builder.image(image, "brand")),
        cover_image: brand
            .cover_image
            .as_ref()
            .map(|image| builder.image(image, "brand")),
        favicon: brand
            .favicon
            .as_ref()
            .map(|image| builder.image(image, "brand")),
        colors: brand.colors.clone().unwrap_or(Json::Null),
    });
    Shop {
        id: input.id.unwrap_or_else(|| stable_id("shop", &name)),
        description: input.description.clone().unwrap_or_default(),
        email: input
            .email
            .clone()
            .unwrap_or_else(|| "hello@example.com".to_string()),
        phone: input.phone.clone().unwrap_or_default(),
        domain: input.domain.clone(),
        permanent_domain: input
            .permanent_domain
            .clone()
            .unwrap_or_else(|| format!("{}.myshopify.com", handleize(&name))),
        money_format: input
            .money_format
            .clone()
            .unwrap_or_else(|| reference::default_money_format(&currency)),
        money_with_currency_format: input
            .money_with_currency_format
            .clone()
            .unwrap_or_else(|| reference::default_money_with_currency_format(&currency)),
        currency,
        timezone: builder.timezone,
        address: input
            .address
            .as_ref()
            .map(|input| address(input, "shop"))
            .unwrap_or_default(),
        policies,
        enabled_payment_types: input.enabled_payment_types.clone().unwrap_or_else(|| {
            [
                "visa",
                "master",
                "american_express",
                "paypal",
                "apple_pay",
                "google_pay",
                "shopify_pay",
            ]
            .map(str::to_string)
            .to_vec()
        }),
        customer_accounts_enabled: input.customer_accounts_enabled.unwrap_or(true),
        customer_accounts_optional: input.customer_accounts_optional.unwrap_or(true),
        taxes_included: input.taxes_included,
        password_message: input.password_message.clone().unwrap_or_default(),
        password: input.password.clone(),
        brand,
        metafields: builder.metafields(&input.metafields),
        name,
    }
}

fn build_products(
    builder: &mut Builder<'_>,
    inputs: &[Sourced<model::ProductInput>],
    shop_name: Option<&str>,
) -> Vec<Product> {
    let mut seen_handles = HashSet::new();
    let mut seen_variant_ids: HashMap<u64, String> = HashMap::new();
    let mut products = Vec::with_capacity(inputs.len());
    for (
        index,
        Sourced {
            value: input,
            origin,
        },
    ) in inputs.iter().enumerate()
    {
        let handle = builder.handle(
            "product",
            &input.handle,
            &input.title,
            origin,
            &mut seen_handles,
        );
        let created_default = epoch() + Duration::days(index as i64);

        // Media: `images` is shorthand for image-only `media`.
        if !input.images.is_empty() && !input.media.is_empty() {
            builder
                .error(
                    "media_conflict",
                    &origin.child("images"),
                    "a product cannot set both \"images\" and \"media\"",
                )
                .hint("move the images into \"media\" (strings are images) and remove \"images\"");
        }
        let media_inputs: Vec<MediaInput> = if input.media.is_empty() {
            input
                .images
                .iter()
                .cloned()
                .map(MediaInput::Image)
                .collect()
        } else {
            input.media.clone()
        };
        let media: Vec<Media> = media_inputs
            .iter()
            .enumerate()
            .map(|(position, media)| {
                build_media(
                    builder,
                    media,
                    &handle,
                    position,
                    &origin.child("media").child(position),
                )
            })
            .collect();

        // Options and variants.
        if input.options.len() > 3 {
            builder.error(
                "too_many_options",
                &origin.child("options"),
                format!(
                    "a product can have at most 3 options, this one has {}",
                    input.options.len()
                ),
            );
        }
        let default_variant;
        let (option_names, variant_inputs): (Vec<String>, &[model::VariantInput]) = if input
            .variants
            .is_empty()
        {
            if !input.options.is_empty() {
                builder
                    .error("missing_variants", origin, "the product declares options but has no variants")
                    .hint("add a \"variants\" array with one entry per combination, each with an \"options\" array");
            }
            if input.price.is_none() {
                builder
                    .error("missing_price", origin, format!("the product \"{}\" has no price", input.title))
                    .hint("add \"price\" to the product, or a \"variants\" array where each variant has a \"price\"");
            }
            default_variant = [model::VariantInput {
                title: Some("Default Title".to_string()),
                options: vec!["Default Title".to_string()],
                ..model::VariantInput::default()
            }];
            (vec!["Title".to_string()], &default_variant[..])
        } else if input.options.is_empty() {
            let has_options = input
                .variants
                .iter()
                .any(|variant| !variant.options.is_empty());
            if has_options {
                builder
                    .error(
                        "missing_options",
                        origin,
                        "the variants have option values but the product does not name its options",
                    )
                    .hint("add \"options\" to the product, e.g. [\"Size\", \"Color\"]");
            }
            (vec!["Title".to_string()], &input.variants[..])
        } else {
            (
                input
                    .options
                    .iter()
                    .map(|option| option.name().to_string())
                    .collect(),
                &input.variants[..],
            )
        };

        let mut variants = Vec::with_capacity(variant_inputs.len());
        let mut seen_combinations = HashSet::new();
        for (position, variant) in variant_inputs.iter().enumerate() {
            let variant_origin = origin.child("variants").child(position);
            let mut options = variant.options.clone();
            if input.options.is_empty() && options.is_empty() {
                // A product without declared options: each variant is its own "Title" value.
                options = vec![variant.title.clone().unwrap_or_else(|| {
                    if variant_inputs.len() == 1 {
                        "Default Title".to_string()
                    } else {
                        format!("Variant {}", position + 1)
                    }
                })];
            } else if options.len() != option_names.len() {
                builder
                    .error(
                        "option_mismatch",
                        &variant_origin.child("options"),
                        format!(
                            "the variant gives {} option value(s) but the product has {} option(s): {}",
                            options.len(),
                            option_names.len(),
                            option_names.join(", ")
                        ),
                    )
                    .hint("give exactly one value per product option, in the same order");
                options.resize(option_names.len(), String::new());
            }
            if !seen_combinations.insert(options.clone()) {
                builder.error(
                    "duplicate_variant",
                    &variant_origin.child("options"),
                    format!("another variant of this product already has the options {options:?}"),
                );
            }
            let price = match variant.price.as_ref().or(input.price.as_ref()) {
                Some(money) => builder.money(money, &variant_origin.child("price")),
                None => {
                    if !input.variants.is_empty() {
                        builder
                            .error(
                                "missing_price",
                                &variant_origin,
                                "the variant has no price and the product has no default \"price\"",
                            )
                            .hint("add \"price\" to the variant or to the product");
                    }
                    0
                }
            };
            let compare_at_price = variant
                .compare_at_price
                .as_ref()
                .or(input.compare_at_price.as_ref())
                .map(|money| builder.money(money, &variant_origin.child("compare_at_price")));
            let tracked = variant
                .inventory_tracked
                .unwrap_or(variant.inventory_quantity.is_some());
            let quantity = variant.inventory_quantity.unwrap_or(0);
            let available = variant.available.or(input.available).unwrap_or(
                !tracked || quantity > 0 || variant.inventory_policy == InventoryPolicy::Continue,
            );
            let media_index = match &variant.image {
                None => None,
                Some(src) => {
                    let found = media.iter().position(|media| {
                        media
                            .preview
                            .as_ref()
                            .is_some_and(|image| image.src == *src)
                    });
                    if found.is_none() {
                        let sources: Vec<&str> = media
                            .iter()
                            .filter_map(|m| m.preview.as_ref())
                            .map(|i| i.src.as_str())
                            .collect();
                        let diagnostic = builder.error(
                            "unknown_image",
                            &variant_origin.child("image"),
                            format!("\"{src}\" is not one of the product's images"),
                        );
                        match closest_match(src, sources.iter().copied()) {
                            Some(suggestion) => diagnostic.hint(format!("did you mean \"{suggestion}\"?")),
                            None => diagnostic.hint("a variant image must repeat the \"src\" of an image listed in the product's \"images\" or \"media\""),
                        };
                    }
                    found
                }
            };
            let id = variant
                .id
                .unwrap_or_else(|| stable_id("variant", &format!("{handle}/{position}")));
            if let Some(previous) = seen_variant_ids.insert(id, handle.clone()) {
                builder.error(
                    "duplicate_id",
                    &variant_origin.child("id"),
                    format!("the variant id {id} is already used by a variant of \"{previous}\""),
                );
            }
            variants.push(Variant {
                id,
                title: variant.title.clone().unwrap_or_else(|| options.join(" / ")),
                options,
                price,
                compare_at_price,
                sku: variant.sku.clone().unwrap_or_default(),
                barcode: variant.barcode.clone().unwrap_or_default(),
                available,
                inventory_quantity: quantity,
                inventory_tracked: tracked,
                inventory_policy: variant.inventory_policy,
                weight: variant.weight.unwrap_or(0),
                weight_unit: variant
                    .weight_unit
                    .clone()
                    .unwrap_or_else(|| "kg".to_string()),
                requires_shipping: variant.requires_shipping.unwrap_or(!input.gift_card),
                taxable: variant.taxable.unwrap_or(true),
                media_index,
                unit_price: variant
                    .unit_price
                    .as_ref()
                    .map(|money| builder.money(money, &variant_origin.child("unit_price"))),
                unit_price_measurement: variant.unit_price_measurement.as_ref().map(|m| {
                    UnitPriceMeasurement {
                        measured_type: m
                            .measured_type
                            .clone()
                            .unwrap_or_else(|| "weight".to_string()),
                        quantity_value: m.quantity_value,
                        quantity_unit: m.quantity_unit.clone(),
                        reference_value: m.reference_value.unwrap_or(1.0),
                        reference_unit: m
                            .reference_unit
                            .clone()
                            .unwrap_or_else(|| m.quantity_unit.clone()),
                    }
                }),
                quantity_rule: {
                    let rule = variant.quantity_rule.clone().unwrap_or_default();
                    QuantityRule {
                        min: rule.min.unwrap_or(1),
                        max: rule.max,
                        increment: rule.increment.unwrap_or(1),
                    }
                },
                quantity_price_breaks: variant
                    .quantity_price_breaks
                    .iter()
                    .map(|price_break| {
                        (
                            price_break.minimum_quantity,
                            builder.money(
                                &price_break.price,
                                &variant_origin.child("quantity_price_breaks"),
                            ),
                        )
                    })
                    .collect(),
                metafields: builder.metafields(&variant.metafields),
            });
        }

        // Option values: the declared order first, then the order they appear in the variants.
        let options = option_names
            .iter()
            .enumerate()
            .map(|(position, name)| {
                let mut values: Vec<String> = match input.options.get(position) {
                    Some(model::OptionInput::Detailed(detail)) => detail.values.clone(),
                    _ => Vec::new(),
                };
                for variant in &variants {
                    if let Some(value) = variant.options.get(position)
                        && !values.contains(value)
                    {
                        values.push(value.clone());
                    }
                }
                ProductOption {
                    name: name.clone(),
                    position: position + 1,
                    values,
                }
            })
            .collect();

        let created_at = builder.date(
            &input.created_at,
            &origin.child("created_at"),
            created_default,
        );
        let published_at = builder.date(
            &input.published_at,
            &origin.child("published_at"),
            created_at,
        );
        let updated_at = builder.date(&input.updated_at, &origin.child("updated_at"), published_at);
        products.push(Product {
            id: input.id.unwrap_or_else(|| stable_id("product", &handle)),
            title: input.title.clone(),
            description: input.description.clone(),
            vendor: input
                .vendor
                .clone()
                .unwrap_or_else(|| shop_name.unwrap_or("Local Store").to_string()),
            product_type: input.product_type.clone(),
            tags: {
                // Shopify keeps tags sorted alphabetically.
                let mut tags = input.tags.clone();
                tags.sort_by_key(|tag| tag.to_lowercase());
                tags.dedup();
                tags
            },
            options,
            variants,
            media,
            template_suffix: input.template_suffix.clone(),
            gift_card: input.gift_card,
            created_at,
            published_at,
            updated_at,
            metafields: builder.metafields(&input.metafields),
            collections: Vec::new(),
            recommendations: Vec::new(),
            handle,
        });
    }
    let mut seen_ids: HashMap<u64, usize> = HashMap::new();
    for (index, product) in products.iter().enumerate() {
        if let Some(previous) = seen_ids.insert(product.id, index) {
            builder.error(
                "duplicate_id",
                &inputs[index].origin.child("id"),
                format!(
                    "the product id {} is already used by \"{}\"",
                    product.id, products[previous].handle
                ),
            );
        }
    }
    products
}

fn build_media(
    builder: &mut Builder<'_>,
    input: &MediaInput,
    handle: &str,
    position: usize,
    origin: &Origin,
) -> Media {
    match input {
        MediaInput::Image(image) => {
            let image = builder.image(image, handle);
            Media {
                id: stable_id("media", &format!("{handle}/{position}")),
                position: position + 1,
                alt: image.alt.clone(),
                aspect_ratio: image.aspect_ratio(),
                preview: Some(image),
                kind: MediaKind::Image,
            }
        }
        MediaInput::Other(detail) => {
            let preview = detail
                .preview_image
                .as_ref()
                .map(|image| builder.image(image, handle));
            let sources = |builder: &mut Builder<'_>| -> Vec<MediaSource> {
                if detail.sources.is_empty() {
                    builder.error(
                        "missing_sources",
                        origin,
                        "this media needs at least one entry in \"sources\"",
                    );
                }
                detail
                    .sources
                    .iter()
                    .map(|source| {
                        let format = source.format.clone().unwrap_or_else(|| {
                            source
                                .url
                                .rsplit('.')
                                .next()
                                .unwrap_or("mp4")
                                .to_lowercase()
                        });
                        MediaSource {
                            mime_type: source.mime_type.clone().unwrap_or_else(|| {
                                match format.as_str() {
                                    "m3u8" => "application/x-mpegURL".to_string(),
                                    "glb" => "model/gltf-binary".to_string(),
                                    "usdz" => "model/vnd.usdz+zip".to_string(),
                                    other => format!("video/{other}"),
                                }
                            }),
                            url: source.url.clone(),
                            format,
                            width: source.width.unwrap_or(1920),
                            height: source.height.unwrap_or(1080),
                        }
                    })
                    .collect()
            };
            let kind = match detail.media_type {
                MediaType::Video => MediaKind::Video {
                    sources: sources(builder),
                    duration: detail.duration.unwrap_or(0),
                },
                MediaType::Model => MediaKind::Model {
                    sources: sources(builder),
                },
                MediaType::ExternalVideo => {
                    if detail.host.is_none() || detail.external_id.is_none() {
                        builder
                            .error("missing_external_video", origin, "an external video needs both \"host\" and \"external_id\"")
                            .hint("e.g. {\"media_type\": \"external_video\", \"host\": \"youtube\", \"external_id\": \"dQw4w9WgXcQ\"}");
                    }
                    MediaKind::ExternalVideo {
                        host: detail.host.unwrap_or(VideoHost::Youtube),
                        external_id: detail.external_id.clone().unwrap_or_default(),
                    }
                }
            };
            Media {
                id: detail
                    .id
                    .unwrap_or_else(|| stable_id("media", &format!("{handle}/{position}"))),
                position: position + 1,
                alt: detail.alt.clone().unwrap_or_default(),
                aspect_ratio: detail
                    .aspect_ratio
                    .or(preview.as_ref().map(Image::aspect_ratio))
                    .unwrap_or(16.0 / 9.0),
                preview,
                kind,
            }
        }
    }
}

fn rule_matches(rule: &model::CollectionRule, product: &Product) -> bool {
    let condition = rule.condition.to_lowercase();
    let text_matches = |value: &str| {
        let value = value.to_lowercase();
        match rule.relation {
            RuleRelation::Equals => value == condition,
            RuleRelation::NotEquals => value != condition,
            RuleRelation::Contains => value.contains(&condition),
            RuleRelation::NotContains => !value.contains(&condition),
            RuleRelation::StartsWith => value.starts_with(&condition),
            RuleRelation::EndsWith => value.ends_with(&condition),
            RuleRelation::GreaterThan => value > condition,
            RuleRelation::LessThan => value < condition,
        }
    };
    let number_matches = |value: i64| {
        let Ok(condition) = rule.condition.parse::<i64>() else {
            return false;
        };
        match rule.relation {
            RuleRelation::Equals => value == condition,
            RuleRelation::NotEquals => value != condition,
            RuleRelation::GreaterThan => value > condition,
            RuleRelation::LessThan => value < condition,
            _ => false,
        }
    };
    match rule.column {
        RuleColumn::Tag => match rule.relation {
            RuleRelation::NotEquals | RuleRelation::NotContains => {
                product.tags.iter().all(|tag| text_matches(tag))
            }
            _ => product.tags.iter().any(|tag| text_matches(tag)),
        },
        RuleColumn::Type => text_matches(&product.product_type),
        RuleColumn::Vendor => text_matches(&product.vendor),
        RuleColumn::Title => text_matches(&product.title),
        RuleColumn::Price => product
            .variants
            .iter()
            .any(|variant| number_matches(variant.price)),
        RuleColumn::CompareAtPrice => product
            .variants
            .iter()
            .any(|variant| variant.compare_at_price.is_some_and(number_matches)),
    }
}

fn build_collections(
    builder: &mut Builder<'_>,
    store: &mut Store,
    inputs: &[Sourced<model::CollectionInput>],
    product_inputs: &[Sourced<model::ProductInput>],
) {
    let mut seen_handles = HashSet::new();
    for (
        index,
        Sourced {
            value: input,
            origin,
        },
    ) in inputs.iter().enumerate()
    {
        let handle = builder.handle(
            "collection",
            &input.handle,
            &input.title,
            origin,
            &mut seen_handles,
        );
        let mut products = Vec::new();
        for (position, product_handle) in input.products.iter().enumerate() {
            match store.product_index(product_handle) {
                Some(product) if !products.contains(&product) => products.push(product),
                Some(_) => {}
                None => {
                    let known: Vec<&str> = store
                        .products
                        .iter()
                        .map(|product| product.handle.as_str())
                        .collect();
                    let diagnostic = builder.error(
                        "unknown_product",
                        &origin.child("products").child(position),
                        format!("there is no product with the handle \"{product_handle}\""),
                    );
                    if let Some(suggestion) = closest_match(product_handle, known.iter().copied()) {
                        diagnostic.hint(format!("did you mean \"{suggestion}\"?"));
                    }
                }
            }
        }
        if !input.rules.is_empty() {
            for (product_index, product) in store.products.iter().enumerate() {
                let matches = if input.disjunctive {
                    input.rules.iter().any(|rule| rule_matches(rule, product))
                } else {
                    input.rules.iter().all(|rule| rule_matches(rule, product))
                };
                if matches && !products.contains(&product_index) {
                    products.push(product_index);
                }
            }
        }
        if handle == "all" && input.products.is_empty() && input.rules.is_empty() {
            products = (0..store.products.len()).collect();
        }
        let published_at = builder.date(
            &input.published_at,
            &origin.child("published_at"),
            epoch() + Duration::days(index as i64),
        );
        let updated_at = builder.date(&input.updated_at, &origin.child("updated_at"), published_at);
        store.collections.push(Collection {
            id: input.id.unwrap_or_else(|| stable_id("collection", &handle)),
            title: input.title.clone(),
            description: input.description.clone(),
            image: input
                .image
                .as_ref()
                .map(|image| builder.image(image, &format!("collections/{handle}"))),
            products,
            sort_order: input.sort_order,
            template_suffix: input.template_suffix.clone(),
            published_at,
            updated_at,
            metafields: builder.metafields(&input.metafields),
            handle,
        });
    }
    store.index();

    // Memberships declared on the product side.
    for (
        product_index,
        Sourced {
            value: input,
            origin,
        },
    ) in product_inputs.iter().enumerate()
    {
        for (position, collection_handle) in input.collections.iter().enumerate() {
            match store.collection_index(collection_handle) {
                Some(collection) => {
                    if !store.collections[collection]
                        .products
                        .contains(&product_index)
                    {
                        store.collections[collection].products.push(product_index);
                    }
                }
                None => {
                    let known: Vec<&str> = store
                        .collections
                        .iter()
                        .map(|collection| collection.handle.as_str())
                        .collect();
                    let diagnostic = builder.error(
                        "unknown_collection",
                        &origin.child("collections").child(position),
                        format!("there is no collection with the handle \"{collection_handle}\""),
                    );
                    match closest_match(collection_handle, known.iter().copied()) {
                        Some(suggestion) => {
                            diagnostic.hint(format!("did you mean \"{suggestion}\"?"))
                        }
                        None => diagnostic.hint("define the collection in \"collections\" first"),
                    };
                }
            }
        }
    }

    // The catch-all collection every store has.
    if store.collection_index("all").is_none() {
        store.collections.push(Collection {
            id: stable_id("collection", "all"),
            title: "Products".to_string(),
            handle: "all".to_string(),
            description: String::new(),
            image: None,
            products: (0..store.products.len()).collect(),
            sort_order: SortOrder::TitleAscending,
            template_suffix: None,
            published_at: epoch(),
            updated_at: epoch(),
            metafields: Metafields::new(),
        });
    }
    store.index();
    for (collection_index, collection) in store.collections.iter().enumerate() {
        if collection.handle == "all" {
            continue;
        }
        for &product in &collection.products {
            store.products[product].collections.push(collection_index);
        }
    }
}

fn build_recommendations(
    builder: &mut Builder<'_>,
    store: &mut Store,
    inputs: &[Sourced<model::ProductInput>],
) {
    for (
        index,
        Sourced {
            value: input,
            origin,
        },
    ) in inputs.iter().enumerate()
    {
        let recommendations = match &input.recommendations {
            Some(handles) => handles
                .iter()
                .enumerate()
                .filter_map(|(position, handle)| {
                    let found = store.product_index(handle);
                    if found.is_none() {
                        let known: Vec<&str> = store
                            .products
                            .iter()
                            .map(|product| product.handle.as_str())
                            .collect();
                        let diagnostic = builder.error(
                            "unknown_product",
                            &origin.child("recommendations").child(position),
                            format!("there is no product with the handle \"{handle}\""),
                        );
                        if let Some(suggestion) = closest_match(handle, known.iter().copied()) {
                            diagnostic.hint(format!("did you mean \"{suggestion}\"?"));
                        }
                    }
                    found
                })
                .collect(),
            // By default: the other products of the same collections, then everything else.
            None => {
                let mut related: Vec<usize> = Vec::new();
                for &collection in &store.products[index].collections {
                    for &product in &store.collections[collection].products {
                        if product != index && !related.contains(&product) {
                            related.push(product);
                        }
                    }
                }
                for product in 0..store.products.len() {
                    if product != index && !related.contains(&product) {
                        related.push(product);
                    }
                }
                related.truncate(10);
                related
            }
        };
        store.products[index].recommendations = recommendations;
    }
}

fn build_pages(builder: &mut Builder<'_>, store: &mut Store, inputs: &[Sourced<model::PageInput>]) {
    let mut seen_handles = HashSet::new();
    for (
        index,
        Sourced {
            value: input,
            origin,
        },
    ) in inputs.iter().enumerate()
    {
        let handle = builder.handle(
            "page",
            &input.handle,
            &input.title,
            origin,
            &mut seen_handles,
        );
        let published_at = builder.date(
            &input.published_at,
            &origin.child("published_at"),
            epoch() + Duration::days(index as i64),
        );
        let updated_at = builder.date(&input.updated_at, &origin.child("updated_at"), published_at);
        store.pages.push(Page {
            id: input.id.unwrap_or_else(|| stable_id("page", &handle)),
            title: input.title.clone(),
            content: input.content.clone(),
            author: input
                .author
                .clone()
                .unwrap_or_else(|| "Shop owner".to_string()),
            template_suffix: input.template_suffix.clone(),
            published_at,
            updated_at,
            metafields: builder.metafields(&input.metafields),
            handle,
        });
    }
}

fn build_blogs(builder: &mut Builder<'_>, store: &mut Store, inputs: &[Sourced<model::BlogInput>]) {
    let mut seen_handles = HashSet::new();
    for Sourced {
        value: input,
        origin,
    } in inputs
    {
        let handle = builder.handle(
            "blog",
            &input.handle,
            &input.title,
            origin,
            &mut seen_handles,
        );
        let mut seen_articles = HashSet::new();
        let count = input.articles.len();
        let mut articles: Vec<Article> = input
            .articles
            .iter()
            .enumerate()
            .map(|(position, article)| {
                let article_origin = origin.child("articles").child(position);
                let article_handle = builder.handle(
                    "article",
                    &article.handle,
                    &article.title,
                    &article_origin,
                    &mut seen_articles,
                );
                // Undated articles keep their written order: the first one is the newest.
                let default_date = epoch() + Duration::days((count - position) as i64);
                let published_at = builder.date(
                    &article.published_at,
                    &article_origin.child("published_at"),
                    default_date,
                );
                let created_at = builder.date(
                    &article.created_at,
                    &article_origin.child("created_at"),
                    published_at,
                );
                let updated_at = builder.date(
                    &article.updated_at,
                    &article_origin.child("updated_at"),
                    published_at,
                );
                Article {
                    id: article.id.unwrap_or_else(|| {
                        stable_id("article", &format!("{handle}/{article_handle}"))
                    }),
                    title: article.title.clone(),
                    author: article
                        .author
                        .clone()
                        .unwrap_or_else(|| "Shop owner".to_string()),
                    content: article.content.clone(),
                    excerpt: article.excerpt.clone().unwrap_or_default(),
                    image: article
                        .image
                        .as_ref()
                        .map(|image| builder.image(image, &format!("articles/{article_handle}"))),
                    tags: article.tags.clone(),
                    comments: article
                        .comments
                        .iter()
                        .enumerate()
                        .map(|(comment_position, comment)| Comment {
                            id: comment.id.unwrap_or_else(|| {
                                stable_id(
                                    "comment",
                                    &format!("{handle}/{article_handle}/{comment_position}"),
                                )
                            }),
                            author: comment.author.clone(),
                            email: comment.email.clone().unwrap_or_default(),
                            content: comment.content.clone(),
                            created_at: builder.date(
                                &comment.created_at,
                                &article_origin
                                    .child("comments")
                                    .child(comment_position)
                                    .child("created_at"),
                                published_at + Duration::hours(comment_position as i64 + 1),
                            ),
                        })
                        .collect(),
                    template_suffix: article.template_suffix.clone(),
                    created_at,
                    published_at,
                    updated_at,
                    metafields: builder.metafields(&article.metafields),
                    handle: article_handle,
                }
            })
            .collect();
        // Blogs list their articles newest first.
        articles.sort_by_key(|article| std::cmp::Reverse(article.published_at));
        store.blogs.push(Blog {
            id: input.id.unwrap_or_else(|| stable_id("blog", &handle)),
            title: input.title.clone(),
            articles,
            comments_enabled: input.comments_enabled,
            moderated: input.moderated,
            template_suffix: input.template_suffix.clone(),
            metafields: builder.metafields(&input.metafields),
            handle,
        });
    }
}

fn build_link(
    builder: &mut Builder<'_>,
    store: &Store,
    input: &model::LinkInput,
    origin: &Origin,
) -> Link {
    let unknown =
        |builder: &mut Builder<'_>, kind: &str, field: &str, handle: &str, known: Vec<&str>| {
            let diagnostic = builder.error(
                "unknown_reference",
                &origin.child(field),
                format!("there is no {kind} with the handle \"{handle}\""),
            );
            if let Some(suggestion) = closest_match(handle, known.iter().copied()) {
                diagnostic.hint(format!("did you mean \"{suggestion}\"?"));
            }
        };
    let targets = [
        input.url.is_some(),
        input.collection.is_some(),
        input.product.is_some(),
        input.page.is_some(),
        input.blog.is_some(),
        input.article.is_some(),
        input.policy.is_some(),
    ];
    if targets.iter().filter(|set| **set).count() > 1 {
        builder
            .error("ambiguous_link", origin, format!("the link \"{}\" points at more than one thing", input.title))
            .hint("keep only one of \"url\", \"collection\", \"product\", \"page\", \"blog\", \"article\", \"policy\"");
    }
    let (url, target) = if let Some(handle) = &input.collection {
        match store.collection_index(handle) {
            Some(_) if handle == "all" => ("/collections/all".to_string(), LinkTarget::Catalog),
            Some(index) => (
                format!("/collections/{handle}"),
                LinkTarget::Collection(index),
            ),
            None => {
                unknown(
                    builder,
                    "collection",
                    "collection",
                    handle,
                    store
                        .collections
                        .iter()
                        .map(|c| c.handle.as_str())
                        .collect(),
                );
                (format!("/collections/{handle}"), LinkTarget::Http)
            }
        }
    } else if let Some(handle) = &input.product {
        match store.product_index(handle) {
            Some(index) => (format!("/products/{handle}"), LinkTarget::Product(index)),
            None => {
                unknown(
                    builder,
                    "product",
                    "product",
                    handle,
                    store.products.iter().map(|p| p.handle.as_str()).collect(),
                );
                (format!("/products/{handle}"), LinkTarget::Http)
            }
        }
    } else if let Some(handle) = &input.page {
        match store.page_index(handle) {
            Some(index) => (format!("/pages/{handle}"), LinkTarget::Page(index)),
            None => {
                unknown(
                    builder,
                    "page",
                    "page",
                    handle,
                    store.pages.iter().map(|p| p.handle.as_str()).collect(),
                );
                (format!("/pages/{handle}"), LinkTarget::Http)
            }
        }
    } else if let Some(handle) = &input.blog {
        match store.blog_index(handle) {
            Some(index) => (format!("/blogs/{handle}"), LinkTarget::Blog(index)),
            None => {
                unknown(
                    builder,
                    "blog",
                    "blog",
                    handle,
                    store.blogs.iter().map(|b| b.handle.as_str()).collect(),
                );
                (format!("/blogs/{handle}"), LinkTarget::Http)
            }
        }
    } else if let Some(path) = &input.article {
        let found = path.split_once('/').and_then(|(blog, article)| {
            let blog_index = store.blog_index(blog)?;
            Some((blog_index, store.article_index(blog_index, article)?))
        });
        match found {
            Some((blog, article)) => (format!("/blogs/{path}"), LinkTarget::Article(blog, article)),
            None => {
                builder
                    .error(
                        "unknown_reference",
                        &origin.child("article"),
                        format!("there is no article \"{path}\""),
                    )
                    .hint("write it as \"<blog-handle>/<article-handle>\"");
                (format!("/blogs/{path}"), LinkTarget::Http)
            }
        }
    } else if let Some(handle) = &input.policy {
        if store.policy(handle).is_none() {
            builder
                .error(
                    "unknown_reference",
                    &origin.child("policy"),
                    format!("the shop has no policy \"{handle}\""),
                )
                .hint("define it under shop.policies, e.g. \"refund_policy\": {\"body\": \"...\"}");
        }
        (
            format!("/policies/{handle}"),
            LinkTarget::Policy(handle.clone()),
        )
    } else {
        let url = input.url.clone().unwrap_or_else(|| "#".to_string());
        let target = match url.as_str() {
            "/" => LinkTarget::Frontpage,
            "/collections/all" => LinkTarget::Catalog,
            "/collections" => LinkTarget::Collections,
            "/search" => LinkTarget::Search,
            path => infer_target(store, path),
        };
        (url, target)
    };
    Link {
        handle: handleize(&input.title),
        title: input.title.clone(),
        url,
        target,
        links: input
            .links
            .iter()
            .enumerate()
            .map(|(position, link)| {
                build_link(builder, store, link, &origin.child("links").child(position))
            })
            .collect(),
    }
}

/// A literal URL that happens to point at a known resource gets that resource's link type,
/// as it would when picked in the Shopify admin.
fn infer_target(store: &Store, path: &str) -> LinkTarget {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match segments.as_slice() {
        ["collections", handle] => store
            .collection_index(handle)
            .map_or(LinkTarget::Http, LinkTarget::Collection),
        ["products", handle] => store
            .product_index(handle)
            .map_or(LinkTarget::Http, LinkTarget::Product),
        ["pages", handle] => store
            .page_index(handle)
            .map_or(LinkTarget::Http, LinkTarget::Page),
        ["blogs", handle] => store
            .blog_index(handle)
            .map_or(LinkTarget::Http, LinkTarget::Blog),
        ["blogs", blog, article] => store
            .blog_index(blog)
            .and_then(|blog_index| {
                Some(LinkTarget::Article(
                    blog_index,
                    store.article_index(blog_index, article)?,
                ))
            })
            .unwrap_or(LinkTarget::Http),
        ["policies", handle] if store.policy(handle).is_some() => {
            LinkTarget::Policy((*handle).to_string())
        }
        _ => LinkTarget::Http,
    }
}

fn build_menus(
    builder: &mut Builder<'_>,
    store: &mut Store,
    inputs: &IndexMap<String, Sourced<model::MenuInput>>,
) {
    for (
        handle,
        Sourced {
            value: input,
            origin,
        },
    ) in inputs
    {
        if handleize(handle) != *handle {
            builder
                .error(
                    "invalid_handle",
                    origin,
                    format!("\"{handle}\" is not a valid menu handle"),
                )
                .hint(format!("use \"{}\"", handleize(handle)));
        }
        let links = input
            .links
            .iter()
            .enumerate()
            .map(|(position, link)| {
                build_link(builder, store, link, &origin.child("links").child(position))
            })
            .collect();
        store.menus.push(Menu {
            handle: handle.clone(),
            title: input.title.clone().unwrap_or_else(|| humanize(handle)),
            links,
        });
    }
    // The two menus every Shopify store starts with.
    let simple = |title: &str, url: &str, target: LinkTarget| Link {
        title: title.to_string(),
        handle: handleize(title),
        url: url.to_string(),
        target,
        links: Vec::new(),
    };
    if !inputs.contains_key("main-menu") {
        store.menus.push(Menu {
            handle: "main-menu".to_string(),
            title: "Main menu".to_string(),
            links: vec![
                simple("Home", "/", LinkTarget::Frontpage),
                simple("Catalog", "/collections/all", LinkTarget::Catalog),
            ],
        });
    }
    if !inputs.contains_key("footer") {
        store.menus.push(Menu {
            handle: "footer".to_string(),
            title: "Footer menu".to_string(),
            links: vec![simple("Search", "/search", LinkTarget::Search)],
        });
    }
}

fn resolve_variant(store: &Store, reference: &VariantRef) -> Option<(usize, usize)> {
    match reference {
        VariantRef::Id(id) => store.variant_location(*id),
        VariantRef::SkuOrHandle(text) => store
            .products
            .iter()
            .enumerate()
            .find_map(|(product_index, product)| {
                product
                    .variants
                    .iter()
                    .position(|variant| !variant.sku.is_empty() && variant.sku == *text)
                    .map(|variant_index| (product_index, variant_index))
            })
            .or_else(|| {
                store
                    .product_index(text)
                    .map(|product_index| (product_index, 0))
            }),
    }
}

/// What to say about a variant reference that matches nothing: the message, and a hint naming
/// the closest SKU or product handle when there is one.
fn describe_unknown_variant(store: &Store, reference: &VariantRef) -> (String, String) {
    const HOW: &str = "reference a variant by its numeric id, its SKU, or a product handle (meaning that product's first variant)";
    match reference {
        VariantRef::Id(id) => (format!("no variant has the id {id}"), HOW.to_string()),
        VariantRef::SkuOrHandle(text) => {
            let candidates = store
                .products
                .iter()
                .flat_map(|product| {
                    product
                        .variants
                        .iter()
                        .map(|variant| variant.sku.as_str())
                        .chain(std::iter::once(product.handle.as_str()))
                })
                .filter(|candidate| !candidate.is_empty());
            let hint = match closest_match(text, candidates) {
                Some(suggestion) => format!("did you mean \"{suggestion}\"?"),
                None => HOW.to_string(),
            };
            (
                format!("\"{text}\" is neither the SKU of a variant nor the handle of a product"),
                hint,
            )
        }
    }
}

fn unknown_variant(
    builder: &mut Builder<'_>,
    store: &Store,
    reference: &VariantRef,
    origin: &Origin,
) {
    let (message, hint) = describe_unknown_variant(store, reference);
    builder.error("unknown_variant", origin, message).hint(hint);
}

fn build_customers(
    builder: &mut Builder<'_>,
    store: &mut Store,
    inputs: &[Sourced<model::CustomerInput>],
) {
    let mut seen_emails = HashSet::new();
    let mut order_number = 1000;
    for Sourced {
        value: input,
        origin,
    } in inputs
    {
        if !input.email.contains('@') {
            builder.error(
                "invalid_email",
                &origin.child("email"),
                format!("\"{}\" is not an email address", input.email),
            );
        }
        if !seen_emails.insert(input.email.to_lowercase()) {
            builder.error(
                "duplicate_email",
                &origin.child("email"),
                format!("another customer already uses \"{}\"", input.email),
            );
        }
        let id = input
            .id
            .unwrap_or_else(|| stable_id("customer", &input.email.to_lowercase()));
        let orders = input
            .orders
            .iter()
            .enumerate()
            .map(|(position, order)| {
                let order_origin = origin.child("orders").child(position);
                order_number += 1;
                let line_items = order
                    .line_items
                    .iter()
                    .enumerate()
                    .filter_map(|(line_position, line)| {
                        let variant = resolve_variant(store, &line.variant);
                        if variant.is_none() {
                            unknown_variant(
                                builder,
                                store,
                                &line.variant,
                                &order_origin
                                    .child("line_items")
                                    .child(line_position)
                                    .child("variant"),
                            );
                        }
                        variant.map(|variant| OrderLine {
                            variant,
                            quantity: line.quantity,
                        })
                    })
                    .collect();
                Order {
                    id: order
                        .id
                        .unwrap_or_else(|| stable_id("order", &format!("{id}/{position}"))),
                    name: order
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("#{order_number}")),
                    order_number,
                    created_at: builder.date(
                        &order.created_at,
                        &order_origin.child("created_at"),
                        epoch() + Duration::days(position as i64),
                    ),
                    financial_status: order
                        .financial_status
                        .clone()
                        .unwrap_or_else(|| "paid".to_string()),
                    fulfillment_status: order
                        .fulfillment_status
                        .clone()
                        .unwrap_or_else(|| "unfulfilled".to_string()),
                    line_items,
                    shipping_address: order
                        .shipping_address
                        .as_ref()
                        .map(|a| address(a, &format!("{id}/order/{position}/shipping"))),
                    billing_address: order
                        .billing_address
                        .as_ref()
                        .map(|a| address(a, &format!("{id}/order/{position}/billing"))),
                    shipping_price: order.shipping_price.as_ref().map_or(0, |money| {
                        builder.money(money, &order_origin.child("shipping_price"))
                    }),
                    tax_price: order.tax_price.as_ref().map_or(0, |money| {
                        builder.money(money, &order_origin.child("tax_price"))
                    }),
                    cancelled: order.cancelled,
                    note: order.note.clone().unwrap_or_default(),
                }
            })
            .collect();
        store.customers.push(Customer {
            id,
            email: input.email.clone(),
            first_name: input.first_name.clone().unwrap_or_default(),
            last_name: input.last_name.clone().unwrap_or_default(),
            phone: input.phone.clone().unwrap_or_default(),
            tags: input.tags.clone(),
            accepts_marketing: input.accepts_marketing,
            has_account: input.has_account.unwrap_or(true),
            tax_exempt: input.tax_exempt,
            addresses: input
                .addresses
                .iter()
                .enumerate()
                .map(|(position, a)| address(a, &format!("{id}/address/{position}")))
                .collect(),
            orders,
            password: input.password.clone(),
            metafields: builder.metafields(&input.metafields),
        });
    }
}

fn build_gift_cards(
    builder: &mut Builder<'_>,
    store: &mut Store,
    inputs: &[Sourced<model::GiftCardInput>],
) {
    let mut seen_codes = HashSet::new();
    for Sourced {
        value: input,
        origin,
    } in inputs
    {
        let code: String = input
            .code
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_uppercase();
        if code.len() < 8 {
            builder
                .error(
                    "invalid_gift_card_code",
                    &origin.child("code"),
                    format!("\"{}\" is too short for a gift card code", input.code),
                )
                .hint("use 8 to 20 letters and digits, e.g. \"WCGX7X97G74JGDGC\"");
        }
        if !seen_codes.insert(code.clone()) {
            builder.error(
                "duplicate_gift_card",
                &origin.child("code"),
                format!("another gift card already uses the code \"{}\"", input.code),
            );
        }
        let initial_value = builder.money(&input.initial_value, &origin.child("initial_value"));
        let balance = input.balance.as_ref().map_or(initial_value, |money| {
            builder.money(money, &origin.child("balance"))
        });
        if balance > initial_value {
            builder.diagnostics.warning(
                "gift_card_balance",
                &origin.file,
                &format!("{}/balance", origin.pointer),
                "the balance is higher than the initial value",
            );
        }
        let customer = input.customer.as_ref().and_then(|email| {
            let found = store.customer_by_email(email).map(|customer| customer.id);
            if found.is_none() {
                let emails: Vec<&str> = store.customers.iter().map(|c| c.email.as_str()).collect();
                let diagnostic = builder.error(
                    "unknown_customer",
                    &origin.child("customer"),
                    format!("there is no customer with the email \"{email}\""),
                );
                match closest_match(email, emails.iter().copied()) {
                    Some(suggestion) => diagnostic.hint(format!("did you mean \"{suggestion}\"?")),
                    None => diagnostic.hint("add the customer to \"customers\" first"),
                };
            }
            found
        });
        let product = input.product.as_ref().and_then(|handle| {
            let found = store.product_index(handle);
            if found.is_none() {
                let handles: Vec<&str> = store.products.iter().map(|p| p.handle.as_str()).collect();
                let diagnostic = builder.error(
                    "unknown_product",
                    &origin.child("product"),
                    format!("there is no product with the handle \"{handle}\""),
                );
                if let Some(suggestion) = closest_match(handle, handles.iter().copied()) {
                    diagnostic.hint(format!("did you mean \"{suggestion}\"?"));
                }
            }
            found
        });
        let expires_on = input
            .expires_on
            .as_ref()
            .map(|date| builder.date(&Some(date.clone()), &origin.child("expires_on"), epoch()));
        let send_on = input
            .send_on
            .as_ref()
            .map(|date| builder.date(&Some(date.clone()), &origin.child("send_on"), epoch()));
        store.gift_cards.push(GiftCard {
            id: stable_id("gift_card", &code),
            token: crate::util::short_hash(&format!("gift_card/{code}")),
            code,
            initial_value,
            balance,
            currency: input
                .currency
                .clone()
                .unwrap_or_else(|| store.shop.currency.clone()),
            expires_on,
            enabled: input.enabled.unwrap_or(true),
            customer,
            message: input.message.clone().unwrap_or_default(),
            recipient: input.recipient.as_ref().map(|recipient| GiftCardRecipient {
                name: recipient.name.clone().unwrap_or_default(),
                email: recipient.email.clone().unwrap_or_default(),
                nickname: recipient.nickname.clone().unwrap_or_default(),
            }),
            send_on,
            product,
            properties: input.properties.clone(),
            template_suffix: input.template_suffix.clone(),
        });
    }
}

fn build_localization(
    builder: &mut Builder<'_>,
    store: &mut Store,
    input: Option<&Sourced<model::LocalizationInput>>,
) {
    let empty = model::LocalizationInput::default();
    let origin = input.map(|input| input.origin.clone()).unwrap_or_default();
    let input = input.map_or(&empty, |input| &input.value);

    store.countries = input
        .countries
        .iter()
        .map(|country| {
            let iso_code = country.iso_code.to_uppercase();
            Country {
                name: country
                    .name
                    .clone()
                    .or_else(|| reference::country_name(&iso_code).map(str::to_string))
                    .unwrap_or_else(|| iso_code.clone()),
                currency: country
                    .currency
                    .clone()
                    .unwrap_or_else(|| store.shop.currency.clone())
                    .to_uppercase(),
                unit_system: country
                    .unit_system
                    .clone()
                    .unwrap_or_else(|| reference::unit_system(&iso_code).to_string()),
                popular: country.popular,
                iso_code,
            }
        })
        .collect();
    if store.countries.is_empty() {
        let iso_code = if store.shop.address.country_code.is_empty() {
            "US".to_string()
        } else {
            store.shop.address.country_code.clone()
        };
        store.countries.push(Country {
            name: reference::country_name(&iso_code)
                .unwrap_or(&iso_code)
                .to_string(),
            currency: store.shop.currency.clone(),
            unit_system: reference::unit_system(&iso_code).to_string(),
            popular: false,
            iso_code,
        });
    }

    let theme_locales = &builder.options.theme_locales;
    let codes: Vec<(String, Option<String>, Option<String>)> = if input.languages.is_empty() {
        vec![(
            theme_locales
                .first()
                .cloned()
                .unwrap_or_else(|| "en".to_string()),
            None,
            None,
        )]
    } else {
        input
            .languages
            .iter()
            .map(|language| {
                (
                    language.iso_code.clone(),
                    language.name.clone(),
                    language.endonym_name.clone(),
                )
            })
            .collect()
    };
    for (position, (iso_code, name, endonym)) in codes.into_iter().enumerate() {
        if !theme_locales.is_empty()
            && !theme_locales
                .iter()
                .any(|locale| locale.eq_ignore_ascii_case(&iso_code))
        {
            builder
                .diagnostics
                .warning(
                    "missing_locale_file",
                    &origin.file,
                    &format!("{}/languages/{position}", origin.pointer),
                    format!("the theme has no locales/{iso_code}.json: translations will fall back to the default language"),
                );
        }
        store.languages.push(Language {
            name: name.unwrap_or_else(|| reference::language_name(&iso_code)),
            endonym_name: endonym.unwrap_or_else(|| reference::language_endonym(&iso_code)),
            root_url: if position == 0 {
                "/".to_string()
            } else {
                format!("/{}", iso_code.to_lowercase())
            },
            primary: position == 0,
            iso_code,
        });
    }
}

fn build_session(
    builder: &mut Builder<'_>,
    store: &mut Store,
    session: &Sourced<model::SessionInput>,
) {
    let Sourced {
        value: input,
        origin,
    } = session;
    if let Some(email) = &input.customer
        && store.customer_by_email(email).is_none()
    {
        let known: Vec<&str> = store
            .customers
            .iter()
            .map(|customer| customer.email.as_str())
            .collect();
        let diagnostic = builder.error(
            "unknown_customer",
            &origin.child("customer"),
            format!("there is no customer with the email \"{email}\""),
        );
        match closest_match(email, known.iter().copied()) {
            Some(suggestion) => diagnostic.hint(format!("did you mean \"{suggestion}\"?")),
            None => diagnostic.hint("add the customer to \"customers\" first"),
        };
    }
    if let Some(country) = &input.country
        && store.country(country).is_none()
    {
        builder.error(
            "unknown_country",
            &origin.child("country"),
            format!("\"{country}\" is not one of the countries in \"localization.countries\""),
        );
    }
    let mut defaults = SessionDefaults {
        customer_email: input.customer.clone(),
        country: input.country.clone(),
        ..SessionDefaults::default()
    };
    if let Some(cart) = &input.cart {
        defaults.cart_note = cart.note.clone().unwrap_or_default();
        defaults.cart_attributes = cart.attributes.clone();
        for (position, line) in cart.items.iter().enumerate() {
            match resolve_variant(store, &line.variant) {
                Some((product, variant)) => defaults.cart_lines.push(CartLine {
                    variant_id: store.products[product].variants[variant].id,
                    quantity: line.quantity,
                    properties: line.properties.clone(),
                }),
                None => unknown_variant(
                    builder,
                    store,
                    &line.variant,
                    &origin
                        .child("cart")
                        .child("items")
                        .child(position)
                        .child("variant"),
                ),
            }
        }
    }
    store.session_defaults = defaults;
}

/// Resolves a cart written in the data format against a built store. Used for the per-session
/// state tests push through the control API.
pub fn resolve_cart(
    store: &Store,
    cart: &model::CartInput,
    diagnostics: &mut Diagnostics,
    file: &str,
) -> Vec<CartLine> {
    cart.items
        .iter()
        .enumerate()
        .filter_map(
            |(position, line)| match resolve_variant(store, &line.variant) {
                Some((product, variant)) => Some(CartLine {
                    variant_id: store.products[product].variants[variant].id,
                    quantity: line.quantity,
                    properties: line.properties.clone(),
                }),
                None => {
                    let (message, hint) = describe_unknown_variant(store, &line.variant);
                    diagnostics
                        .error(
                            "unknown_variant",
                            file,
                            &format!("/cart/items/{position}/variant"),
                            message,
                        )
                        .hint(hint);
                    None
                }
            },
        )
        .collect()
}
