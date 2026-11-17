//! The Storefront API that a checkout or customer account UI extension queries with
//! `shopify.query`, when it has the `api_access` capability ("Allows your extension to query the
//! Storefront API", shopify.dev's capabilities page). The answer is read from the store data,
//! with the GraphQL executor of the Function inputs ([`crate::checkout::input::exec`]).
//!
//! It answers what checkout extensions read about the lines of a cart:
//!
//! - `node(id:)`, `nodes(ids:)` and `product(id:, handle:)` for products and product variants;
//! - their scalar fields, `ProductVariant.product`, `price` and `compareAtPrice` (`MoneyV2`),
//!   `selectedOptions`;
//! - `metafield(namespace:, key:)` and `metafields(identifiers:)` on both, from the metafields of
//!   the store data.
//!
//! A field it does not answer is an error that names it (`unknown field `variants` on type
//! `Product``), so that a test sees what is missing; so is an id of another type than a product
//! or a variant. A product that is not on the storefront (a draft, or one not published) is
//! `null`, as are its variants, as the Storefront API hides them. `quantityAvailable` and
//! `totalInventory` are refused as Shopify refuses them: the scopes of `api_access` do not include
//! `unauthenticated_read_product_inventory`. Amounts are in the shop's currency.

use graphql_parser::query::{Definition, OperationDefinition};
use lsf_core::store::{Metafields, Product, Variant};
use serde_json::{Value as Json, json};

use super::input::exec::{self, Args, Graph, Resolved};
use super::input::{decimal, store_metafield};

/// Runs `query` with `variables` against the store: `{"data": …}`, or `{"errors": [{"message":
/// …}]}` when the query is not valid or asks for something lsf does not answer.
pub fn query(store: &lsf_core::Store, query: &str, variables: Option<&Json>) -> Json {
    match run(store, query, variables) {
        Ok(data) => json!({ "data": data }),
        Err(message) => json!({ "errors": [{ "message": message }] }),
    }
}

fn run(store: &lsf_core::Store, query: &str, variables: Option<&Json>) -> Result<Json, String> {
    let document = graphql_parser::parse_query::<String>(query)
        .map_err(|error| format!("invalid GraphQL: {error}"))?;
    let operations: Vec<&OperationDefinition<String>> = document
        .definitions
        .iter()
        .filter_map(|definition| match definition {
            Definition::Operation(operation) => Some(operation),
            Definition::Fragment(_) => None,
        })
        .collect();
    if operations.len() > 1 {
        return Err(
            "the document has several operations: shopify.query runs one, give one only"
                .to_string(),
        );
    }
    if let Some(OperationDefinition::Mutation(_) | OperationDefinition::Subscription(_)) =
        operations.first()
    {
        return Err(
            "lsf's Storefront API answers queries only: a mutation (a cart, a customer) is not run"
                .to_string(),
        );
    }
    let empty = serde_json::Map::new();
    let variables = match variables {
        None | Some(Json::Null) => &empty,
        Some(Json::Object(map)) => map,
        Some(other) => return Err(format!("the variables must be a JSON object, got {other}")),
    };
    exec::execute(&Storefront { store }, &document, variables)
}

struct Storefront<'s> {
    store: &'s lsf_core::Store,
}

#[derive(Clone)]
enum Node<'s> {
    Query,
    Product(&'s Product),
    Variant(&'s Product, &'s Variant),
    Money(i64),
    /// `(name, value)` of an option of a variant.
    SelectedOption(&'s str, &'s str),
    Metafield(Box<FoundMetafield>),
}

#[derive(Clone)]
struct FoundMetafield {
    owner: String,
    namespace: String,
    key: String,
    kind: String,
    value: String,
}

/// The types of the Storefront API a fragment may name. Those lsf answers no object of are never
/// matched.
const TYPES: &[&str] = &[
    "QueryRoot",
    "Node",
    "HasMetafields",
    "Merchandise",
    "OnlineStorePublishable",
    "Trackable",
    "Product",
    "ProductVariant",
    "MoneyV2",
    "SelectedOption",
    "Metafield",
    "Collection",
    "Metaobject",
    "Page",
    "Blog",
    "Article",
    "Shop",
    "Location",
    "Market",
    "Menu",
    "MenuItem",
    "MediaImage",
    "Video",
    "ExternalVideo",
    "Model3d",
    "Cart",
    "CartLine",
    "ComponentizableCartLine",
    "Customer",
    "Order",
    "SellingPlan",
    "ShopPolicy",
    "UrlRedirect",
    "Comment",
];

/// The interfaces and unions each type lsf answers belongs to.
fn implements(type_name: &str, condition: &str) -> bool {
    match type_name {
        "Product" => matches!(
            condition,
            "Node" | "HasMetafields" | "OnlineStorePublishable" | "Trackable"
        ),
        "ProductVariant" => matches!(condition, "Node" | "HasMetafields" | "Merchandise"),
        _ => false,
    }
}

impl<'s> Graph for Storefront<'s> {
    type Node = Node<'s>;

