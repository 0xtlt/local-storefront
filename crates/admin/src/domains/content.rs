//! The shop and its content: the `Shop`, its domains and policies, metafields and their
//! definitions, metaobjects and theirs, pages, blogs, articles and comments, menus, markets,
//! locales and files.
//!
//! The records and their ids: the "Records" section of `crates/admin/docs/design.md`.

pub mod files;
mod metafield_definitions;
pub mod metafields;
mod metaobjects;
pub mod navigation;
mod online_store;
#[cfg(test)]
mod tests;
mod translations;
pub mod types;

pub use online_store::refresh_blog;

use chrono::Offset;
use lsf_core::store::{
    Article, Blog, CheckoutMetafield, Link, LinkTarget, Metafield, Metafields, Metaobject, Page,
    Policy, reference,
};
use lsf_core::util::{handleize, stable_id};
use serde_json::{Value, json};

use super::catalog;
use crate::state::seed::Seed;
use crate::state::value::{self, date_time, optional, text};
use crate::state::{Record, ShopState, gid, gid_type};

pub fn shop_gid(shop: &lsf_core::store::Shop) -> String {
    gid("Shop", shop.id)
}

pub fn domain_gid(host: &str) -> String {
    gid("Domain", stable_id("domain", &host.to_ascii_lowercase()))
}

pub fn policy_gid(handle: &str) -> String {
    gid("ShopPolicy", stable_id("shop_policy", handle))
}

pub fn page_gid(page: &Page) -> String {
    gid("Page", page.id)
}

pub fn blog_gid(blog: &Blog) -> String {
    gid("Blog", blog.id)
}

pub fn article_gid(article: &Article) -> String {
    gid("Article", article.id)
}

pub fn menu_gid(handle: &str) -> String {
    gid("Menu", stable_id("menu", handle))
}

/// A metaobject: stable for its type and handle, as the checkout's `Shop.metaobject` and
/// lsf-core's `metaobject_by_reference` have it.
pub fn metaobject_gid(kind: &str, handle: &str) -> String {
    gid(
        "Metaobject",
        stable_id("metaobject", &format!("{kind}/{handle}")),
    )
}

pub fn metaobject_definition_gid(kind: &str) -> String {
    gid(
        "MetaobjectDefinition",
        stable_id("metaobject_definition", kind),
    )
}

/// A market: the one the data declares by its handle, or a country of its own by its
/// upper-case ISO code, as the checkout's `Market.id` has it.
pub fn market_gid(key: &str) -> String {
    gid("Market", stable_id("market", key))
}

/// A metafield: stable for its owner, its namespace (as the Admin API writes it) and its key.
pub fn metafield_gid(owner: &str, namespace: &str, key: &str) -> String {
    gid(
        "Metafield",
        stable_id("metafield", &format!("{owner}/{namespace}.{key}")),
    )
}

pub fn metafield_definition_gid(owner_type: &str, namespace: &str, key: &str) -> String {
    gid(
        "MetafieldDefinition",
        stable_id(
            "metafield_definition",
            &format!("{owner_type}/{namespace}.{key}"),
        ),
    )
}

/// The namespace of a metafield as the Admin API writes it: the reserved namespace of the app,
/// written `$app` or `$app:<name>` in the store data and in queries, is `app--<app id>` or
/// `app--<app id>--<name>`. A metaobject type of the app is written the same way (`$app:author`
/// is `app--<app id>--author`).
pub fn admin_namespace(app_id: u64, namespace: &str) -> String {
    if namespace == "$app" {
        format!("app--{app_id}")
    } else if let Some(name) = namespace.strip_prefix("$app:") {
        format!("app--{app_id}--{name}")
    } else {
        namespace.to_string()
    }
}

/// A metaobject type as the store data writes it, the inverse of [`admin_namespace`]: the app's
/// own `app--<app id>--author` is `$app:author`. What the ids of metaobjects are made of
/// ([`metaobject_gid`]), as lsf-core makes them from the store data, and what store sync writes.
pub fn data_type(app_id: u64, kind: &str) -> String {
    match kind
        .strip_prefix(&format!("app--{app_id}--"))
        .filter(|name| !name.is_empty())
    {
        Some(name) => format!("$app:{name}"),
        None => kind.to_string(),
    }
}

