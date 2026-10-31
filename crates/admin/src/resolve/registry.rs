//! The registry where the domains plug in what the generic rules do not answer: custom field
//! resolvers by `(type, field)` (root query fields being those of `QueryRoot`), mutations, sort
//! keys and search filters by node type.
//!
//! Each domain module has a `register(&mut Registry)` that [`Registry::standard`] calls:
//!
//! ```
//! use lsf_admin::graphql::Output;
//! use lsf_admin::resolve::Registry;
//! use lsf_admin::resolve::sort::SortValue;
//! use serde_json::Value;
//!
//! let mut registry = Registry::new();
//! registry
//!     // `Product.totalVariants`, deprecated: computed from the variants.
//!     .field("Product", "totalVariants", |r, _ctx, object| {
//!         Ok(Output::value(r.items_of(object, "variants").len()))
//!     })
//!     // `products(sortKey: INVENTORY_TOTAL)` reads `totalInventory`.
//!     .sort_key("Product", "INVENTORY_TOTAL", |r, item| {
//!         SortValue::of(r.item_map(item).and_then(|product| product.get("totalInventory")))
//!     });
//! assert!(registry.check(lsf_admin::schema::admin()).is_empty());
//! ```
//!
//! Functions are plain `fn` pointers (closures that capture nothing coerce to them): a resolver
//! keeps no state of its own; what it needs is in the [`ShopResolver`] it is given.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use super::sort::SortValue;
use super::{Mutation, Obj, ShopResolver};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::schema::Schema;
use crate::search::SearchValue;
use crate::state::Record;

/// A custom resolver of a field: the resolver (the shop, read-only, and the generic rules), the
/// field (its arguments, its type), the object it is asked of.
pub type FieldFn =
    fn(&ShopResolver<'_>, &FieldContext<'_>, &Obj) -> Result<Output<Obj>, FieldError>;

/// A mutation: changes the shop through [`Mutation`] and answers its payload as JSON, record
/// references as GIDs (`{"product": "gid://shopify/Product/1", "userErrors": []}`). A payload
/// with user errors, or an error, leaves the shop as it was.
pub type MutationFn = fn(&mut Mutation<'_>, &FieldContext<'_>) -> Result<Value, FieldError>;

