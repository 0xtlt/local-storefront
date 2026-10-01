//! `metafields`, `metafield`, `metaobjects` and `metaobject`.

use std::any::Any;
use std::borrow::Cow;
use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::Value as Json;
use slt_liquid::time::Time;
use slt_liquid::{Object, Value};

use super::media::ImageDrop;
use super::{SiteRef, hash};
use crate::store::{Image, Metafield, Metafields, Metaobject};
use crate::util::stable_id;

/// `resource.metafields`: namespaces of metafields.
pub struct MetafieldsDrop {
    site: SiteRef,
    metafields: Metafields,
}

impl MetafieldsDrop {
    pub fn value(site: &SiteRef, metafields: &Metafields) -> Value {
        Value::object(MetafieldsDrop {
            site: site.clone(),
            metafields: metafields.clone(),
        })
    }
}

impl Object for MetafieldsDrop {
    fn type_name(&self) -> &str {
        "metafields"
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.metafields.get(key).map(|fields| {
            Value::object(FieldsDrop {
                site: self.site.clone(),
                fields: fields.clone(),
                kind: "metafield_namespace",
            })
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A set of named metafields: a metafield namespace, or the fields of a metaobject.
struct FieldsDrop {
    site: SiteRef,
    fields: IndexMap<String, Metafield>,
    kind: &'static str,
}

impl Object for FieldsDrop {
    fn type_name(&self) -> &str {
        self.kind
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.fields
            .get(key)
            .map(|field| MetafieldDrop::value(&self.site, field))
    }

    /// Iterating yields `[key, metafield]` pairs, like a hash.
    fn items(&self) -> Option<Arc<Vec<Value>>> {
        Some(Arc::new(
            self.fields
                .iter()
                .map(|(key, field)| {
                    Value::array(vec![
                        Value::from(key),
                        MetafieldDrop::value(&self.site, field),
                    ])
                })
                .collect(),
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct MetafieldDrop {
    pub site: SiteRef,
    pub field: Metafield,
}

impl MetafieldDrop {
    pub fn value(site: &SiteRef, field: &Metafield) -> Value {
        Value::object(MetafieldDrop {
            site: site.clone(),
            field: field.clone(),
        })
    }

    pub fn is_list(&self) -> bool {
        self.field.kind.starts_with("list.")
    }

    /// The type of the items for list metafields, the type itself otherwise.
    pub fn base_kind(&self) -> &str {
        self.field
            .kind
            .strip_prefix("list.")
            .unwrap_or(&self.field.kind)
    }

    pub fn typed_value(&self) -> Value {
        if self.is_list() {
            let items = self.field.value.as_array().cloned().unwrap_or_default();
            Value::array(
                items
                    .iter()
                    .map(|item| typed(&self.site, self.base_kind(), item))
                    .collect(),
            )
        } else {
            typed(&self.site, &self.field.kind, &self.field.value)
        }
    }
}

/// A file reference: an image when the extension says so.
fn file_value(site: &SiteRef, src: &str) -> Value {
    let extension = src.rsplit('.').next().unwrap_or_default().to_lowercase();
    if matches!(
        extension.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "svg" | "bmp"
    ) {
        return ImageDrop::value(site, &image_from_src(site, src));
    }
    hash([
        ("url", Value::from(crate::urls::file_url(site, src))),
        ("media_type", Value::str("generic_file")),
        ("id", Value::Int(stable_id("file", src) as i64)),
        ("alt", Value::empty_string()),
        ("preview_image", Value::Nil),
    ])
}

/// An image known only by its path in `files/`, with the metadata declared for that file.
pub fn image_from_src(site: &SiteRef, src: &str) -> Image {
    let meta = site.store.files.get(src);
    Image {
        id: stable_id("image", src),
        src: src.to_string(),
        alt: meta.map(|meta| meta.alt.clone()).unwrap_or_default(),
        width: meta.and_then(|meta| meta.width).unwrap_or(1200),
        height: meta.and_then(|meta| meta.height).unwrap_or(1200),
        focal_point: meta.and_then(|meta| meta.focal_point),
    }
}

fn typed(site: &SiteRef, kind: &str, value: &Json) -> Value {
    let text = || value.as_str().unwrap_or_default();
    match kind {
        "number_integer" => value
            .as_i64()
            .map_or_else(|| Value::from(value), Value::Int),
        "number_decimal" => match value {
            Json::String(text) => text
                .parse::<f64>()
                .map_or_else(|_| Value::from(value), Value::Float),
            other => Value::from(other),
        },
        "boolean" => Value::Bool(value.as_bool().unwrap_or(false)),
        "date" | "date_time" => Time::parse_iso(text(), site.store.shop.timezone)
            .map(Value::object)
            .or_else(|| {
                slt_liquid::time::parse_time(
                    &text().to_lowercase(),
                    site.store.shop.timezone,
                    site.now,
                )
                .map(Time::value)
            })
            .unwrap_or_else(|| Value::from(value)),
        "money" => match value {
            Json::Object(map) => {
                let cents = map
                    .get("amount")
                    .and_then(|amount| match amount {
                        Json::String(text) => {
                            crate::store::model::Money::Decimal(text.clone()).cents()
                        }
                        Json::Number(number) => number
                            .as_f64()
                            .map(|amount| (amount * 100.0).round() as i64),
                        _ => None,
                    })
                    .unwrap_or(0);
                Value::Int(cents)
            }
            other => Value::from(other),
        },
        "rating" => hash([
            ("rating", decimal(value.get("value"))),
            ("scale_min", decimal(value.get("scale_min"))),
            ("scale_max", decimal(value.get("scale_max"))),
        ]),
        "weight" | "volume" | "dimension" => hash([
            ("type", Value::str(kind)),
            ("value", decimal(value.get("value"))),
            (
                "unit",
                Value::from(value.get("unit").and_then(Json::as_str).unwrap_or_default()),
            ),
        ]),
        "product_reference" => site
            .store
            .product_index(text())
            .map_or(Value::Nil, |index| {
                super::product::ProductDrop::value(site, index)
            }),
        "collection_reference" => site
            .store
            .collection_index(text())
            .map_or(Value::Nil, |index| {
                super::collection::CollectionDrop::value(site, index)
            }),
        "page_reference" => site.store.page_index(text()).map_or(Value::Nil, |index| {
            super::content::PageDrop::value(site, index)
        }),
        "variant_reference" => {
            let location =
                match value {
                    Json::Number(number) => number
                        .as_u64()
                        .and_then(|id| site.store.variant_location(id)),
                    _ => site.store.products.iter().enumerate().find_map(
                        |(product_index, product)| {
                            product
                                .variants
                                .iter()
                                .position(|variant| variant.sku == text())
                                .map(|variant_index| (product_index, variant_index))
                        },
                    ),
                };
            location.map_or(Value::Nil, |(product, variant)| {
                super::product::VariantDrop::value(site, product, variant)
            })
        }
        "file_reference" => file_value(site, text()),
        "metaobject_reference" | "mixed_reference" => text()
            .split_once('/')
            .and_then(|(kind, handle)| site.store.metaobject(kind, handle))
            .map_or(Value::Nil, |entry| MetaobjectDrop::value(site, entry)),
        _ => Value::from(value),
    }
}

fn decimal(value: Option<&Json>) -> Value {
    match value {
        Some(Json::String(text)) => text
            .parse::<f64>()
            .map_or_else(|_| Value::from(text), Value::Float),
        Some(other) => Value::from(other),
        None => Value::Nil,
    }
}

impl Object for MetafieldDrop {
    fn type_name(&self) -> &str {
        "metafield"
    }

    fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "value" => self.typed_value(),
            "type" => Value::from(&self.field.kind),
            "list?" => Value::Bool(self.is_list()),
            _ => return None,
        })
    }

    /// Printing a metafield prints its value, as Shopify still does for legacy themes.
    fn render(&self) -> Cow<'_, str> {
        Cow::Owned(self.typed_value().to_str().into_owned())
    }

    fn to_json(&self) -> Json {
        self.field.value.clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct MetaobjectDrop {
    site: SiteRef,
    entry: Metaobject,
}

impl MetaobjectDrop {
    pub fn value(site: &SiteRef, entry: &Metaobject) -> Value {
        Value::object(MetaobjectDrop {
            site: site.clone(),
            entry: entry.clone(),
        })
    }
}

impl Object for MetaobjectDrop {
    fn type_name(&self) -> &str {
        "metaobject"
    }

    fn get(&self, key: &str) -> Option<Value> {
        if key == "system" {
            return Some(hash([
                ("type", Value::from(&self.entry.kind)),
                ("handle", Value::from(&self.entry.handle)),
                (
                    "id",
                    Value::Int(stable_id(
                        "metaobject",
                        &format!("{}/{}", self.entry.kind, self.entry.handle),
                    ) as i64),
                ),
                (
                    "url",
                    Value::from(
                        self.site.request.localized(&format!(
                            "/pages/{}/{}",
                            self.entry.kind, self.entry.handle
                        )),
                    ),
                ),
            ]));
        }
        self.entry
            .fields
            .get(key)
            .map(|field| MetafieldDrop::value(&self.site, field))
    }

    fn identity(&self) -> Option<String> {
        Some(format!(
            "metaobject:{}/{}",
            self.entry.kind, self.entry.handle
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The global `metaobjects`: `metaobjects.<type>.<handle>` and `metaobjects.<type>.values`.
pub struct MetaobjectsDrop {
    pub site: SiteRef,
}

struct MetaobjectTypeDrop {
    site: SiteRef,
    kind: String,
}

impl Object for MetaobjectsDrop {
    fn type_name(&self) -> &str {
        "metaobjects"
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.site
            .store
            .metaobjects
            .iter()
            .any(|entry| entry.kind == key)
            .then(|| {
                Value::object(MetaobjectTypeDrop {
                    site: self.site.clone(),
                    kind: key.to_string(),
                })
            })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Object for MetaobjectTypeDrop {
    fn type_name(&self) -> &str {
        "metaobject_definition"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let entries = || {
            self.site
                .store
                .metaobjects
                .iter()
                .filter(|entry| entry.kind == self.kind)
        };
        match key {
            "values" => Some(Value::array(
                entries()
                    .map(|entry| MetaobjectDrop::value(&self.site, entry))
                    .collect(),
            )),
            "values_count" => Some(Value::from(entries().count())),
            handle => entries()
                .find(|entry| entry.handle == handle)
                .map(|entry| MetaobjectDrop::value(&self.site, entry)),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