/// The `Shop`, its domains and its policies: seeded first, since other families read the
/// currency and the domain from it.
pub fn seed_shop(seed: &mut Seed) {
    let store = seed.store;
    let shop = &store.shop;
    let id = shop_gid(shop);
    let created = Seed::epoch_string();
    let domain = seed.state.domain().to_string();
    let primary_host = seed.origin.trim_start_matches("https://").to_string();
    let mut domains = vec![seed.insert(json!({
        "__typename": "Domain",
        "id": domain_gid(&primary_host),
        "host": primary_host,
        "url": seed.origin,
        "sslEnabled": true,
        "createdAt": created,
    }))];
    if primary_host != domain {
        domains.push(seed.insert(json!({
            "__typename": "Domain",
            "id": domain_gid(&domain),
            "host": domain,
            "url": format!("https://{domain}"),
            "sslEnabled": true,
            "createdAt": created,
        })));
    }
    let policies: Vec<String> = shop
        .policies
        .iter()
        .map(|policy| seed_policy(seed, policy, &created))
        .collect();
    let now = seed.state.now().with_timezone(&shop.timezone);
    let offset = now.offset().fix().local_minus_utc() / 60;
    let shop_country = shop.address.country_code.to_ascii_uppercase();
    let imperial = store
        .country(&shop_country)
        .map_or(shop_country == "US", |country| {
            country.unit_system == "imperial"
        });
    let mut presentment = vec![seed.currency.clone()];
    for country in &store.countries {
        let currency = country.currency.to_ascii_uppercase();
        if !presentment.contains(&currency) {
            presentment.push(currency);
        }
    }
    let address = &shop.address;
    let shop_address = json!({
        "id": gid("ShopAddress", stable_id("shop_address", &shop.id.to_string())),
        "address1": text(&address.address1),
        "address2": text(&address.address2),
        "city": text(&address.city),
        "company": text(&address.company),
        "province": text(&address.province),
        "provinceCode": text(&address.province_code),
        "country": text(&address.country),
        "countryCodeV2": text(&shop_country),
        "zip": text(&address.zip),
        "phone": text(&address.phone),
        "formatted": value::formatted(address),
        "formattedArea": value::formatted_area(address),
        "coordinatesValidated": false,
    });
    let metafields = seed.metafields(&id, "SHOP", &created, &shop.metafields);
    seed.insert(json!({
        "__typename": "Shop",
        "id": id,
        "name": shop.name,
        "email": shop.email,
        "contactEmail": shop.email,
        "description": text(&shop.description),
        "myshopifyDomain": domain,
        "primaryDomain": domains[0],
        "domains": domains,
        "url": seed.origin,
        "currencyCode": seed.currency,
        "enabledPresentmentCurrencies": presentment,
        "currencyFormats": {
            "moneyFormat": shop.money_format,
            "moneyWithCurrencyFormat": shop.money_with_currency_format,
            "moneyInEmailsFormat": shop.money_format,
            "moneyWithCurrencyInEmailsFormat": shop.money_with_currency_format,
        },
        "ianaTimezone": shop.timezone.name(),
        "timezoneAbbreviation": now.format("%Z").to_string(),
        "timezoneOffset": now.format("%z").to_string(),
        "timezoneOffsetMinutes": offset,
        "shopAddress": shop_address,
        "billingAddress": shop_address,
        "unitSystem": if imperial { "IMPERIAL_SYSTEM" } else { "METRIC_SYSTEM" },
        "weightUnit": if imperial { "POUNDS" } else { "KILOGRAMS" },
        "taxesIncluded": shop.taxes_included,
        "customerAccounts": if !shop.customer_accounts_enabled {
            "DISABLED"
        } else if shop.customer_accounts_optional {
            "OPTIONAL"
        } else {
            "REQUIRED"
        },
        "plan": {
            "partnerDevelopment": true,
            "publicDisplayName": "Development",
            "displayName": "Development",
            // B2B is a Shopify Plus feature: a store data with companies is a Plus shop.
            "shopifyPlus": !store.companies.is_empty(),
        },
        "shopPolicies": policies,
        "shipsToCountries": store
            .countries
            .iter()
            .map(|country| country.iso_code.to_ascii_uppercase())
            .collect::<Vec<_>>(),
        "orderNumberFormatPrefix": "#",
        "orderNumberFormatSuffix": "",
        "createdAt": created,
        "updatedAt": created,
        "metafields": metafields,
    }));
}

fn seed_policy(seed: &mut Seed, policy: &Policy, created: &str) -> String {
    let kind = match policy.handle.as_str() {
        "privacy-policy" => "PRIVACY_POLICY",
        "refund-policy" => "REFUND_POLICY",
        "shipping-policy" => "SHIPPING_POLICY",
        "subscription-policy" => "SUBSCRIPTION_POLICY",
        "contact-information" => "CONTACT_INFORMATION",
        "legal-notice" => "LEGAL_NOTICE",
        "terms-of-sale" => "TERMS_OF_SALE",
        _ => "TERMS_OF_SERVICE",
    };
    seed.insert(json!({
        "__typename": "ShopPolicy",
        "id": policy_gid(&policy.handle),
        "type": kind,
        "title": policy.title,
        "body": policy.body,
        "url": seed.url(&format!("/policies/{}", policy.handle)),
        "createdAt": created,
        "updatedAt": created,
    }))
}

