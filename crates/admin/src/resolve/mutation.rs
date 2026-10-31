//! The mutation framework: a mutation runs on the locked shop, validates its input as Shopify
//! does, and answers its payload as JSON. It changes nothing when it fails.
//!
//! - **Validation**: what is wrong with the input is a [`UserError`] in the payload's
//!   `userErrors` (`field`, `message`, and `code` where the payload's error type has one), the
//!   payload's objects `null`. Shopify answers `200` with them; a GraphQL error is for what the
//!   request itself cannot do.
//! - **Nothing on failure**: [`Mutation::state_mut`] keeps a copy of the shop the first time it
//!   is called; a payload with user errors, or an error, puts that copy back. A mutation can
//!   therefore validate as it goes. Validating first, then changing, costs no copy when it fails.
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
use crate::state::{Record, ShopState};
use crate::webhooks::Webhooks;

/// The `extensions.code` of a mutation lsf does not simulate.
pub const LSF_NOT_SIMULATED: &str = "LSF_NOT_SIMULATED";

/// What a mutation works with: the shop (copied before its first change, to be put back if the
/// mutation fails), the app's webhooks, the token's grant.
pub struct Mutation<'m> {
    state: &'m mut ShopState,
    snapshot: Option<ShopState>,
    registry: &'m Registry,
    grant: Option<&'m Grant>,
    webhooks: Option<&'m Webhooks>,
    api_version: &'m str,
    origin: &'m str,
    notes: Vec<String>,
}

impl Mutation<'_> {
    /// The shop, to read and validate against.
    pub fn state(&self) -> &ShopState {
        self.state
    }

    /// The shop, to change. The first call keeps a copy of it, which a failure puts back.
    pub fn state_mut(&mut self) -> &mut ShopState {
        if self.snapshot.is_none() {
            self.snapshot = Some(self.state.clone());
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
        self.api_version
    }

    /// Where the request reached lsf (`http://127.0.0.1:9292`): the origin of the URLs a
    /// mutation answers that point back at lsf (a confirmation page, an upload target).
    pub fn origin(&self) -> &str {
        self.origin
    }

    /// A resolver over the same shop, the same token and webhooks: what runs the GraphQL of a
    /// bulk operation (its query, or its mutation once per line). What it changes is changed
    /// here (the shop is copied first, as [`Mutation::state_mut`] does), and its events are the
    /// request's.
    pub fn resolver(&mut self) -> ShopResolver<'_> {
        let (registry, grant, webhooks, api_version, origin) = (
            self.registry,
            self.grant,
            self.webhooks,
            self.api_version,
            self.origin,
        );
        let mut resolver = ShopResolver::new(self.state_mut(), registry).with_origin(origin);
        resolver.grant = grant;
        resolver.webhooks = webhooks;
        resolver.api_version = api_version;
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
        snapshot: None,
        registry,
        grant: resolver.grant,
        webhooks: resolver.webhooks,
        api_version: resolver.api_version,
        origin: resolver.origin,
        notes: Vec::new(),
    };
    let result = run(&mut mutation, ctx);
    let Mutation {
        snapshot, notes, ..
    } = mutation;
    let failed = match &result {
        Ok(payload) => has_user_errors(payload),
        Err(_) => true,
    };
    if failed && let Some(before) = snapshot {
        *resolver.state = before;
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
