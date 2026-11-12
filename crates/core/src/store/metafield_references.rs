//! What the value of a reference metafield names (`product_reference`, `metaobject_reference`,
//! …), for Liquid and for `validate` alike: what one resolves, the other takes.
//!
//! The store data names an entity by the key it gives it: a handle (`"ceramic-mug"`), a
//! `blog/article`, the `type/handle` of a metaobject entry, the SKU or the id (a number) of a
//! variant. Shopify's own global id (`gid://shopify/Product/<id>`) names the entity of that id
//! in the store: the `id` the data gives it or lsf's own, which Liquid shows (`product.id`,
//! `metaobject.system.id`) and the Admin API answers.

use serde_json::Value as Json;

use super::{Metaobject, Store};
use crate::util::{handleize, stable_id};

/// An entity a reference names, by its place in the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Referenced {
    /// An index of [`Store::products`].
    Product(usize),
    /// A product index and a variant index.
    Variant(usize, usize),
    /// An index of [`Store::collections`].
    Collection(usize),
    /// An index of [`Store::pages`].
    Page(usize),
    /// A blog index and an article index.
    Article(usize, usize),
    /// An index of [`Store::metaobjects`].
    Metaobject(usize),
}

/// The reference types lsf resolves, which read `nil` in Liquid when they name nothing.
pub fn is_resolved(kind: &str) -> bool {
    matches!(
        kind,
        "product_reference"
            | "variant_reference"
            | "collection_reference"
            | "page_reference"
            | "article_reference"
            | "metaobject_reference"
            | "mixed_reference"
    )
}

/// How the store data names what a reference type takes, for messages.
pub fn accepted_form(kind: &str) -> &'static str {
    match kind {
        "product_reference" => "a product by its handle",
        "variant_reference" => "a variant by its SKU or its id (a number)",
        "collection_reference" => "a collection by its handle",
        "page_reference" => "a page by its handle",
        "article_reference" => "an article as \"blog/article\"",
        "metaobject_reference" | "mixed_reference" => "a metaobject entry as \"type/handle\"",
        "file_reference" => "a file by its path inside files/",
        _ => "an entity by its handle",
    }
}

/// What a reference type names, for messages: `product`, `metaobject entry`.
pub fn entity(kind: &str) -> &'static str {
    match kind {
        "product_reference" => "product",
        "variant_reference" => "variant",
        "collection_reference" => "collection",
        "page_reference" => "page",
        "article_reference" => "article",
        "metaobject_reference" | "mixed_reference" => "metaobject entry",
        "file_reference" => "file",
        _ => "entity",
    }
}

/// The type of the GIDs a reference type takes: `Product` for a `product_reference`.
pub fn gid_type(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "product_reference" => "Product",
        "variant_reference" => "ProductVariant",
        "collection_reference" => "Collection",
        "page_reference" => "Page",
        "article_reference" => "Article",
        "metaobject_reference" | "mixed_reference" => "Metaobject",
        _ => return None,
    })
}

/// The type and the number of a `gid://shopify/<type>/<id>`.
pub fn parse_gid(text: &str) -> Option<(&str, u64)> {
    let (kind, id) = text.strip_prefix("gid://shopify/")?.split_once('/')?;
    Some((kind, id.parse().ok()?))
}

impl Metaobject {
    /// The id of the entry: `metaobject.system.id` in Liquid, the number of its GID in the
    /// Admin API. The store data gives none: it is made from the type and the handle.
    pub fn id(&self) -> u64 {
        stable_id("metaobject", &format!("{}/{}", self.kind, self.handle))
    }
}

