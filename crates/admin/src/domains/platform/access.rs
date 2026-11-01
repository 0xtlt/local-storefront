//! What an app does to its own access: `appUninstall`, `appRevokeAccessScopes`,
//! `appDowngradeAccessScopes` (2027-01), `delegateAccessTokenCreate`,
//! `delegateAccessTokenDestroy`.
//!
//! As shopify.dev describes them (the reference of each mutation, "Manage access scopes" and
//! "Delegate API access", read on 2026-10-31):
//!
//! - **`appUninstall`** "Uninstalls an App from a shop. Apps use this mutation to uninstall
//!   themselves": the same uninstall as `POST /__lsf/admin/uninstall` (the app's tokens revoked,
//!   its subscription cancelled, `app/uninstalled` delivered, then the shop's webhook
//!   subscriptions deleted). The payload names the app; the token that asked works no more.
//! - **`appRevokeAccessScopes`**: "You can only revoke scopes that are configured as
//!   `optional_scopes` and that were dynamically granted. Access scopes configured in the
//!   `scopes` field can't be revoked dynamically." The codes are Shopify's:
//!   `CANNOT_REVOKE_REQUIRED_SCOPES`, `CANNOT_REVOKE_UNDECLARED_SCOPES`,
//!   `CANNOT_REVOKE_IMPLIED_SCOPES` (a `read_x` the granted `write_x` implies), `UNKNOWN_SCOPES`;
//!   their messages are the descriptions of the codes in the schema (Shopify publishes no
//!   other). A scope not granted is not revoked (`revoked` lists what was). The installation's
//!   scopes change, `app/scopes_update` is delivered, and every token of the installation loses
//!   them.
//! - **`appDowngradeAccessScopes`** (2027-01): "Downgrades granted optional write access scopes
//!   to their matching read access scopes … Only write scopes that the app declares as optional
//!   can be downgraded. Required scopes can't be downgraded. Write scopes that aren't currently
//!   granted are ignored."
//! - **Delegate access tokens** "with a subset of the parent token's permissions": `shppa_` and
//!   32 hex digits (the guide's `shppa_12345678910`), scopes the parent was granted, expiring
//!   when the parent does unless `expiresIn` says sooner ("You can't set this longer than the
//!   parent token's remaining lifetime"); "A delegate access token can't create more delegate
//!   access tokens" (`DELEGATE_ACCESS_TOKEN`); "If your app is later re-authorized with fewer
//!   scopes, every existing delegate token loses the scopes that are no longer authorized"; an
//!   uninstall revokes them with the rest. They authenticate the Admin API as any token does.
//!
//! The token store hears of each through an event of [`AUTH`] (see the module above).

use chrono::Duration;
use lsf_core::util::stable_id;
use serde_json::{Value, json};

use crate::auth::scopes;
use crate::auth::tokens::GrantSource;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Event, Record, ShopState, value};

/// The topic of the events that tell the token store what a mutation changed: no
/// subscription can name it. Its snapshot is an [`AuthEffect`] as JSON.
pub const AUTH: &str = "lsf/auth";

/// What the tokens must do after a mutation.
#[derive(Clone, Debug, PartialEq)]
pub enum AuthEffect {
    /// The app uninstalled itself: its tokens go, as `POST /__lsf/admin/uninstall` does.
    Uninstall,
    /// The installation's scopes are now these: every token of it carries them (a delegate
    /// token keeps those of its own still granted).
    Scopes(Vec<String>),
    /// A delegate token: its scopes, and when it expires (`None`: never, as its parent).
    Delegate {
        token: String,
        scopes: Vec<String>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// A delegate token destroyed.
    Destroy { token: String },
}

impl AuthEffect {
    fn to_record(&self) -> Record {
        let value = match self {
            AuthEffect::Uninstall => json!({"effect": "uninstall"}),
            AuthEffect::Scopes(scopes) => json!({"effect": "scopes", "scopes": scopes}),
            AuthEffect::Delegate {
                token,
                scopes,
                expires_at,
            } => json!({
                "effect": "delegate",
                "token": token,
                "scopes": scopes,
                "expiresAt": expires_at.map(value::date_time),
            }),
            AuthEffect::Destroy { token } => json!({"effect": "destroy", "token": token}),
        };
        value::object(value)
    }

