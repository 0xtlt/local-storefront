//! Storefront search over the store data: `search`, `predictive_search`, `recommendations`.

use lsf_core::store::SortOrder;
use lsf_liquid::Value;

use super::collection::{
    CollectionDrop, criteria, matches as passes_filters, options_value, sort_products,
    storefront_filters,
};
use super::content::{ArticleDrop, PageDrop};
use super::product::ProductDrop;
use super::{PaginatedList, SiteRef, hash, strings};
use crate::render::cost;

/// The resource types a search covers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchType {
    Product,
    Article,
    Page,
    Collection,
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// One alternative of a query: free-text terms and `field:value` conditions that must all hold.
#[derive(Default)]
struct Clause {
    terms: Vec<String>,
    /// `(field, value)`, both lowercase.
    fields: Vec<(String, String)>,
}

/// Parses a search query: clauses separated by `OR`, each made of terms and `field:value`
/// conditions (`id:123`, `tag:sale`, `vendor:acme`). A trailing `*` marks a prefix.
fn clauses(query: &str) -> Vec<Clause> {
    let mut out = vec![Clause::default()];
    for word in query.split_whitespace() {
        if word == "OR" {
            out.push(Clause::default());
            continue;
        }
        if word == "AND" {
            continue;
        }
        let word = word.trim_end_matches('*').trim_matches('"').to_lowercase();
        if word.is_empty() {
            continue;
        }
        let clause = out.last_mut().expect("at least one clause");
        match word.split_once(':') {
            Some((field, value)) if !field.is_empty() && !value.is_empty() => {
                clause.fields.push((field.to_string(), value.to_string()));
            }
            _ => clause.terms.push(word),
        }
    }
    out.retain(|clause| !clause.terms.is_empty() || !clause.fields.is_empty());
    out
}

/// A searchable resource: its free text and the values of the fields it can be queried by.
struct Document {
    text: String,
    fields: Vec<(&'static str, String)>,
}

fn matches(clauses: &[Clause], document: &Document) -> bool {
    let text = document.text.to_lowercase();
    clauses.iter().any(|clause| {
        clause.terms.iter().all(|term| text.contains(term.as_str()))
            && clause.fields.iter().all(|(field, value)| {
                document
                    .fields
                    .iter()
                    .any(|(name, candidate)| name == field && candidate.to_lowercase() == *value)
            })
    })
}

/// The sort orders of the search page: `(value, label)`. `relevance` is the default.
/// https://shopify.dev/docs/storefronts/themes/navigation-search/search
const SORT_OPTIONS: [(&str, &str); 3] = [
    ("relevance", "Relevance"),
    ("price-ascending", "Price, low to high"),
    ("price-descending", "Price, high to low"),
];

/// The order requested by `sort_by`: only the price orders reorder products, anything else
/// (`relevance`, a collection sort order, an unknown value) keeps the order of the data.
fn sort_order(site: &SiteRef) -> SortOrder {
    match site.request.param("sort_by").and_then(SortOrder::parse) {
        Some(order @ (SortOrder::PriceAscending | SortOrder::PriceDescending)) => order,
        _ => SortOrder::Manual,
    }
}

fn applied_sort_by(order: SortOrder) -> &'static str {
    match order {
        SortOrder::Manual => "relevance",
        order => order.as_str(),
    }
}

/// What a search does with the products that cannot be bought (`unavailable_products`): `show`
/// them in their place, `hide` them, or put them `last`, the default.
/// https://shopify.dev/docs/storefronts/themes/navigation-search/search
/// https://shopify.dev/docs/api/ajax/reference/predictive-search
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    Show,
    Hide,
    Last,
}

impl Unavailable {
    fn parse(value: Option<&str>) -> Unavailable {
        match value {
            Some("show") => Unavailable::Show,
            Some("hide") => Unavailable::Hide,
            _ => Unavailable::Last,
        }
    }
}

/// Whether a result is a product that cannot be bought.
fn unavailable(site: &SiteRef, result: &Value) -> bool {
    result
        .downcast::<ProductDrop>()
        .is_some_and(|drop| !site.store.products[drop.index].available())
}

