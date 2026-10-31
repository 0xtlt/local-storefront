//! The mutation framework: a mutation runs on the locked shop, validates its input as Shopify
//! does, and answers its payload as JSON. It changes nothing when it fails.
//!
//! - **Validation**: what is wrong with the input is a [`UserError`] in the payload's
//!   `userErrors` (`field`, `message`, and `code` where the payload's error type has one), the
//!   payload's objects `null`. Shopify answers `200` with them; a GraphQL error is for what the
//!   request itself cannot do.
//! - **Nothing on failure**: [`Mutation::state_mut`] opens a savepoint of the shop the first
//!   time it is called ([`ShopState::savepoint`]): from then on each write keeps what it
//!   overwrote, and a payload with user errors, or an error, puts it back. A mutation can
//!   therefore validate as it goes; what a failure costs is what the mutation wrote, not the
//!   size of the shop.
//! - **Events**: what changed is emitted through the state (`state_mut().emit("products/update",
//!   &gid)`; `state_mut().delete(&gid, "products/delete")` for a deletion, which keeps the record
//!   for the payload). The api layer hands them to the webhooks once the shop is unlocked.
//! - **Not simulated**: a mutation no domain registered answers a GraphQL error with
//!   `extensions.code = "LSF_NOT_SIMULATED"` ([`not_simulated`]), never a silent success; an
//!   input a mutation ignores is said in `extensions.lsf.notes` ([`Mutation::note`]).
//!
//! The payload is resolved like any object: record references are GIDs, the `shop: Shop!` some
//! payloads have is filled in when the mutation leaves it out.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use super::{Obj, Registry, ShopResolver};
use crate::auth::Grant;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::state::{Record, Savepoint, ShopState};
use crate::version::ApiVersion;
use crate::webhooks::Webhooks;

/// The `extensions.code` of a mutation lsf does not simulate.
pub const LSF_NOT_SIMULATED: &str = "LSF_NOT_SIMULATED";

/// What a mutation works with: the shop (with a savepoint opened before its first change, which
/// a failure rolls back to), the app's webhooks, the token's grant.
pub struct Mutation<'m> {
    state: &'m mut ShopState,
    savepoint: Option<Savepoint>,
    registry: &'m Registry,
    grant: Option<&'m Grant>,
    webhooks: Option<&'m Webhooks>,
    version: ApiVersion,
    origin: &'m str,
    notes: Vec<String>,
    /// Whether the changes stay when the payload has user errors (a partial update).
    keep: bool,
}

