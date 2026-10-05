//! `collection`, its storefront filters and its sort options.

use std::any::Any;
use std::collections::BTreeMap;

use lsf_liquid::{Object, Value};
use serde_json::{Value as Json, json};

use super::media::ImageDrop;
use super::metafield::MetafieldsDrop;
use super::product::{ProductDrop, swatch_value};
use super::{Memo, PaginatedList, SiteRef, hash, strings, time_value};
use crate::render::cost;
use crate::site::Request;
use crate::store::{Collection, Product, SortOrder, Swatch};
use crate::urls::encode_component;
use crate::util::handleize;

/// The sort orders offered on collection pages: `(value, label)`.
pub const SORT_OPTIONS: [(&str, &str); 8] = [
    ("manual", "Featured"),
    ("best-selling", "Best selling"),
    ("title-ascending", "Alphabetically, A-Z"),
    ("title-descending", "Alphabetically, Z-A"),
    ("price-ascending", "Price, low to high"),
    ("price-descending", "Price, high to low"),
    ("created-ascending", "Date, old to new"),
    ("created-descending", "Date, new to old"),
];

pub fn sort_products(products: &[Product], indexes: &mut [usize], order: SortOrder) {
    match order {
        SortOrder::Manual | SortOrder::BestSelling => {}
        SortOrder::TitleAscending => {
            indexes.sort_by_key(|&index| products[index].title.to_lowercase())
        }
        SortOrder::TitleDescending => {
            indexes.sort_by_key(|&index| products[index].title.to_lowercase());
            indexes.reverse();
        }
        SortOrder::PriceAscending => indexes.sort_by_key(|&index| products[index].price_min()),
        SortOrder::PriceDescending => {
            indexes.sort_by_key(|&index| std::cmp::Reverse(products[index].price_min()))
        }
        SortOrder::CreatedAscending => indexes.sort_by_key(|&index| products[index].created_at),
        SortOrder::CreatedDescending => {
            indexes.sort_by_key(|&index| std::cmp::Reverse(products[index].created_at))
        }
    }
}

/// One selected storefront filter.
#[derive(Clone, Debug, PartialEq)]
enum Criterion {
    Availability(bool),
    PriceMin(i64),
    PriceMax(i64),
    Vendor(String),
    ProductType(String),
    Tag(String),
    /// `(option name handle, value)`.
    VariantOption(String, String),
}

/// Parses a price typed in the storefront's currency units (`12.50`) into cents.
fn parse_price(value: &str) -> Option<i64> {
    let amount: f64 = value.replace(',', ".").parse().ok()?;
    Some((amount * 100.0).round() as i64)
}

fn criteria(request: &Request) -> Vec<Criterion> {
    request
        .query
        .iter()
        .filter_map(|(key, value)| {
            Some(match key.as_str() {
                "filter.v.availability" => Criterion::Availability(value == "1"),
                "filter.v.price.gte" => Criterion::PriceMin(parse_price(value)?),
                "filter.v.price.lte" => Criterion::PriceMax(parse_price(value)?),
                "filter.p.vendor" => Criterion::Vendor(value.clone()),
                "filter.p.product_type" => Criterion::ProductType(value.clone()),
                "filter.p.tag" => Criterion::Tag(value.clone()),
                other => Criterion::VariantOption(other.strip_prefix("filter.v.option.")?.to_string(), value.clone()),
            })
        })
        .filter(|criterion| !matches!(criterion, Criterion::VariantOption(_, value) | Criterion::Vendor(value) | Criterion::ProductType(value) | Criterion::Tag(value) if value.is_empty()))
        .collect()
}

