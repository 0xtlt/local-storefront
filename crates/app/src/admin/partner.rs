//! A local Partner API: what an app on Shopify App Pricing reads of its plan and of its history,
//! from the billing state lsf keeps (`domains::app::partner`), served from lsf's origin at
//! Shopify's path (`/{partner_id}/api/{version}/graphql.json`), so that an app changes only the
//! origin of `https://partners.shopify.com`.
//!
//! What Shopify documents (read on 2026-11-09):
//!
//! - The endpoint and its auth ([Partner API reference](https://shopify.dev/docs/api/partner)):
//!   `POST https://partners.shopify.com/{partner_id}/api/2026-10/graphql.json`, the Partner API
//!   client's token in `X-Shopify-Access-Token`. The 2026-10 reference names the number the Partner
//!   ID, "the number in your Partner Dashboard URL", and warns that the Dev Dashboard's
//!   organization ID is another number; older pages call it `{organization_id}`. lsf follows the
//!   current reference. Errors: `401` `{"errors": [{"message": "Invalid access token",
//!   "extensions": {"code": "401"}}]}`, `404` "Invalid API version", `400` "Maximum query length
//!   is 50000 characters", `429` "Too many requests" past four requests a second (lsf does not
//!   throttle).
//! - Plan reading ([Shopify App Pricing](https://shopify.dev/docs/apps/launch/billing/shopify-app-pricing),
//!   [Active subscription](https://shopify.dev/docs/api/partner/2026-07/active-subscription)):
//!   `activeSubscription(appId:, shopId:)`, Shopify GIDs (`gid://shopify/App/1234`,
//!   `gid://shopify/Shop/5678`), `null` without an active contract, "Shop not found" for a shop
//!   that is not one. The pages disagree on what it covers: Shopify App Pricing says "whether it
//!   bills through Shopify App Pricing or the Billing API", the reference "The shop must have an
//!   active managed pricing contract for the app" (and the schema "The active managed pricing
//!   subscription"). lsf follows the reference: a subscription of the Billing API is `null`.
//! - The history ([Historical events](https://shopify.dev/docs/api/partner/2026-07/historical-events)):
//!   `events(filter:, orderBy:, first:, after:, last:, before:)`, newest first by default, the
//!   last 30 days unless `occurredAtMin` or `occurredAtMax` says otherwise (365 days at most),
//!   APP events unless `subjectType` says THEME, 250 a page at most, "You must specify a
//!   subjectId when filtering by shopId." And the older `app(id:) { events }`, its own event
//!   types (`AppEventTypes`) and Partner GIDs (`gid://partners/App/1234`,
//!   `gid://partners/Shop/1234`).
//! - Versions: the pinned `@shopify/dev-mcp` lists 2026-01, 2026-04, 2026-07, 2026-10 (latest),
//!   2027-01 (release candidate) and unstable; `activeSubscription` and `events` came in 2026-07,
//!   and 2026-07, 2026-10 and 2027-01 have the same introspection (`tools/admin/partner_schema.mjs`
//!   says so). lsf serves those three with one schema, Shopify's whole 2026-10 one
//!   (`schemas/partner/2026-10/partner.graphql`), so that a query valid on Shopify is valid here.
//!
//! What lsf answers: `activeSubscription`, `events`, and `app(id:)` with its `events`, the
//! fields an app reads its plan and its history with. Every other root field (`transactions`,
//! `transaction`, `publicApiVersions`, the mutations `appSubscriptionCancel` and
//! `appCreditCreate`) answers an error with `extensions.code: LSF_NOT_SIMULATED`, as the Admin
//! API does for what lsf does not simulate.
//!
//! lsf's choices, where Shopify does not say:
//!
//! - The Partner ID is a number of seven digits, the same every run for the app's client id
//!   ([`partner_id`]); the token `prtapi_lsf_` and 32 hex digits ([`token`]), the same every run
//!   too. Shopify documents no format. Its real tokens are `prtapi_` and 32 hex digits, which
//!   GitHub's secret scanning stops as a "Shopify Partner API Token": the `lsf_` keeps lsf's out
//!   of that shape, so that an app's repository can hold it in a test or a `.env` and still be
//!   pushed. A wrong token, or another Partner ID in the path, is the `401`.
//! - The app's number is its Admin API one (`gid://shopify/App/<n>`, what
//!   `currentAppInstallation { app { id } }` answers), and `gid://partners/App/<n>` the same
//!   number; a shop's is its `shop { id }` number, under both prefixes too. `activeSubscription`
//!   and `events` take the Shopify GIDs alone, as their pages write them ("must use Shopify global
//!   IDs", App subscription cancel); `app(id:)` and `App.events(shopId:)` take either.
//! - `App.events` lists newest first, with no default window.
//! - Without `first` or `last`, a page has 250 events; more than 250 asked is an error, in lsf's
//!   words, as are the errors of a date range over 365 days and of a cursor lsf did not give.