/// Pages, blogs, menus, metaobjects, markets and locales.
pub fn seed(seed: &mut Seed) {
    let store = seed.store;
    for page in &store.pages {
        let id = page_gid(page);
        let created = date_time(page.published_at);
        let metafields = seed.metafields(&id, "PAGE", &created, &page.metafields);
        seed.insert(json!({
            "__typename": "Page",
            "id": id,
            "title": page.title,
            "handle": page.handle,
            "body": page.content,
            "isPublished": true,
            "publishedAt": created,
            "templateSuffix": optional(page.template_suffix.as_deref()),
            "createdAt": created,
            "updatedAt": date_time(page.updated_at),
            "metafields": metafields,
        }));
    }
    for blog in &store.blogs {
        seed_blog(seed, blog);
    }
    for menu in &store.menus {
        let items: Vec<Value> = menu
            .links
            .iter()
            .enumerate()
            .map(|(index, link)| menu_item(seed, link, &format!("{}/{index}", menu.handle)))
            .collect();
        seed.insert(json!({
            "__typename": "Menu",
            "id": menu_gid(&menu.handle),
            "handle": menu.handle,
            "title": menu.title,
            "isDefault": matches!(menu.handle.as_str(), "main-menu" | "footer"),
            "items": items,
            "createdAt": Seed::epoch_string(),
        }));
    }
    seed_metaobjects(seed);
    seed_markets(seed);
    for language in &store.languages {
        seed.state.insert_as(
            language.iso_code.clone(),
            value::object(json!({
                "__typename": "ShopLocale",
                "locale": language.iso_code,
                "name": language.name,
                "primary": language.primary,
                "published": true,
                "marketWebPresences": [],
            })),
        );
    }
}

fn seed_blog(seed: &mut Seed, blog: &Blog) {
    let id = blog_gid(blog);
    let created = blog
        .articles
        .iter()
        .map(|article| article.created_at)
        .min()
        .map_or_else(Seed::epoch_string, date_time);
    let updated = blog
        .articles
        .iter()
        .map(|article| article.updated_at)
        .max()
        .map_or_else(|| created.clone(), date_time);
    let mut articles = Vec::new();
    let mut tags: Vec<String> = Vec::new();
    for article in &blog.articles {
        let article_id = article_gid(article);
        let article_created = date_time(article.created_at);
        let mut comments = Vec::new();
        for comment in &article.comments {
            let comment_created = date_time(comment.created_at);
            comments.push(seed.insert(json!({
                "__typename": "Comment",
                "id": gid("Comment", comment.id),
                "article": article_id,
                "author": {"name": comment.author, "email": comment.email},
                "body": value::plain_text(&comment.content),
                "bodyHtml": comment.content,
                "status": "PUBLISHED",
                "isPublished": true,
                "publishedAt": comment_created,
                "ip": null,
                "userAgent": null,
                "createdAt": comment_created,
                "updatedAt": comment_created,
            })));
        }
        for tag in &article.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
        let metafields = seed.metafields(
            &article_id,
            "ARTICLE",
            &article_created,
            &article.metafields,
        );
        articles.push(seed.insert(json!({
            "__typename": "Article",
            "id": article_id,
            "title": article.title,
            "handle": article.handle,
            "blog": id,
            "author": {"name": article.author},
            "body": article.content,
            "summary": text(&article.excerpt),
            "image": article.image.as_ref().map(|image| seed.image(image, "ArticleImage")),
            "tags": article.tags,
            "isPublished": true,
            "publishedAt": date_time(article.published_at),
            "templateSuffix": optional(article.template_suffix.as_deref()),
            "comments": comments,
            "createdAt": article_created,
            "updatedAt": date_time(article.updated_at),
            "metafields": metafields,
        })));
    }
    let metafields = seed.metafields(&id, "BLOG", &created, &blog.metafields);
    seed.insert(json!({
        "__typename": "Blog",
        "id": id,
        "title": blog.title,
        "handle": blog.handle,
        "commentPolicy": match (blog.comments_enabled, blog.moderated) {
            (false, _) => "CLOSED",
            (true, true) => "MODERATED",
            (true, false) => "AUTO_PUBLISHED",
        },
        "articles": articles,
        "tags": tags,
        "templateSuffix": optional(blog.template_suffix.as_deref()),
        "feed": null,
        "createdAt": created,
        "updatedAt": updated,
        "metafields": metafields,
    }));
}

/// A `MenuItem`, held inline by its menu with its own items. `path` is the menu's handle and
/// the position of the item at each level (`main-menu/1/0`): what its id is made of.
fn menu_item(seed: &Seed, link: &Link, path: &str) -> Value {
    let store = seed.store;
    let (kind, resource) = match &link.target {
        LinkTarget::Http => ("HTTP", None),
        LinkTarget::Frontpage => ("FRONTPAGE", None),
        LinkTarget::Catalog => ("CATALOG", None),
        LinkTarget::Collections => ("COLLECTIONS", None),
        LinkTarget::Search => ("SEARCH", None),
        // `/collections/all` is the catalog on Shopify, not a collection.
        LinkTarget::Collection(index) => match store.collections.get(*index) {
            Some(collection) if !catalog::is_implicit_all(store, collection) => {
                ("COLLECTION", Some(catalog::collection_gid(collection)))
            }
            _ => ("CATALOG", None),
        },
        LinkTarget::Product(index) => (
            "PRODUCT",
            store.products.get(*index).map(catalog::product_gid),
        ),
        LinkTarget::Page(index) => ("PAGE", store.pages.get(*index).map(page_gid)),
        LinkTarget::Blog(index) => ("BLOG", store.blogs.get(*index).map(blog_gid)),
        LinkTarget::Article(blog, article) => (
            "ARTICLE",
            store
                .blogs
                .get(*blog)
                .and_then(|blog| blog.articles.get(*article))
                .map(article_gid),
        ),
        LinkTarget::Policy(handle) => ("SHOP_POLICY", Some(policy_gid(handle))),
    };
    let items: Vec<Value> = link
        .links
        .iter()
        .enumerate()
        .map(|(index, child)| menu_item(seed, child, &format!("{path}/{index}")))
        .collect();
    json!({
        "id": gid("MenuItem", stable_id("menu_item", path)),
        "title": link.title,
        "type": kind,
        "url": link.url,
        "resourceId": resource,
        "tags": [],
        "items": items,
    })
}

