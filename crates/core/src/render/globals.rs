//! The global objects every template can read: `shop`, `cart`, `settings`, `request`, and the
//! resource of the page (`product`, `collection`, ...).

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use slt_liquid::filters::escape_html;
use slt_liquid::{Object, Value};

use super::page::{Page, Resource};
use crate::drops::cart::CartDrop;
use crate::drops::collection::CollectionDrop;
use crate::drops::content::{ArticleDrop, BlogDrop, PageDrop};
use crate::drops::customer::{CustomerDrop, find_order};
use crate::drops::gift_card::GiftCardDrop;
use crate::drops::lists::{Kind, Lookup};
use crate::drops::localization::LocalizationDrop;
use crate::drops::metafield::MetaobjectsDrop;
use crate::drops::navigation::LinkListsDrop;
use crate::drops::product::ProductDrop;
use crate::drops::request::{RequestDrop, RoutesDrop, TemplateDrop};
use crate::drops::search::{predictive_search_value, recommendations_value, search_value};
use crate::drops::shop::{ShopDrop, policy_value};
use crate::drops::{hash, strings};
use crate::filters::misc_json;
use crate::site::Site;
use crate::urls;

pub struct Globals {
    site: Arc<Site>,
    page: Page,
    cache: Mutex<HashMap<String, Value>>,
}