use std::sync::OnceLock;

use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Map, Value, json};

use crate::admin::domains::app::partner as history;
use crate::admin::graphql::{self, FieldContext, FieldError, Output, Request, Resolver, Response};
use crate::admin::schema::Schema;
use crate::admin::state::{ShopState, gid_number, value};

/// The versions lsf serves: those whose schema is the one of `schemas/partner/2026-10`.
pub const VERSIONS: [&str; 3] = ["2026-07", "2026-10", "2027-01"];

/// The version lsf prints and documents: Shopify's latest stable one.
pub const DEFAULT_VERSION: &str = "2026-10";

/// The longest query Shopify takes ("Maximum query length is 50000 characters").
pub const MAX_QUERY_LENGTH: usize = 50_000;

/// The most events a page has ("The page size limit is 250").
const MAX_PAGE_SIZE: usize = 250;

/// How far back `events` looks without `occurredAtMin` or `occurredAtMax` ("By default the
/// `events` query returns the last 30 days").
const DEFAULT_WINDOW_DAYS: i64 = 30;

/// The longest range of `events` ("maximum range is 365 days").
const MAX_RANGE_DAYS: i64 = 365;

/// The code of an error for what lsf does not simulate, as the Admin API's.
pub const NOT_SIMULATED: &str = "LSF_NOT_SIMULATED";

/// The schema of the Partner API, parsed the first time a request asks for it: Shopify's 2026-10
/// introspection, written by `tools/admin/partner_schema.mjs`.
pub fn schema() -> &'static Schema {
    static PARSED: OnceLock<Schema> = OnceLock::new();
    PARSED.get_or_init(|| {
        Schema::parse(
            include_str!("../../schemas/partner/2026-10/partner.graphql"),
            include_str!("../../schemas/partner/2026-10/introspection.graphql"),
        )
        .unwrap_or_else(|error| panic!("the Partner API schema of lsf parses: {error}"))
    })
}

/// The Partner ID of the organization that owns the app of `client_id`: seven digits, as the
/// Partner Dashboard's are, the same every run.
pub fn partner_id(client_id: &str) -> u64 {
    1_000_000 + lsf_core::util::stable_id("partner organization", client_id) % 9_000_000
}

/// The token of the organization's Partner API client: `prtapi_lsf_` and 32 hex digits, the
/// same every run. Not `prtapi_` and 32 hex digits, the shape of a real one, which GitHub's push
/// protection refuses in a commit.
pub fn token(client_id: &str) -> String {
    use crate::admin::auth::signing::{hex, sha256};
    let digest = sha256(format!("lsf partner api token\0{client_id}").as_bytes());
    format!("prtapi_lsf_{}", &hex(&digest)[..32])
}