fn seed_metaobjects(seed: &mut Seed) {
    let store = seed.store;
    let created = Seed::epoch_string();
    // The types in the order their first entry comes, with the fields their entries have.
    let mut kinds: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for entry in &store.metaobjects {
        let at = match kinds.iter().position(|(kind, _)| *kind == entry.kind) {
            Some(at) => at,
            None => {
                kinds.push((&entry.kind, Vec::new()));
                kinds.len() - 1
            }
        };
        for (key, field) in &entry.fields {
            if !kinds[at].1.iter().any(|(known, _)| known == key) {
                kinds[at].1.push((key, &field.kind));
            }
        }
    }
    for (kind, fields) in &kinds {
        let display = display_key(fields.iter().map(|(key, _)| *key));
        let entries: Vec<&Metaobject> = store
            .metaobjects
            .iter()
            .filter(|entry| entry.kind == *kind)
            .collect();
        let field_definitions: Vec<Value> = fields
            .iter()
            .map(|(key, field_kind)| field_definition(key, field_kind))
            .collect();
        let mut ids = Vec::new();
        for entry in &entries {
            ids.push(seed_metaobject(seed, entry, display, &created));
        }
        // An app's own type, `$app:author` in the store data, is `app--<id>--author` in the
        // Admin API; its ids are made of the store data's spelling, as lsf-core makes them.
        let name = kind.strip_prefix("$app:").unwrap_or(kind);
        seed.insert(json!({
            "__typename": "MetaobjectDefinition",
            "id": metaobject_definition_gid(kind),
            "type": admin_namespace(seed.app_id, kind),
            "name": value::humanize(name),
            "description": null,
            "displayNameKey": display,
            "fieldDefinitions": field_definitions,
            "hasThumbnailField": false,
            "metaobjects": ids,
            "metaobjectsCount": entries.len(),
            "capabilities": {
                "publishable": {"enabled": true},
                "translatable": {"enabled": false},
                "onlineStore": {"enabled": true},
            },
            "createdAt": created,
            "updatedAt": created,
        }));
    }
}

/// The field a metaobject is named after: `name`, else `title`, when its type has one.
fn display_key<'k>(mut keys: impl Iterator<Item = &'k str> + Clone) -> Option<&'k str> {
    ["name", "title"]
        .into_iter()
        .find(|wanted| keys.clone().any(|key| key == *wanted))
        .and_then(|wanted| keys.find(|key| *key == wanted))
}

/// A `MetaobjectFieldDefinition`, inferred from the field of the entries.
fn field_definition(key: &str, kind: &str) -> Value {
    json!({
        "key": key,
        "name": value::humanize(key),
        "description": null,
        "required": false,
        "type": definition_type(kind),
        "validations": [],
    })
}

/// A `MetafieldDefinitionType`, by the name of a metafield type.
fn definition_type(kind: &str) -> Value {
    types::definition_type(kind)
}

fn seed_metaobject(
    seed: &mut Seed,
    entry: &Metaobject,
    display: Option<&str>,
    created: &str,
) -> String {
    let id = metaobject_gid(&entry.kind, &entry.handle);
    let mut fields = Vec::new();
    for (key, field) in &entry.fields {
        let converted = convert(seed, &field.kind, &field.value);
        let mut value = json!({
            "key": key,
            "type": field.kind,
            "value": converted.value,
            "jsonValue": converted.json,
            "definition": field_definition(key, &field.kind),
        });
        if let Some(reference) = converted.reference {
            value["reference"] = json!(reference);
        }
        if let Some(references) = converted.references {
            value["references"] = json!(references);
        }
        fields.push(value);
    }
    let display_name = display
        .and_then(|key| entry.fields.get(key))
        .and_then(|field| field.value.as_str())
        .filter(|name| !name.is_empty())
        .map_or_else(|| value::humanize(&entry.handle), str::to_string);
    seed.insert(json!({
        "__typename": "Metaobject",
        "id": id,
        "type": admin_namespace(seed.app_id, &entry.kind),
        "handle": entry.handle,
        "displayName": display_name,
        "definition": metaobject_definition_gid(&entry.kind),
        "fields": fields,
        "capabilities": {
            "publishable": {"status": "ACTIVE"},
            "onlineStore": {"templateSuffix": entry.template_suffix},
        },
        "createdAt": created,
        "updatedAt": created,
    }))
}

