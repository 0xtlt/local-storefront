//! Maps a URL to the template that renders it and the resource it is about.

use super::globals::meta_description;
use super::page::{Page, Resource};
use crate::drops::collection::CollectionScope;
use crate::drops::request::TemplateName;
use crate::site::Site;
use crate::theme::Theme;

fn template_exists(theme: &Theme, name: &TemplateName) -> bool {
    let path = format!("templates/{}", name.full());
    theme.files().exists(&format!("{path}.json")) || theme.files().exists(&format!("{path}.liquid"))
}

/// Applies an alternate template (`?view=` or the resource's `template_suffix`) when the theme
/// has it.
fn with_suffix(theme: &Theme, mut name: TemplateName, suffix: Option<&str>) -> TemplateName {
    if let Some(suffix) = suffix.filter(|suffix| !suffix.is_empty()) {
        name.suffix = Some(suffix.to_string());
        if !template_exists(theme, &name) {
            name.suffix = None;
        }
    }
    name
}

fn not_found() -> Page {
    let mut page = Page::new("404", Resource::NotFound);
    page.status = 404;
    page.title = "404 Not Found".to_string();
    page
}

fn customers(template: &str, title: &str, path: &str, order: Option<u64>) -> Page {
    let mut page = Page::new(template, Resource::Customers { order });
    page.template.directory = Some("customers".to_string());
    page.page_type = format!("customers/{template}");
    page.title = title.to_string();
    page.canonical_path = path.to_string();
    page
}

