//! The resolver of a shop's records: [`ShopResolver`] answers every field of the Admin API from
//! the records of a [`ShopState`], by generic rules, and lets the domains plug in what needs
//! code through a [`Registry`]: custom field resolvers, root fields, mutations, sort keys and
//! search fields.
//!
//! How a field is resolved, in order:
//!
//! 1. Access: a field the token's scopes do not cover answers `ACCESS_DENIED`, as Shopify words
//!    it ([`access`]).
//! 2. A mutation runs its [`MutationFn`] on the shop, which is rolled back when it fails
//!    ([`mutation`]); a mutation lsf does not simulate answers `LSF_NOT_SIMULATED`.
//! 3. A custom resolver registered for `(type, field)` answers it.
//! 4. A root field follows the conventions of the Admin API: `products` lists the records of a
//!    type, `product(id:)` reads one, `node`/`nodes` read any, `productsCount` counts,
//!    `…ByHandle` and `…ByIdentifier` find one, `shop` is the only record of its type.
//! 5. Otherwise the object's own value, converted by the schema type of the field: a GID is the
//!    record it names, an object is an inline object, an array of a connection is paginated
//!    ([`connection`]: `first`, `last`, `after`, `before`, `reverse`, `sortKey`, `query`), an
//!    array of a list is mapped. `metafield`/`metafields` read the owner's metafields;
//!    `legacyResourceId`, `defaultCursor` and the `Count` of an array (`variantsCount`) are
//!    computed.
//! 6. Otherwise a default ([`defaults`]): `null`, or for a non-null field an empty list, `""`,
//!    `0`, `false`, the first enum value, an empty object. Its path goes into
//!    `extensions.lsf.defaulted`, so that nobody mistakes it for data (lists and connections,
//!    empty by nature, are not reported).
//!
//! See "How a field is resolved" and "Writing a resolver or a mutation" in
//! `crates/admin/docs/design.md`.

pub mod access;
pub mod connection;
mod defaults;
pub mod money;
pub mod mutation;
pub mod registry;
pub mod search;
pub mod selection;
pub mod sort;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexSet;
use serde_json::{Map, Value, json};

use crate::auth::Grant;
use crate::graphql::{self, FieldContext, FieldError, Output};
use crate::schema::{Kind, Schema, TypeRef};
use crate::state::{Record, ShopState, gid_type, parse_gid};
use crate::version::{ApiVersion, adapt};
use crate::webhooks::Webhooks;

pub use mutation::{
    Mutation, UserError, coded, done_job, input_path, not_simulated, payload, payload_at,
};
pub use registry::{FieldFn, MutationFn, Registry, SearchFn, SortFn, TextFn};

/// An object of a response: what the executor hands back to the resolver for the fields below
/// it. Cheap to clone.
#[derive(Clone, Debug)]
pub enum Obj {
    /// `QueryRoot`.
    Query,
    /// `Mutation`.
    Mutation,
    /// A record of the shop, by its key (its GID, or the key of a record without one).
    Record(Rc<str>),
    /// An object that is not a record: a value nested in a record (`MoneyV2`, `SEO`), an item of
    /// an array of inline objects (`MailingAddress`), a connection and its edges, a `Count`, a
    /// mutation payload.
    Inline(Rc<Inline>),
}

/// An object that is not a record, with its type.
#[derive(Debug)]
pub struct Inline {
    /// The object type: `MoneyV2`, `ProductConnection`, `ProductCreatePayload`.
    pub type_name: String,
    pub value: Record,
    /// Whether a field missing here is reported in `extensions.lsf.defaulted`: the values of a
    /// record are (what the seed did not set, lsf does not simulate), what lsf builds whole is
    /// not (a payload, an object made of defaults).
    pub reported: bool,
}

impl Obj {
    /// A record, by its key.
    pub fn record(key: &str) -> Obj {
        Obj::Record(Rc::from(key))
    }

    /// An inline object of a type, whose missing fields are reported as defaulted.
    pub fn inline(type_name: &str, value: Record) -> Obj {
        Obj::Inline(Rc::new(Inline {
            type_name: type_name.to_string(),
            value,
            reported: true,
        }))
    }

    /// An inline object lsf builds whole (a payload): its missing fields are `null` by design,
    /// not reported.
    pub fn built(type_name: &str, value: Record) -> Obj {
        Obj::Inline(Rc::new(Inline {
            type_name: type_name.to_string(),
            value,
            reported: false,
        }))
    }

    /// The key of a record.
    pub fn key(&self) -> Option<&str> {
        match self {
            Obj::Record(key) => Some(key),
            _ => None,
        }
    }
}

/// What a request tells the client besides its data: Shopify's `extensions.search`, and lsf's
/// own `extensions.lsf`.
#[derive(Debug, Default)]
struct Report {
    /// The paths of the fields answered with a default.
    defaulted: IndexSet<String>,
    /// The same, as `Type.field`.
    defaulted_fields: IndexSet<String>,
    /// `extensions.search`: the warnings of the `query` arguments.
    search: Vec<Value>,
    /// What lsf did not simulate in this request, in plain words.
    notes: IndexSet<String>,
}

