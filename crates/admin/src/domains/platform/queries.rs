//! The queries of apps, installations and staff members: `app`, `appByHandle`, `appByKey`,
//! `appInstallation`, `appInstallations`, `currentAppInstallation`, `staffMember`,
//! `staffMembers`, `currentStaffMember`, and what their objects derive.
//!
//! - **Apps**: the app of the server is the only one lsf knows (`App`, seeded from
//!   `shopify.app.toml`). What its listing on the App Store would give (an icon, a banner, a
//!   category, a pricing summary) is lsf's: an app in development has no listing. Its
//!   `launchUrl` is its installation's.
//! - **Access scopes**: `AccessScope.description` is made from the handle (`write_products`:
//!   "Modify products"), lsf's wording: Shopify's text for each scope is not published.
//! - **Staff**: the shop owner is `StaffMember/1`, the user of the online tokens of
//!   `POST /__lsf/admin/session-token` (`auth::tokens::AssociatedUser::local`); another user an
//!   online token acts for is a staff member made from the token's `associated_user`.
//!   `currentStaffMember` is the token's user, `null` for an offline token, which acts for no
//!   staff member.

use serde_json::{Value, json};

use crate::auth::tokens::AssociatedUser;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Obj, Registry, ShopResolver};
use crate::state::{Record, value};

pub fn register(registry: &mut Registry) {
    registry
        .field("AccessScope", "description", access_scope_description)
        .query("appByKey", app_by_key)
        .query("appInstallations", app_installations)
        .query("currentStaffMember", current_staff_member)
        .query("financeKycInformation", finance_kyc_information)
        .query("staffMember", staff_member)
        .field("App", "appStoreAppUrl", null_field)
        .field("App", "appStoreDeveloperUrl", null_field)
        .field("App", "banner", app_image)
        .field("App", "icon", app_image)
        .field("App", "description", app_description)
        .field("App", "developerName", app_developer_name)
        .field("App", "developerUrl", app_developer_url)
        .field("App", "feedback", null_field)
        .field("App", "installUrl", null_field)
        .field("App", "launchUrl", app_launch_url)
        .field("App", "pricingDetails", null_field)
        .field("App", "pricingDetailsSummary", app_pricing_summary)
        .field("App", "privacyPolicyUrl", null_field)
        .field("App", "publicCategory", app_public_category)
        .field("App", "uninstallMessage", app_uninstall_message)
        .field("App", "uninstallUrl", null_field)
        .field("AppInstallation", "uninstallUrl", null_field)
        .field("StaffMember", "avatar", staff_avatar)
        .field("StaffMember", "initials", staff_initials)
        .field("StaffMember", "privateData", staff_private_data);
}

/// What an `AccessScope` says of itself: "Read products", "Modify customers", lsf's wording.
pub fn scope_description(handle: &str) -> String {
    let (prefix, rest) = handle
        .strip_prefix("unauthenticated_")
        .map_or(("", handle), |rest| (" (Storefront API)", rest));
    let (verb, resource) = if let Some(resource) = rest.strip_prefix("read_") {
        ("Read", resource)
    } else if let Some(resource) = rest.strip_prefix("write_") {
        ("Modify", resource)
    } else {
        ("Access", rest)
    };
    format!("{verb} {}{prefix}", resource.replace('_', " "))
}

