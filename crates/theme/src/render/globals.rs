//! The global objects every template can read: `shop`, `cart`, `settings`, `request`, and the
//! resource of the page (`product`, `collection`, ...).

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use lsf_liquid::filters::escape_html;
use lsf_liquid::{Object, Value};

use super::page::{Page, Resource};
use crate::drops::cart::CartDrop;
use crate::drops::collection::CollectionDrop;
use crate::drops::content::{ArticleDrop, BlogDrop, PageDrop};
use crate::drops::customer::CustomerDrop;
use crate::drops::gift_card::GiftCardDrop;
use crate::drops::lists::{Kind, Lookup};
use crate::drops::localization::LocalizationDrop;
use crate::drops::metafield::{MetaobjectDrop, MetaobjectsDrop};
use crate::drops::navigation::LinkListsDrop;
use crate::drops::product::{ProductDrop, VariantDrop};
use crate::drops::request::{RequestDrop, RoutesDrop, TemplateDrop};
use crate::drops::search::{predictive_search_value, recommendations_value, search_value};
use crate::drops::shop::{ShopDrop, policy_value};
use crate::drops::{hash, strings};
use crate::filters::shopify_text;
use crate::site::Site;

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

    /// `page_title`, and on a search with terms Shopify's
    /// `Search: 3 results found for &quot;shirt&quot;`, in the theme's words when it overrides
    /// them, HTML-escaped as a store prints it.
    fn page_title(&self) -> String {
        // The terms without their HTML, as Rails' `strip_tags` gives them: a search for `<b>`
        // has the title of a search without terms (shopify.dev's playground, 2026-11-16).
        let terms = self
            .site
            .request
            .param("q")
            .map(strip_tags)
            .filter(|terms| !terms.trim().is_empty());
        match (&self.page.resource, terms) {
            (Resource::Search, Some(terms)) => {
                let count = self
                    .get("search")
                    .map_or(Value::Nil, |search| search.get("results_count"));
                escape_html(&shopify_text(
                    &self.site,
                    "shopify.page_titles.search_results",
                    &[
                        ("count".to_string(), count),
                        ("terms".to_string(), Value::from(terms.as_str())),
                    ],
                ))
            }
            _ => self.page.title.clone(),
        }
    }

    fn theme_value(&self) -> Value {
        let (id, name, role) = super::platform::served_theme(&self.site.theme);
        hash([
            ("id", Value::Int(id as i64)),
            ("name", Value::from(name)),
            ("role", Value::from(role)),
        ])
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
            "page_title" => Value::from(self.page_title()),
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
            "content_for_header" => Value::from(super::platform::content_for_header(site, page)),
            "content_for_additional_checkout_buttons" => Value::empty_string(),
            // The `<option>`s of a country selector: the countries of the store, each with the
            // provinces it ships to, which the theme's script fills a province selector with.
            "country_option_tags" => {
                Value::from(country_options(store.countries.iter().map(|country| {
                    let provinces = country
                        .provinces
                        .iter()
                        .map(|province| [province.name.as_str(), province.label.as_str()]);
                    (country.name.as_str(), provinces.collect())
                })))
            }
            // Every country Shopify lists, by name, with every province.
            "all_country_option_tags" => {
                let codes = lsf_core::store::reference::country_codes();
                Value::from(country_options(codes.into_iter().filter_map(|code| {
                    let name = lsf_core::store::reference::country_name(code)?;
                    let provinces = lsf_core::store::reference::provinces(code);
                    let pairs = provinces.iter().map(|(_, name, label)| [*name, *label]);
                    Some((name, pairs.collect()))
                })))
            }
            "additional_checkout_buttons" => Value::Bool(false),
            "scripts" => Value::Nil,
            "shop_locale" => crate::drops::localization::language_value(site, site.language()),

            // The resource the page is about.
            "product" => match &page.resource {
                Resource::Product { product, .. } => ProductDrop::value(site, *product),
                _ => return None,
            },
            // Only where a section is rendered for a variant (`/variants/<id>?section_id=`).
            "product_variant" => match (&page.resource, page.product_variant) {
                (Resource::Product { product, .. }, Some(variant))
                | (Resource::Variant { product, .. }, Some(variant)) => {
                    VariantDrop::value(site, *product, variant)
                }
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
            "metaobject" => match &page.resource {
                Resource::Metaobject(index) => {
                    MetaobjectDrop::value(site, &store.metaobjects[*index])
                }
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
            // Outside blocks, `closest` is the resource of the page.
            "closest" => {
                let mut closest = lsf_liquid::Hash::new();
                for name in ["product", "collection", "article", "blog", "page"] {
                    if let Some(value) = Object::get(self, name) {
                        closest.insert(name.to_string(), value);
                    }
                }
                Value::object(super::section::ClosestDrop(Arc::new(closest)))
            }
            "robots" => super::seo::robots_value(site),
            "search" => search_value(site),
            "predictive_search" => predictive_search_value(site),
            "recommendations" => recommendations_value(site),
            _ => return None,
        })
    }
}