/// How many defaulted paths `extensions.lsf.defaulted` lists; `defaultedFields` names them all.
const DEFAULTED_PATHS: usize = 50;

/// The resolver of the records of one shop, for one request. Build it, run the request with
/// [`graphql::run`], then add [`ShopResolver::extensions`] to the answer:
///
/// ```
/// use lsf_admin::graphql::{self, Request};
/// use lsf_admin::resolve::{Registry, ShopResolver};
/// use lsf_admin::state::ShopState;
///
/// # let store = lsf_core::store::load::load(&lsf_core::store::load::DataSource::Demo, &Default::default()).0;
/// let mut shop = ShopState::seeded(&store, None);
/// let mut resolver = ShopResolver::new(&mut shop, Registry::standard());
/// let request = Request::new("{ products(first: 2) { nodes { title } } }");
/// let mut answer = graphql::run(lsf_admin::schema::admin(), &mut resolver, &request, None);
/// answer.extensions.extend(resolver.extensions());
/// assert_eq!(answer.to_json()["data"]["products"]["nodes"].as_array().unwrap().len(), 2);
/// ```
pub struct ShopResolver<'a> {
    state: &'a mut ShopState,
    registry: &'a Registry,
    /// What the token grants; `None` grants every scope.
    grant: Option<&'a Grant>,
    /// The webhooks of the app, for the subscriptions the Admin API reads and writes.
    webhooks: Option<&'a Webhooks>,
    /// The version of the request: its schema, its access table, its search filters, its
    /// adapters.
    version: ApiVersion,
    /// Adapters a test gives in place of the version's.
    #[cfg(test)]
    adapters: Option<&'static [crate::version::Adapter]>,
    /// Where the request reached lsf (`http://127.0.0.1:9292`): the origin of the URLs lsf
    /// answers that point back at itself (a charge's confirmation page, an upload target).
    origin: &'a str,
    /// What the request's sources gave, fetched before the shop was locked
    /// ([`crate::sources`]).
    fetched: Option<&'a crate::sources::Fetched>,
    /// Whether the request is the query of a bulk operation: its connections take every item
    /// when they are given neither `first` nor `last`, and no limit of 250.
    bulk: bool,
    report: RefCell<Report>,
}

/// The origin lsf's URLs take when the request does not say where it reached lsf (a resolver
/// run alone): the address `lsf` serves on by default.
pub const DEFAULT_ORIGIN: &str = "http://127.0.0.1:9292";