/// The markets: those the data declares, then a market of its own for each country in none of
/// them, as the checkout sees them.
fn seed_markets(seed: &mut Seed) {
    let store = seed.store;
    let shop_country = store.shop.address.country_code.to_ascii_uppercase();
    let mut markets: Vec<MarketSeed> = store
        .markets
        .iter()
        .map(|market| MarketSeed {
            key: market.handle.clone(),
            handle: market.handle.clone(),
            name: value::humanize(&market.handle),
            countries: market
                .countries
                .iter()
                .map(|code| code.to_ascii_uppercase())
                .collect(),
            metafields: Some(&market.metafields),
        })
        .collect();
    for country in &store.countries {
        let code = country.iso_code.to_ascii_uppercase();
        if store.market_of(&code).is_none() {
            markets.push(MarketSeed {
                key: code.clone(),
                handle: handleize(&country.name),
                name: country.name.clone(),
                countries: vec![code],
                metafields: None,
            });
        }
    }
    let created = Seed::epoch_string();
    for MarketSeed {
        key,
        handle,
        name,
        countries,
        metafields,
    } in markets
    {
        let id = market_gid(&key);
        let regions: Vec<Value> = countries
            .iter()
            .map(|code| {
                let country = store.country(code);
                let currency = country.map_or(seed.currency.clone(), |country| {
                    country.currency.to_ascii_uppercase()
                });
                json!({
                    "__typename": "MarketRegionCountry",
                    "id": gid("MarketRegionCountry", stable_id("market_region", code)),
                    "code": code,
                    "name": country.map_or_else(
                        || reference::country_name(code).unwrap_or(code).to_string(),
                        |country| country.name.clone(),
                    ),
                    "currency": currency_setting(&currency),
                })
            })
            .collect();
        let local_currency = countries
            .iter()
            .filter_map(|code| store.country(code))
            .map(|country| country.currency.to_ascii_uppercase())
            .find(|currency| *currency != seed.currency);
        let metafields = match metafields {
            Some(metafields) => seed.metafields(&id, "MARKET", &created, metafields),
            None => json!([]),
        };
        seed.insert(json!({
            "__typename": "Market",
            "id": id,
            "name": name,
            "handle": handle,
            "type": "REGION",
            "status": "ACTIVE",
            "enabled": true,
            "primary": countries.contains(&shop_country),
            "conditions": {
                "conditionTypes": ["REGION"],
                "regionsCondition": {"regions": regions},
            },
            "regions": regions,
            "currencySettings": {
                "baseCurrency": currency_setting(local_currency.as_deref().unwrap_or(&seed.currency)),
                "localCurrencies": local_currency.is_some(),
            },
            "metafields": metafields,
        }));
    }
}

/// A market about to be seeded: the key its id is made of (the handle of a declared market, the
/// code of a country of its own), its handle, name, countries and metafields.
struct MarketSeed<'a> {
    key: String,
    handle: String,
    name: String,
    countries: Vec<String>,
    metafields: Option<&'a Metafields>,
}

fn currency_setting(code: &str) -> Value {
    json!({
        "currencyCode": code,
        "currencyName": reference::currency_name(code),
        "enabled": true,
    })
}

/// The files of the shop: those the data describes (`files`) and those something refers to (a
/// file reference, a swatch). Runs last, once every reference is known.
pub fn seed_files(seed: &mut Seed) {
    let store = seed.store;
    let created = Seed::epoch_string();
    let mut paths: Vec<String> = store.files.keys().cloned().collect();
    for path in &seed.files {
        if !paths.contains(path) {
            paths.push(path.clone());
        }
    }
    for path in paths {
        let meta = store.files.get(&path);
        let alt = meta.map_or("", |meta| meta.alt.as_str());
        let id = catalog::file_gid(&path);
        let url = seed.file_url(&path);
        let (width, height) = store.image_size(&path).unwrap_or((1200, 1200));
        let record = match gid_type(&id) {
            Some("MediaImage") => {
                let image = json!({
                    "id": gid("ImageSource", stable_id("image", &path)),
                    "url": url,
                    "altText": text(alt),
                    "width": width,
                    "height": height,
                });
                json!({
                    "__typename": "MediaImage",
                    "id": id,
                    "alt": text(alt),
                    "mediaContentType": "IMAGE",
                    "mimeType": catalog::mime_type(&path),
                    "image": image,
                    "preview": {"image": image, "status": "READY"},
                    "originalSource": {"url": url},
                    "status": "READY",
                    "fileStatus": "READY",
                    "fileErrors": [],
                    "mediaErrors": [],
                    "mediaWarnings": [],
                    "createdAt": created,
                    "updatedAt": created,
                })
            }
            Some("Video") => {
                let (width, height) = store
                    .files
                    .get(&path)
                    .and_then(|meta| Some((meta.width?, meta.height?)))
                    .unwrap_or((1920, 1080));
                let source = json!({
                    "url": url,
                    "mimeType": catalog::mime_type(&path).replace("quicktime", "mp4"),
                    "format": "mp4",
                    "width": width,
                    "height": height,
                });
                json!({
                    "__typename": "Video",
                    "id": id,
                    "alt": text(alt),
                    "mediaContentType": "VIDEO",
                    "filename": path.rsplit('/').next().unwrap_or(&path),
                    "sources": [source],
                    "originalSource": source,
                    "status": "READY",
                    "fileStatus": "READY",
                    "fileErrors": [],
                    "mediaErrors": [],
                    "mediaWarnings": [],
                    "createdAt": created,
                    "updatedAt": created,
                })
            }
            _ => json!({
                "__typename": "GenericFile",
                "id": id,
                "alt": text(alt),
                "url": url,
                "mimeType": catalog::mime_type(&path),
                "fileStatus": "READY",
                "fileErrors": [],
                "createdAt": created,
                "updatedAt": created,
            }),
        };
        seed.insert(record);
    }
}