/// Whether a product passes the filters. Values of the same filter are alternatives (OR),
/// different filters must all match (AND). Variant-level filters must match on one variant.
fn matches(product: &Product, criteria: &[Criterion]) -> bool {
    let any_or_none =
        |values: Vec<bool>| values.is_empty() || values.into_iter().any(|matched| matched);
    let product_level = any_or_none(
        criteria
            .iter()
            .filter_map(|c| match c {
                Criterion::Vendor(vendor) => Some(product.vendor == *vendor),
                _ => None,
            })
            .collect(),
    ) && any_or_none(
        criteria
            .iter()
            .filter_map(|c| match c {
                Criterion::ProductType(kind) => Some(product.product_type == *kind),
                _ => None,
            })
            .collect(),
    ) && any_or_none(
        criteria
            .iter()
            .filter_map(|c| match c {
                Criterion::Tag(tag) => Some(product.tags.iter().any(|t| t == tag)),
                _ => None,
            })
            .collect(),
    );
    if !product_level {
        return false;
    }
    product.variants.iter().any(|variant| {
        let availability = any_or_none(
            criteria
                .iter()
                .filter_map(|c| match c {
                    Criterion::Availability(wanted) => Some(variant.available == *wanted),
                    _ => None,
                })
                .collect(),
        );
        let price = criteria.iter().all(|c| match c {
            Criterion::PriceMin(min) => variant.price >= *min,
            Criterion::PriceMax(max) => variant.price <= *max,
            _ => true,
        });
        let mut by_option: BTreeMap<&str, Vec<bool>> = BTreeMap::new();
        for criterion in criteria {
            if let Criterion::VariantOption(option, value) = criterion {
                let position = product
                    .options
                    .iter()
                    .position(|o| handleize(&o.name) == *option);
                let matched =
                    position.is_some_and(|position| variant.options.get(position) == Some(value));
                by_option.entry(option.as_str()).or_default().push(matched);
            }
        }
        availability && price && by_option.into_values().all(any_or_none)
    })
}

/// What a collection page is narrowed down to by its URL.
#[derive(Clone, Debug, Default)]
pub struct CollectionScope {
    /// Tags from `/collections/<handle>/<tag>+<tag>`.
    pub tags: Vec<String>,
    /// `/collections/vendors?q=<vendor>`.
    pub vendor: Option<String>,
    /// `/collections/types?q=<type>`.
    pub product_type: Option<String>,
}

pub struct CollectionDrop {
    pub site: SiteRef,
    pub index: usize,
    /// Set for the collection the current page is about: it follows the URL's sort order,
    /// filters and tags. Other collections show their default order.
    pub scope: Option<CollectionScope>,
    memo: Memo,
}

impl CollectionDrop {
    pub fn value(site: &SiteRef, index: usize) -> Value {
        cost::loaded(cost::COLLECTION, index);
        Value::object(CollectionDrop {
            site: site.clone(),
            index,
            scope: None,
            memo: Memo::default(),
        })
    }

    pub fn for_page(site: &SiteRef, index: usize, scope: CollectionScope) -> Value {
        cost::loaded(cost::COLLECTION, index);
        Value::object(CollectionDrop {
            site: site.clone(),
            index,
            scope: Some(scope),
            memo: Memo::default(),
        })
    }

    pub fn collection(&self) -> &Collection {
        &self.site.store.collections[self.index]
    }

    pub fn url(&self) -> String {
        collection_url(&self.site, self.collection())
    }

    /// The collection's products before storefront filters: scoped by tags, vendor or type.
    fn base_products(&self) -> Vec<usize> {
        let products = &self.site.store.products;
        let mut indexes = self.collection().products.clone();
        if let Some(scope) = &self.scope {
            indexes.retain(|&index| {
                let product = &products[index];
                scope
                    .tags
                    .iter()
                    .all(|tag| product.tags.iter().any(|t| handleize(t) == handleize(tag)))
                    && scope
                        .vendor
                        .as_ref()
                        .is_none_or(|vendor| product.vendor.eq_ignore_ascii_case(vendor))
                    && scope
                        .product_type
                        .as_ref()
                        .is_none_or(|kind| product.product_type.eq_ignore_ascii_case(kind))
            });
        }
        indexes
    }

    fn sort_order(&self) -> SortOrder {
        self.scope
            .as_ref()
            .and_then(|_| self.site.request.param("sort_by"))
            .and_then(SortOrder::parse)
            .unwrap_or(self.collection().sort_order)
    }

    /// The products after sorting and filtering, across all pages.
    pub fn product_indexes(&self) -> Vec<usize> {
        let products = &self.site.store.products;
        let mut indexes = self.base_products();
        if self.scope.is_some() {
            let criteria = criteria(&self.site.request);
            indexes.retain(|&index| matches(&products[index], &criteria));
        }
        sort_products(products, &mut indexes, self.sort_order());
        indexes
    }