    fn from_record(record: &Record) -> Option<AuthEffect> {
        let strings = |field: &str| -> Vec<String> {
            record
                .get(field)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        };
        let token = || {
            record
                .get("token")
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        Some(match record.get("effect")?.as_str()? {
            "uninstall" => AuthEffect::Uninstall,
            "scopes" => AuthEffect::Scopes(strings("scopes")),
            "delegate" => AuthEffect::Delegate {
                token: token()?,
                scopes: strings("scopes"),
                expires_at: value::date_time_of(record.get("expiresAt")),
            },
            "destroy" => AuthEffect::Destroy { token: token()? },
            _ => return None,
        })
    }
}

/// The effects the events of a request carry, in order.
pub fn effects(events: &[Event]) -> Vec<AuthEffect> {
    events
        .iter()
        .filter(|event| event.topic == AUTH)
        .filter_map(|event| AuthEffect::from_record(event.snapshot.as_ref()?))
        .collect()
}

fn emit_effect(state: &mut ShopState, key: &str, effect: &AuthEffect) {
    state.emit_with(AUTH, key, effect.to_record());
}

pub fn register(registry: &mut Registry) {
    registry
        .mutation("appUninstall", app_uninstall)
        .mutation("appRevokeAccessScopes", app_revoke_access_scopes)
        .mutation("appDowngradeAccessScopes", app_downgrade_access_scopes)
        .mutation("delegateAccessTokenCreate", delegate_create)
        .mutation("delegateAccessTokenDestroy", delegate_destroy);
}

fn handles(record: Option<&Record>, field: &str) -> Vec<String> {
    record
        .and_then(|record| record.get(field))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|scope| scope.get("handle")?.as_str())
        .map(str::to_string)
        .collect()
}

/// The app and its installation: their keys.
fn app_and_installation(state: &ShopState) -> (Option<String>, Option<String>) {
    (
        state.keys("App").next().map(str::to_string),
        state.keys("AppInstallation").next().map(str::to_string),
    )
}

fn app_uninstall(m: &mut Mutation<'_>, _ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let (app, installation) = app_and_installation(m.state());
    let (Some(app), Some(installation)) = (app, installation) else {
        let error = UserError::new(&[], "The app is not installed.").code("APP_NOT_INSTALLED");
        return Ok(payload(json!({"app": null}), &[error]));
    };
    let state = m.state_mut();
    // Shopify cancels the app's subscription when it is uninstalled.
    super::super::app::billing::cancel_on_uninstall(state);
    if let Some(shop) = state.shop_gid().map(str::to_string) {
        state.emit("app/uninstalled", &shop);
    }
    emit_effect(state, &installation, &AuthEffect::Uninstall);
    Ok(payload(json!({"app": app}), &[]))
}

/// The scopes of the installation, the app's required and optional ones.
struct Declared {
    installation: String,
    granted: Vec<String>,
    required: Vec<String>,
    optional: Vec<String>,
}

fn declared(state: &ShopState) -> Option<Declared> {
    let (app, installation) = app_and_installation(state);
    let app = app.and_then(|key| state.get(&key));
    let installation = installation?;
    Some(Declared {
        granted: handles(state.get(&installation), "accessScopes"),
        required: handles(app, "requestedAccessScopes"),
        optional: handles(app, "optionalAccessScopes"),
        installation,
    })
}

/// Sets the installation's scopes, delivers `app/scopes_update` (the payload reads the scopes
/// before from the snapshot) and tells the tokens.
fn change_scopes(state: &mut ShopState, installation: &str, scopes: &[String]) {
    let Some(before) = state.get(installation).cloned() else {
        return;
    };
    if let Some(record) = state.get_mut(installation) {
        record.insert(
            "accessScopes".into(),
            Value::Array(
                scopes
                    .iter()
                    .map(|scope| json!({"handle": scope}))
                    .collect(),
            ),
        );
    }
    state.emit_with("app/scopes_update", installation, before);
    emit_effect(state, installation, &AuthEffect::Scopes(scopes.to_vec()));
}

fn access_scope(handle: &str) -> Value {
    json!({"handle": handle, "description": super::queries::scope_description(handle)})
}

fn app_revoke_access_scopes(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let asked = string_list(ctx, "scopes");
    let Some(declared) = declared(m.state()) else {
        let error = UserError::new(&[], "App is not installed on shop.").code("APP_NOT_INSTALLED");
        return Ok(payload(json!({"revoked": null}), &[error]));
    };
    let mut errors = Vec::new();
    let error = |message: &str, code: &str| UserError::new(&["scopes"], message).code(code);
    if asked.iter().any(|scope| !known_scope(scope)) {
        errors.push(error(
            "The requested list of scopes to revoke includes invalid handles.",
            "UNKNOWN_SCOPES",
        ));
    } else if asked.iter().any(|scope| declared.required.contains(scope)) {
        errors.push(error(
            "Required scopes cannot be revoked.",
            "CANNOT_REVOKE_REQUIRED_SCOPES",
        ));
    } else if asked.iter().any(|scope| !declared.optional.contains(scope)) {
        errors.push(error(
            "Cannot revoke optional scopes that haven't been declared.",
            "CANNOT_REVOKE_UNDECLARED_SCOPES",
        ));
    } else if asked.iter().any(|scope| {
        // A `read_x` that stays implied by a `write_x` kept.
        scope.starts_with("read_")
            && declared.granted.iter().any(|granted| {
                !asked.contains(granted) && scopes::implied(granted).as_deref() == Some(scope)
            })
    }) {
        errors.push(error(
            "Already granted implied scopes cannot be revoked.",
            "CANNOT_REVOKE_IMPLIED_SCOPES",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"revoked": null}), &errors));
    }
    let revoked: Vec<String> = asked
        .iter()
        .filter(|scope| declared.granted.contains(scope))
        .cloned()
        .collect();
    if !revoked.is_empty() {
        let kept: Vec<String> = declared
            .granted
            .iter()
            .filter(|scope| !revoked.contains(scope))
            .cloned()
            .collect();
        change_scopes(m.state_mut(), &declared.installation, &kept);
    }
    let revoked: Vec<Value> = revoked.iter().map(|scope| access_scope(scope)).collect();
    Ok(payload(json!({"revoked": revoked}), &[]))
}

