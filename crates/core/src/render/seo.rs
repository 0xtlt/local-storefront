//! What a storefront tells crawlers: `robots.txt`, rendered from `templates/robots.txt.liquid`
//! when the theme has one, and the sitemaps Shopify generates from the store.

use std::any::Any;
use std::borrow::Cow;

use chrono::{DateTime, Utc};
use lsf_liquid::filters::escape_html;
use lsf_liquid::{Object, Value};

use crate::drops::hash;
use crate::site::Site;
use crate::urls;

/// A line of `robots.txt`: `rule`, `user_agent` and `sitemap` are all a directive and a value,
/// and print as that line, without a line break. The line breaks of a `robots.txt` are those
/// of its template, which the way storefronts trim whitespace leaves in place
/// (`Environment::set_bug_compatible_whitespace_trimming`).
struct Directive {
    kind: &'static str,
    directive: &'static str,
    value: String,
    line: String,
}

impl Directive {
    fn value(kind: &'static str, directive: &'static str, value: impl Into<String>) -> Value {
        let value = value.into();
        Value::object(Directive {
            kind,
            directive,
            line: format!("{directive}: {value}"),
            value,
        })
    }
}

impl Object for Directive {
    fn type_name(&self) -> &str {
        self.kind
    }

    fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "directive" => Value::str(self.directive),
            "value" => Value::from(&self.value),
            _ => return None,
        })
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.line)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// The default groups below are what a Shopify store gives `robots.txt.liquid`, read from a
// store that renders its own template. `{shop}` stands for the id of the shop.