fn access_scope_description(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let record = r.map_of(object);
    if let Some(description) = record.and_then(|record| record.get("description"))
        && description.is_string()
    {
        return Ok(Output::Value(description.clone()));
    }
    let handle = record
        .and_then(|record| record.get("handle"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(Output::Value(json!(scope_description(handle))))
}

fn null_field(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    match r
        .map_of(object)
        .and_then(|record| record.get(ctx.field_name()))
    {
        Some(value) if !value.is_null() => r.generic(ctx, object),
        _ => Ok(Output::Null),
    }
}

fn text_field(r: &ShopResolver<'_>, object: &Obj, field: &str) -> Option<String> {
    r.map_of(object)
        .and_then(|record| record.get(field))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A field the record has, else `fallback`.
fn or_value(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
    fallback: Value,
) -> Result<Output<Obj>, FieldError> {
    match r
        .map_of(object)
        .and_then(|record| record.get(ctx.field_name()))
    {
        Some(value) if !value.is_null() => r.generic(ctx, object),
        _ => r.convert(ctx, &fallback, true),
    }
}

/// An app in development has no listing image: lsf draws a neutral square, as a data URL (the
/// storefront and the admin have no network to fetch one).
fn app_image(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\"><rect width=\"64\" height=\"64\" rx=\"12\" fill=\"#e7e5e1\"/></svg>";
    let url = format!(
        "data:image/svg+xml;base64,{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, svg)
    );
    let title = text_field(r, object, "title").unwrap_or_default();
    or_value(
        r,
        ctx,
        object,
        json!({
            "id": null,
            "url": url,
            "altText": title,
            "width": 64,
            "height": 64,
            "thumbhash": null,
        }),
    )
}

fn app_description(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    or_value(r, ctx, object, Value::Null)
}

fn app_developer_name(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let title = text_field(r, object, "title").unwrap_or_default();
    or_value(r, ctx, object, json!(title))
}

/// The app's own URL: its installation's launch URL stands for the developer's site.
fn app_developer_url(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let url = installation_launch_url(r, object);
    or_value(r, ctx, object, json!(url))
}

fn installation_launch_url(r: &ShopResolver<'_>, object: &Obj) -> String {
    let installation = text_field(r, object, "installation")
        .and_then(|key| r.state().get(&key))
        .and_then(|record| record.get("launchUrl"))
        .and_then(Value::as_str)
        .map(str::to_string);
    installation.unwrap_or_else(|| format!("https://{}/admin/apps", r.state().domain()))
}

fn app_launch_url(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let url = installation_launch_url(r, object);
    or_value(r, ctx, object, json!(url))
}

fn app_pricing_summary(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    // An app in development is not listed: it has no price on the App Store.
    or_value(r, ctx, object, json!("Free"))
}

fn app_public_category(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    // `PRIVATE`: "The app's public category is private" — an app not on the App Store.
    or_value(r, ctx, object, json!("PRIVATE"))
}

fn app_uninstall_message(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    or_value(r, ctx, object, json!(""))
}

/// `appByKey(apiKey:)`: the app whose client id it is.
fn app_by_key(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let key = ctx.arg_str("apiKey").unwrap_or_default();
    Ok(r.find("App", |app| {
        app.get("apiKey").and_then(Value::as_str) == Some(key)
    })
    .map_or(Output::Null, Output::Object))
}

/// `appInstallations`: the installation of the app, the only one lsf knows, whatever the
/// `category` and `privacy` asked (a development app is private).
fn app_installations(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, r.records_of("AppInstallation"))
}

/// `financeKycInformation`: the shop's KYC information as Shopify Payments has it. A
/// development store has no Shopify Payments account: `null`, unless a record says otherwise
/// (what `finance_kyc_information/update` is built from).
fn finance_kyc_information(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let key = r
        .state()
        .keys("FinanceKycInformation")
        .next()
        .map(str::to_string);
    Ok(key.map_or(Output::Null, |key| Output::Object(Obj::record(&key))))
}

// ---- staff -------------------------------------------------------------------------------

/// The `StaffMember` record of a user of the shop.
pub fn staff_record(user: &AssociatedUser, domain: &str, created: &str) -> Value {
    let name = format!("{} {}", user.first_name, user.last_name)
        .trim()
        .to_string();
    json!({
        "__typename": "StaffMember",
        "id": super::staff_gid(user.id),
        "firstName": user.first_name,
        "lastName": user.last_name,
        "name": name,
        "email": user.email,
        "phone": null,
        "locale": user.locale,
        "isShopOwner": user.account_owner,
        "accountType": if user.collaborator { "COLLABORATOR" } else { "REGULAR" },
        "active": true,
        "exists": true,
        "lsfCreatedAt": created,
        "lsfShop": domain,
    })
}

/// The staff member of the token, a record when the shop has one, else made from the token's
/// user.
fn token_staff(r: &ShopResolver<'_>) -> Option<Obj> {
    let user = r.grant()?.user.as_ref()?;
    let key = super::staff_gid(user.id);
    if let Some(found) = r.reference(&key, "StaffMember") {
        return Some(found);
    }
    let record = value::object(staff_record(
        user,
        r.state().domain(),
        &crate::state::seed::Seed::epoch_string(),
    ));
    Some(Obj::built("StaffMember", record))
}

fn current_staff_member(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(token_staff(r).map_or(Output::Null, Output::Object))
}

/// `staffMember(id:)`: the one named, else the token's.
fn staff_member(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let found = match ctx.arg_str("id") {
        Some(id) => r.reference(id, "StaffMember").or_else(|| {
            let user = r.grant()?.user.as_ref()?;
            (super::staff_gid(user.id) == id)
                .then(|| token_staff(r))
                .flatten()
        }),
        None => token_staff(r),
    };
    Ok(found.map_or(Output::Null, Output::Object))
}

fn staff_initials(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let initials: Vec<String> = ["firstName", "lastName"]
        .iter()
        .filter_map(|field| text_field(r, object, field))
        .filter_map(|name| name.chars().next())
        .map(|initial| initial.to_uppercase().to_string())
        .collect();
    Ok(Output::Value(json!(initials)))
}

/// The avatar of a staff member without a picture: Shopify's default is an image of their
/// initials; lsf's is a neutral circle, as a data URL.
fn staff_avatar(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 40 40\"><circle cx=\"20\" cy=\"20\" r=\"20\" fill=\"#c9c6c0\"/></svg>";
    let url = format!(
        "data:image/svg+xml;base64,{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, svg)
    );
    let name = text_field(r, object, "name").unwrap_or_default();
    r.convert(
        ctx,
        &json!({"id": null, "url": url, "altText": name, "width": 40, "height": 40, "thumbhash": null}),
        true,
    )
}

fn staff_private_data(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let created = text_field(r, object, "lsfCreatedAt")
        .unwrap_or_else(crate::state::seed::Seed::epoch_string);
    let id = text_field(r, object, "id")
        .and_then(|id| crate::state::parse_gid(&id).map(|(_, n)| n))
        .unwrap_or(1);
    let private: Record = value::object(json!({
        "accountSettingsUrl": format!("https://{}/admin/settings/account/{id}", r.state().domain()),
        "createdAt": created,
        "permissions": [],
    }));
    Ok(Output::Object(Obj::built(ctx.ty().named(), private)))
}