/// What an item of a connection sorts by, for one sort key: the item is a GID or an inline
/// object ([`ShopResolver::item_map`] gives its record).
pub type SortFn = fn(&ShopResolver<'_>, &Value) -> SortValue;

/// The values of a search filter for a record (`None`: the filter does not apply, and the term
/// is ignored). The item, then its record.
pub type SearchFn = fn(&ShopResolver<'_>, &Value, &Record) -> Option<Vec<SearchValue>>;

/// The texts a term without a field is searched in, for a record.
pub type TextFn = fn(&ShopResolver<'_>, &Record) -> Vec<String>;

/// The custom resolvers, mutations, sort keys and search filters of the domains.
#[derive(Default)]
pub struct Registry {
    fields: HashMap<(String, String), FieldFn>,
    mutations: HashMap<String, MutationFn>,
    sort_keys: HashMap<(String, String), SortFn>,
    search_fields: HashMap<(String, String), SearchFn>,
    default_texts: HashMap<String, TextFn>,
}

impl Registry {
    /// An empty registry: the generic rules alone.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// The registry of every domain (`crate::domains::register`), built once.
    pub fn standard() -> &'static Registry {
        static STANDARD: OnceLock<Registry> = OnceLock::new();
        STANDARD.get_or_init(|| {
            let mut registry = Registry::new();
            crate::domains::register(&mut registry);
            registry
        })
    }

    /// Resolves `field` of the object type `type_name` (`"Product"`, `"totalVariants"`) with
    /// `resolve`. For a root query field, `type_name` is `QueryRoot` ([`Registry::query`]).
    pub fn field(&mut self, type_name: &str, field: &str, resolve: FieldFn) -> &mut Registry {
        self.fields
            .insert((type_name.to_string(), field.to_string()), resolve);
        self
    }

    /// Resolves the root query field `field` with `resolve`.
    pub fn query(&mut self, field: &str, resolve: FieldFn) -> &mut Registry {
        self.field("QueryRoot", field, resolve)
    }

    /// Runs the mutation `name` (`"productCreate"`) with `run`.
    pub fn mutation(&mut self, name: &str, run: MutationFn) -> &mut Registry {
        self.mutations.insert(name.to_string(), run);
        self
    }

    /// Sorts the connections of `node_type` by `key` (`"Product"`, `"INVENTORY_TOTAL"`) with
    /// `read`. A key that is a camelCase field of the record needs nothing.
    pub fn sort_key(&mut self, node_type: &str, key: &str, read: SortFn) -> &mut Registry {
        self.sort_keys
            .insert((node_type.to_string(), key.to_ascii_uppercase()), read);
        self
    }

    /// Reads the search filter `filter` (`"sku"`) of the records of `node_type` with `read`, in
    /// every connection of them that documents it.
    pub fn search_field(&mut self, node_type: &str, filter: &str, read: SearchFn) -> &mut Registry {
        self.search_fields
            .insert((node_type.to_string(), filter.to_ascii_lowercase()), read);
        self
    }

    /// The texts a term without a field searches in the records of `node_type`.
    pub fn default_text(&mut self, node_type: &str, read: TextFn) -> &mut Registry {
        self.default_texts.insert(node_type.to_string(), read);
        self
    }

    pub fn field_fn(&self, type_name: &str, field: &str) -> Option<FieldFn> {
        self.fields
            .get(&(type_name.to_string(), field.to_string()))
            .copied()
    }

    pub fn mutation_fn(&self, name: &str) -> Option<MutationFn> {
        self.mutations.get(name).copied()
    }

    pub fn sort_fn(&self, node_type: &str, key: &str) -> Option<SortFn> {
        self.sort_keys
            .get(&(node_type.to_string(), key.to_ascii_uppercase()))
            .copied()
    }

    pub fn search_fn(&self, node_type: &str, filter: &str) -> Option<SearchFn> {
        self.search_fields
            .get(&(node_type.to_string(), filter.to_ascii_lowercase()))
            .copied()
    }

    pub fn text_fn(&self, node_type: &str) -> Option<TextFn> {
        self.default_texts.get(node_type).copied()
    }

    /// The mutations lsf simulates, by name, sorted.
    pub fn mutations(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.mutations.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// What is registered for something the schema does not have: a field of no type, a
    /// mutation that does not exist, a sort key or a search filter of a type that is not one.
    /// A test of the crate runs it on [`Registry::standard`].
    pub fn check(&self, schema: &Schema) -> Vec<String> {
        let mut problems = Vec::new();
        for (type_name, field) in self.fields.keys() {
            if schema.field(type_name, field).is_none() {
                problems.push(format!("{type_name}.{field} is not a field of the schema"));
            }
        }
        let mutation = schema.mutation_type().unwrap_or("Mutation");
        for name in self.mutations.keys() {
            if schema.field(mutation, name).is_none() {
                problems.push(format!("{name} is not a mutation of the schema"));
            }
        }
        let types = self
            .sort_keys
            .keys()
            .chain(self.search_fields.keys())
            .map(|(type_name, _)| type_name)
            .chain(self.default_texts.keys());
        for type_name in types {
            if schema.get(type_name).is_none() {
                problems.push(format!("{type_name} is not a type of the schema"));
            }
        }
        problems.sort();
        problems.dedup();
        problems
    }

    /// What is registered for a field, a mutation or a type that no version lsf knows has: a
    /// resolver of something only some versions have is fine (the schema of a request of
    /// another version refuses it before it is reached).
    pub fn check_versions(&self) -> Vec<String> {
        let mut problems: Option<Vec<String>> = None;
        for version in crate::version::ApiVersion::all() {
            let found = self.check(version.schema());
            problems = Some(match problems {
                None => found,
                Some(before) => before
                    .into_iter()
                    .filter(|problem| found.contains(problem))
                    .collect(),
            });
        }
        problems.unwrap_or_default()
    }
}