    fn unique(&self, indexes: &[usize], pick: impl Fn(&Product) -> Vec<String>) -> Value {
        let mut values: Vec<String> = Vec::new();
        for &index in indexes {
            for value in pick(&self.site.store.products[index]) {
                if !value.is_empty() && !values.contains(&value) {
                    values.push(value);
                }
            }
        }
        values.sort_by_key(|value| value.to_lowercase());
        strings(&values)
    }

    fn filters(&self) -> Value {
        let site = &self.site;
        let products = &site.store.products;
        let base = self.base_products();
        let active = if self.scope.is_some() {
            criteria(&site.request)
        } else {
            Vec::new()
        };
        let page = site.request.localized(&site.request.path);
        // The query without a given filter parameter (or one of its values), keeping the rest.
        let query_without = |name: &str, value: Option<&str>| -> String {
            let pairs: Vec<String> = site
                .request
                .query
                .iter()
                .filter(|(key, existing)| {
                    key != "page" && !(key == name && value.is_none_or(|value| value == existing))
                })
                .map(|(key, value)| {
                    format!("{}={}", encode_component(key), encode_component(value))
                })
                .collect();
            if pairs.is_empty() {
                page.clone()
            } else {
                format!("{page}?{}", pairs.join("&"))
            }
        };
        let query_with = |name: &str, value: &str| -> String {
            let mut pairs: Vec<String> = site
                .request
                .query
                .iter()
                .filter(|(key, _)| key != "page")
                .map(|(key, value)| {
                    format!("{}={}", encode_component(key), encode_component(value))
                })
                .collect();
            pairs.push(format!(
                "{}={}",
                encode_component(name),
                encode_component(value)
            ));
            format!("{page}?{}", pairs.join("&"))
        };
        // Counts are computed with every other filter applied, as Shopify's facets are.
        let count_with =
            |criterion: Criterion, same_filter: &dyn Fn(&Criterion) -> bool| -> usize {
                let mut others: Vec<Criterion> =
                    active.iter().filter(|c| !same_filter(c)).cloned().collect();
                others.push(criterion);
                base.iter()
                    .filter(|&&index| matches(&products[index], &others))
                    .count()
            };
        let list_filter = |label: &str,
                           param: &str,
                           values: Vec<(String, String, Criterion)>,
                           swatch_of: &dyn Fn(&str) -> Option<Swatch>,
                           same_filter: &dyn Fn(&Criterion) -> bool|
         -> Value {
            // A filter whose values have swatches is presented with them.
            let has_swatches = values.iter().any(|(value, ..)| swatch_of(value).is_some());
            let values: Vec<Value> = values
                .into_iter()
                .map(|(value, label, criterion)| {
                    let is_active = active.contains(&criterion);
                    hash([
                        ("param_name", Value::from(param)),
                        ("value", Value::from(&value)),
                        ("label", Value::from(label)),
                        ("active", Value::Bool(is_active)),
                        ("count", Value::from(count_with(criterion, same_filter))),
                        ("url_to_add", Value::from(query_with(param, &value))),
                        (
                            "url_to_remove",
                            Value::from(query_without(param, Some(&value))),
                        ),
                        ("swatch", swatch_value(site, swatch_of(&value).as_ref())),
                        ("image", Value::Nil),
                    ])
                })
                .collect();
            let (active_values, inactive_values): (Vec<Value>, Vec<Value>) = values
                .iter()
                .cloned()
                .partition(|value| value.get("active").is_truthy());
            hash([
                ("param_name", Value::from(param)),
                ("label", Value::from(label)),
                ("type", Value::str("list")),
                ("operator", Value::str("OR")),
                (
                    "presentation",
                    Value::str(if has_swatches { "swatch" } else { "text" }),
                ),
                ("values", Value::array(values)),
                ("active_values", Value::array(active_values)),
                ("inactive_values", Value::array(inactive_values)),
                ("url_to_remove", Value::from(query_without(param, None))),
            ])
        };

        let mut filters = Vec::new();
        filters.push(list_filter(
            "Availability",
            "filter.v.availability",
            vec![
                (
                    "1".to_string(),
                    "In stock".to_string(),
                    Criterion::Availability(true),
                ),
                (
                    "0".to_string(),
                    "Out of stock".to_string(),
                    Criterion::Availability(false),
                ),
            ],
            &|_| None,
            &|c| matches!(c, Criterion::Availability(_)),
        ));

        let range_max = base
            .iter()
            .map(|&index| products[index].price_max())
            .max()
            .unwrap_or(0);
        let bound = |name: &str| -> Value {
            let current = site.request.param(name).and_then(parse_price);
            hash([
                ("param_name", Value::from(name)),
                ("value", current.map_or(Value::Nil, Value::Int)),
                ("active", Value::Bool(current.is_some())),
                ("label", Value::Nil),
                ("count", Value::Nil),
                ("url_to_remove", Value::from(query_without(name, None))),
            ])
        };
        filters.push(hash([
            ("param_name", Value::str("filter.v.price")),
            ("label", Value::str("Price")),
            ("type", Value::str("price_range")),
            ("operator", Value::str("AND")),
            ("presentation", Value::str("text")),
            ("min_value", bound("filter.v.price.gte")),
            ("max_value", bound("filter.v.price.lte")),
            ("range_max", Value::Int(range_max)),
            ("values", Value::array(Vec::new())),
            ("active_values", Value::array(Vec::new())),
            ("inactive_values", Value::array(Vec::new())),
            (
                "url_to_remove",
                Value::from({
                    let pairs: Vec<String> = site
                        .request
                        .query
                        .iter()
                        .filter(|(key, _)| key != "page" && !key.starts_with("filter.v.price."))
                        .map(|(key, value)| {
                            format!("{}={}", encode_component(key), encode_component(value))
                        })
                        .collect();
                    if pairs.is_empty() {
                        page.clone()
                    } else {
                        format!("{page}?{}", pairs.join("&"))
                    }
                }),
            ),
        ]));

        let distinct = |pick: &dyn Fn(&Product) -> Vec<String>| -> Vec<String> {
            let mut values: Vec<String> = Vec::new();
            for &index in &base {
                for value in pick(&products[index]) {
                    if !value.is_empty() && !values.contains(&value) {
                        values.push(value);
                    }
                }
            }
            values
        };
        let mut types = distinct(&|product| vec![product.product_type.clone()]);
        types.sort();
        if types.len() > 1 {
            filters.push(list_filter(
                "Product type",
                "filter.p.product_type",
                types
                    .into_iter()
                    .map(|kind| (kind.clone(), kind.clone(), Criterion::ProductType(kind)))
                    .collect(),
                &|_| None,
                &|c| matches!(c, Criterion::ProductType(_)),
            ));
        }
        let mut vendors = distinct(&|product| vec![product.vendor.clone()]);
        vendors.sort();
        if vendors.len() > 1 {
            filters.push(list_filter(
                "Brand",
                "filter.p.vendor",
                vendors
                    .into_iter()
                    .map(|vendor| (vendor.clone(), vendor.clone(), Criterion::Vendor(vendor)))
                    .collect(),
                &|_| None,
                &|c| matches!(c, Criterion::Vendor(_)),
            ));
        }
        // One filter per option name, in order of first appearance.
        let mut option_names: Vec<String> = Vec::new();
        for &index in &base {
            for option in &products[index].options {
                if option.name != "Title" && !option_names.contains(&option.name) {
                    option_names.push(option.name.clone());
                }
            }
        }
        for name in option_names {
            let handle = handleize(&name);
            let values = distinct(&|product| {
                product
                    .options
                    .iter()
                    .filter(|option| option.name == name)
                    .flat_map(|option| option.values.clone())
                    .collect()
            });
            let same_handle = handle.clone();
            // The swatch of a value: the one a product of the collection gives it.
            let swatch_of = |value: &str| {
                base.iter().find_map(|&index| {
                    products[index]
                        .options
                        .iter()
                        .filter(|option| option.name == name)
                        .find_map(|option| option.swatch(value))
                        .cloned()
                })
            };
            filters.push(list_filter(
                &name,
                &format!("filter.v.option.{handle}"),
                values
                    .into_iter()
                    .map(|value| (value.clone(), value.clone(), Criterion::VariantOption(handle.clone(), value)))
                    .collect(),
                &swatch_of,
                &move |c| matches!(c, Criterion::VariantOption(option, _) if *option == same_handle),
            ));
        }
        Value::array(filters)
    }
}