impl Globals {
    pub fn new(site: Arc<Site>, page: Page) -> Self {
        Globals {
            site,
            page,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Defines or replaces a global, e.g. `settings` and `content_for_layout`, which are only
    /// known once rendering has started.
    pub fn set(&self, name: &str, value: Value) {
        self.cache
            .lock()
            .expect("globals poisoned")
            .insert(name.to_string(), value);
    }

    fn theme_value(&self) -> Value {
        let name = self
            .site
            .theme
            .json("config/settings_schema.json")
            .ok()
            .flatten()
            .and_then(|schema| {
                schema
                    .as_array()?
                    .iter()
                    .find(|group| {
                        group.get("name").and_then(|name| name.as_str()) == Some("theme_info")
                    })?
                    .get("theme_name")?
                    .as_str()
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "Local theme".to_string());
        hash([
            ("id", Value::Int(urls::THEME_ID as i64)),
            ("name", Value::from(name)),
            ("role", Value::str("development")),
        ])
    }

    /// The scripts Shopify injects through `content_for_header`: the `Shopify` JavaScript
    /// global themes rely on, and the bundles built from `{% stylesheet %}` and
    /// `{% javascript %}` tags.
    fn content_for_header(&self) -> String {
        let site = &self.site;
        let shopify = serde_json::json!({
            "shop": site.store.shop.permanent_domain,
            "locale": site.request.locale,
            "currency": { "active": site.currency(), "rate": "1.0" },
            "country": site.country().iso_code,
            "theme": {
                "name": self.theme_value().get("name").to_str(),
                "id": urls::THEME_ID,
                "schema_name": self.theme_value().get("name").to_str(),
                "theme_store_id": null,
                "role": "development",
            },
            "routes": { "root": format!("{}/", site.request.root) },
            "cdnHost": site.request.host,
            "designMode": false,
        });
        let compiled = format!(
            "{}/cdn/shop/t/{}/compiled_assets",
            urls::cdn_origin(site),
            urls::THEME_ID
        );
        let version = crate::render::compiled_version(&site.theme);
        format!(
            "<script>window.Shopify = Object.assign(window.Shopify || {{}}, {});</script>\n<link rel=\"stylesheet\" href=\"{compiled}/styles.css?v={version}\" media=\"all\">\n<script src=\"{compiled}/scripts.js?v={version}\" defer=\"defer\"></script>",
            misc_json(&shopify)
        )
    }

    fn page_image(&self) -> Value {
        let site = &self.site;
        let image = match &self.page.resource {
            Resource::Product { product, .. } => site.store.products[*product].images().next(),
            Resource::Collection { collection, .. } => {
                site.store.collections[*collection].image.as_ref()
            }
            Resource::Article { blog, article } => {
                site.store.blogs[*blog].articles[*article].image.as_ref()
            }
            _ => None,
        };
        crate::drops::media::ImageDrop::optional(site, image)
    }

    fn compute(&self, name: &str) -> Option<Value> {
        let site = &self.site;
        let store = &site.store;
        let page = &self.page;
        Some(match name {
            "shop" => ShopDrop::value(site),
            "cart" => CartDrop::value(site),
            "customer" => site.customer().map_or(Value::Nil, |customer| {
                CustomerDrop::value(site, customer.id)
            }),
            "request" => Value::object(RequestDrop {
                site: site.clone(),
                page_type: page.page_type.clone(),
            }),
            "routes" => Value::object(RoutesDrop { site: site.clone() }),
            "localization" => Value::object(LocalizationDrop { site: site.clone() }),
            "template" => Value::object(TemplateDrop(page.template.clone())),
            "theme" => self.theme_value(),
            "linklists" => Value::object(LinkListsDrop { site: site.clone() }),
            "collections" => Lookup::value(site, Kind::Collections),
            "all_products" => Lookup::value(site, Kind::AllProducts),
            "pages" => Lookup::value(site, Kind::Pages),
            "blogs" => Lookup::value(site, Kind::Blogs),
            "articles" => Lookup::value(site, Kind::Articles),
            "images" => Lookup::value(site, Kind::Images),
            "metaobjects" => Value::object(MetaobjectsDrop { site: site.clone() }),
            "canonical_url" => Value::from(format!(
                "{}{}",
                site.request.origin(),
                site.request.localized(&page.canonical_path)
            )),
            "page_title" => Value::from(&page.title),
            "page_description" => {
                if page.description.is_empty() {
                    Value::Nil
                } else {
                    Value::from(&page.description)
                }
            }
            "page_image" => self.page_image(),
            "handle" => page.handle.as_ref().map_or(Value::Nil, Value::from),
            "current_page" => Value::Int(
                site.request
                    .param("page")
                    .and_then(|page| page.parse::<i64>().ok())
                    .filter(|page| *page >= 1)
                    .unwrap_or(1),
            ),
            "current_tags" => match &page.resource {
                Resource::Collection { scope, .. } if !scope.tags.is_empty() => {
                    strings(&scope.tags)
                }
                Resource::Blog { tags, .. } if !tags.is_empty() => strings(tags),
                _ => Value::Nil,
            },
            "powered_by_link" => Value::str(
                "<a target=\"_blank\" rel=\"nofollow\" href=\"https://www.shopify.com?utm_campaign=poweredby&amp;utm_medium=shopify&amp;utm_source=onlinestore\">Powered by Shopify</a>",
            ),
            "content_for_header" => Value::from(self.content_for_header()),
            "content_for_additional_checkout_buttons" => Value::empty_string(),
            // The `<option>`s of a country selector. Provinces are not known locally, so the
            // province selector the theme's script fills from `data-provinces` stays empty.
            "country_option_tags" => {
                let names: Vec<&str> = store
                    .countries
                    .iter()
                    .map(|country| country.name.as_str())
                    .collect();
                Value::from(country_options(&names))
            }
            "all_country_option_tags" => {
                Value::from(country_options(&crate::store::reference::country_names()))
            }
            "additional_checkout_buttons" => Value::Bool(false),
            "scripts" => Value::Nil,
            "shop_locale" => crate::drops::localization::language_value(site, site.language()),

            // The resource the page is about.
            "product" => match &page.resource {
                Resource::Product { product, .. } => ProductDrop::value(site, *product),
                _ => return None,
            },
            "collection" => match &page.resource {
                Resource::Collection { collection, scope } => {
                    CollectionDrop::for_page(site, *collection, scope.clone())
                }
                Resource::Product {
                    collection: Some(collection),
                    ..
                } => CollectionDrop::value(site, *collection),
                _ => return None,
            },
            "page" => match &page.resource {
                Resource::Page(index) => PageDrop::value(site, *index),
                _ => return None,
            },
            "blog" => match &page.resource {
                Resource::Blog { blog, tags } => BlogDrop::tagged(site, *blog, tags.clone()),
                Resource::Article { blog, .. } => BlogDrop::value(site, *blog),
                _ => return None,
            },
            "article" => match &page.resource {
                Resource::Article { blog, article } => ArticleDrop::value(site, *blog, *article),
                _ => return None,
            },
            "gift_card" => match &page.resource {
                Resource::GiftCard(index) => GiftCardDrop::value(site, *index),
                _ => return None,
            },
            "policy" => match &page.resource {
                Resource::Policy(handle) => store
                    .policy(handle)
                    .map_or(Value::Nil, |policy| policy_value(site, policy)),
                _ => return None,
            },
            "order" => match &page.resource {
                Resource::Customers { order: Some(order) } => {
                    find_order(site, *order).unwrap_or(Value::Nil)
                }
                _ => return None,
            },
            // Outside blocks, `closest` is the resource of the page.
            "closest" => {
                let mut closest = slt_liquid::Hash::new();
                for name in ["product", "collection", "article", "blog", "page"] {
                    if let Some(value) = Object::get(self, name) {
                        closest.insert(name.to_string(), value);
                    }
                }
                Value::object(super::section::ClosestDrop(Arc::new(closest)))
            }
            "search" => search_value(site),
            "predictive_search" => predictive_search_value(site),
            "recommendations" => recommendations_value(site),
            _ => return None,
        })
    }
}

fn country_options(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| {
            let name = escape_html(name);
            format!("<option value=\"{name}\" data-provinces=\"[]\">{name}</option>")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Object for Globals {
    fn type_name(&self) -> &str {
        "globals"
    }

    fn get(&self, key: &str) -> Option<Value> {
        if let Some(value) = self.cache.lock().expect("globals poisoned").get(key) {
            return Some(value.clone());
        }
        // Computed outside the lock: building one global may read another.
        let value = self.compute(key)?;
        let mut cache = self.cache.lock().expect("globals poisoned");
        Some(cache.entry(key.to_string()).or_insert(value).clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The text of a page's meta description: HTML stripped, whitespace collapsed, shortened.
pub fn meta_description(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let unescaped = collapsed.replace("&amp;", "&").replace("&nbsp;", " ");
    let truncated: String = unescaped.chars().take(320).collect();
    escape_html(&truncated).replace("&#39;", "'")
}