impl Store {
    /// What the value of a reference of type `kind` (an item's type for a list:
    /// `product_reference`) names on the storefront, or `None`: a reference that names nothing,
    /// a product that is not on the storefront, and the types lsf does not resolve (files,
    /// customers, companies, taxonomy values).
    pub fn metafield_reference(&self, kind: &str, value: &Json) -> Option<Referenced> {
        let text = value.as_str();
        if let Some(text) = text.filter(|text| text.starts_with("gid://")) {
            let (gid_kind, id) = parse_gid(text)?;
            if Some(gid_kind) != gid_type(kind) {
                return None;
            }
            return self.by_id(kind, id);
        }
        match kind {
            "product_reference" => self.product_index(text?).map(Referenced::Product),
            "variant_reference" => {
                let (product, variant) = match value {
                    Json::Number(number) => self.variant_location(number.as_u64()?)?,
                    _ => self.variant_by_sku(text?)?,
                };
                Some(Referenced::Variant(product, variant))
            }
            "collection_reference" => self.collection_index(text?).map(Referenced::Collection),
            "page_reference" => self.page_index(text?).map(Referenced::Page),
            "article_reference" => {
                let text = text?;
                let (blog, article) = match text.split_once('/') {
                    Some((blog, article)) => {
                        let blog = self.blog_index(blog)?;
                        (blog, self.article_index(blog, article)?)
                    }
                    None => self.only_article(text)?,
                };
                Some(Referenced::Article(blog, article))
            }
            "metaobject_reference" | "mixed_reference" => {
                let (kind, handle) = text?.split_once('/')?;
                self.metaobject_index(kind, handle)
                    .map(Referenced::Metaobject)
            }
            _ => None,
        }
    }

    /// Whether a `product_reference` names a product that is not on the storefront: it reads
    /// `nil` in Liquid, as on Shopify, and is no mistake of the data.
    pub fn names_unlisted_product(&self, value: &Json) -> bool {
        let Some(text) = value.as_str() else {
            return false;
        };
        match parse_gid(text) {
            Some(("Product", id)) => self.unlisted_products.iter().any(|p| p.id == id),
            _ => self.unlisted_products.iter().any(|p| p.handle == text),
        }
    }

    /// For a reference that names nothing, the value that names what it seems to mean, as the
    /// store data writes it: `"faq/size"` for `"faq.size"`, `"faq:size"` or `"size"` (the only
    /// entry with that handle), `"ceramic-mug"` for `"/products/ceramic-mug"` or
    /// `"Ceramic Mug"`, the number `4410` for the variant id `"4410"`.
    pub fn reference_written_as_the_data_does(&self, kind: &str, value: &Json) -> Option<Json> {
        let text = value.as_str()?.trim();
        if text.starts_with("gid://") {
            return None;
        }
        // The last part of a path or URL, without its query: `/products/ceramic-mug?variant=1`.
        let last = |text: &str| {
            let text = text.split(['?', '#']).next().unwrap_or_default();
            handleize(
                text.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default(),
            )
        };
        let found = match kind {
            "product_reference" => {
                let handle = last(text);
                self.product_index(&handle).map(|_| Json::from(handle))
            }
            "collection_reference" => {
                let handle = last(text);
                self.collection_index(&handle).map(|_| Json::from(handle))
            }
            "page_reference" => {
                let handle = last(text);
                self.page_index(&handle).map(|_| Json::from(handle))
            }
            "variant_reference" => match text.parse::<u64>() {
                Ok(id) if self.variant_location(id).is_some() => Some(Json::from(id)),
                _ => self
                    .products
                    .iter()
                    .flat_map(|product| &product.variants)
                    .find(|variant| {
                        !variant.sku.is_empty() && variant.sku.eq_ignore_ascii_case(text)
                    })
                    .map(|variant| Json::from(variant.sku.clone())),
            },
            "article_reference" => {
                let found = split_pair(text).and_then(|(blog, article)| {
                    let blog = self.blog_index(&handleize(blog))?;
                    Some((blog, self.article_index(blog, &handleize(article))?))
                });
                found.map(|(blog, article)| {
                    let blog = &self.blogs[blog];
                    Json::from(format!("{}/{}", blog.handle, blog.articles[article].handle))
                })
            }
            "metaobject_reference" | "mixed_reference" => {
                let entry = match split_pair(text) {
                    Some((kind, handle)) => self.metaobject_index(kind, &handleize(handle)),
                    None => {
                        // A handle alone, when one entry only has it.
                        let handle = handleize(text);
                        let mut entries = self
                            .metaobjects
                            .iter()
                            .enumerate()
                            .filter(|(_, entry)| entry.handle == handle);
                        match (entries.next(), entries.next()) {
                            (Some((index, _)), None) => Some(index),
                            _ => None,
                        }
                    }
                };
                entry.map(|entry| {
                    let entry = &self.metaobjects[entry];
                    Json::from(format!("{}/{}", entry.kind, entry.handle))
                })
            }
            _ => None,
        };
        found.filter(|found| found != value)
    }