/// The URL of the Partner API on lsf: `http://127.0.0.1:9292/1234567/api/2026-10/graphql.json`.
pub fn endpoint(origin: &str, partner_id: u64) -> String {
    format!("{origin}/{partner_id}/api/{DEFAULT_VERSION}/graphql.json")
}

/// The Partner ID and the version of a path of the Partner API (`/1234567/api/2026-10/graphql.json`),
/// as written: `None` for another path.
pub fn parse_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix('/')?;
    let (partner, rest) = rest.split_once('/')?;
    if partner.is_empty() || !partner.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let rest = rest.strip_prefix("api/")?;
    let (version, file) = rest.split_once('/')?;
    (file == "graphql.json").then_some((partner, version))
}

/// Whether lsf serves a version.
pub fn serves(version: &str) -> bool {
    VERSIONS.contains(&version)
}

/// The organization as a request of the Partner API sees it: the app and the history of each shop
/// (read once per request, each shop locked in turn).
#[derive(Clone, Debug)]
pub struct Organization {
    pub partner_id: u64,
    pub app: AppView,
    pub shops: Vec<ShopView>,
}

/// The app of the organization: lsf runs one.
#[derive(Clone, Debug)]
pub struct AppView {
    /// Its number: `gid://shopify/App/<n>`, `gid://partners/App/<n>`.
    pub number: u64,
    pub name: String,
    /// Its client id.
    pub api_key: String,
}

/// A shop, as the Partner API reads it.
#[derive(Clone, Debug)]
pub struct ShopView {
    /// Its number: `gid://shopify/Shop/<n>`, `gid://partners/Shop/<n>`.
    pub number: u64,
    pub domain: String,
    pub name: String,
    /// The shop's clock: what `events` counts its default window back from.
    pub now: DateTime<Utc>,
    /// `activeSubscription` without `app` and `shop` ([`history::active_subscription`]).
    pub active: Option<Value>,
    /// The app's history on the shop, oldest first ([`history::history`]).
    pub history: Vec<Value>,
}

impl ShopView {
    /// The shop as the Partner API reads it, `legacy` saying whether the plans keep the legacy
    /// features (`legacySubscriptionId`).
    pub fn of(state: &ShopState, legacy: bool) -> ShopView {
        let name = state
            .shop()
            .and_then(|shop| shop.get("name"))
            .and_then(Value::as_str)
            .unwrap_or(state.domain())
            .to_string();
        ShopView {
            number: state.shop_gid().map_or(0, gid_number),
            domain: state.domain().to_string(),
            name,
            now: state.now(),
            active: history::active_subscription(state, legacy),
            history: history::history(state),
        }
    }
}

/// Runs a request of the Partner API over the organization: the answer's JSON, `errors` first
/// when there are some, then `data`. No `extensions.cost`: the Partner API has no query cost.
pub fn execute(org: &Organization, request: &Request) -> Value {
    let prepared = match graphql::prepare(
        schema(),
        &request.query,
        request.operation_name.as_deref(),
        &request.variables,
    ) {
        Ok(prepared) => prepared,
        Err(errors) => return Response::from_errors(errors).to_json(),
    };
    let executed = prepared.execute(&mut PartnerResolver { org });
    Response {
        errors: executed.errors,
        data: Some(executed.data),
        extensions: Map::new(),
    }
    .to_json()
}

/// The resolver of the Partner API: its objects are JSON values, each with its `__typename`; the
/// root fields and the connections are answered from the organization.
struct PartnerResolver<'o> {
    org: &'o Organization,
}