/// The rules for every crawler.
const EVERYONE: &[(&str, &str)] = &[
    ("Disallow", "/a/downloads/-/*"),
    ("Disallow", "/admin"),
    ("Disallow", "/cart"),
    ("Disallow", "/orders"),
    ("Disallow", "/checkouts/"),
    ("Disallow", "/checkout"),
    ("Disallow", "/{shop}/checkouts"),
    ("Disallow", "/{shop}/orders"),
    ("Disallow", "/carts"),
    ("Disallow", "/account"),
    ("Disallow", "/collections/*sort_by*"),
    ("Disallow", "/*/collections/*sort_by*"),
    ("Disallow", "/collections/*+*"),
    ("Disallow", "/collections/*%2B*"),
    ("Disallow", "/collections/*%2b*"),
    ("Disallow", "/*/collections/*+*"),
    ("Disallow", "/*/collections/*%2B*"),
    ("Disallow", "/*/collections/*%2b*"),
    ("Disallow", "*/collections/*filter*&*filter*"),
    ("Disallow", "/blogs/*+*"),
    ("Disallow", "/blogs/*%2B*"),
    ("Disallow", "/blogs/*%2b*"),
    ("Disallow", "/*/blogs/*+*"),
    ("Disallow", "/*/blogs/*%2B*"),
    ("Disallow", "/*/blogs/*%2b*"),
    ("Disallow", "/*?*oseid=*"),
    ("Disallow", "/*preview_theme_id*"),
    ("Disallow", "/*preview_script_id*"),
    ("Disallow", "/policies/"),
    ("Disallow", "/*/policies/"),
    ("Disallow", "/*/*?*ls=*&ls=*"),
    ("Disallow", "/*/*?*ls%3D*%3Fls%3D*"),
    ("Disallow", "/*/*?*ls%3d*%3fls%3d*"),
    ("Disallow", "/search"),
    ("Disallow", "/apple-app-site-association"),
    ("Disallow", "/.well-known/shopify/monorail"),
    ("Disallow", "/cdn/wpm/*.js"),
    ("Disallow", "/recommendations/products"),
    ("Disallow", "/*/recommendations/products"),
    ("Disallow", "/services/login_with_shop"),
    (
        "Disallow",
        "/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
    (
        "Disallow",
        "/*/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
    (
        "Disallow",
        "/collections/*/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
    (
        "Disallow",
        "/*/collections/*/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
];

/// Google's ads crawler ignores the rules for `*`: the ones that matter are repeated for it.
const ADSBOT: &[(&str, &str)] = &[
    ("Disallow", "/checkouts/"),
    ("Disallow", "/checkout"),
    ("Disallow", "/carts"),
    ("Disallow", "/orders"),
    ("Disallow", "/{shop}/checkouts"),
    ("Disallow", "/{shop}/orders"),
    ("Disallow", "/*?*oseid=*"),
    ("Disallow", "/*preview_theme_id*"),
    ("Disallow", "/*preview_script_id*"),
    ("Disallow", "/cdn/wpm/*.js"),
    (
        "Disallow",
        "/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
    (
        "Disallow",
        "/*/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
    (
        "Disallow",
        "/collections/*/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
    (
        "Disallow",
        "/*/collections/*/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
    ("Disallow", "/services/login_with_shop"),
];

/// The crawlers of Ahrefs get the rules of everyone, and are asked to slow down.
const AHREFS: &[(&str, &str)] = &[
    ("Crawl-delay", "10"),
    ("Disallow", "/a/downloads/-/*"),
    ("Disallow", "/admin"),
    ("Disallow", "/cart"),
    ("Disallow", "/orders"),
    ("Disallow", "/checkouts/"),
    ("Disallow", "/checkout"),
    ("Disallow", "/{shop}/checkouts"),
    ("Disallow", "/{shop}/orders"),
    ("Disallow", "/carts"),
    ("Disallow", "/account"),
    ("Disallow", "/collections/*sort_by*"),
    ("Disallow", "/*/collections/*sort_by*"),
    ("Disallow", "/collections/*+*"),
    ("Disallow", "/collections/*%2B*"),
    ("Disallow", "/collections/*%2b*"),
    ("Disallow", "/*/collections/*+*"),
    ("Disallow", "/*/collections/*%2B*"),
    ("Disallow", "/*/collections/*%2b*"),
    ("Disallow", "*/collections/*filter*&*filter*"),
    ("Disallow", "/blogs/*+*"),
    ("Disallow", "/blogs/*%2B*"),
    ("Disallow", "/blogs/*%2b*"),
    ("Disallow", "/*/blogs/*+*"),
    ("Disallow", "/*/blogs/*%2B*"),
    ("Disallow", "/*/blogs/*%2b*"),
    ("Disallow", "/*?*oseid=*"),
    ("Disallow", "/*preview_theme_id*"),
    ("Disallow", "/*preview_script_id*"),
    ("Disallow", "/policies/"),
    ("Disallow", "/*/policies/"),
    ("Disallow", "/*/*?*ls=*&ls=*"),
    ("Disallow", "/*/*?*ls%3D*%3Fls%3D*"),
    ("Disallow", "/*/*?*ls%3d*%3fls%3d*"),
    ("Disallow", "/search"),
    ("Disallow", "/apple-app-site-association"),
    ("Disallow", "/.well-known/shopify/monorail"),
    ("Disallow", "/cdn/wpm/*.js"),
    ("Disallow", "/services/login_with_shop"),
    (
        "Disallow",
        "/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
    (
        "Disallow",
        "/*/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
    (
        "Disallow",
        "/collections/*/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
    (
        "Disallow",
        "/*/collections/*/products/*-[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9]-remote",
    ),
];

/// The rules of a group: `(directive, value)`.
type Rules = &'static [(&'static str, &'static str)];

/// The groups, in order: the crawler, its rules, and whether the group names the sitemap.
const GROUPS: &[(&str, Rules, bool)] = &[
    ("*", EVERYONE, true),
    ("adsbot-google", ADSBOT, false),
    ("Nutch", &[("Disallow", "/")], false),
    ("AhrefsBot", AHREFS, true),
    ("AhrefsSiteAudit", AHREFS, true),
    ("MJ12bot", &[("Crawl-delay", "10")], false),
    ("Pinterest", &[("Crawl-delay", "1")], false),
];

fn sitemap_url(site: &Site) -> String {
    format!("{}/sitemap.xml", site.request.origin())
}

/// The `robots` object: the groups of rules `robots.txt.liquid` starts from.
pub fn robots_value(site: &Site) -> Value {
    let shop = site.store.shop.id.to_string();
    let groups = GROUPS
        .iter()
        .map(|(agent, rules, with_sitemap)| {
            hash([
                (
                    "user_agent",
                    Directive::value("user_agent", "User-agent", *agent),
                ),
                (
                    "rules",
                    Value::array(
                        rules
                            .iter()
                            .map(|(directive, value)| {
                                Directive::value("rule", directive, value.replace("{shop}", &shop))
                            })
                            .collect(),
                    ),
                ),
                (
                    "sitemap",
                    if *with_sitemap {
                        Directive::value("sitemap", "Sitemap", sitemap_url(site))
                    } else {
                        Value::Nil
                    },
                ),
            ])
        })
        .collect();
    hash([("default_groups", Value::array(groups))])
}

/// `/robots.txt` for a theme without `templates/robots.txt.liquid`: the default groups, with
/// the comments Shopify writes around them.
pub fn default_robots(site: &Site) -> String {
    let shop = site.store.shop.id.to_string();
    let mut out = String::from("# we use Shopify as our ecommerce platform\n");
    for (agent, rules, with_sitemap) in GROUPS {
        out.push('\n');
        if *agent == "adsbot-google" {
            out.push_str("# Google adsbot ignores robots.txt unless specifically named!\n");
        }
        out.push_str(&format!("User-agent: {agent}\n"));
        for (directive, value) in *rules {
            out.push_str(&format!(
                "{directive}: {}\n",
                value.replace("{shop}", &shop)
            ));
        }
        if *with_sitemap {
            out.push_str(&format!("Sitemap: {}\n", sitemap_url(site)));
        }
    }
    out
}

/// The kinds of child sitemaps, as they appear in their URL: `sitemap_<kind>_1.xml`.
pub const SITEMAP_KINDS: [&str; 4] = ["products", "pages", "collections", "blogs"];

const URLSET_OPEN: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\" xmlns:image=\"http://www.google.com/schemas/sitemap-image/1.1\">\n";

/// One `<url>` of a sitemap.
struct Entry {
    path: String,
    modified: Option<DateTime<Utc>>,
    frequency: &'static str,
    /// The image shown for the page: its source, its title and, for products, its caption.
    image: Option<(String, String, Option<String>)>,
}

fn entries(site: &Site, kind: &str) -> Vec<Entry> {
    let store = &site.store;
    match kind {
        "products" => std::iter::once(Entry {
            path: "/".to_string(),
            modified: None,
            frequency: "daily",
            image: None,
        })
        .chain(store.products.iter().map(|product| Entry {
            path: format!("/products/{}", product.handle),
            modified: Some(product.updated_at),
            frequency: "daily",
            image: product.images().next().map(|image| {
                (
                    image.src.clone(),
                    product.title.clone(),
                    Some(image.alt.clone()),
                )
            }),
        }))
        .collect(),
        "pages" => store
            .pages
            .iter()
            .map(|page| Entry {
                path: format!("/pages/{}", page.handle),
                modified: Some(page.updated_at),
                frequency: "weekly",
                image: None,
            })
            .collect(),
        "collections" => store
            .collections
            .iter()
            // The catalog is not a collection of the store.
            .filter(|collection| collection.handle != "all")
            .map(|collection| Entry {
                path: format!("/collections/{}", collection.handle),
                modified: Some(collection.updated_at),
                frequency: "daily",
                image: None,
            })
            .collect(),
        "blogs" => store
            .blogs
            .iter()
            .flat_map(|blog| {
                std::iter::once(Entry {
                    path: format!("/blogs/{}", blog.handle),
                    modified: blog.articles.iter().map(|article| article.updated_at).max(),
                    frequency: "weekly",
                    image: None,
                })
                .chain(blog.articles.iter().map(move |article| {
                    Entry {
                        path: format!("/blogs/{}/{}", blog.handle, article.handle),
                        modified: Some(article.updated_at),
                        frequency: "weekly",
                        image: article
                            .image
                            .as_ref()
                            .map(|image| (image.src.clone(), article.title.clone(), None)),
                    }
                }))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `/sitemap.xml`: the index of the child sitemaps that have something in them.
pub fn sitemap_index(site: &Site) -> String {
    let store = &site.store;
    let origin = site.request.origin();
    // Shopify names the range of ids a child sitemap covers.
    let range = |ids: Vec<u64>| match (ids.iter().min(), ids.iter().max()) {
        (Some(from), Some(to)) => format!("?from={from}&amp;to={to}"),
        _ => String::new(),
    };
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for kind in SITEMAP_KINDS {
        if entries(site, kind).is_empty() {
            continue;
        }
        let query = match kind {
            "products" => range(store.products.iter().map(|product| product.id).collect()),
            "pages" => range(store.pages.iter().map(|page| page.id).collect()),
            "collections" => range(
                store
                    .collections
                    .iter()
                    .filter(|collection| collection.handle != "all")
                    .map(|collection| collection.id)
                    .collect(),
            ),
            _ => String::new(),
        };
        out.push_str(&format!(
            "  <sitemap>\n    <loc>{origin}/sitemap_{kind}_1.xml{query}</loc>\n  </sitemap>\n"
        ));
    }
    out.push_str("</sitemapindex>\n");
    out
}

/// `/sitemap_<kind>_1.xml`, or nothing for a kind that does not exist.
pub fn sitemap(site: &Site, kind: &str) -> Option<String> {
    if !SITEMAP_KINDS.contains(&kind) {
        return None;
    }
    let origin = site.request.origin();
    let timezone = site.store.shop.timezone;
    let mut out = String::from(URLSET_OPEN);
    for entry in entries(site, kind) {
        out.push_str(&format!("  <url>\n    <loc>{origin}{}</loc>\n", entry.path));
        if let Some(modified) = entry.modified {
            out.push_str(&format!(
                "    <lastmod>{}</lastmod>\n",
                modified
                    .with_timezone(&timezone)
                    .format("%Y-%m-%dT%H:%M:%S%:z")
            ));
        }
        out.push_str(&format!(
            "    <changefreq>{}</changefreq>\n",
            entry.frequency
        ));
        if let Some((src, title, caption)) = entry.image {
            out.push_str(&format!(
                "    <image:image>\n      <image:loc>{}:{}</image:loc>\n      <image:title>{}</image:title>\n",
                site.request.scheme,
                escape_html(&urls::file_url(site, &src)),
                escape_html(&title)
            ));
            if let Some(caption) = caption {
                out.push_str(&format!(
                    "      <image:caption>{}</image:caption>\n",
                    escape_html(&caption)
                ));
            }
            out.push_str("    </image:image>\n");
        }
        out.push_str("  </url>\n");
    }
    out.push_str("</urlset>\n");
    Some(out)
}