    fn by_id(&self, kind: &str, id: u64) -> Option<Referenced> {
        match kind {
            "product_reference" => self.product_index_by_id(id).map(Referenced::Product),
            "variant_reference" => self
                .variant_location(id)
                .map(|(product, variant)| Referenced::Variant(product, variant)),
            "collection_reference" => self
                .collections
                .iter()
                .position(|collection| collection.id == id)
                .map(Referenced::Collection),
            "page_reference" => self
                .pages
                .iter()
                .position(|page| page.id == id)
                .map(Referenced::Page),
            "article_reference" => self.blogs.iter().enumerate().find_map(|(blog, b)| {
                let article = b.articles.iter().position(|article| article.id == id)?;
                Some(Referenced::Article(blog, article))
            }),
            "metaobject_reference" | "mixed_reference" => self
                .metaobjects
                .iter()
                .position(|entry| entry.id() == id)
                .map(Referenced::Metaobject),
            _ => None,
        }
    }

    fn variant_by_sku(&self, sku: &str) -> Option<(usize, usize)> {
        self.products
            .iter()
            .enumerate()
            .find_map(|(product_index, product)| {
                let variant = product.variants.iter().position(|v| v.sku == sku)?;
                Some((product_index, variant))
            })
    }

    /// The article of that handle, when one blog only has one.
    fn only_article(&self, handle: &str) -> Option<(usize, usize)> {
        let mut found = (0..self.blogs.len())
            .filter_map(|blog| Some((blog, self.article_index(blog, handle)?)));
        match (found.next(), found.next()) {
            (Some(found), None) => Some(found),
            _ => None,
        }
    }

    /// Whether several blogs have an article of that handle: an `article_reference` by the
    /// handle alone then names none of them.
    pub fn article_handle_is_ambiguous(&self, handle: &str) -> bool {
        (0..self.blogs.len())
            .filter(|blog| self.article_index(*blog, handle).is_some())
            .count()
            > 1
    }

    fn metaobject_index(&self, kind: &str, handle: &str) -> Option<usize> {
        self.metaobjects
            .iter()
            .position(|entry| entry.is_of(kind) && entry.handle == handle)
    }
}

/// `type/handle` written with another separator, or as a path: `faq.size`, `faq:size`,
/// `faq/size`. Handles have none of `/ . :`, so the last one splits; an app's own type keeps
/// its `$app:` (`$app:author.jane`).
fn split_pair(text: &str) -> Option<(&str, &str)> {
    let (first, last) = text.trim_matches('/').rsplit_once(['/', '.', ':'])?;
    let first = first.rsplit('/').next().unwrap_or(first);
    (!first.is_empty() && !last.is_empty()).then_some((first, last))
}

#[cfg(test)]
mod tests {
    use super::split_pair;

    #[test]
    fn a_pair_is_split_on_its_last_separator() {
        assert_eq!(split_pair("faq.size"), Some(("faq", "size")));
        assert_eq!(split_pair("faq:size"), Some(("faq", "size")));
        assert_eq!(
            split_pair("$app:author.jane"),
            Some(("$app:author", "jane"))
        );
        assert_eq!(
            split_pair("$app:author:jane"),
            Some(("$app:author", "jane"))
        );
        assert_eq!(
            split_pair("/blogs/news/first-post"),
            Some(("news", "first-post"))
        );
        assert_eq!(split_pair("size"), None);
    }
}
