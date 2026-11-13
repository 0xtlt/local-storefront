//! Shopify's Liquid objects ("drops"), backed by the store, the session and the request.
//!
//! Each drop resolves its properties on demand: nothing is computed for properties a template
//! never reads, and relations (a variant's product, a product's collections) can be circular.

pub mod cart;
pub mod collection;
pub mod color;
pub mod company;
pub mod content;
pub mod customer;
pub mod font;
pub mod gift_card;
pub mod lists;
pub mod localization;
pub mod media;
pub mod metafield;
pub mod navigation;
pub mod product;
pub mod request;
pub mod search;
pub mod selling_plan;
pub mod shop;

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use lsf_liquid::time::Time;
use lsf_liquid::{Hash, Value};

use crate::site::Site;

/// Memoizes lazily computed properties, so that repeated lookups return the same value (and
/// the same `Arc`, which pagination relies on).
#[derive(Default)]
pub(crate) struct Memo(Mutex<Vec<(&'static str, Value)>>);

impl Memo {
    pub fn get(&self, key: &'static str, compute: impl FnOnce() -> Value) -> Value {
        if let Some(value) = self.peek(key) {
            return value;
        }
        // Computed outside the lock: the computation may read other properties of the same drop.
        let value = compute();
        let mut slots = self.0.lock().expect("memo poisoned");
        match slots.iter().find(|(slot, _)| *slot == key) {
            Some((_, existing)) => existing.clone(),
            None => {
                slots.push((key, value.clone()));
                value
            }
        }
    }

    fn peek(&self, key: &'static str) -> Option<Value> {
        let slots = self.0.lock().expect("memo poisoned");
        slots
            .iter()
            .find(|(slot, _)| *slot == key)
            .map(|(_, value)| value.clone())
    }
}

/// A time displayed in the shop's time zone.
pub(crate) fn time_value(site: &Site, time: DateTime<Utc>) -> Value {
    Time::value(time.with_timezone(&site.store.shop.timezone))
}

pub(crate) fn strings(items: &[String]) -> Value {
    Value::array(items.iter().map(Value::from).collect())
}

/// A plain hash from literal pairs.
pub(crate) fn hash<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    let map: Hash = pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    Value::hash(map)
}

pub(crate) type SiteRef = Arc<Site>;

/// What Shopify gives for a handle that names nothing (`collections['nope']`,
/// `all_products['nope']`, `pages`, `blogs`, `linklists`), a metafield namespace a resource
/// does not have, and a resource setting whose handle no longer exists: the `empty` object.
/// It is truthy, `== empty` and `== blank`, and its lists (`products`, `articles`, `links`)
/// are empty, so their `size` is 0. Its other properties are nil.
///
/// <https://shopify.dev/docs/api/liquid/basics#empty>: "An `empty` object is returned if you
/// try to access an object that is defined, but has no value. For example a page or product
/// that's been deleted".
pub struct EmptyDrop {
    /// The properties that are lists on the object that was asked for.
    lists: &'static [&'static str],
    /// The handle a resource setting holds: it prints it, for the themes that still read the
    /// handle of a setting, and it is `== nil`, as on Shopify.
    setting: Option<String>,
}

impl EmptyDrop {
    pub const COLLECTION: &'static [&'static str] = &["products"];
    pub const BLOG: &'static [&'static str] = &["articles"];
    pub const LINK_LIST: &'static [&'static str] = &["links"];
    pub const NONE: &'static [&'static str] = &[];

    /// The `empty` object of a lookup that found nothing.
    pub fn value(lists: &'static [&'static str]) -> Value {
        Value::object(EmptyDrop {
            lists,
            setting: None,
        })
    }

    /// The value of a resource setting whose handle names nothing.
    pub fn setting(lists: &'static [&'static str], handle: &str) -> Value {
        Value::object(EmptyDrop {
            lists,
            setting: Some(handle.to_string()),
        })
    }
}

impl lsf_liquid::Object for EmptyDrop {
    fn type_name(&self) -> &str {
        "empty"
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.lists.contains(&key).then(|| Value::array(Vec::new()))
    }

    fn size(&self) -> Option<usize> {
        Some(0)
    }

    fn equals(&self, other: &Value) -> Option<bool> {
        (self.setting.is_some() && other.is_nil()).then_some(true)
    }

    fn render(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed(self.setting.as_deref().unwrap_or_default())
    }

    fn to_json(&self) -> serde_json::Value {
        match self.setting {
            Some(_) => serde_json::Value::Null,
            None => serde_json::json!({ "error": "json not allowed for this object" }),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The page size Shopify applies to a collection that is iterated without `paginate`.
pub const DEFAULT_PAGE_LIMIT: usize = 50;

/// An array that `{% paginate %}` can restrict to one page.
///
/// Outside `paginate` it yields at most [`DEFAULT_PAGE_LIMIT`] items, like Shopify.
pub struct PaginatedList {
    all: Arc<Vec<Value>>,
    window: Mutex<Option<(usize, usize)>>,
}

impl PaginatedList {
    pub fn value(items: Vec<Value>) -> Value {
        Value::object(PaginatedList {
            all: Arc::new(items),
            window: Mutex::new(None),
        })
    }

    /// The number of items across all pages.
    pub fn total(&self) -> usize {
        self.all.len()
    }

    /// Restricts the list to `size` items starting at `offset`, or lifts the restriction.
    pub fn set_window(&self, window: Option<(usize, usize)>) {
        *self.window.lock().expect("window poisoned") = window;
    }
}

impl lsf_liquid::Object for PaginatedList {
    fn type_name(&self) -> &str {
        "array"
    }

    fn items(&self) -> Option<Arc<Vec<Value>>> {
        let (offset, size) = self
            .window
            .lock()
            .expect("window poisoned")
            .unwrap_or((0, DEFAULT_PAGE_LIMIT));
        if offset == 0 && size >= self.all.len() {
            return Some(self.all.clone());
        }
        let start = offset.min(self.all.len());
        let end = (offset + size).min(self.all.len());
        Some(Arc::new(self.all[start..end].to_vec()))
    }

    fn index(&self, index: i64) -> Option<Value> {
        let items = self.items()?;
        let len = items.len() as i64;
        let index = if index < 0 { index + len } else { index };
        (index >= 0 && index < len).then(|| items[index as usize].clone())
    }

    fn render(&self) -> std::borrow::Cow<'_, str> {
        let mut out = String::new();
        for item in self.items().unwrap_or_default().iter() {
            item.render_to(&mut out);
        }
        std::borrow::Cow::Owned(out)
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.items()
                .unwrap_or_default()
                .iter()
                .map(Value::to_json)
                .collect(),
        )
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