/// Runs a search and returns the matching resources as drops, grouped by type. Products are
/// sorted by `order`, then the unavailable ones hidden or put after the others.
pub fn run(
    site: &SiteRef,
    query: &str,
    types: &[SearchType],
    order: SortOrder,
    unavailable: Unavailable,
) -> Vec<(SearchType, Vec<Value>)> {
    cost::asked(cost::SEARCH);
    let clauses = clauses(query);
    let store = &site.store;
    let mut out = Vec::new();
    if clauses.is_empty() {
        return out;
    }
    for kind in types {
        let results: Vec<Value> = match kind {
            SearchType::Product => {
                let mut indexes: Vec<usize> = store
                    .products
                    .iter()
                    .enumerate()
                    .filter(|(_, product)| {
                        let variants: Vec<&str> = product
                            .variants
                            .iter()
                            .flat_map(|v| [v.title.as_str(), v.sku.as_str()])
                            .collect();
                        let mut fields = vec![
                            ("id", product.id.to_string()),
                            ("handle", product.handle.clone()),
                            ("title", product.title.clone()),
                            ("vendor", product.vendor.clone()),
                            ("product_type", product.product_type.clone()),
                        ];
                        fields.extend(product.tags.iter().map(|tag| ("tag", tag.clone())));
                        fields.extend(
                            product
                                .variants
                                .iter()
                                .map(|variant| ("sku", variant.sku.clone())),
                        );
                        fields.extend(
                            product
                                .variants
                                .iter()
                                .map(|variant| ("variants.sku", variant.sku.clone())),
                        );
                        let document = Document {
                            text: format!(
                                "{} {} {} {} {} {}",
                                product.title,
                                strip_tags(&product.description),
                                product.vendor,
                                product.product_type,
                                product.tags.join(" "),
                                variants.join(" ")
                            ),
                            fields,
                        };
                        matches(&clauses, &document)
                    })
                    .map(|(index, _)| index)
                    .collect();
                sort_products(&store.products, &mut indexes, order);
                match unavailable {
                    Unavailable::Show => {}
                    Unavailable::Hide => indexes.retain(|&index| store.products[index].available()),
                    // A stable sort: the order is kept within each part.
                    Unavailable::Last => {
                        indexes.sort_by_key(|&index| !store.products[index].available())
                    }
                }
                indexes
                    .into_iter()
                    .map(|index| ProductDrop::value(site, index))
                    .collect()
            }
            SearchType::Page => store
                .pages
                .iter()
                .enumerate()
                .filter(|(_, page)| {
                    let document = Document {
                        text: format!("{} {}", page.title, strip_tags(&page.content)),
                        fields: vec![
                            ("id", page.id.to_string()),
                            ("handle", page.handle.clone()),
                            ("title", page.title.clone()),
                        ],
                    };
                    matches(&clauses, &document)
                })
                .map(|(index, _)| PageDrop::value(site, index))
                .collect(),
            SearchType::Article => {
                let mut found = Vec::new();
                for (blog_index, blog) in store.blogs.iter().enumerate() {
                    for (article_index, article) in blog.articles.iter().enumerate() {
                        let mut fields = vec![
                            ("id", article.id.to_string()),
                            ("handle", article.handle.clone()),
                            ("title", article.title.clone()),
                            ("author", article.author.clone()),
                        ];
                        fields.extend(article.tags.iter().map(|tag| ("tag", tag.clone())));
                        let document = Document {
                            text: format!(
                                "{} {} {}",
                                article.title,
                                strip_tags(&article.content),
                                article.tags.join(" ")
                            ),
                            fields,
                        };
                        if matches(&clauses, &document) {
                            found.push(ArticleDrop::value(site, blog_index, article_index));
                        }
                    }
                }
                found
            }
            SearchType::Collection => store
                .collections
                .iter()
                .enumerate()
                .filter(|(_, collection)| {
                    let document = Document {
                        text: format!(
                            "{} {}",
                            collection.title,
                            strip_tags(&collection.description)
                        ),
                        fields: vec![
                            ("id", collection.id.to_string()),
                            ("handle", collection.handle.clone()),
                            ("title", collection.title.clone()),
                        ],
                    };
                    matches(&clauses, &document)
                })
                .map(|(index, _)| CollectionDrop::value(site, index))
                .collect(),
        };
        out.push((*kind, results));
    }
    out
}

fn parse_types(value: Option<&str>, default: &[SearchType]) -> Vec<SearchType> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return default.to_vec();
    };
    value
        .split(',')
        .filter_map(|name| match name.trim() {
            "product" => Some(SearchType::Product),
            "article" => Some(SearchType::Article),
            "page" => Some(SearchType::Page),
            "collection" => Some(SearchType::Collection),
            _ => None,
        })
        .collect()
}

fn type_names(types: &[SearchType]) -> Value {
    strings(
        &types
            .iter()
            .map(|kind| {
                match kind {
                    SearchType::Product => "product",
                    SearchType::Article => "article",
                    SearchType::Page => "page",
                    SearchType::Collection => "collection",
                }
                .to_string()
            })
            .collect::<Vec<_>>(),
    )
}

