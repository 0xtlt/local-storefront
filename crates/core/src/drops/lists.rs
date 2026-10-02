//! The global lookup objects: `collections`, `all_products`, `pages`, `blogs`, `articles` and
//! `images`.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use lsf_liquid::{Object, Value};

use super::collection::CollectionDrop;
use super::content::{ArticleDrop, BlogDrop, PageDrop};
use super::media::ImageDrop;
use super::metafield::image_from_src;
use super::product::ProductDrop;
use super::{DEFAULT_PAGE_LIMIT, PaginatedList, SiteRef};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Collections,
    AllProducts,
    Pages,
    Blogs,
    Articles,
    Images,
}

/// Looks resources up by handle and, for most kinds, iterates over all of them.
pub struct Lookup {
    site: SiteRef,
    kind: Kind,
    /// Drops are cached per handle so that `collections['x'].products` is the same list every
    /// time, which `paginate` relies on.
    cache: Mutex<HashMap<String, Value>>,
    all: Mutex<Option<Value>>,
}

impl Lookup {
    pub fn value(site: &SiteRef, kind: Kind) -> Value {
        Value::object(Lookup {
            site: site.clone(),
            kind,
            cache: Mutex::new(HashMap::new()),
            all: Mutex::new(None),
        })
    }

    fn find(&self, handle: &str) -> Option<Value> {
        let site = &self.site;
        let store = &site.store;
        match self.kind {
            Kind::Collections => store
                .collection_index(handle)
                .map(|index| CollectionDrop::value(site, index)),
            Kind::AllProducts => store
                .product_index(handle)
                .map(|index| ProductDrop::value(site, index)),
            Kind::Pages => store
                .page_index(handle)
                .map(|index| PageDrop::value(site, index)),
            Kind::Blogs => store
                .blog_index(handle)
                .map(|index| BlogDrop::value(site, index)),
            Kind::Articles => {
                let (blog, article) = handle.split_once('/')?;
                let blog_index = store.blog_index(blog)?;
                Some(ArticleDrop::value(
                    site,
                    blog_index,
                    store.article_index(blog_index, article)?,
                ))
            }
            Kind::Images => Some(ImageDrop::value(site, &image_from_src(site, handle))),
        }
    }

    fn all(&self) -> Option<Value> {
        let site = &self.site;
        let store = &site.store;
        let items: Vec<Value> = match self.kind {
            Kind::Collections => (0..store.collections.len())
                .map(|index| self.cached(&store.collections[index].handle))
                .collect(),
            Kind::Pages => (0..store.pages.len())
                .map(|index| PageDrop::value(site, index))
                .collect(),
            Kind::Blogs => (0..store.blogs.len())
                .map(|index| BlogDrop::value(site, index))
                .collect(),
            // `all_products`, `articles` and `images` can only be looked up by handle.
            Kind::AllProducts | Kind::Articles | Kind::Images => return None,
        };
        Some(PaginatedList::value(items))
    }

    fn cached(&self, handle: &str) -> Value {
        if let Some(value) = self
            .cache
            .lock()
            .expect("lookup cache poisoned")
            .get(handle)
        {
            return value.clone();
        }
        let value = self.find(handle).unwrap_or(Value::Nil);
        self.cache
            .lock()
            .expect("lookup cache poisoned")
            .insert(handle.to_string(), value.clone());
        value
    }

    fn list(&self) -> Option<Value> {
        let mut all = self.all.lock().expect("lookup cache poisoned");
        if all.is_none() {
            *all = self.all();
        }
        all.clone()
    }
}

impl Object for Lookup {
    fn type_name(&self) -> &str {
        match self.kind {
            Kind::Collections => "collections",
            Kind::AllProducts => "all_products",
            Kind::Pages => "pages",
            Kind::Blogs => "blogs",
            Kind::Articles => "articles",
            Kind::Images => "images",
        }
    }

    fn get(&self, key: &str) -> Option<Value> {
        Some(self.cached(key))
    }

    fn items(&self) -> Option<Arc<Vec<Value>>> {
        self.list()?.items()
    }

    fn size(&self) -> Option<usize> {
        let store = &self.site.store;
        Some(match self.kind {
            Kind::Collections => store.collections.len(),
            Kind::AllProducts => store.products.len().min(DEFAULT_PAGE_LIMIT),
            Kind::Pages => store.pages.len(),
            Kind::Blogs => store.blogs.len(),
            Kind::Articles => store.blogs.iter().map(|blog| blog.articles.len()).sum(),
            Kind::Images => store.files.len(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Lookup {
    /// The underlying list, for `{% paginate collections by 10 %}`.
    pub fn paginated(&self) -> Option<Value> {
        self.list()
    }
}