impl Resolver for PartnerResolver<'_> {
    type Object = Value;

    fn query_root(&mut self) -> Value {
        json!({"__typename": "QueryRoot"})
    }

    fn mutation_root(&mut self) -> Value {
        json!({"__typename": "MutationRoot"})
    }

    fn type_name(&self, object: &Value) -> String {
        object
            .get("__typename")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn field(
        &mut self,
        ctx: &FieldContext<'_>,
        object: &Value,
    ) -> Result<Output<Value>, FieldError> {
        match (ctx.parent_type(), ctx.field_name()) {
            ("QueryRoot", "activeSubscription") => self.active_subscription(ctx),
            ("QueryRoot", "events") => self.events(ctx),
            ("QueryRoot", "app") => Ok(Output::object_or_null(
                ctx.arg_str("id")
                    .filter(|id| self.is_app(id, &["partners", "shopify"]))
                    .map(|_| self.app_object()),
            )),
            ("QueryRoot" | "MutationRoot", name) => Err(not_simulated(name)),
            ("App", "events") => self.app_events(ctx),
            (_, name) => Ok(output(object.get(name).unwrap_or(&Value::Null))),
        }
    }
}

/// The error of a root field lsf does not simulate.
fn not_simulated(field: &str) -> FieldError {
    FieldError::new(format!(
        "lsf does not simulate the Partner API's {field}: it answers activeSubscription, events \
         and app {{ events }}, what an app reads of its plan and its history"
    ))
    .with_code(NOT_SIMULATED)
}

/// A JSON value as the executor takes it: an object (with its `__typename`), a list, a scalar.
fn output(value: &Value) -> Output<Value> {
    match value {
        Value::Null => Output::Null,
        Value::Object(_) => Output::Object(value.clone()),
        Value::Array(items) => Output::List(items.iter().map(output).collect()),
        scalar => Output::Value(scalar.clone()),
    }
}

/// The type and number of a GID under a prefix: `gid://shopify/App/12` is `("shopify", "App",
/// 12)`.
fn parse_gid(gid: &str) -> Option<(&str, &str, u64)> {
    let rest = gid.strip_prefix("gid://")?;
    let mut parts = rest.splitn(3, '/');
    let namespace = parts.next()?;
    let kind = parts.next()?;
    let number = parts.next()?.parse().ok()?;
    Some((namespace, kind, number))
}

/// A `DateTime` argument: `None` when it is not given, an error when it is not one.
fn date_time_arg(value: Option<&Value>, name: &str) -> Result<Option<DateTime<Utc>>, FieldError> {
    match value.filter(|value| !value.is_null()) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .and_then(value::parse_date_time)
            .map(Some)
            .ok_or_else(|| {
                FieldError::new(format!(
                    "{name} is not an ISO-8601 date and time (2026-04-01T00:00:00Z): {value}"
                ))
            }),
    }
}