/// Converts a value of the store data: references (handles, paths, `type/handle`, SKUs) become
/// GIDs, `money` gets its currency, everything that is not a string becomes the JSON text
/// Shopify keeps.
fn convert(seed: &mut Seed, kind: &str, value: &Value) -> types::Checked {
    let base = kind.strip_prefix("list.");
    let reference = kind.ends_with("_reference");
    match base {
        Some(base) => {
            let items: Vec<Value> = match value {
                Value::Array(items) => items.clone(),
                Value::String(text) => serde_json::from_str::<Vec<Value>>(text)
                    .unwrap_or_else(|_| vec![Value::String(text.clone())]),
                Value::Null => Vec::new(),
                other => vec![other.clone()],
            };
            let converted: Vec<Value> = items
                .iter()
                .map(|item| {
                    if reference {
                        Value::String(
                            resolve_reference(seed, base, item).unwrap_or_else(|| text_of(item)),
                        )
                    } else {
                        list_item(seed, base, item)
                    }
                })
                .collect();
            let references = reference.then(|| {
                converted
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|gid| gid.starts_with("gid://"))
                    .map(str::to_string)
                    .collect()
            });
            types::Checked {
                value: Value::Array(converted.clone()).to_string(),
                json: Value::Array(converted),
                reference: None,
                references,
            }
        }
        None if reference => {
            let resolved = resolve_reference(seed, kind, value);
            let text = resolved.clone().unwrap_or_else(|| text_of(value));
            types::Checked {
                json: Value::String(text.clone()),
                value: text,
                reference: resolved,
                references: None,
            }
        }
        None => {
            let item = list_item(seed, kind, value);
            let value = text_of(&item);
            types::Checked {
                json: json_value(kind, &value),
                value,
                reference: None,
                references: None,
            }
        }
    }
}

/// A value that is not a reference, as Shopify keeps it: `money` with a string amount and its
/// currency, numbers and booleans as they are, the rest as given.
fn list_item(seed: &Seed, kind: &str, value: &Value) -> Value {
    match kind {
        "money" => {
            let mut money = value.as_object().cloned().unwrap_or_default();
            let amount = match money.get("amount") {
                Some(Value::Number(number)) => number.to_string(),
                Some(Value::String(text)) => text.clone(),
                _ => "0.0".to_string(),
            };
            let currency = money
                .get("currency_code")
                .and_then(Value::as_str)
                .filter(|code| !code.is_empty())
                .map_or_else(|| seed.currency.clone(), str::to_ascii_uppercase);
            money.insert("amount".to_string(), Value::String(amount));
            money.insert("currency_code".to_string(), Value::String(currency));
            Value::Object(money)
        }
        "boolean" => match value {
            Value::String(text) => Value::Bool(text == "true"),
            other => other.clone(),
        },
        _ => value.clone(),
    }
}

/// The text of a value: a string as it is, anything else as compact JSON.
fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A metafield's `jsonValue`: its value as JSON, according to its type (as the checkout's
/// Function input has it).
pub fn json_value(kind: &str, value: &str) -> Value {
    let text = || Value::String(value.to_string());
    match kind {
        "boolean" => Value::Bool(value == "true"),
        "number_integer" => value.parse::<i64>().map_or_else(|_| text(), Value::from),
        "single_line_text_field"
        | "multi_line_text_field"
        | "number_decimal"
        | "url"
        | "color"
        | "date"
        | "date_time"
        | "id"
        | "language" => text(),
        kind if kind.ends_with("_reference") && !kind.starts_with("list.") => text(),
        _ => serde_json::from_str(value).unwrap_or_else(|_| text()),
    }
}