/// `appDowngradeAccessScopes` (2027-01).
fn app_downgrade_access_scopes(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let asked = string_list(ctx, "scopes");
    let Some(declared) = declared(m.state()) else {
        let error =
            UserError::new(&[], "The app isn't installed on the shop.").code("APP_NOT_INSTALLED");
        return Ok(payload(json!({"downgraded": null}), &[error]));
    };
    let error = |message: &str, code: &str| UserError::new(&["scopes"], message).code(code);
    let failure = if asked.iter().any(|scope| !known_scope(scope)) {
        Some(error(
            "The list of scopes to downgrade includes invalid handles.",
            "UNKNOWN_SCOPES",
        ))
    } else if asked
        .iter()
        .any(|scope| !scope.starts_with("write_") || scopes::implied(scope).is_none())
    {
        Some(error(
            "Only write scopes that have a matching read scope can be downgraded.",
            "CANNOT_DOWNGRADE_NON_WRITE_SCOPES",
        ))
    } else if asked.iter().any(|scope| declared.required.contains(scope)) {
        Some(error(
            "Scopes that are declared as required can't be downgraded.",
            "CANNOT_DOWNGRADE_REQUIRED_SCOPES",
        ))
    } else if asked.iter().any(|scope| !declared.optional.contains(scope)) {
        Some(error(
            "Only scopes that the app declares as optional can be downgraded.",
            "CANNOT_DOWNGRADE_UNDECLARED_SCOPES",
        ))
    } else {
        None
    };
    if let Some(error) = failure {
        return Ok(payload(json!({"downgraded": null}), &[error]));
    }
    let downgraded: Vec<String> = asked
        .iter()
        .filter(|scope| declared.granted.contains(scope))
        .cloned()
        .collect();
    if !downgraded.is_empty() {
        let mut kept: Vec<String> = Vec::new();
        for scope in &declared.granted {
            let scope = if downgraded.contains(scope) {
                scopes::implied(scope).unwrap_or_else(|| scope.clone())
            } else {
                scope.clone()
            };
            if !kept.contains(&scope) {
                kept.push(scope);
            }
        }
        change_scopes(m.state_mut(), &declared.installation, &kept);
    }
    let downgraded: Vec<Value> = downgraded.iter().map(|scope| access_scope(scope)).collect();
    Ok(payload(json!({"downgraded": downgraded}), &[]))
}

/// Whether a handle is an access scope of a version lsf knows.
fn known_scope(scope: &str) -> bool {
    scopes::ALL_SCOPES.contains(&scope)
}

fn string_list(ctx: &FieldContext<'_>, name: &str) -> Vec<String> {
    ctx.arg(name)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|scope| scope.trim().to_string())
        .collect()
}

/// The key of the record of a delegate token: made of the token, which the record does not
/// keep.
fn delegate_key(token: &str) -> String {
    format!("delegate-access-token/{}", stable_id("delegate", token))
}

