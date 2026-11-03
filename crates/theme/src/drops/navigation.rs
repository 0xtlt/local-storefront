//! `linklists`, `linklist` and `link`.

use std::any::Any;
use std::sync::Arc;

use lsf_liquid::{Object, Value};

use super::SiteRef;
use crate::render::cost;
use crate::store::{Link, LinkTarget, Menu};

/// The global `linklists`: menus by handle.
pub struct LinkListsDrop {
    pub site: SiteRef,
}

impl Object for LinkListsDrop {
    fn type_name(&self) -> &str {
        "linklists"
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.site
            .store
            .menu(key)
            .map(|menu| LinkListDrop::value(&self.site, menu))
    }

    fn items(&self) -> Option<Arc<Vec<Value>>> {
        Some(Arc::new(
            self.site
                .store
                .menus
                .iter()
                .map(|menu| LinkListDrop::value(&self.site, menu))
                .collect(),
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct LinkListDrop {
    site: SiteRef,
    menu: Menu,
}

impl LinkListDrop {
    pub fn value(site: &SiteRef, menu: &Menu) -> Value {
        cost::loaded(cost::MENU, &menu.handle);
        Value::object(LinkListDrop {
            site: site.clone(),
            menu: menu.clone(),
        })
    }
}

fn links(site: &SiteRef, links: &[Link]) -> Value {
    Value::array(
        links
            .iter()
            .map(|link| {
                Value::object(LinkDrop {
                    site: site.clone(),
                    link: link.clone(),
                })
            })
            .collect(),
    )
}

impl Object for LinkListDrop {
    fn type_name(&self) -> &str {
        "linklist"
    }

    fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "handle" => Value::from(&self.menu.handle),
            "title" => Value::from(&self.menu.title),
            "levels" => Value::from(self.menu.levels()),
            "links" => links(&self.site, &self.menu.links),
            _ => return None,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("linklist:{}", self.menu.handle))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct LinkDrop {
    site: SiteRef,
    link: Link,
}

impl LinkDrop {
    /// The URL with the locale prefix for links into the storefront.
    fn url(&self) -> String {
        link_url(&self.site, &self.link)
    }
}

fn link_url(site: &SiteRef, link: &Link) -> String {
    if link.url.starts_with('/') && !link.url.starts_with("//") {
        site.request.localized(&link.url)
    } else {
        link.url.clone()
    }
}

/// Whether the link points at the page being rendered.
fn is_current(site: &SiteRef, link: &Link) -> bool {
    let path = link.url.split(['?', '#']).next().unwrap_or(&link.url);
    path.starts_with('/') && path.trim_end_matches('/') == site.request.path.trim_end_matches('/')
}

/// Whether the page being rendered is the link's target or lives below it.
fn is_active(site: &SiteRef, link: &Link) -> bool {
    if is_current(site, link) {
        return true;
    }
    let path = link
        .url
        .split(['?', '#'])
        .next()
        .unwrap_or(&link.url)
        .trim_end_matches('/');
    !path.is_empty() && path.starts_with('/') && site.request.path.starts_with(&format!("{path}/"))
}

fn any_descendant(link: &Link, test: &dyn Fn(&Link) -> bool) -> bool {
    link.links
        .iter()
        .any(|child| test(child) || any_descendant(child, test))
}

impl Object for LinkDrop {
    fn type_name(&self) -> &str {
        "link"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let link = &self.link;
        Some(match key {
            "title" => Value::from(&link.title),
            "handle" => Value::from(&link.handle),
            "url" => Value::from(self.url()),
            "type" => Value::str(link.target.type_name()),
            "levels" => Value::from(link.levels()),
            "links" => links(site, &link.links),
            "current" => Value::Bool(is_current(site, link)),
            "active" => Value::Bool(is_active(site, link)),
            "child_current" => Value::Bool(any_descendant(link, &|child| is_current(site, child))),
            "child_active" => Value::Bool(any_descendant(link, &|child| is_active(site, child))),
            "object" => match &link.target {
                LinkTarget::Collection(index) => {
                    super::collection::CollectionDrop::value(site, *index)
                }
                LinkTarget::Catalog => site
                    .store
                    .collection_index("all")
                    .map_or(Value::Nil, |index| {
                        super::collection::CollectionDrop::value(site, index)
                    }),
                LinkTarget::Product(index) => super::product::ProductDrop::value(site, *index),
                LinkTarget::Page(index) => super::content::PageDrop::value(site, *index),
                LinkTarget::Blog(index) => super::content::BlogDrop::value(site, *index),
                LinkTarget::Article(blog, article) => {
                    super::content::ArticleDrop::value(site, *blog, *article)
                }
                LinkTarget::Policy(handle) => site
                    .store
                    .policy(handle)
                    .map_or(Value::Nil, |policy| super::shop::policy_value(site, policy)),
                _ => Value::Nil,
            },
            _ => return None,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
