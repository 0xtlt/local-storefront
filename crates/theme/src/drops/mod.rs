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

use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use lsf_liquid::time::Time;
use lsf_liquid::{Hash, Value};

use crate::site::{PaginateWindow, Site};

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
    all: Vec<OnceLock<Value>>,
    /// Makes an item of a list made with [`PaginatedList::fetched`] the first time a page reads
    /// it.
    make: Option<Box<dyn Fn(usize) -> Value + Send + Sync>>,
    window: PaginateWindow,
    /// The type of one of Shopify's list drops, when the list is one: see
    /// [`PaginatedList::list_drop`].
    list: Option<&'static str>,
    /// The query parameter of its page, when it has one of its own: see
    /// [`PaginatedList::paged_list_drop`].
    page_param: Option<String>,
}

impl PaginatedList {
    pub fn value(items: Vec<Value>) -> Value {
        Value::object(PaginatedList {
            all: items.into_iter().map(OnceLock::from).collect(),
            make: None,
            window: PaginateWindow::default(),
            list: None,
            page_param: None,
        })
    }

    /// A list of `len` items that a storefront fetches as a page reads them, `make` making
    /// item `index`: what loading an item charges a profile is charged for the items read only.
    /// Shopify fetches the page of a `paginate`, else at most [`DEFAULT_PAGE_LIMIT`], and for
    /// `collection.products` the `limit` of a `for` loop below that, outside `paginate`
    /// (shopify.dev, "Limit how many items a Liquid array fetches").
    pub fn fetched(len: usize, make: impl Fn(usize) -> Value + Send + Sync + 'static) -> Value {
        Value::object(PaginatedList {
            all: (0..len).map(|_| OnceLock::new()).collect(),
            make: Some(Box::new(make)),
            window: PaginateWindow::default(),
            list: None,
            page_param: None,
        })
    }

    /// One of Shopify's list drops (`product_list` prints `ProductListDrop`): the value of a
    /// `list.product_reference` or `list.metaobject_reference` metafield, of a `product_list`
    /// setting and of `metaobjects.<type>.values`. It is not an array: `.first`, `.last` and
    /// `[n]` give nothing, `.count` and `.size` the number of items. The `first` and `last`
    /// filters, `for` and `paginate` reach its items, the first [`DEFAULT_PAGE_LIMIT`] outside
    /// `paginate`.
    ///
    /// <https://shopify.dev/docs/api/liquid/objects/metafield#metafield-determining-the-length-of-a-list-metafield>:
    /// a list of a reference type has `count`; the others "are rendered as arrays".
    pub fn list_drop(kind: &'static str, items: Vec<Value>) -> Value {
        Value::object(PaginatedList {
            all: items.into_iter().map(OnceLock::from).collect(),
            make: None,
            window: PaginateWindow::default(),
            list: Some(kind),
            page_param: None,
        })
    }

    /// A list drop that `paginate` pages with a query parameter of its own, `page_<hash>`, as
    /// Shopify pages a list of references and `metaobjects.<type>.values`
    /// (`/products/widget?page_b72aba52=2`). `key` names the list: each lookup of the same list
    /// has the same parameter, and shows the page `paginate` set.
    ///
    /// <https://shopify.dev/docs/apps/build/metafields/metafield-limits>: "a loop over a list
    /// of references [...] returns only the first 50 items. To access more items, use the
    /// paginate tag, which returns up to 250 items per page."
    pub fn paged_list_drop(site: &Site, kind: &'static str, key: &str, items: Vec<Value>) -> Value {
        let hash = lsf_core::util::stable_id("paginate", key) & 0xffff_ffff;
        let page_param = format!("page_{hash:08x}");
        Value::object(PaginatedList {
            all: items.into_iter().map(OnceLock::from).collect(),
            make: None,
            window: site.paginate_window(&page_param),
            list: Some(kind),
            page_param: Some(page_param),
        })
    }

    /// The query parameter of the list's page: `page`, or its own.
    pub fn page_param(&self) -> &str {
        self.page_param.as_deref().unwrap_or("page")
    }

    /// The number of items across all pages.
    pub fn total(&self) -> usize {
        self.all.len()
    }

    /// Restricts the list to `size` items starting at `offset`, or lifts the restriction.
    pub fn set_window(&self, window: Option<(usize, usize)>) {
        *self.window.lock().expect("window poisoned") = window;
    }

    fn window(&self) -> Option<(usize, usize)> {
        *self.window.lock().expect("window poisoned")
    }

    /// Items `start..end`, made when they were not.
    fn range(&self, start: usize, end: usize) -> Arc<Vec<Value>> {
        let end = end.min(self.all.len());
        let start = start.min(end);
        Arc::new(
            self.all[start..end]
                .iter()
                .enumerate()
                .map(|(at, item)| {
                    item.get_or_init(|| match &self.make {
                        Some(make) => make(start + at),
                        None => Value::Nil,
                    })
                    .clone()
                })
                .collect(),
        )
    }
}

impl lsf_liquid::Object for PaginatedList {
    fn type_name(&self) -> &str {
        self.list.unwrap_or("array")
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.list?;
        match key {
            "count" => Some(Value::from(self.total())),
            "first" | "last" => Some(Value::Nil),
            _ => None,
        }
    }

    /// The size of a list drop is all of its items, even those a loop does not reach, as
    /// Shopify's: "returns the total number of referenced entries, even when a loop without
    /// paginate returns only the first 50".
    fn size(&self) -> Option<usize> {
        match self.list {
            Some(_) => Some(self.total()),
            None => self.items().map(|items| items.len()),
        }
    }

    fn items(&self) -> Option<Arc<Vec<Value>>> {
        let (offset, size) = self.window().unwrap_or((0, DEFAULT_PAGE_LIMIT));
        Some(self.range(offset, offset.saturating_add(size)))
    }

    /// A `for` loop with a `limit`, outside `paginate`, fetches only the items it reaches.
    fn items_until(&self, to: usize) -> Option<Arc<Vec<Value>>> {
        match self.window() {
            None if self.make.is_some() && to < DEFAULT_PAGE_LIMIT => Some(self.range(0, to)),
            _ => self.items(),
        }
    }

    fn index(&self, index: i64) -> Option<Value> {
        if self.list.is_some() {
            return Some(Value::Nil);
        }
        let items = self.items()?;
        let len = items.len() as i64;
        let index = if index < 0 { index + len } else { index };
        (index >= 0 && index < len).then(|| items[index as usize].clone())
    }

    fn render(&self) -> std::borrow::Cow<'_, str> {
        if let Some(kind) = self.list {
            let mut name: String = kind
                .split('_')
                .map(|word| {
                    let mut chars = word.chars();
                    chars.next().map_or_else(String::new, |first| {
                        first.to_uppercase().chain(chars).collect()
                    })
                })
                .collect();
            name.push_str("Drop");
            return std::borrow::Cow::Owned(name);
        }
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