impl<'a> ShopResolver<'a> {
    /// A resolver over a shop, with every scope, without webhooks.
    pub fn new(state: &'a mut ShopState, registry: &'a Registry) -> ShopResolver<'a> {
        ShopResolver {
            state,
            registry,
            grant: None,
            webhooks: None,
            version: ApiVersion::default_version(),
            #[cfg(test)]
            adapters: None,
            origin: DEFAULT_ORIGIN,
            fetched: None,
            bulk: false,
            report: RefCell::new(Report::default()),
        }
    }

    /// Runs the request in a version of the Admin API: the default one otherwise. Run it with
    /// the version's schema too (`graphql::run(version.schema(), …)`).
    pub fn with_version(mut self, version: ApiVersion) -> ShopResolver<'a> {
        self.version = version;
        self
    }

    /// Says where the request reached lsf: `http://127.0.0.1:9292`.
    pub fn with_origin(mut self, origin: &'a str) -> ShopResolver<'a> {
        self.origin = origin;
        self
    }

    /// Gives the mutations what the request's sources gave, fetched before the shop was locked.
    pub fn with_fetched(mut self, fetched: &'a crate::sources::Fetched) -> ShopResolver<'a> {
        self.fetched = Some(fetched);
        self
    }

    /// Runs the query of a bulk operation: connections without `first` or `last` take every
    /// item, and neither is limited to 250 (shopify.dev, "Perform bulk operations").
    pub fn with_bulk(mut self) -> ShopResolver<'a> {
        self.bulk = true;
        self
    }

    /// Where the request reached lsf: the origin of the URLs that point back at it.
    pub fn origin(&self) -> &str {
        self.origin
    }

    /// Whether this is the query of a bulk operation.
    pub fn is_bulk(&self) -> bool {
        self.bulk
    }

    /// Checks the fields against what a token grants.
    pub fn with_grant(mut self, grant: &'a Grant) -> ShopResolver<'a> {
        self.grant = Some(grant);
        self
    }

    /// Gives the webhooks of the app to the subscription queries and mutations.
    pub fn with_webhooks(mut self, webhooks: &'a Webhooks) -> ShopResolver<'a> {
        self.webhooks = Some(webhooks);
        self
    }

    /// The shop.
    pub fn state(&self) -> &ShopState {
        self.state
    }

    /// The registry of custom resolvers.
    pub fn registry(&self) -> &Registry {
        self.registry
    }

    /// What the token grants; `None` when every scope is granted.
    pub fn grant(&self) -> Option<&Grant> {
        self.grant
    }

    /// The webhooks of the app, when the server has them.
    pub fn webhooks(&self) -> Option<&Webhooks> {
        self.webhooks
    }

    /// The API version of the request: `2026-10`.
    pub fn api_version(&self) -> &str {
        self.version.name()
    }

    /// The version of the request. A field or a mutation only some versions have needs no
    /// check (the schema of the request refuses it elsewhere); what reads differently is an
    /// [`crate::version::Adapter`] of the version; what behaves differently asks
    /// `r.version().since("2027-01")`.
    pub fn version(&self) -> ApiVersion {
        self.version
    }

    /// The schema of the request's version.
    pub fn schema(&self) -> &'static Schema {
        crate::schema::for_version(self.version)
    }

    /// How the request's version reads the records differently ([`crate::version::adapt`]).
    fn adapters(&self) -> &'static [crate::version::Adapter] {
        #[cfg(test)]
        if let Some(adapters) = self.adapters {
            return adapters;
        }
        self.version.adapters()
    }

    /// Reads through `adapters` rather than the version's: what a test of the adapters runs.
    #[cfg(test)]
    pub(crate) fn with_adapters(
        mut self,
        adapters: &'static [crate::version::Adapter],
    ) -> ShopResolver<'a> {
        self.adapters = Some(adapters);
        self
    }

    /// What the answer's `extensions` get from this request: `search` (Shopify's warnings of
    /// the `query` arguments) and `lsf` (`defaulted`, `defaultedFields`, `notes`), each only
    /// when there is something to say.
    pub fn extensions(&self) -> Map<String, Value> {
        let report = self.report.borrow();
        let mut extensions = Map::new();
        if !report.search.is_empty() {
            extensions.insert("search".into(), Value::Array(report.search.clone()));
        }
        let mut lsf = Map::new();
        if !report.defaulted.is_empty() {
            let paths: Vec<&String> = report.defaulted.iter().take(DEFAULTED_PATHS).collect();
            lsf.insert("defaulted".into(), json!(paths));
            if report.defaulted.len() > DEFAULTED_PATHS {
                lsf.insert("defaultedCount".into(), json!(report.defaulted.len()));
            }
            lsf.insert("defaultedFields".into(), json!(report.defaulted_fields));
        }
        if !report.notes.is_empty() {
            lsf.insert("notes".into(), json!(report.notes));
        }
        if !lsf.is_empty() {
            extensions.insert("lsf".into(), Value::Object(lsf));
        }
        extensions
    }

    /// Says something in `extensions.lsf.notes`: what lsf did not simulate (an input it ignored,
    /// a search field it does not read), in plain words.
    pub fn note(&self, text: impl Into<String>) {
        self.report.borrow_mut().notes.insert(text.into());
    }

    /// The record or the inline value of an object, `None` for the roots and for a record that
    /// is gone.
    pub fn map_of<'s>(&'s self, object: &'s Obj) -> Option<&'s Record> {
        match object {
            Obj::Record(key) => self.state.get(key),
            Obj::Inline(inline) => Some(&inline.value),
            Obj::Query | Obj::Mutation => None,
        }
    }

    /// The object type of an object.
    pub fn type_of<'s>(&'s self, object: &'s Obj) -> &'s str {
        match object {
            Obj::Query => "QueryRoot",
            Obj::Mutation => "Mutation",
            Obj::Record(key) => {
                adapt::typename(self.adapters(), self.state.type_of(key).unwrap_or_default())
            }
            Obj::Inline(inline) => &inline.type_name,
        }
    }

    /// The key of the record a GID names, following Shopify's aliases: `DiscountNode/<n>` is
    /// the `DiscountAutomaticNode/<n>` or `DiscountCodeNode/<n>` of the same discount,
    /// `Catalog/<n>` the catalog of that number, whatever its type.
    pub fn lookup(&self, gid: &str) -> Option<String> {
        if self.state.contains(gid) {
            return Some(gid.to_string());
        }
        let (type_name, id) = parse_gid(gid)?;
        if type_name == "DiscountNode" {
            return ["DiscountAutomaticNode", "DiscountCodeNode"]
                .into_iter()
                .map(|alias| crate::state::gid(alias, id))
                .find(|alias| self.state.contains(alias));
        }
        if type_name == "Catalog" {
            return crate::domains::markets::catalog_key(self.state, gid);
        }
        None
    }

    /// The record a GID names as an object of type `expected` (or of a type that is one of
    /// `expected`'s), `None` when there is none.
    pub fn reference(&self, gid: &str, expected: &str) -> Option<Obj> {
        let key = self.lookup(gid)?;
        let type_name = adapt::typename(self.adapters(), self.state.type_of(&key)?);
        self.schema()
            .is_possible(expected, type_name)
            .then(|| Obj::record(&key))
    }

    /// The generic answer for a field: the root conventions for a root field, the object's own
    /// value otherwise, else a default. What a custom resolver falls back to.
    pub fn generic(&self, ctx: &FieldContext<'_>, object: &Obj) -> Result<Output<Obj>, FieldError> {
        match object {
            Obj::Query => self.root(ctx),
            Obj::Mutation => Err(not_simulated(ctx.field_name())),
            _ => self.value(ctx, object),
        }
    }

    /// The value of a field of a record or an inline object (rule 5, then 6).
    fn value(&self, ctx: &FieldContext<'_>, object: &Obj) -> Result<Output<Obj>, FieldError> {
        let Some(map) = self.map_of(object) else {
            return Ok(Output::Null);
        };
        let name = ctx.field_name();
        let reported = self.reported(object);
        if let Some(output) = self.metafields(ctx, map)? {
            return Ok(output);
        }
        if let Some(output) = self.live_count(ctx, map) {
            return Ok(output);
        }
        // A field this version shows differently from the records reads through its adapter.
        let adapted = adapt::field(self.adapters(), ctx.parent_type(), name).map(|read| read(map));
        let stored = match &adapted {
            Some(value) => value.as_ref(),
            None => map.get(name),
        };
        match stored {
            Some(Value::Null) if !ctx.ty().is_non_null() => return Ok(Output::Null),
            Some(value) if !value.is_null() => return self.convert(ctx, value, reported),
            _ => {}
        }
        if let Some(output) = self.computed(ctx, object, map)? {
            return Ok(output);
        }
        self.default(ctx, reported)
    }

    /// An `Int` count of the records of an array of the object (`MetafieldDefinition
    /// .metafieldsCount` of `metafields`): the records that are still there, as the list shows
    /// them, rather than a count stored when the array was written.
    fn live_count(&self, ctx: &FieldContext<'_>, map: &Record) -> Option<Output<Obj>> {
        let base = ctx.field_name().strip_suffix("Count")?;
        if ctx.ty().named() != "Int" {
            return None;
        }
        let Some(Value::Array(items)) = map.get(base) else {
            return None;
        };
        let schema = ctx.schema();
        let listed = schema.field(ctx.parent_type(), base)?.ty.named();
        let node_type = schema.connection_node(listed).unwrap_or(listed);
        let count = items
            .iter()
            .filter(|item| !self.gone(schema, node_type, item))
            .count();
        Some(Output::value(count))
    }

    /// Whether an item of a list or a connection of records of type `named` names a record
    /// that is no more (or is not of the type): Shopify never lists a deleted resource, and a
    /// `null` in a list of non-null records would null the whole answer.
    fn gone(&self, schema: &Schema, named: &str, item: &Value) -> bool {
        let Value::String(key) = item else {
            return false;
        };
        schema.get(named).is_some_and(|def| def.is_composite())
            && self.reference(key, named).is_none()
    }

    /// Whether the missing fields of an object are reported.
    fn reported(&self, object: &Obj) -> bool {
        match object {
            Obj::Inline(inline) => inline.reported,
            _ => true,
        }
    }

    /// Converts a JSON value to the answer of the field, by the field's type: what the generic
    /// rule does with a record's value, for a custom resolver that computed one.
    pub fn convert(
        &self,
        ctx: &FieldContext<'_>,
        value: &Value,
        reported: bool,
    ) -> Result<Output<Obj>, FieldError> {
        let ty = ctx.ty();
        // `Product.options(first:)`: a list that is not a connection, cut to `first`.
        if ty.is_list()
            && let (Value::Array(items), Some(first)) = (value, ctx.arg_i64("first"))
        {
            let first = usize::try_from(first.max(0)).unwrap_or(0);
            let items: Vec<Value> = items.iter().take(first).cloned().collect();
            return self.convert_type(ctx, ty, &Value::Array(items), reported);
        }
        self.convert_type(ctx, ty, value, reported)
    }

    fn convert_type(
        &self,
        ctx: &FieldContext<'_>,
        ty: &TypeRef,
        value: &Value,
        reported: bool,
    ) -> Result<Output<Obj>, FieldError> {
        if value.is_null() {
            return Ok(Output::Null);
        }
        let schema = ctx.schema();
        if let Some(item) = ty.item() {
            let items: Vec<&Value> = match value {
                Value::Array(items) => items.iter().collect(),
                other => vec![other],
            };
            let mut outputs = Vec::with_capacity(items.len());
            for value in items {
                if self.gone(schema, item.named(), value) {
                    continue;
                }
                outputs.push(self.convert_type(ctx, item, value, reported)?);
            }
            return Ok(Output::List(outputs));
        }
        let named = ty.named();
        let Some(def) = schema.get(named) else {
            return Ok(Output::Null);
        };
        if def.is_leaf() {
            return Ok(Output::Value(value.clone()));
        }
        if schema.is_connection(named) {
            let items = match value {
                Value::Array(items) => items.clone(),
                Value::Object(map) => map
                    .get("nodes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            return self.connection_of(ctx, items, reported);
        }
        Ok(match value {
            Value::String(key) => Output::object_or_null(self.reference(key, named)),
            Value::Object(map) => Output::Object(self.inline_of(schema, named, map, reported)),
            _ => Output::Null,
        })
    }

    /// An inline object of a value of type `named`: an object type is the type; an interface or
    /// a union takes the `__typename` of the value, else its first possible type.
    fn inline_of(&self, schema: &Schema, named: &str, map: &Record, reported: bool) -> Obj {
        let type_name = match schema.get(named) {
            Some(def) if def.kind == Kind::Object => named.to_string(),
            _ => map
                .get("__typename")
                .and_then(Value::as_str)
                .map(|name| adapt::typename(self.adapters(), name))
                .filter(|name| schema.is_possible(named, name))
                .map(str::to_string)
                .or_else(|| schema.possible_types(named).first().cloned())
                .unwrap_or_else(|| named.to_string()),
        };
        Obj::Inline(Rc::new(Inline {
            type_name,
            value: map.clone(),
            reported,
        }))
    }

    /// A page of `items` (GIDs of records, or inline objects), as the connection the field
    /// answers: the field's `query`, `sortKey`, `reverse`, `first`, `last`, `after` and
    /// `before` applied. Root connections sort by their `sortKey` (its default included);
    /// a record's array keeps its order unless the query asks for another `sortKey`.
    pub fn connection(
        &self,
        ctx: &FieldContext<'_>,
        items: Vec<Value>,
    ) -> Result<Output<Obj>, FieldError> {
        self.connection_of(ctx, items, true)
    }

    fn connection_of(
        &self,
        ctx: &FieldContext<'_>,
        items: Vec<Value>,
        reported: bool,
    ) -> Result<Output<Obj>, FieldError> {
        let schema = ctx.schema();
        let items = match schema.connection_node(ctx.ty().named()) {
            Some(node_type) => items
                .into_iter()
                .filter(|item| !self.gone(schema, node_type, item))
                .collect(),
            None => items,
        };
        connection::page(self, ctx, items, reported)
    }

    /// The items of a list, the `query` argument of the field applied: what a `…Count` field
    /// counts, and a connection pages over.
    pub fn search(&self, ctx: &FieldContext<'_>, node_type: &str, items: Vec<Value>) -> Vec<Value> {
        search::filter(self, ctx, node_type, items)
    }

    /// The items a search query matches, by the filters the connection `field` of `parent`
    /// documents (`("QueryRoot", "draftOrders")`): what a bulk mutation's `search` argument
    /// selects.
    pub fn search_query(
        &self,
        node_type: &str,
        connection: (&str, &str),
        text: &str,
        items: Vec<Value>,
    ) -> Vec<Value> {
        search::filter_text(self, node_type, connection, text, items)
    }

    /// A `Count`: `{count, precision}`, `AT_LEAST` past the `limit` argument of the field.
    pub fn count(&self, ctx: &FieldContext<'_>, count: usize) -> Output<Obj> {
        let limit = ctx
            .arg_i64("limit")
            .map(|limit| usize::try_from(limit.max(0)).unwrap_or(0));
        let (count, precision) = match limit {
            Some(limit) if count > limit => (limit, "AT_LEAST"),
            _ => (count, "EXACT"),
        };
        let mut value = Record::new();
        value.insert("count".into(), json!(count));
        value.insert("precision".into(), json!(precision));
        Output::Object(Obj::built("Count", value))
    }

    /// The items of an object's array field (`variants` of a product), as they are stored:
    /// GIDs or inline objects.
    pub fn items_of(&self, object: &Obj, field: &str) -> Vec<Value> {
        self.map_of(object)
            .and_then(|map| map.get(field))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The keys of the records of a type, or of every type of an interface or a union, in
    /// creation order: what a root connection lists.
    pub fn records_of(&self, type_name: &str) -> Vec<Value> {
        let schema = self.schema();
        let possible = schema.possible_types(type_name);
        if possible.len() <= 1 {
            let only = possible.first().map_or(type_name, String::as_str);
            return self
                .state
                .keys(only)
                .map(|key| Value::String(key.to_string()))
                .collect();
        }
        let types: Vec<&str> = possible.iter().map(String::as_str).collect();
        self.state
            .iter()
            .filter(|(key, _)| {
                self.state
                    .type_of(key)
                    .is_some_and(|type_name| types.contains(&type_name))
            })
            .map(|(key, _)| Value::String(key.to_string()))
            .collect()
    }

    /// The record of an item of a list: the record of a GID, or the inline object itself.
    pub fn item_map<'s>(&'s self, item: &'s Value) -> Option<&'s Record> {
        match item {
            Value::String(key) => self.lookup(key).and_then(|key| self.state.get(&key)),
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    /// The default of a field the object has no value for (rule 6), reported unless it is
    /// empty by nature or the object is not reported.
    pub fn default(
        &self,
        ctx: &FieldContext<'_>,
        reported: bool,
    ) -> Result<Output<Obj>, FieldError> {
        let schema = ctx.schema();
        let named = ctx.ty().named();
        if schema.is_connection(named) {
            return self.connection_of(ctx, Vec::new(), reported);
        }
        if reported && !ctx.ty().is_list() {
            let mut report = self.report.borrow_mut();
            report.defaulted.insert(ctx.path_string());
            report
                .defaulted_fields
                .insert(format!("{}.{}", ctx.parent_type(), ctx.field_name()));
        }
        Ok(defaults::output(schema, ctx.ty()))
    }

    /// The fields lsf computes rather than stores: `legacyResourceId`, `defaultCursor`, and the
    /// `Count` of an array (`variantsCount` of `variants`).
    fn computed(
        &self,
        ctx: &FieldContext<'_>,
        object: &Obj,
        map: &Record,
    ) -> Result<Option<Output<Obj>>, FieldError> {
        let name = ctx.field_name();
        let key = object
            .key()
            .or_else(|| map.get("id").and_then(Value::as_str));
        match name {
            "legacyResourceId" => {
                return Ok(key
                    .and_then(parse_gid)
                    .map(|(_, id)| Output::value(id.to_string())));
            }
            "defaultCursor" => {
                return Ok(key.map(|key| {
                    let position = gid_type(key)
                        .map(|type_name| {
                            let mut ids: Vec<u64> = self
                                .state
                                .keys(type_name)
                                .filter_map(|key| parse_gid(key).map(|(_, id)| id))
                                .collect();
                            ids.sort_unstable();
                            let id = parse_gid(key).map_or(0, |(_, id)| id);
                            ids.partition_point(|known| *known < id)
                        })
                        .unwrap_or(0);
                    Output::value(connection::id_cursor(position, key))
                }));
            }
            _ => {}
        }
        if ctx.ty().named() == "Count"
            && let Some(base) = name.strip_suffix("Count")
            && let Some(Value::Array(items)) = map.get(base)
        {
            let node_type = ctx
                .schema()
                .field(ctx.parent_type(), base)
                .map(|field| {
                    let named = field.ty.named();
                    ctx.schema()
                        .connection_node(named)
                        .unwrap_or(named)
                        .to_string()
                })
                .unwrap_or_default();
            let items = items
                .iter()
                .filter(|item| !self.gone(ctx.schema(), &node_type, item))
                .cloned()
                .collect();
            let items = self.search(ctx, &node_type, items);
            return Ok(Some(self.count(ctx, items.len())));
        }
        Ok(None)
    }

    /// `metafield(namespace:, key:)` and `metafields(namespace:, keys:)` of an owner, the
    /// namespace `$app` (or `$app:x`) read as the app's reserved one (`app--<id>`,
    /// `app--<id>--x`). A `metafield` without a namespace is in the app's.
    fn metafields(
        &self,
        ctx: &FieldContext<'_>,
        map: &Record,
    ) -> Result<Option<Output<Obj>>, FieldError> {
        let name = ctx.field_name();
        let named = ctx.ty().named();
        let is_owner = map.contains_key("metafields")
            || ctx
                .schema()
                .get(ctx.parent_type())
                .is_some_and(|def| def.interfaces.iter().any(|i| i == "HasMetafields"));
        if !is_owner || !matches!(name, "metafield" | "metafields") {
            return Ok(None);
        }
        let owned: Vec<Value> = map
            .get("metafields")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let field = |metafield: &Record, field: &str| -> String {
            metafield
                .get(field)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        if name == "metafield" && named == "Metafield" {
            let namespace = self.app_namespace(ctx.arg_str("namespace").unwrap_or("$app"));
            let key = ctx.arg_str("key").unwrap_or_default();
            let found = owned.iter().find_map(|item| {
                let metafield = self.item_map(item)?;
                (field(metafield, "namespace") == namespace && field(metafield, "key") == key)
                    .then(|| item.as_str().map(str::to_string))
                    .flatten()
            });
            return Ok(Some(Output::object_or_null(
                found.and_then(|gid| self.reference(&gid, "Metafield")),
            )));
        }
        if name == "metafields" && ctx.schema().connection_node(named) == Some("Metafield") {
            let namespace = ctx.arg_str("namespace").map(|ns| self.app_namespace(ns));
            let keys: Option<Vec<String>> = ctx.arg("keys").and_then(Value::as_array).map(|keys| {
                keys.iter()
                    .filter_map(Value::as_str)
                    .map(|key| match key.rsplit_once('.') {
                        Some((namespace, key)) => {
                            format!("{}.{key}", self.app_namespace(namespace))
                        }
                        None => key.to_string(),
                    })
                    .collect()
            });
            let items = owned
                .into_iter()
                .filter(|item| {
                    let Some(metafield) = self.item_map(item) else {
                        return false;
                    };
                    let (ns, key) = (field(metafield, "namespace"), field(metafield, "key"));
                    namespace.as_ref().is_none_or(|wanted| *wanted == ns)
                        && keys
                            .as_ref()
                            .is_none_or(|keys| keys.contains(&format!("{ns}.{key}")))
                })
                .collect();
            return self.connection_of(ctx, items, true).map(Some);
        }
        Ok(None)
    }

    /// A namespace as the Admin API stores it: `$app` and `$app:x` are the app's reserved
    /// namespaces, `app--<id>` and `app--<id>--x`.
    pub fn app_namespace(&self, namespace: &str) -> String {
        let app_id = self
            .state
            .keys("App")
            .next()
            .and_then(parse_gid)
            .map_or(0, |(_, id)| id);
        crate::domains::content::admin_namespace(app_id, namespace)
    }

    /// The root conventions (rule 4).
    fn root(&self, ctx: &FieldContext<'_>) -> Result<Output<Obj>, FieldError> {
        let schema = ctx.schema();
        let name = ctx.field_name();
        let ty = ctx.ty();
        let named = ty.named();
        match name {
            "node" => {
                let id = ctx.arg_str("id").unwrap_or_default();
                return Ok(match self.reference(id, "Node") {
                    Some(object) => {
                        access::check_record(self, &object, name)?;
                        Output::Object(object)
                    }
                    None => Output::Null,
                });
            }
            "nodes" => {
                let ids = ctx
                    .arg("ids")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut nodes = Vec::with_capacity(ids.len());
                for id in ids {
                    let object = id.as_str().and_then(|id| self.reference(id, "Node"));
                    if let Some(object) = &object {
                        access::check_record(self, object, name)?;
                    }
                    nodes.push(Output::object_or_null(object));
                }
                return Ok(Output::List(nodes));
            }
            _ => {}
        }
        if !ty.is_list()
            && let Some(node_type) = schema.connection_node(named)
        {
            return self.connection(ctx, self.records_of(node_type));
        }
        if named == "Count"
            && let Some(base) = name.strip_suffix("Count")
            && let Some(node_type) = schema
                .field_type("QueryRoot", base)
                .and_then(|ty| schema.connection_node(ty.named()))
        {
            let items = self.search(ctx, node_type, self.records_of(node_type));
            return Ok(self.count(ctx, items.len()));
        }
        if schema.get(named).is_some_and(|def| def.is_composite()) {
            if ty.is_list() && ctx.definition().args.values().all(|arg| !arg.is_required()) {
                let records = self.records_of(named);
                return Ok(Output::List(
                    records
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|key| Output::Object(Obj::record(key)))
                        .collect(),
                ));
            }
            if let Some(id) = ctx.arg_str("id") {
                return Ok(Output::object_or_null(self.reference(id, named)));
            }
            if name.ends_with("ByHandle")
                && let Some(handle) = ctx.arg_str("handle")
            {
                return Ok(Output::object_or_null(self.find(named, |record| {
                    record.get("handle").and_then(Value::as_str) == Some(handle)
                })));
            }
            if let Some(identifier) = ctx.arg_object("identifier") {
                return Ok(Output::object_or_null(
                    self.by_identifier(named, identifier),
                ));
            }
            if ctx.definition().args.values().all(|arg| !arg.is_required())
                && self.state.count(named) == 1
            {
                return Ok(Output::object_or_null(
                    self.state.keys(named).next().map(Obj::record),
                ));
            }
        }
        self.default(ctx, true)
    }

    /// The first record of a type (or of the types of an interface) that `matches`.
    pub fn find(&self, type_name: &str, matches: impl Fn(&Record) -> bool) -> Option<Obj> {
        self.records_of(type_name).iter().find_map(|item| {
            let key = item.as_str()?;
            matches(self.state.get(key)?).then(|| Obj::record(key))
        })
    }

    /// The record an `…Identifier` input names (`productByIdentifier(identifier: {handle})`,
    /// `customerByIdentifier(identifier: {emailAddress})`): `id` is its GID, `customId` a
    /// metafield `{namespace, key, value}` it owns, and any other field equals the record's
    /// (`emailAddress` its `email`, `phoneNumber` its `phone`).
    pub fn by_identifier(&self, type_name: &str, identifier: &Record) -> Option<Obj> {
        if let Some(id) = identifier.get("id").and_then(Value::as_str) {
            return self.reference(id, type_name);
        }
        self.find(type_name, |record| {
            identifier
                .iter()
                .all(|(field, wanted)| match field.as_str() {
                    "customId" => {
                        let wanted_field = |name: &str| wanted.get(name).and_then(Value::as_str);
                        let namespace = wanted_field("namespace").map(|ns| self.app_namespace(ns));
                        record
                            .get("metafields")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|item| self.item_map(item))
                            .any(|metafield| {
                                let get = |name: &str| metafield.get(name).and_then(Value::as_str);
                                get("namespace") == namespace.as_deref()
                                    && get("key") == wanted_field("key")
                                    && get("value") == wanted_field("value")
                            })
                    }
                    other => {
                        let stored = match other {
                            "emailAddress" => "email",
                            "phoneNumber" => "phone",
                            other => other,
                        };
                        match (record.get(stored), wanted) {
                            (Some(Value::String(have)), Value::String(want)) => {
                                have.eq_ignore_ascii_case(want)
                            }
                            (Some(have), want) => have == want,
                            (None, _) => false,
                        }
                    }
                })
        })
    }

    /// The options of a search at the shop's clock, in the shop's time zone.
    pub fn search_options(&self) -> crate::search::Options {
        crate::search::Options::of(self.state)
    }

    fn report_search(&self, entry: Value) {
        self.report.borrow_mut().search.push(entry);
    }
}

impl graphql::Resolver for ShopResolver<'_> {
    type Object = Obj;

    fn query_root(&mut self) -> Obj {
        Obj::Query
    }

    fn mutation_root(&mut self) -> Obj {
        Obj::Mutation
    }

    fn type_name(&self, object: &Obj) -> String {
        self.type_of(object).to_string()
    }

    fn field(&mut self, ctx: &FieldContext<'_>, object: &Obj) -> Result<Output<Obj>, FieldError> {
        // Arguments this version gives in another shape than the resolvers read.
        if let Some(write) = adapt::arguments(self.adapters(), ctx.parent_type(), ctx.field_name())
        {
            let mut arguments = ctx.arguments().clone();
            write(&mut arguments);
            return self.resolve_field(&ctx.with_arguments(&arguments), object);
        }
        self.resolve_field(ctx, object)
    }
}

impl ShopResolver<'_> {
    /// A field of an object, its arguments in the shape the resolvers read: access, then a
    /// mutation, a custom resolver, or the generic rules.
    fn resolve_field(
        &mut self,
        ctx: &FieldContext<'_>,
        object: &Obj,
    ) -> Result<Output<Obj>, FieldError> {
        access::check(self, ctx, object)?;
        if let Obj::Mutation = object {
            // An amount past what lsf takes is refused before the mutation runs (`money`).
            if self.registry.mutation_fn(ctx.field_name()).is_some() {
                let oversized = money::oversized(ctx);
                if !oversized.is_empty() {
                    let type_name = ctx.ty().named();
                    let mut payload = money::refused(ctx.schema(), type_name, &oversized);
                    if ctx.schema().field(type_name, "shop").is_some() {
                        payload.insert("shop".into(), json!(self.state.shop_gid()));
                    }
                    return Ok(Output::Object(Obj::built(type_name, payload)));
                }
            }
            // One webhook per topic and record of this mutation (`ShopState::merge_events_from`).
            let start = self.state.event_count();
            // The mutation and what follows it are one change: a panic in either puts the shop
            // back as it was before the mutation (`mutation::internal_error`).
            let savepoint = self.state.savepoint();
            let mut output = mutation::run(self, ctx);
            let (webhooks, shop) = (self.webhooks, self.state.domain().to_string());
            let followed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // What follows a change on its own: listings, auto-publishing
                // (`domains::markets`).
                crate::domains::markets::after_mutation(self.state, start);
                // The segments the customers it changed joined or left, and their sales.
                crate::domains::segments::follow(self.state, start, &|topic| {
                    webhooks.is_some_and(|webhooks| webhooks.subscribes(&shop, topic))
                });
                // The product feeds' syncs, from the listings (`domains::catalog`).
                crate::domains::catalog::after_mutation(self.state, start);
            }));
            match followed {
                Ok(()) => self.state.release(savepoint),
                Err(panic) => {
                    self.state.rollback_unwound(savepoint);
                    output = Err(mutation::internal_error(&*panic));
                }
            }
            self.state.merge_events_from(start);
            return output;
        }
        if let Some(resolve) = self.registry.field_fn(ctx.parent_type(), ctx.field_name()) {
            return resolve(self, ctx, object);
        }
        self.generic(ctx, object)
    }
}

/// `snake_case` (a search filter, a sort key in lowercase) in `camelCase`: `product_type` →
/// `productType`.
pub fn camel_case(snake: &str) -> String {
    let mut camel = String::with_capacity(snake.len());
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            camel.extend(c.to_uppercase());
            upper = false;
        } else {
            camel.push(c);
        }
    }
    camel
}