/// The `<option>` of each country, by name, with its provinces in `data-provinces`: a JSON
/// list of pairs, the name a form sends (`address[province]`) and the name to show. Shopify
/// puts a `---` option, which names no country, before them.
fn country_options<'a>(countries: impl Iterator<Item = (&'a str, Vec<[&'a str; 2]>)>) -> String {
    std::iter::once(("---", Vec::new()))
        .chain(countries)
        .map(|(name, pairs)| {
            let provinces = serde_json::to_string(&pairs).unwrap_or_else(|_| "[]".to_string());
            let name = escape_html(name);
            format!(
                "<option value=\"{name}\" data-provinces=\"{}\">{name}</option>",
                escape_html(&provinces)
            )
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

/// A text without its HTML, as Rails' `strip_tags` gives it, which Shopify runs on the terms of
/// a search's title: a text without `<` is as it is; otherwise its tags, comments and the
/// content of `script` and `style` go, a `<` that opens nothing stays, and the text comes out
/// with `&`, `<` and `>` as entities (`a < b` is `a &lt; b`). Read on shopify.dev's playground
/// (2026-11-16): `<b>x` is `x`, `a<b` is `a`, `x>y<z` is `x&gt;y`, `a<!--c-->b` is `ab`.
pub fn strip_tags(text: &str) -> String {
    if !text.contains('<') {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut plain = String::new();
    let mut i = 0;
    let find = |from: usize, needle: &str| -> Option<usize> {
        let rest: String = chars[from..].iter().collect();
        rest.find(needle)
            .map(|at| from + rest[..at].chars().count())
    };
    while i < chars.len() {
        let c = chars[i];
        if c != '<' {
            plain.push(c);
            i += 1;
            continue;
        }
        let next = chars.get(i + 1).copied();
        let opens_tag = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphabetic());
        if next.is_none() {
            // A `<` that ends the text is dropped.
            i += 1;
        } else if chars[i + 1..].starts_with(&['!', '-', '-']) {
            i = find(i + 4, "-->").map_or(chars.len(), |end| end + 3);
        } else if matches!(next, Some('!' | '?'))
            || opens_tag(next)
            || (next == Some('/') && opens_tag(chars.get(i + 2).copied()))
        {
            let name: String = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            let end = find(i, ">").map_or(chars.len(), |end| end + 1);
            i = end;
            // The content of `script` and `style` goes with them.
            if matches!(name.as_str(), "script" | "style") {
                let rest: String = chars[i..].iter().collect::<String>().to_ascii_lowercase();
                i = match rest.find(&format!("</{name}")) {
                    Some(at) => {
                        let close = i + rest[..at].chars().count();
                        find(close, ">").map_or(chars.len(), |end| end + 1)
                    }
                    None => chars.len(),
                };
            }
        } else {
            plain.push('<');
            i += 1;
        }
    }
    let decoded = decode_entities(&plain);
    let mut out = String::with_capacity(decoded.len());
    for c in decoded.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// The text of HTML: its common entities (`&amp;`, `&lt;`, `&#39;`) as the characters.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest[1..].find(';').map(|end| &rest[1..=end]);
        let decoded = entity.and_then(|entity| match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => {
                let number = entity.strip_prefix('#')?;
                let code = match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => number.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (entity, decoded) {
            (Some(entity), Some(c)) => {
                out.push(c);
                rest = &rest[entity.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod strip_tags_tests {
    use super::strip_tags;

    #[test]
    fn strips_as_rails_does_on_shopify() {
        for (text, stripped) in [
            ("<b>", ""),
            ("<b>x", "x"),
            ("x<b>", "x"),
            ("a<b", "a"),
            ("<", ""),
            (">", ">"),
            ("&", "&"),
            ("a < b", "a &lt; b"),
            ("1<2", "1&lt;2"),
            ("x>y<z", "x&gt;y"),
            ("a<!--c-->b", "ab"),
            ("<script>alert(1)</script>x", "x"),
            ("a&amp;b", "a&amp;b"),
            ("&lt;b&gt;", "&lt;b&gt;"),
        ] {
            assert_eq!(strip_tags(text), stripped, "{text}");
        }
    }
}