impl Mutation<'_> {
    /// The shop, to read and validate against.
    pub fn state(&self) -> &ShopState {
        self.state
    }

    /// The shop, to change. The first call opens a savepoint, which a failure rolls back to.
    pub fn state_mut(&mut self) -> &mut ShopState {
        if self.savepoint.is_none() {
            self.savepoint = Some(self.state.savepoint());
        }
        self.state
    }

    /// The registry, for a mutation that runs another's code.
    pub fn registry(&self) -> &Registry {
        self.registry
    }

    /// What the token grants; `None` when every scope is granted.
    pub fn grant(&self) -> Option<&Grant> {
        self.grant
    }

    /// The webhooks of the app: `webhookSubscriptionCreate` and the others change their
    /// registry. `None` when the server has none (a resolver run alone, in a test).
    pub fn webhooks(&self) -> Option<&Webhooks> {
        self.webhooks
    }

    /// The API version of the request: what a subscription created now asks for.
    pub fn api_version(&self) -> &str {
        self.version.name()
    }

    /// The version of the request (see `ShopResolver::version`).
    pub fn version(&self) -> ApiVersion {
        self.version
    }

    /// The schema of the request's version.
    pub fn schema(&self) -> &'static crate::schema::Schema {
        crate::schema::for_version(self.version)
    }

    /// Where the request reached lsf (`http://127.0.0.1:9292`): the origin of the URLs a
    /// mutation answers that point back at lsf (a confirmation page, an upload target).
    pub fn origin(&self) -> &str {
        self.origin
    }

    /// A resolver over the same shop, the same token and webhooks: what runs the GraphQL of a
    /// bulk operation (its query, or its mutation once per line). What it changes is changed
    /// here (within this mutation's savepoint, as [`Mutation::state_mut`] opens it), and its
    /// events are the request's.
    pub fn resolver(&mut self) -> ShopResolver<'_> {
        let (registry, grant, webhooks, version, origin) = (
            self.registry,
            self.grant,
            self.webhooks,
            self.version,
            self.origin,
        );
        let mut resolver = ShopResolver::new(self.state_mut(), registry).with_origin(origin);
        resolver.grant = grant;
        resolver.webhooks = webhooks;
        resolver.version = version;
        resolver
    }

    /// The domain of the shop: `my-shop.myshopify.com`.
    pub fn shop(&self) -> &str {
        self.state.domain()
    }

    /// The time of the shop: its frozen clock, or the system's.
    pub fn now(&self) -> DateTime<Utc> {
        self.state.now()
    }

    /// [`Mutation::now`] as a GraphQL `DateTime`.
    pub fn timestamp(&self) -> String {
        self.state.timestamp()
    }

    /// Says what the mutation did not simulate (an input it ignored), in `extensions.lsf.notes`.
    pub fn note(&mut self, text: impl Into<String>) {
        self.notes.push(text.into());
    }

    /// Keeps the changes made even though the payload reports user errors: a mutation that
    /// applies what it can and reports the rest (`productVariantsBulkUpdate` with
    /// `allowPartialUpdates`). An error still puts the shop back.
    pub fn keep_changes(&mut self) {
        self.keep = true;
    }
}

/// A `userErrors` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserError {
    /// The path of the input field at fault (`["product", "title"]`), `null` when the error is
    /// not about one.
    pub field: Option<Vec<String>>,
    pub message: String,
    /// The code, for the payloads whose error type has one (`ProductVariantsBulkCreateUserError
    /// .code`): `BLANK`, `TAKEN`, `NOT_FOUND`, … An error type without `code` ignores it.
    pub code: Option<String>,
}

impl UserError {
    /// An error about the input field at `field` (empty: about none).
    pub fn new(field: &[&str], message: impl Into<String>) -> UserError {
        UserError {
            field: (!field.is_empty()).then(|| field.iter().map(|part| part.to_string()).collect()),
            message: message.into(),
            code: None,
        }
    }

    /// Sets the code.
    pub fn code(mut self, code: &str) -> UserError {
        self.code = Some(code.to_string());
        self
    }

    pub fn to_json(&self) -> Value {
        let mut error = Map::new();
        error.insert("field".into(), json!(self.field));
        error.insert("message".into(), json!(self.message));
        if let Some(code) = &self.code {
            error.insert("code".into(), json!(code));
        }
        Value::Object(error)
    }
}

/// A `Job` that is already done, as a record of the shop (so that `node(id:)` and `job(id:)`
/// find it): what lsf answers for the work Shopify does in the background, having done it at
/// once. `kind` names the work (`collection_add_products`, `order_cancel`, …); the id is a UUID,
/// as Shopify's job ids are, different for each job of the shop. Returns its GID.
pub fn done_job(state: &mut ShopState, kind: &str) -> String {
    let id = format!(
        "gid://shopify/Job/{}",
        crate::state::value::uuid(&format!(
            "job/{}/{kind}/{}",
            state.domain(),
            state.count("Job")
        ))
    );
    let mut record = Record::new();
    record.insert("__typename".into(), json!("Job"));
    record.insert("id".into(), json!(id));
    record.insert("done".into(), json!(true));
    record.insert("query".into(), Value::Null);
    state.insert_as(id.clone(), record);
    id
}