impl PartnerResolver<'_> {
    /// Whether `id` is the app's GID under one of `namespaces`.
    fn is_app(&self, id: &str, namespaces: &[&str]) -> bool {
        parse_gid(id).is_some_and(|(namespace, kind, number)| {
            namespaces.contains(&namespace) && kind == "App" && number == self.org.app.number
        })
    }

    /// The shop of a GID under one of `namespaces`.
    fn shop(&self, id: &str, namespaces: &[&str]) -> Option<&ShopView> {
        let (namespace, kind, number) = parse_gid(id)?;
        if !namespaces.contains(&namespace) || kind != "Shop" {
            return None;
        }
        self.org.shops.iter().find(|shop| shop.number == number)
    }

    /// The app, as `app(id:)` answers it.
    fn app_object(&self) -> Value {
        let app = &self.org.app;
        json!({
            "__typename": "App",
            "id": format!("gid://partners/App/{}", app.number),
            "name": app.name,
            "apiKey": app.api_key,
        })
    }

    /// The app, as the subject of an event or of a subscription (`AppReference`).
    fn app_reference(&self) -> Value {
        let app = &self.org.app;
        json!({
            "__typename": "AppReference",
            "id": format!("gid://shopify/App/{}", app.number),
            "name": app.name,
            "apiKey": app.api_key,
        })
    }

    fn shop_reference(shop: &ShopView) -> Value {
        json!({
            "__typename": "ShopReference",
            "id": format!("gid://shopify/Shop/{}", shop.number),
            "myshopifyDomain": shop.domain,
            "name": shop.name,
        })
    }

    /// A shop as the events of `App.events` name it (`Shop`).
    fn partner_shop(shop: &ShopView) -> Value {
        json!({
            "__typename": "Shop",
            "id": format!("gid://partners/Shop/{}", shop.number),
            "myshopifyDomain": shop.domain,
            "name": shop.name,
            "avatarUrl": null,
        })
    }

    /// `activeSubscription(appId:, shopId:)`.
    fn active_subscription(&self, ctx: &FieldContext<'_>) -> Result<Output<Value>, FieldError> {
        let app_id = ctx.arg_str("appId").unwrap_or_default();
        if !self.is_app(app_id, &["shopify"]) {
            let app = &self.org.app;
            let message = if self.is_app(app_id, &["partners"]) {
                format!(
                    "activeSubscription takes the app's Shopify GID, gid://shopify/App/{}, not its Partner API GID {app_id}",
                    app.number
                )
            } else {
                format!(
                    "lsf runs one app, {} (gid://shopify/App/{}): {app_id} is not it",
                    app.name, app.number
                )
            };
            return Err(FieldError::new(message));
        }
        let shop_id = ctx.arg_str("shopId").unwrap_or_default();
        let Some(shop) = self.shop(shop_id, &["shopify"]) else {
            return Err(FieldError::new("Shop not found"));
        };
        Ok(match &shop.active {
            None => Output::Null,
            Some(active) => {
                let mut active = active.clone();
                active["app"] = self.app_reference();
                active["shop"] = Self::shop_reference(shop);
                Output::Object(active)
            }
        })
    }

    /// `events(filter:, orderBy:, …)`: the history of the app on every shop of the organization.
    fn events(&self, ctx: &FieldContext<'_>) -> Result<Output<Value>, FieldError> {
        let filter = ctx.arg_object_or_default("filter");
        let get = |name: &str| filter.get(name).and_then(Value::as_str);
        let subject = get("subjectId");
        let shop_id = get("shopId");
        if shop_id.is_some() && subject.is_none() {
            return Err(FieldError::new(
                "You must specify a subjectId when filtering by shopId.",
            ));
        }
        let min = date_time_arg(filter.get("occurredAtMin"), "occurredAtMin")?;
        let max = date_time_arg(filter.get("occurredAtMax"), "occurredAtMax")?;
        if let (Some(min), Some(max)) = (min, max)
            && max - min > Duration::days(MAX_RANGE_DAYS)
        {
            return Err(FieldError::new(format!(
                "The range between occurredAtMin and occurredAtMax is {} days: it can be {MAX_RANGE_DAYS} days at most",
                (max - min).num_days()
            )));
        }
        let types: Option<Vec<&str>> = filter
            .get("eventTypes")
            .and_then(Value::as_array)
            .map(|types| types.iter().filter_map(Value::as_str).collect());
        // Only APP events unless THEME is asked: lsf has no theme.
        let apps = get("subjectType").is_none_or(|kind| kind == "APP");
        let ours = subject.is_none_or(|subject| self.is_app(subject, &["shopify"]));
        let window = Duration::days(DEFAULT_WINDOW_DAYS);
        let mut items: Vec<(DateTime<Utc>, usize, usize, String, Value)> = Vec::new();
        for (index, shop) in self.org.shops.iter().enumerate() {
            if !apps || !ours {
                break;
            }
            if let Some(shop_id) = shop_id
                && self
                    .shop(shop_id, &["shopify"])
                    .is_none_or(|found| found.number != shop.number)
            {
                continue;
            }
            let (from, to) = match (min, max) {
                (None, None) => (shop.now - window, shop.now),
                (Some(min), None) => (min, (min + window).min(shop.now)),
                (None, Some(max)) => (max - window, max),
                (Some(min), Some(max)) => (min, max),
            };
            for (seq, event) in shop.history.iter().enumerate() {
                let Some(kind) = event.get("eventType").and_then(Value::as_str) else {
                    continue;
                };
                let Some(at) = value::date_time_of(event.get("at")) else {
                    continue;
                };
                if at < from
                    || at > to
                    || types.as_ref().is_some_and(|types| !types.contains(&kind))
                {
                    continue;
                }
                let node = self.partner_event(shop, seq, event, kind);
                items.push((at, index, seq, cursor(&shop.domain, seq), node));
            }
        }
        items.sort_by_key(|(at, index, seq, _, _)| (*at, *index, *seq));
        if ctx.arg_str("orderBy") != Some("OCCURRED_AT_ASC") {
            items.reverse();
        }
        let items = items
            .into_iter()
            .map(|(_, _, _, cursor, node)| (cursor, node))
            .collect();
        let page = paginate(ctx, items)?;
        Ok(Output::Object(json!({
            "__typename": "PartnerEventConnection",
            "edges": page.edges("PartnerEventEdge"),
            "pageInfo": {
                "__typename": "HistoricalEventsPageInfo",
                "hasNextPage": page.has_next,
                "hasPreviousPage": page.has_previous,
                "startCursor": page.start,
                "endCursor": page.end,
            },
        })))
    }

    /// An event of the history as `events` answers it (`Relationship`, `SubscriptionStatus`).
    fn partner_event(&self, shop: &ShopView, seq: usize, event: &Value, kind: &str) -> Value {
        let mut node = json!({
            "id": value::uuid(&format!("partner-event/{}/{seq}", shop.domain)),
            "occurredAt": event.get("at").cloned().unwrap_or(Value::Null),
            "eventType": kind,
            "shop": Self::shop_reference(shop),
            "subject": self.app_reference(),
        });
        if let Some(state) = kind.strip_prefix("RELATIONSHIP_") {
            node["__typename"] = json!("Relationship");
            node["state"] = json!(state);
            node["reason"] = Value::Null;
            node["reasonDescription"] = Value::Null;
        } else {
            node["__typename"] = json!("SubscriptionStatus");
            for field in ["state", "plan", "cancelEffectiveOn"] {
                node[field] = event.get(field).cloned().unwrap_or(Value::Null);
            }
        }
        node
    }

    /// `App.events(types:, shopId:, chargeId:, occurredAtMin:, occurredAtMax:, …)`: the events of
    /// the older system, newest first.
    fn app_events(&self, ctx: &FieldContext<'_>) -> Result<Output<Value>, FieldError> {
        let types: Option<Vec<&str>> = ctx
            .arg("types")
            .and_then(Value::as_array)
            .map(|types| types.iter().filter_map(Value::as_str).collect());
        let shop_id = ctx.arg_str("shopId");
        let charge = ctx.arg_str("chargeId");
        let min = date_time_arg(ctx.arg("occurredAtMin"), "occurredAtMin")?;
        let max = date_time_arg(ctx.arg("occurredAtMax"), "occurredAtMax")?;
        let mut items: Vec<(DateTime<Utc>, usize, usize, String, Value)> = Vec::new();
        for (index, shop) in self.org.shops.iter().enumerate() {
            if let Some(shop_id) = shop_id
                && self
                    .shop(shop_id, &["partners", "shopify"])
                    .is_none_or(|found| found.number != shop.number)
            {
                continue;
            }
            for (seq, event) in shop.history.iter().enumerate() {
                let Some(kind) = event.get("appEventType").and_then(Value::as_str) else {
                    continue;
                };
                let Some(at) = value::date_time_of(event.get("at")) else {
                    continue;
                };
                let charge_id = event.pointer("/charge/id").and_then(Value::as_str);
                if min.is_some_and(|min| at < min)
                    || max.is_some_and(|max| at > max)
                    || types.as_ref().is_some_and(|types| !types.contains(&kind))
                    || charge.is_some_and(|charge| charge_id != Some(charge))
                {
                    continue;
                }
                let mut node = json!({
                    "__typename": pascal_case(kind),
                    "type": kind,
                    "occurredAt": event.get("at").cloned().unwrap_or(Value::Null),
                    "app": self.app_object(),
                    "shop": Self::partner_shop(shop),
                });
                if let Some(charge) = event.get("charge").filter(|charge| !charge.is_null()) {
                    node["charge"] = charge.clone();
                }
                if kind == "RELATIONSHIP_UNINSTALLED" {
                    node["reason"] = Value::Null;
                    node["description"] = Value::Null;
                }
                items.push((at, index, seq, cursor(&shop.domain, seq), node));
            }
        }
        items.sort_by_key(|(at, index, seq, _, _)| (*at, *index, *seq));
        items.reverse();
        let items = items
            .into_iter()
            .map(|(_, _, _, cursor, node)| (cursor, node))
            .collect();
        let page = paginate(ctx, items)?;
        Ok(Output::Object(json!({
            "__typename": "AppEventConnection",
            "edges": page.edges("AppEventEdge"),
            "pageInfo": {
                "__typename": "PageInfo",
                "hasNextPage": page.has_next,
                "hasPreviousPage": page.has_previous,
            },
        })))
    }
}

