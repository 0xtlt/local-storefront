//! Storefront search over the store data: `search`, `predictive_search`, `recommendations`.

use lsf_liquid::Value;

use super::collection::{CollectionDrop, sort_options};
use super::content::{ArticleDrop, PageDrop};
use super::product::ProductDrop;
use super::{PaginatedList, SiteRef, hash, strings};

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

/// Runs a search and returns the matching resources as drops, grouped by type.
pub fn run(site: &SiteRef, query: &str, types: &[SearchType]) -> Vec<(SearchType, Vec<Value>)> {
    let clauses = clauses(query);
    let store = &site.store;
    let mut out = Vec::new();
    if clauses.is_empty() {
        return out;
    }
    for kind in types {
        let results: Vec<Value> = match kind {
            SearchType::Product => store
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
                .map(|(index, _)| ProductDrop::value(site, index))
                .collect(),
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
    let results: Vec<Value> = run(site, &query, &types)
        .into_iter()
        .flat_map(|(_, results)| results)
        .collect();
    hash([
        ("terms", Value::from(&query)),
        ("performed", Value::Bool(performed)),
        ("results_count", Value::from(results.len())),
        ("results", PaginatedList::value(results)),
        ("types", type_names(&types)),
        ("filters", Value::array(Vec::new())),
        ("sort_options", sort_options()),
        (
            "sort_by",
            site.request
                .param("sort_by")
                .map_or_else(|| Value::str("relevance"), Value::from),
        ),
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
    for (kind, mut results) in run(site, &query, &types) {
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