/// A payload: `fields` (an object), with `userErrors` set to `errors`.
pub fn payload(fields: Value, errors: &[UserError]) -> Value {
    let mut payload = match fields {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    payload.insert(
        "userErrors".into(),
        Value::Array(errors.iter().map(UserError::to_json).collect()),
    );
    Value::Object(payload)
}

/// The error of a mutation lsf does not simulate.
pub fn not_simulated(mutation: &str) -> FieldError {
    FieldError::new(format!(
        "lsf does not simulate the mutation {mutation}: nothing was changed. GET /__lsf/admin \
         lists the mutations it simulates."
    ))
    .with_code(LSF_NOT_SIMULATED)
}

/// Whether a payload reports user errors: a non-empty `userErrors` (or `…UserErrors`, as
/// `customerUserErrors`).
fn has_user_errors(payload: &Value) -> bool {
    payload.as_object().is_some_and(|map| {
        map.iter().any(|(key, value)| {
            (key == "userErrors" || key.ends_with("UserErrors"))
                && value.as_array().is_some_and(|errors| !errors.is_empty())
        })
    })
}

/// Runs the mutation of a field of `Mutation`.
pub(super) fn run(
    resolver: &mut ShopResolver<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Output<Obj>, FieldError> {
    let name = ctx.field_name();
    let registry = resolver.registry;
    let Some(run) = registry.mutation_fn(name) else {
        return Err(not_simulated(name));
    };
    let mut mutation = Mutation {
        state: &mut *resolver.state,
        savepoint: None,
        registry,
        grant: resolver.grant,
        webhooks: resolver.webhooks,
        version: resolver.version,
        origin: resolver.origin,
        notes: Vec::new(),
        keep: false,
    };
    let result = run(&mut mutation, ctx);
    let Mutation {
        savepoint,
        notes,
        keep,
        ..
    } = mutation;
    let failed = match &result {
        Ok(payload) => has_user_errors(payload) && !keep,
        Err(_) => true,
    };
    if let Some(savepoint) = savepoint {
        if failed {
            resolver.state.rollback(savepoint);
        } else {
            resolver.state.release(savepoint);
        }
    }
    for note in notes {
        resolver.note(note);
    }
    let Value::Object(mut payload) = result? else {
        return Ok(Output::Null);
    };
    let type_name = ctx.ty().named();
    if ctx.schema().field(type_name, "shop").is_some() && !payload.contains_key("shop") {
        payload.insert("shop".into(), json!(resolver.state.shop_gid()));
    }
    Ok(Output::Object(Obj::built(type_name, payload as Record)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::{self, Request};
    use crate::state::{Blob, Clock};

    #[test]
    fn each_done_job_is_a_record_of_its_own() {
        let mut state = ShopState::new("a.myshopify.com", Clock::System);
        // Two families, one job each: never the same id.
        let cancel = done_job(&mut state, "order_cancel");
        let mandate = done_job(&mut state, "order_create_mandate_payment");
        let again = done_job(&mut state, "order_cancel");
        assert_ne!(cancel, mandate);
        assert_ne!(cancel, again);
        let record = state.get(&cancel).unwrap();
        assert_eq!(record["__typename"], "Job");
        assert_eq!(record["done"], true);
        assert_eq!(record["query"], Value::Null);
    }

    /// A `productCreate` that writes a lot, then answers as its `title` says: `fail` (user
    /// errors), `keep` (user errors, changes kept), `error` (a GraphQL error, changes kept
    /// asked), `ok`.
    fn writes_then_answers(
        m: &mut Mutation<'_>,
        ctx: &FieldContext<'_>,
    ) -> Result<Value, FieldError> {
        let outcome = ctx.arguments()["product"]["title"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let products: Vec<String> = m.state().keys("Product").map(str::to_string).collect();
        let variants: Vec<String> = m
            .state()
            .keys("ProductVariant")
            .map(str::to_string)
            .collect();
        let state = m.state_mut();
        for product in &products {
            if let Some(record) = state.get_mut(product) {
                record.insert("title".into(), json!(format!("Renamed by {outcome}")));
            }
            state.touch(product);
            state.emit("products/update", product);
        }
        for variant in variants.iter().step_by(2) {
            state.delete(variant, "variants/delete");
        }
        for n in 0..100 {
            let mut record = Record::new();
            record.insert("__typename".into(), json!("Product"));
            record.insert("title".into(), json!(format!("New {n}")));
            let gid = state.insert(record);
            state.emit("products/create", &gid);
        }
        state.set_memo(format!("memo/{outcome}"), json!(outcome));
        state.put_blob(
            format!("blob/{outcome}"),
            Blob::new(outcome.clone(), "text/plain"),
        );
        match outcome.as_str() {
            "fail" => Ok(payload(
                json!({"product": null}),
                &[UserError::new(&["product"], "No")],
            )),
            "keep" => {
                m.keep_changes();
                Ok(payload(
                    json!({"product": null}),
                    &[UserError::new(&["product"], "Some")],
                ))
            }
            "error" => {
                m.keep_changes();
                Err(FieldError::new("Broken"))
            }
            _ => Ok(payload(json!({"product": null}), &[])),
        }
    }

    fn run_titled(shop: &mut ShopState, registry: &Registry, titles: &[&str]) -> Value {
        let fields: Vec<String> = titles
            .iter()
            .map(|title| {
                format!(r#"{title}: productCreate(product: {{title: "{title}"}}) {{ userErrors {{ message }} }}"#)
            })
            .collect();
        let query = format!("mutation {{ {} }}", fields.join(" "));
        let mut resolver = ShopResolver::new(shop, registry);
        graphql::run(
            crate::schema::admin(),
            &mut resolver,
            &Request::new(&query),
            None,
        )
        .to_json()
    }

    #[test]
    fn a_failed_mutation_leaves_the_shop_as_it_was_after_many_writes() {
        let (store, _) = lsf_core::store::load::load(
            &lsf_core::store::load::DataSource::Demo,
            &Default::default(),
        );
        let mut shop = ShopState::seeded(&store, None);
        let mut registry = Registry::new();
        registry.mutation("productCreate", writes_then_answers);
        let before = shop.picture();

        // User errors, then an error even with the changes kept: nothing changed, no event.
        let answer = run_titled(&mut shop, &registry, &["fail", "error"]);
        assert_eq!(answer["data"]["fail"]["userErrors"][0]["message"], "No");
        assert_eq!(answer["errors"][0]["message"], "Broken");
        assert_eq!(shop.picture(), before);

        // A partial update keeps its changes; a mutation that succeeds too, and the failure that
        // follows it in the same request undoes only its own.
        let answer = run_titled(&mut shop, &registry, &["keep", "ok", "fail"]);
        assert_eq!(answer["data"]["keep"]["userErrors"][0]["message"], "Some");
        assert_eq!(shop.memo("memo/keep"), Some(&json!("keep")));
        assert_eq!(shop.memo("memo/ok"), Some(&json!("ok")));
        assert_eq!(shop.memo("memo/fail"), None);
        assert!(shop.blob("blob/ok").is_some() && shop.blob("blob/fail").is_none());
        assert!(
            shop.all("Product")
                .all(|product| product["title"] != json!("Renamed by fail"))
        );
        let created = shop
            .events()
            .iter()
            .filter(|event| event.topic == "products/create")
            .count();
        assert_eq!(created, 200);
    }

    #[test]
    fn writes_user_errors_and_payloads() {
        let error = UserError::new(&["product", "title"], "Title can't be blank").code("BLANK");
        assert_eq!(
            error.to_json(),
            json!({"field": ["product", "title"], "message": "Title can't be blank", "code": "BLANK"})
        );
        assert_eq!(
            UserError::new(&[], "Nope").to_json(),
            json!({"field": null, "message": "Nope"})
        );
        let failed = payload(json!({"product": null}), &[error]);
        assert!(has_user_errors(&failed));
        assert!(!has_user_errors(&payload(json!({"product": "x"}), &[])));
        assert!(has_user_errors(
            &json!({"customerUserErrors": [{"message": "x"}], "userErrors": []})
        ));
    }
}