/// Resolves the request's path. Unknown paths and missing resources give the 404 page.
pub fn resolve(site: &Site) -> Page {
    let store = &site.store;
    let theme = &site.theme;
    let request = &site.request;
    let view = request.param("view");
    let path = request.path.trim_matches('/');
    let segments: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };

    // A locked storefront shows the password page for everything.
    if store.shop.password.is_some()
        && !site.session.password_unlocked
        && segments.first() != Some(&"password")
    {
        let mut page = Page::new("password", Resource::Password);
        page.title = store.shop.name.clone();
        page.canonical_path = "/password".to_string();
        return page;
    }

    let mut page = match segments.as_slice() {
        [] => {
            let mut page = Page::new("index", Resource::Index);
            page.title = store.shop.name.clone();
            page.description = store.shop.description.clone();
            page
        }
        ["products", handle] | ["collections", _, "products", handle] => {
            let Some(product) = store.product_index(handle) else {
                return not_found();
            };
            let collection = match segments.as_slice() {
                ["collections", collection, ..] => store.collection_index(collection),
                _ => None,
            };
            let data = &store.products[product];
            let mut page = Page::new(
                "product",
                Resource::Product {
                    product,
                    collection,
                },
            );
            page.template = with_suffix(
                theme,
                page.template,
                view.or(data.template_suffix.as_deref()),
            );
            page.title = data.title.clone();
            page.description = meta_description(&data.description);
            page.canonical_path = format!("/products/{}", data.handle);
            page.handle = Some(data.handle.clone());
            page
        }
        ["collections"] => {
            let mut page = Page::new("list-collections", Resource::ListCollections);
            page.title = "Collections".to_string();
            page.canonical_path = "/collections".to_string();
            page
        }
        ["collections", handle] | ["collections", handle, _] => {
            let query = request.param("q").unwrap_or_default().to_string();
            // `/collections/vendors?q=` and `/collections/types?q=` list all products of a
            // vendor or type.
            let (collection, scope) = match *handle {
                "vendors" | "types" if store.collection_index(handle).is_none() => {
                    let scope = if *handle == "vendors" {
                        CollectionScope {
                            vendor: Some(query),
                            ..CollectionScope::default()
                        }
                    } else {
                        CollectionScope {
                            product_type: Some(query),
                            ..CollectionScope::default()
                        }
                    };
                    (store.collection_index("all"), scope)
                }
                _ => {
                    let tags = match segments.get(2) {
                        Some(tags) => tags.split('+').map(str::to_string).collect(),
                        None => Vec::new(),
                    };
                    (
                        store.collection_index(handle),
                        CollectionScope {
                            tags,
                            ..CollectionScope::default()
                        },
                    )
                }
            };
            let Some(collection) = collection else {
                return not_found();
            };
            let data = &store.collections[collection];
            let mut page = Page::new(
                "collection",
                Resource::Collection {
                    collection,
                    scope: scope.clone(),
                },
            );
            page.template = with_suffix(
                theme,
                page.template,
                view.or(data.template_suffix.as_deref()),
            );
            page.title = scope
                .vendor
                .clone()
                .or(scope.product_type.clone())
                .unwrap_or_else(|| data.title.clone());
            page.description = meta_description(&data.description);
            page.canonical_path = format!("/collections/{handle}");
            page.handle = Some(data.handle.clone());
            page
        }
        ["pages", handle] => {
            let Some(index) = store.page_index(handle) else {
                return not_found();
            };
            let data = &store.pages[index];
            let mut page = Page::new("page", Resource::Page(index));
            page.template = with_suffix(
                theme,
                page.template,
                view.or(data.template_suffix.as_deref()),
            );
            page.title = data.title.clone();
            page.description = meta_description(&data.content);
            page.canonical_path = format!("/pages/{}", data.handle);
            page.handle = Some(data.handle.clone());
            page
        }
        ["blogs", handle] | ["blogs", handle, "tagged", _] => {
            let Some(blog) = store.blog_index(handle) else {
                return not_found();
            };
            let tags = match segments.get(3) {
                Some(tags) => tags.split('+').map(str::to_string).collect(),
                None => Vec::new(),
            };
            let data = &store.blogs[blog];
            let mut page = Page::new("blog", Resource::Blog { blog, tags });
            page.template = with_suffix(
                theme,
                page.template,
                view.or(data.template_suffix.as_deref()),
            );
            page.title = data.title.clone();
            page.canonical_path = format!("/blogs/{}", data.handle);
            page.handle = Some(data.handle.clone());
            page
        }
        ["blogs", blog_handle, article_handle] => {
            let found = store
                .blog_index(blog_handle)
                .and_then(|blog| Some((blog, store.article_index(blog, article_handle)?)));
            let Some((blog, article)) = found else {
                return not_found();
            };
            let data = &store.blogs[blog].articles[article];
            let mut page = Page::new("article", Resource::Article { blog, article });
            page.template = with_suffix(
                theme,
                page.template,
                view.or(data.template_suffix.as_deref()),
            );
            page.title = data.title.clone();
            page.description = meta_description(if data.excerpt.is_empty() {
                &data.content
            } else {
                &data.excerpt
            });
            page.canonical_path = format!("/blogs/{blog_handle}/{}", data.handle);
            page.handle = Some(format!("{blog_handle}/{}", data.handle));
            page
        }
        ["cart"] => {
            let mut page = Page::new("cart", Resource::Cart);
            page.title = "Your Shopping Cart".to_string();
            page.canonical_path = "/cart".to_string();
            page
        }
        ["search"] => {
            let mut page = Page::new("search", Resource::Search);
            page.title = match request.param("q").filter(|query| !query.is_empty()) {
                Some(query) => format!("Search: results found for \"{query}\""),
                None => "Search".to_string(),
            };
            page.canonical_path = "/search".to_string();
            page
        }
        ["policies", handle] => {
            let Some(policy) = store.policy(handle) else {
                return not_found();
            };
            let mut page = Page::new("policy", Resource::Policy(policy.handle.clone()));
            page.title = policy.title.clone();
            page.canonical_path = format!("/policies/{handle}");
            page.handle = Some(policy.handle.clone());
            page
        }
        ["password"] => {
            let mut page = Page::new("password", Resource::Password);
            page.title = store.shop.name.clone();
            page.canonical_path = "/password".to_string();
            page
        }
        ["gift_cards", _, token] => {
            let Some(index) = store
                .gift_cards
                .iter()
                .position(|card| card.token == *token)
            else {
                return not_found();
            };
            let card = &store.gift_cards[index];
            let mut page = Page::new("gift_card", Resource::GiftCard(index));
            page.template = with_suffix(
                theme,
                page.template,
                view.or(card.template_suffix.as_deref()),
            );
            page.title = "Gift card".to_string();
            page.canonical_path = card.path(store.shop.id);
            page
        }
        ["account"] => customers("account", "Account", "/account", None),
        ["account", "login"] => customers("login", "Account", "/account/login", None),
        ["account", "register"] => {
            customers("register", "Create Account", "/account/register", None)
        }
        ["account", "addresses"] => customers("addresses", "Addresses", "/account/addresses", None),
        ["account", "reset", ..] => {
            customers("reset_password", "Reset Account", "/account/reset", None)
        }
        ["account", "activate", ..] => customers(
            "activate_account",
            "Activate Account",
            "/account/activate",
            None,
        ),
        ["account", "orders", id] => customers("order", "Order", "/account", id.parse().ok()),
        _ => return not_found(),
    };

    if let Some(view) = view
        && page.template.suffix.is_none()
    {
        page.template = with_suffix(theme, page.template, Some(view));
    }
    page
}