pub fn collection_url(site: &SiteRef, collection: &Collection) -> String {
    site.request
        .localized(&format!("/collections/{}", collection.handle))
}

pub fn sort_options() -> Value {
    Value::array(
        SORT_OPTIONS
            .iter()
            .map(|(value, name)| hash([("name", Value::str(name)), ("value", Value::str(value))]))
            .collect(),
    )
}

impl Object for CollectionDrop {
    fn type_name(&self) -> &str {
        "collection"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let collection = self.collection();
        Some(match key {
            "id" => Value::Int(collection.id as i64),
            "title" => match self
                .scope
                .as_ref()
                .and_then(|scope| scope.vendor.clone().or(scope.product_type.clone()))
            {
                Some(title) => Value::from(title),
                None => Value::from(&collection.title),
            },
            "handle" => Value::from(&collection.handle),
            "description" => Value::from(&collection.description),
            "url" => Value::from(self.url()),
            "template_suffix" => collection
                .template_suffix
                .as_ref()
                .map_or_else(Value::empty_string, Value::from),
            "image" | "featured_image" => {
                // A collection without an image falls back to its first product's image.
                match (&collection.image, key) {
                    (Some(image), _) => ImageDrop::value(site, image),
                    (None, "featured_image") => self
                        .collection()
                        .products
                        .iter()
                        .find_map(|&index| site.store.products[index].images().next())
                        .map_or(Value::Nil, |image| ImageDrop::value(site, image)),
                    (None, _) => Value::Nil,
                }
            }
            "products" => self.memo.get("products", || {
                PaginatedList::value(
                    self.product_indexes()
                        .into_iter()
                        .map(|index| ProductDrop::value(site, index))
                        .collect(),
                )
            }),
            "products_count" => Value::from(self.product_indexes().len()),
            "all_products_count" => Value::from(self.base_products().len()),
            "all_tags" => self.unique(&self.collection().products, |product| product.tags.clone()),
            "tags" => self.unique(&self.product_indexes(), |product| product.tags.clone()),
            "all_types" => self.unique(&self.collection().products, |product| {
                vec![product.product_type.clone()]
            }),
            "all_vendors" => self.unique(&self.collection().products, |product| {
                vec![product.vendor.clone()]
            }),
            "current_vendor" => self
                .scope
                .as_ref()
                .and_then(|scope| scope.vendor.clone())
                .map_or(Value::Nil, Value::from),
            "current_type" => self
                .scope
                .as_ref()
                .and_then(|scope| scope.product_type.clone())
                .map_or(Value::Nil, Value::from),
            "sort_options" => sort_options(),
            "sort_by" => self
                .scope
                .as_ref()
                .and_then(|_| site.request.param("sort_by"))
                .map_or_else(Value::empty_string, Value::from),
            "default_sort_by" => Value::str(collection.sort_order.as_str()),
            "filters" => self.memo.get("filters", || self.filters()),
            "published_at" => time_value(site, collection.published_at),
            "updated_at" => time_value(site, collection.updated_at),
            "metafields" => self.memo.get("metafields", || {
                MetafieldsDrop::value(site, &collection.metafields)
            }),
            "next_product" | "previous_product" => Value::Nil,
            _ => return None,
        })
    }

    fn to_json(&self) -> Json {
        let collection = self.collection();
        json!({
            "id": collection.id,
            "handle": collection.handle,
            "title": collection.title,
            "updated_at": super::product::iso(&self.site, collection.updated_at),
            "body_html": collection.description,
            "published_at": super::product::iso(&self.site, collection.published_at),
            "sort_order": collection.sort_order.as_str(),
            "template_suffix": collection.template_suffix,
            "disjunctive": false,
            "rules": [],
            "published_scope": "web",
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("collection:{}", self.collection().id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