/// The `search` object of the search page.
pub fn search_value(site: &SiteRef) -> Value {
    let query = site.request.param("q").unwrap_or_default().to_string();
    let types = parse_types(
        site.request.param("type"),
        &[SearchType::Product, SearchType::Article, SearchType::Page],
    );
    let performed = site.request.param("q").is_some();
    let order = sort_order(site);
    // Under a price sort the non-product results go to the end of the list, in their own order.
    // https://shopify.dev/docs/storefronts/themes/navigation-search/search
    let mut searched = types.clone();
    if order != SortOrder::Manual {
        searched.sort_by_key(|kind| *kind != SearchType::Product);
    }
    // Storefront filters narrow the products, and leave out every other result: "If you apply
    // filters on the search results page, then all non-product results are filtered out."
    // https://shopify.dev/docs/storefronts/themes/navigation-search/filtering/storefront-filtering/support-storefront-filtering
    let active = criteria(&site.request);
    if !active.is_empty() {
        searched.retain(|kind| *kind == SearchType::Product);
    }
    let unavailable_products =
        Unavailable::parse(site.request.param("options[unavailable_products]"));
    let found = run(site, &query, &searched, order, unavailable_products);
    // The filters are those of the products found, before the URL's filters narrow them.
    let base: Vec<usize> = found
        .iter()
        .filter(|(kind, _)| *kind == SearchType::Product)
        .flat_map(|(_, results)| results)
        .filter_map(|result| result.downcast::<ProductDrop>().map(|drop| drop.index))
        .collect();
    let products = &site.store.products;
    // Pagination slices this list, so it is sorted before `paginate` sees it.
    let mut results: Vec<Value> = found
        .into_iter()
        .flat_map(|(_, results)| results)
        .filter(|result| {
            result
                .downcast::<ProductDrop>()
                .is_none_or(|drop| passes_filters(&products[drop.index], &active))
        })
        .collect();
    let filters = if base.is_empty() {
        Value::array(Vec::new())
    } else {
        storefront_filters(site, base, active)
    };
    // `last` puts them "after all other matching results", pages and articles too.
    if unavailable_products == Unavailable::Last {
        results.sort_by_key(|result| unavailable(site, result));
    }
    hash([
        ("terms", Value::from(&query)),
        ("performed", Value::Bool(performed)),
        ("results_count", Value::from(results.len())),
        ("results", PaginatedList::value(results)),
        ("types", type_names(&types)),
        ("filters", filters),
        ("sort_options", options_value(&SORT_OPTIONS)),
        ("sort_by", Value::str(applied_sort_by(order))),
        ("default_sort_by", Value::str("relevance")),
    ])
}

/// The `predictive_search` object, for `/search/suggest`.
pub fn predictive_search_value(site: &SiteRef) -> Value {
    let query = site.request.param("q").unwrap_or_default().to_string();
    let types = parse_types(
        site.request.param("resources[type]"),
        &[
            SearchType::Product,
            SearchType::Collection,
            SearchType::Page,
            SearchType::Article,
        ],
    );
    let limit = site
        .request
        .param("resources[limit]")
        .and_then(|limit| limit.parse::<usize>().ok())
        .unwrap_or(10)
        .clamp(1, 10);
    let mut products = Vec::new();
    let mut collections = Vec::new();
    let mut pages = Vec::new();
    let mut articles = Vec::new();
    let unavailable_products = Unavailable::parse(
        site.request
            .param("resources[options][unavailable_products]"),
    );
    for (kind, mut results) in run(
        site,
        &query,
        &types,
        SortOrder::Manual,
        unavailable_products,
    ) {
        results.truncate(limit);
        match kind {
            SearchType::Product => products = results,
            SearchType::Collection => collections = results,
            SearchType::Page => pages = results,
            SearchType::Article => articles = results,
        }
    }
    hash([
        ("performed", Value::Bool(!query.is_empty())),
        ("terms", Value::from(&query)),
        ("types", type_names(&types)),
        (
            "resources",
            hash([
                ("products", Value::array(products)),
                ("collections", Value::array(collections)),
                ("pages", Value::array(pages)),
                ("articles", Value::array(articles)),
                ("queries", Value::array(Vec::new())),
            ]),
        ),
    ])
}

/// The `recommendations` object, for `/recommendations/products?product_id=...`.
pub fn recommendations_value(site: &SiteRef) -> Value {
    cost::asked(cost::SEARCH);
    let product = site
        .request
        .param("product_id")
        .and_then(|id| id.parse::<u64>().ok())
        .and_then(|id| site.store.product_index_by_id(id));
    let limit = site
        .request
        .param("limit")
        .and_then(|limit| limit.parse::<usize>().ok())
        .unwrap_or(10)
        .clamp(1, 10);
    let intent = site
        .request
        .param("intent")
        .unwrap_or("related")
        .to_string();
    let products: Vec<Value> = product
        .map(|index| {
            site.store.products[index]
                .recommendations
                .iter()
                .take(limit)
                .map(|&recommended| ProductDrop::value(site, recommended))
                .collect()
        })
        .unwrap_or_default();
    hash([
        ("performed?", Value::Bool(product.is_some())),
        ("performed", Value::Bool(product.is_some())),
        ("products_count", Value::from(products.len())),
        ("products", Value::array(products)),
        ("intent", Value::from(intent)),
    ])
}
