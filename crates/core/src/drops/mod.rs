//! Shopify's Liquid objects ("drops"), backed by the store, the session and the request.
//!
//! Each drop resolves its properties on demand: nothing is computed for properties a template
//! never reads, and relations (a variant's product, a product's collections) can be circular.

pub mod cart;
pub mod collection;
pub mod color;
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
pub mod shop;

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use slt_liquid::time::Time;
use slt_liquid::{Hash, Value};

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

impl slt_liquid::Object for PaginatedList {
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