/// The GID a reference of the store data names: a handle, a path of `files/`, `type/handle` of
/// a metaobject, a variant id or SKU, or a GID already. `None` when it names nothing.
fn resolve_reference(seed: &mut Seed, kind: &str, value: &Value) -> Option<String> {
    let store = seed.store;
    let text = value.as_str().map(str::trim);
    if let Some(gid) = text.filter(|text| text.starts_with("gid://shopify/")) {
        return Some(gid.to_string());
    }
    match kind {
        "product_reference" => match value {
            Value::Number(number) => number.as_u64().map(|id| gid("Product", id)),
            _ => store.product_by_handle(text?).map(catalog::product_gid),
        },
        "variant_reference" => {
            let variant = match value {
                Value::Number(number) => store.variant(number.as_u64()?).map(|(_, v)| v),
                _ => {
                    let sku = text?;
                    store
                        .products
                        .iter()
                        .flat_map(|product| &product.variants)
                        .find(|variant| variant.sku == sku)
                }
            };
            variant.map(catalog::variant_gid)
        }
        "collection_reference" => store
            .collection_by_handle(text?)
            .filter(|collection| !catalog::is_implicit_all(store, collection))
            .map(catalog::collection_gid),
        "page_reference" => store
            .page_index(text?)
            .map(|index| page_gid(&store.pages[index])),
        "article_reference" => {
            let text = text?;
            let found = match text.split_once('/') {
                Some((blog, article)) => store
                    .blog_index(blog)
                    .and_then(|blog| Some((blog, store.article_index(blog, article)?))),
                None => {
                    let mut matches = (0..store.blogs.len())
                        .filter_map(|blog| Some((blog, store.article_index(blog, text)?)));
                    match (matches.next(), matches.next()) {
                        (Some(found), None) => Some(found),
                        _ => None,
                    }
                }
            };
            found.map(|(blog, article)| article_gid(&store.blogs[blog].articles[article]))
        }
        "file_reference" => Some(seed.file(text.filter(|text| !text.is_empty())?)),
        "metaobject_reference" | "mixed_reference" => {
            let (kind, handle) = text?.split_once('/')?;
            store
                .metaobject(kind, handle)
                .map(|entry| metaobject_gid(&entry.kind, &entry.handle))
        }
        "customer_reference" => match value {
            Value::Number(number) => number.as_u64().map(|id| gid("Customer", id)),
            _ => store
                .customer_by_email(text?)
                .map(|customer| gid("Customer", customer.id)),
        },
        "company_reference" => {
            let name = text?;
            store
                .companies
                .iter()
                .find(|company| company.name.eq_ignore_ascii_case(name))
                .map(|company| gid("Company", company.id))
        }
        _ => None,
    }
}

/// Seeds the metafields of an owner (see [`Seed::metafields`]).
pub fn seed_metafields(
    seed: &mut Seed,
    owner: &str,
    owner_type: &str,
    created_at: &str,
    metafields: &Metafields,
) -> Value {
    let mut ids = Vec::new();
    for (namespace, fields) in metafields {
        for (key, field) in fields {
            ids.push(seed_metafield(
                seed, owner, owner_type, created_at, namespace, key, field,
            ));
        }
    }
    json!(ids)
}

/// Seeds the metafields of a checkout setting, whose values are already strings.
pub fn seed_checkout_metafields(
    seed: &mut Seed,
    owner: &str,
    owner_type: &str,
    created_at: &str,
    metafields: &[CheckoutMetafield],
) -> Value {
    let mut ids = Vec::new();
    for metafield in metafields {
        let value = match metafield.kind.as_str() {
            kind if kind == "json" || kind.starts_with("list.") || kind == "money" => {
                serde_json::from_str(&metafield.value)
                    .unwrap_or_else(|_| Value::String(metafield.value.clone()))
            }
            _ => Value::String(metafield.value.clone()),
        };
        let field = Metafield {
            kind: metafield.kind.clone(),
            value,
        };
        ids.push(seed_metafield(
            seed,
            owner,
            owner_type,
            created_at,
            &metafield.namespace,
            &metafield.key,
            &field,
        ));
    }
    json!(ids)
}

fn seed_metafield(
    seed: &mut Seed,
    owner: &str,
    owner_type: &str,
    created_at: &str,
    namespace: &str,
    key: &str,
    field: &Metafield,
) -> String {
    let namespace = admin_namespace(seed.app_id, namespace);
    let converted = convert(seed, &field.kind, &field.value);
    let mut record = value::object(json!({
        "__typename": "Metafield",
        "id": metafield_gid(owner, &namespace, key),
        "namespace": namespace,
        "key": key,
        "type": field.kind,
        "value": converted.value,
        "jsonValue": converted.json,
        "owner": owner,
        "ownerType": owner_type,
        "definition": null,
        "sizeInBytes": converted.value.len(),
        "createdAt": created_at,
        "updatedAt": created_at,
    }));
    if field.kind.ends_with("_reference") {
        if field.kind.starts_with("list.") {
            record.insert(
                "references".to_string(),
                json!(converted.references.unwrap_or_default()),
            );
        } else {
            record.insert("reference".to_string(), json!(converted.reference));
        }
    }
    seed.insert_record(record)
}

/// Gives every metafield a definition, inferred from the metafields that exist: one per owner
/// type, namespace and key, with the type of the first metafield. The reserved namespaces of
/// the app (`app--…`) get none: their definitions come from the app's configuration, which lsf
/// does not read.
pub fn infer_metafield_definitions(state: &mut ShopState) {
    let metafields: Vec<String> = state.keys("Metafield").map(str::to_string).collect();
    for metafield in metafields {
        attach_definition(state, &metafield);
    }
}