fn delegate_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = |name: &str| ctx.arg("input").and_then(|input| input.get(name));
    let asked: Vec<String> = input("delegateAccessScope")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|scope| scope.trim().to_string())
        .filter(|scope| !scope.is_empty())
        .collect();
    let expires_in = input("expiresIn").and_then(Value::as_i64);
    let parent = m.grant().cloned();
    let now = m.now();
    let refuse = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"delegateAccessToken": null}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    if parent
        .as_ref()
        .is_some_and(|grant| grant.source == GrantSource::Delegate)
    {
        return refuse(
            &[],
            "The parent access token can't be a delegate token.",
            "DELEGATE_ACCESS_TOKEN",
        );
    }
    if asked.is_empty() {
        return refuse(
            &["input", "delegateAccessScope"],
            "The access scope can't be empty.",
            "EMPTY_ACCESS_SCOPE",
        );
    }
    let unknown: Vec<&str> = asked
        .iter()
        .map(String::as_str)
        .filter(|scope| {
            !known_scope(scope)
                || parent
                    .as_ref()
                    .is_some_and(|grant| !scopes::covers(&grant.scopes, scope))
        })
        .collect();
    if !unknown.is_empty() {
        // Shopify names the code ("Unknown scopes."), not the message: lsf says which, and
        // that a scope the app was not granted cannot be delegated ("You can only delegate
        // scopes your app was already granted").
        return refuse(
            &["input", "delegateAccessScope"],
            &format!(
                "Unknown scopes: {} (a delegate token gets only scopes the app was granted)",
                unknown.join(", ")
            ),
            "UNKNOWN_SCOPES",
        );
    }
    if expires_in.is_some_and(|seconds| seconds <= 0) {
        return refuse(
            &["input", "expiresIn"],
            "The expires_in value must be greater than 0.",
            "NEGATIVE_EXPIRES_IN",
        );
    }
    let parent_expiry = parent.as_ref().and_then(|grant| grant.expires_at);
    let asked_expiry = expires_in.map(|seconds| now + Duration::seconds(seconds));
    if let (Some(asked), Some(parent)) = (asked_expiry, parent_expiry)
        && asked > parent
    {
        return refuse(
            &["input", "expiresIn"],
            "The delegate token can't expire after the parent token.",
            "EXPIRES_AFTER_PARENT",
        );
    }
    let expires_at = asked_expiry.or(parent_expiry);
    let state = m.state_mut();
    let count = state.count("DelegateAccessToken");
    let token = format!(
        "shppa_{}",
        value::uuid(&format!(
            "delegate/{}/{count}/{}",
            state.domain(),
            crate::webhooks::sign::new_id()
        ))
        .replace('-', "")
    );
    let created = state.timestamp();
    let expires_in = expires_at.map(|at| (at - now).num_seconds().max(0));
    let key = delegate_key(&token);
    state.insert_as(
        key.clone(),
        value::object(json!({
            "__typename": "DelegateAccessToken",
            "accessScopes": asked,
            "createdAt": created,
            "expiresIn": expires_in,
        })),
    );
    emit_effect(
        state,
        &key,
        &AuthEffect::Delegate {
            token: token.clone(),
            scopes: asked.clone(),
            expires_at,
        },
    );
    Ok(payload(
        json!({"delegateAccessToken": {
            "accessToken": token,
            "accessScopes": asked,
            "createdAt": created,
            "expiresIn": expires_in,
        }}),
        &[],
    ))
}

fn delegate_destroy(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let token = ctx
        .arg_str("accessToken")
        .unwrap_or_default()
        .trim()
        .to_string();
    let refuse = |message: &str, code: &str| {
        Ok(payload(
            json!({"status": null}),
            &[UserError::new(&["accessToken"], message).code(code)],
        ))
    };
    if m.grant()
        .is_some_and(|grant| grant.source == GrantSource::Delegate)
    {
        return refuse(
            "Access denied: a delegate token cannot destroy delegate tokens.",
            "ACCESS_DENIED",
        );
    }
    let key = delegate_key(&token);
    if !m.state().contains(&key) {
        if token.starts_with("shpat_") {
            return refuse(
                "Cannot delete parent access token.",
                "CAN_ONLY_DELETE_DELEGATE_TOKENS",
            );
        }
        return refuse("Access token not found.", "ACCESS_TOKEN_NOT_FOUND");
    }
    let state = m.state_mut();
    state.remove(&key);
    emit_effect(state, &key, &AuthEffect::Destroy { token });
    Ok(payload(json!({"status": true}), &[]))
}