    fn root(&self) -> Node<'s> {
        Node::Query
    }

    fn type_name(&self, node: &Node<'s>) -> &'static str {
        match node {
            Node::Query => "QueryRoot",
            Node::Product(_) => "Product",
            Node::Variant(..) => "ProductVariant",
            Node::Money(_) => "MoneyV2",
            Node::SelectedOption(..) => "SelectedOption",
            Node::Metafield(_) => "Metafield",
        }
    }

    fn applies(&self, node: &Node<'s>, condition: &str) -> Option<bool> {
        if !TYPES.contains(&condition) {
            return None;
        }
        let name = self.type_name(node);
        Some(name == condition || implements(name, condition))
    }

    fn field(
        &self,
        node: &Node<'s>,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Node<'s>>>, String> {
        match node {
            Node::Query => self.query_root(name, args),
            Node::Product(product) => self.product(product, name, args),
            Node::Variant(product, variant) => self.variant(product, variant, name, args),
            Node::Money(cents) => Ok(match name {
                "amount" => leaf(decimal(*cents)),
                "currencyCode" => leaf(self.store.currency()),
                _ => None,
            }),
            Node::SelectedOption(option, value) => Ok(match name {
                "name" => leaf(option),
                "value" => leaf(value),
                _ => None,
            }),
            Node::Metafield(found) => Ok(match name {
                "id" => leaf(format!(
                    "gid://shopify/Metafield/{}",
                    lsf_core::util::stable_id(
                        "metafield",
                        &format!("{}/{}.{}", found.owner, found.namespace, found.key)
                    )
                )),
                "namespace" => leaf(&found.namespace),
                "key" => leaf(&found.key),
                "type" => leaf(&found.kind),
                "value" => leaf(&found.value),
                _ => None,
            }),
        }
    }
}

impl<'s> Storefront<'s> {
    fn query_root(
        &self,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Node<'s>>>, String> {
        Ok(match name {
            "node" => {
                let id = args.required_string("id")?;
                Some(self.node(&id)?.map_or(Resolved::Null, Resolved::Object))
            }
            "nodes" => {
                let ids = args.string_list("ids")?;
                if ids.len() > 250 {
                    return Err(format!(
                        "`ids` holds {} ids, over the 250 the Storefront API takes",
                        ids.len()
                    ));
                }
                let mut nodes = Vec::new();
                for id in &ids {
                    nodes.push(self.node(id)?);
                }
                Some(Resolved::NullableList(nodes))
            }
            "product" => {
                let id = args.string("id")?;
                let handle = args.string("handle")?;
                let product = match (id, handle) {
                    (Some(id), None) => match parse_gid(&id) {
                        Some(("Product", number)) => self.product_by_id(number),
                        _ => return Err(format!("invalid id `{id}`: not a Product's")),
                    },
                    (None, Some(handle)) => self.store.product_by_handle(&handle),
                    _ => return Err("give exactly one of `id` and `handle`".to_string()),
                };
                Some(
                    product
                        .filter(|product| product.listed())
                        .map_or(Resolved::Null, |product| {
                            Resolved::Object(Node::Product(product))
                        }),
                )
            }
            _ => None,
        })
    }

    /// The product or variant an id names; `None` for one that does not exist or is not on the
    /// storefront.
    fn node(&self, id: &str) -> Result<Option<Node<'s>>, String> {
        let Some((kind, number)) = parse_gid(id) else {
            return Err(format!("invalid global id `{id}`"));
        };
        Ok(match kind {
            "Product" => self
                .product_by_id(number)
                .filter(|product| product.listed())
                .map(Node::Product),
            "ProductVariant" => self
                .store
                .variant(number)
                .filter(|(product, _)| product.listed())
                .map(|(product, variant)| Node::Variant(product, variant)),
            other => {
                return Err(format!(
                    "`{id}` is a {other}: lsf's Storefront API answers `node` and `nodes` for products and product variants only"
                ));
            }
        })
    }

    fn product_by_id(&self, id: u64) -> Option<&'s Product> {
        self.store
            .product_index_by_id(id)
            .map(|index| &self.store.products[index])
    }

    fn product(
        &self,
        product: &'s Product,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Node<'s>>>, String> {
        let owner = format!("gid://shopify/Product/{}", product.id);
        Ok(match name {
            "id" => leaf(&owner),
            "handle" => leaf(&product.handle),
            "title" => leaf(&product.title),
            "description" => leaf(crate::admin::state::value::plain_text(&product.description)),
            "descriptionHtml" => leaf(&product.description),
            "vendor" => leaf(&product.vendor),
            "productType" => leaf(&product.product_type),
            "tags" => leaf(&product.tags),
            "availableForSale" => leaf(product.available()),
            "isGiftCard" => leaf(product.gift_card),
            "requiresSellingPlan" => leaf(product.requires_selling_plan),
            "createdAt" => leaf(date_time(product.created_at)),
            "updatedAt" => leaf(date_time(product.updated_at)),
            "publishedAt" => leaf(date_time(product.published_at)),
            "totalInventory" => return Err(inventory_denied(name)),
            "metafield" | "metafields" => {
                metafields(self.store, &product.metafields, &owner, name, args)?
            }
            _ => None,
        })
    }

    fn variant(
        &self,
        product: &'s Product,
        variant: &'s Variant,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Node<'s>>>, String> {
        let owner = format!("gid://shopify/ProductVariant/{}", variant.id);
        Ok(match name {
            "id" => leaf(&owner),
            "title" => leaf(&variant.title),
            "sku" => Some(if variant.sku.is_empty() {
                Resolved::Null
            } else {
                Resolved::Leaf(json!(variant.sku))
            }),
            "barcode" => Some(if variant.barcode.is_empty() {
                Resolved::Null
            } else {
                Resolved::Leaf(json!(variant.barcode))
            }),
            "availableForSale" => leaf(variant.available),
            // Sold although none is in stock: the policy continues selling.
            "currentlyNotInStock" => leaf(
                variant.available && variant.inventory_tracked && variant.inventory_quantity <= 0,
            ),
            "requiresShipping" => leaf(variant.requires_shipping),
            "taxable" => leaf(variant.taxable),
            "requiresComponents" => leaf(!variant.components.is_empty()),
            "weight" => leaf(lsf_core::store::weight_in_unit(
                variant.weight,
                &variant.weight_unit,
            )),
            "weightUnit" => leaf(match variant.weight_unit.as_str() {
                "g" => "GRAMS",
                "oz" => "OUNCES",
                "lb" => "POUNDS",
                _ => "KILOGRAMS",
            }),
            "price" => object(Node::Money(variant.price)),
            "compareAtPrice" => Some(
                variant
                    .compare_at_price
                    .filter(|price| *price > variant.price)
                    .map_or(Resolved::Null, |price| Resolved::Object(Node::Money(price))),
            ),
            "selectedOptions" => Some(Resolved::List(
                product
                    .options
                    .iter()
                    .zip(&variant.options)
                    .map(|(option, value)| Node::SelectedOption(&option.name, value))
                    .collect(),
            )),
            "product" => object(Node::Product(product)),
            "quantityAvailable" => return Err(inventory_denied(name)),
            "metafield" | "metafields" => {
                metafields(self.store, &variant.metafields, &owner, name, args)?
            }
            _ => None,
        })
    }
}

/// `metafield(namespace:, key:)` and `metafields(identifiers: [{namespace, key}])`. A metafield
/// the owner does not have is `null`, in the list too.
fn metafields<'s>(
    store: &lsf_core::Store,
    metafields: &Metafields,
    owner: &str,
    name: &str,
    args: &mut Args,
) -> Result<Option<Resolved<Node<'s>>>, String> {
    let find = |namespace: String, key: String| {
        store_metafield(store, metafields, &namespace, &key).map(|(kind, value)| {
            Node::Metafield(Box::new(FoundMetafield {
                owner: owner.to_string(),
                namespace,
                key,
                kind,
                value,
            }))
        })
    };
    if name == "metafield" {
        // Without a namespace, the app's reserved one, as on Shopify.
        let namespace = args
            .string("namespace")?
            .unwrap_or_else(|| "$app".to_string());
        let key = args.required_string("key")?;
        return Ok(Some(
            find(namespace, key).map_or(Resolved::Null, Resolved::Object),
        ));
    }
    let Some(identifiers) = args.take("identifiers") else {
        return Err("missing required argument `identifiers`".to_string());
    };
    let identifiers = match identifiers {
        Json::Array(items) => items,
        single => vec![single],
    };
    let mut found = Vec::new();
    for identifier in identifiers {
        let text = |field: &str| {
            identifier
                .get(field)
                .and_then(Json::as_str)
                .map(str::to_string)
        };
        let Some(key) = text("key") else {
            return Err(format!(
                "each of `identifiers` needs a `key`, got {identifier}"
            ));
        };
        let namespace = text("namespace").unwrap_or_else(|| "$app".to_string());
        found.push(find(namespace, key));
    }
    Ok(Some(Resolved::NullableList(found)))
}

/// What Shopify answers for an inventory field without `unauthenticated_read_product_inventory`,
/// which `api_access` does not give.
fn inventory_denied(field: &str) -> String {
    format!(
        "Access denied for {field} field. Required access: `unauthenticated_read_product_inventory` access scope (the `api_access` capability does not give it)"
    )
}

fn leaf<'s>(value: impl serde::Serialize) -> Option<Resolved<Node<'s>>> {
    Some(Resolved::Leaf(json!(value)))
}

fn object(node: Node<'_>) -> Option<Resolved<Node<'_>>> {
    Some(Resolved::Object(node))
}

/// `("ProductVariant", 42)` of `gid://shopify/ProductVariant/42`.
fn parse_gid(id: &str) -> Option<(&str, u64)> {
    let rest = id.strip_prefix("gid://shopify/")?;
    let (kind, number) = rest.split_once('/')?;
    let number = number.split('?').next()?.parse().ok()?;
    Some((kind, number))
}

/// A date as GraphQL `DateTime`: `2024-01-01T12:00:00Z`.
fn date_time(date: chrono::DateTime<chrono::Utc>) -> String {
    date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests;