/// Links a metafield to the definition of its owner type, namespace and key, creating the
/// definition when there is none (with the metafield's type). What the seed runs
/// ([`infer_metafield_definitions`]); a mutation links a metafield to a definition that exists
/// and creates none ([`metafields::link_definition`]). Returns the GID of the definition, `None`
/// for an app-reserved namespace.
pub fn attach_definition(state: &mut ShopState, metafield: &str) -> Option<String> {
    let record = state.get(metafield)?;
    let field = |name: &str| {
        record
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let (namespace, key, owner_type, kind) = (
        field("namespace"),
        field("key"),
        field("ownerType"),
        field("type"),
    );
    if namespace.starts_with("app--") {
        return None;
    }
    // The definition of the owner type, namespace and key, whatever its id
    // (`metafieldDefinitionCreate` gives one of the counter), else the seed's.
    let id = metafields::find_definition(state, &owner_type, &namespace, &key)
        .unwrap_or_else(|| metafield_definition_gid(&owner_type, &namespace, &key));
    if !state.contains(&id) {
        state.insert(value::object(json!({
            "__typename": "MetafieldDefinition",
            "id": id,
            "namespace": namespace,
            "key": key,
            "name": value::humanize(&key),
            "description": null,
            "ownerType": owner_type,
            "type": definition_type(&kind),
            "validations": [],
            "validationStatus": "ALL_VALID",
            "pinnedPosition": null,
            "useAsCollectionCondition": false,
            "access": {"admin": "PUBLIC_READ_WRITE", "storefront": "PUBLIC_READ", "customerAccount": "NONE"},
            "capabilities": {
                "adminFilterable": {"eligible": true, "enabled": false, "status": "NOT_FILTERABLE"},
                "analyticsQueryable": {"eligible": true, "enabled": false},
                "cartToOrderCopyable": {"eligible": true, "enabled": false},
                "smartCollectionCondition": {"eligible": true, "enabled": false},
                "uniqueValues": {"eligible": true, "enabled": false},
            },
            "constraints": null,
            "standardTemplate": null,
            "metafields": [],
            "metafieldsCount": 0,
        })));
    }
    let definition: &mut Record = state.get_mut(&id)?;
    let same_type = definition
        .get("type")
        .and_then(|kind| kind.get("name"))
        .and_then(Value::as_str)
        == Some(kind.as_str());
    if let Some(Value::Array(metafields)) = definition.get_mut("metafields")
        && !metafields
            .iter()
            .any(|known| known.as_str() == Some(metafield))
    {
        metafields.push(json!(metafield));
        let count = metafields.len();
        definition.insert("metafieldsCount".to_string(), json!(count));
    }
    if !same_type {
        definition.insert("validationStatus".to_string(), json!("SOME_INVALID"));
    }
    if let Some(record) = state.get_mut(metafield) {
        record.insert("definition".to_string(), json!(id));
    }
    Some(id)
}

/// The numeric id of the app (`gid://shopify/App/<id>`), which its reserved namespaces name.
pub fn app_id(state: &ShopState) -> u64 {
    state
        .keys("App")
        .next()
        .and_then(crate::state::parse_gid)
        .map_or(0, |(_, id)| id)
}

/// The custom resolvers and the mutations of this family: see "Writing a resolver or a
/// mutation" in `crates/admin/docs/design.md`.
pub fn register(registry: &mut crate::resolve::Registry) {
    metafields::register(registry);
    metafield_definitions::register(registry);
    metaobjects::register(registry);
    online_store::register(registry);
    navigation::register(registry);
    files::register(registry);
    translations::register(registry);
}

/// A code of a user error as the error type's code enum has it: itself when the enum has it,
/// else `INVALID` when the enum has that, else none (the checks of metafield inputs, shared by
/// the mutations of several types, have codes some of their enums lack).
pub fn fit_code(code: Option<&str>, code_enum: &str) -> Option<String> {
    let values = &crate::schema::admin().get(code_enum)?.enum_values;
    let code = code?;
    if values.contains_key(code) {
        Some(code.to_string())
    } else {
        values
            .contains_key("INVALID")
            .then(|| "INVALID".to_string())
    }
}

#[cfg(test)]
mod seed_tests {
    use super::*;

    #[test]
    fn names_the_app_namespace_as_shopify() {
        assert_eq!(admin_namespace(42, "$app"), "app--42");
        assert_eq!(admin_namespace(42, "$app:config"), "app--42--config");
        assert_eq!(admin_namespace(42, "custom"), "custom");
    }

    #[test]
    fn reads_json_values() {
        assert_eq!(json_value("number_integer", "42"), json!(42));
        assert_eq!(json_value("boolean", "true"), json!(true));
        assert_eq!(json_value("number_decimal", "4.5"), json!("4.5"));
        assert_eq!(json_value("json", r#"{"a":1}"#), json!({"a": 1}));
        assert_eq!(
            json_value("product_reference", "gid://shopify/Product/1"),
            json!("gid://shopify/Product/1")
        );
        assert_eq!(
            json_value("list.product_reference", r#"["gid://shopify/Product/1"]"#),
            json!(["gid://shopify/Product/1"])
        );
        assert_eq!(types::category("list.weight"), Some("MEASUREMENT"));
        assert_eq!(types::category("product_reference"), Some("REFERENCE"));
        assert_eq!(types::category("id"), Some("TEXT"));
    }
}
