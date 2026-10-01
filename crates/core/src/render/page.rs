//! What a URL resolves to: the template to render and the resource it is about.

use crate::drops::collection::CollectionScope;
use crate::drops::request::TemplateName;

#[derive(Clone, Debug)]
pub enum Resource {
    Index,
    Product {
        product: usize,
        /// The collection in `/collections/<handle>/products/<handle>`.
        collection: Option<usize>,
    },
    Collection {
        collection: usize,
        scope: CollectionScope,
    },
    ListCollections,
    Page(usize),
    Blog {
        blog: usize,
        tags: Vec<String>,
    },
    Article {
        blog: usize,
        article: usize,
    },
    Cart,
    Search,
    Policy(String),
    Password,
    /// A classic customer account page: `login`, `register`, `account`, `addresses`, `order`...
    Customers {
        order: Option<u64>,
    },
    GiftCard(usize),
    NotFound,
}

#[derive(Clone, Debug)]
pub struct Page {
    pub template: TemplateName,
    pub resource: Resource,
    /// `request.page_type`.
    pub page_type: String,
    pub status: u16,
    /// The title before the shop name is appended.
    pub title: String,
    pub description: String,
    /// The canonical path of the page, without the locale prefix.
    pub canonical_path: String,
    /// The global `handle`.
    pub handle: Option<String>,
}

impl Page {
    pub fn new(template: &str, resource: Resource) -> Page {
        Page {
            template: TemplateName::new(template),
            resource,
            page_type: template.to_string(),
            status: 200,
            title: String::new(),
            description: String::new(),
            canonical_path: "/".to_string(),
            handle: None,
        }
    }
}