/// `RELATIONSHIP_INSTALLED` as its object type names it: `RelationshipInstalled`.
fn pascal_case(kind: &str) -> String {
    kind.split('_')
        .map(|word| {
            let lower = word.to_ascii_lowercase();
            let mut chars = lower.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// The cursor of an event: opaque, the shop and the event's place in its history.
fn cursor(domain: &str, seq: usize) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{domain}/{seq}"))
}

/// A page of a connection.
struct Page {
    items: Vec<(String, Value)>,
    has_next: bool,
    has_previous: bool,
    start: Option<String>,
    end: Option<String>,
}

impl Page {
    fn edges(&self, typename: &str) -> Value {
        Value::Array(
            self.items
                .iter()
                .map(|(cursor, node)| json!({"__typename": typename, "cursor": cursor, "node": node}))
                .collect(),
        )
    }
}

/// The page `first`, `after`, `last` and `before` ask of `items` (in the connection's order).
fn paginate(ctx: &FieldContext<'_>, items: Vec<(String, Value)>) -> Result<Page, FieldError> {
    let size = |name: &str| -> Result<Option<usize>, FieldError> {
        match ctx.arg_i64(name) {
            None => Ok(None),
            Some(size) if size < 0 => Err(FieldError::new(format!(
                "{name} is a page size: 0 or more, not {size}"
            ))),
            Some(size) if size as usize > MAX_PAGE_SIZE => Err(FieldError::new(format!(
                "{name} asks {size} events: a page has {MAX_PAGE_SIZE} at most"
            ))),
            Some(size) => Ok(Some(size as usize)),
        }
    };
    let (first, last) = (size("first")?, size("last")?);
    let position = |name: &str| -> Result<Option<usize>, FieldError> {
        match ctx.arg_str(name) {
            None => Ok(None),
            Some(given) => items
                .iter()
                .position(|(cursor, _)| cursor == given)
                .map(Some)
                .ok_or_else(|| {
                    FieldError::new(format!(
                        "{name}: {given} is not a cursor of this connection"
                    ))
                }),
        }
    };
    let mut from = position("after")?.map_or(0, |at| at + 1);
    let mut to = position("before")?.unwrap_or(items.len()).max(from);
    match (first, last) {
        (Some(first), _) => to = to.min(from + first),
        (None, Some(last)) => from = from.max(to.saturating_sub(last)),
        (None, None) => to = to.min(from + MAX_PAGE_SIZE),
    }
    if let (Some(_), Some(last)) = (first, last) {
        from = from.max(to.saturating_sub(last));
    }
    let has_previous = from > 0;
    let has_next = to < items.len();
    let page: Vec<(String, Value)> = items.into_iter().skip(from).take(to - from).collect();
    Ok(Page {
        start: page.first().map(|(cursor, _)| cursor.clone()),
        end: page.last().map(|(cursor, _)| cursor.clone()),
        items: page,
        has_next,
        has_previous,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_is_shopifys_partner_api() {
        let schema = schema();
        assert_eq!(schema.query_type(), "QueryRoot");
        assert_eq!(schema.mutation_type(), Some("MutationRoot"));
        assert_eq!(
            schema
                .types()
                .filter(|ty| !ty.name.starts_with("__"))
                .count(),
            118
        );
        let active = schema.field("QueryRoot", "activeSubscription").unwrap();
        assert_eq!(active.ty.to_string(), "ActiveSubscription");
        assert_eq!(active.arg("appId").unwrap().ty.to_string(), "ID!");
        let events = schema.field("QueryRoot", "events").unwrap();
        assert_eq!(events.ty.to_string(), "PartnerEventConnection");
        assert_eq!(
            events.arg("orderBy").unwrap().default_literal.as_deref(),
            Some("OCCURRED_AT_DESC")
        );
        assert!(schema.is_possible("PartnerEvent", "SubscriptionStatus"));
        assert!(schema.is_possible("AppEvent", "SubscriptionChargeActivated"));
        assert!(schema.is_possible("Price", "FlatRatePrice"));
    }

    #[test]
    fn paths_of_the_partner_api() {
        assert_eq!(
            parse_path("/1234567/api/2026-10/graphql.json"),
            Some(("1234567", "2026-10"))
        );
        assert_eq!(parse_path("/1234567/api/2026-10/other.json"), None);
        assert_eq!(parse_path("/abc/api/2026-10/graphql.json"), None);
        assert_eq!(parse_path("/admin/api/2026-10/graphql.json"), None);
    }

    #[test]
    fn ids_and_tokens_are_the_same_every_run() {
        let id = partner_id("abc");
        assert_eq!(id, partner_id("abc"));
        assert!((1_000_000..10_000_000).contains(&id), "{id}");
        assert_ne!(partner_id("abc"), partner_id("abd"));
        let token = token("abc");
        assert!(
            token.starts_with("prtapi_lsf_") && token.len() == 43,
            "{token}"
        );
        assert_eq!(token, super::token("abc"));
        assert_eq!(
            endpoint("http://127.0.0.1:9292", 1234567),
            "http://127.0.0.1:9292/1234567/api/2026-10/graphql.json"
        );
        assert!(serves("2026-07") && serves("2026-10") && serves("2027-01"));
        assert!(!serves("2026-04") && !serves("unstable"));
    }

    #[test]
    fn event_types_name_their_object_types() {
        assert_eq!(
            pascal_case("RELATIONSHIP_INSTALLED"),
            "RelationshipInstalled"
        );
        assert_eq!(
            pascal_case("SUBSCRIPTION_CHARGE_CANCELED"),
            "SubscriptionChargeCanceled"
        );
        assert_eq!(
            pascal_case("ONE_TIME_CHARGE_ACCEPTED"),
            "OneTimeChargeAccepted"
        );
        // Each is an object type of the schema that implements AppEvent.
        for kind in schema().enum_values("AppEventTypes").unwrap() {
            assert!(
                schema().is_possible("AppEvent", &pascal_case(&kind.name)),
                "{}",
                kind.name
            );
        }
    }

    #[test]
    fn gids_under_both_prefixes() {
        assert_eq!(
            parse_gid("gid://shopify/App/12"),
            Some(("shopify", "App", 12))
        );
        assert_eq!(
            parse_gid("gid://partners/Shop/34"),
            Some(("partners", "Shop", 34))
        );
        assert_eq!(parse_gid("gid://shopify/App/x"), None);
        assert_eq!(parse_gid("App/12"), None);
    }
}
