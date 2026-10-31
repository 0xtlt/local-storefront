//! Companies and their locations (B2B): `companyCreate`, `companyUpdate`, `companyDelete`,
//! `companiesDelete`, `companyLocationCreate`, `companyLocationUpdate`, `companyLocationDelete`,
//! `companyLocationsDelete`, `companyLocationAssignAddress`, `companyAddressDelete`,
//! `companyLocationTaxSettingsUpdate`, `companyLocationAssignTaxExemptions`,
//! `companyLocationRevokeTaxExemptions`.
//!
//! The records are the seed's (`seed_company`): a company has Shopify's two contact roles
//! ("Location admin", "Ordering only", the default), with the ids the seed gives them; a
//! location its addresses inline (`CompanyAddress`, with an id), its tax settings, its role
//! assignments. As shopify.dev's reference of 2026-10 and the Help Center describe them:
//!
//! - `companyCreate` creates the company, a location ("When you create a company, one company
//!   location is automatically created", Help Center; named after the company and without an
//!   address when `companyLocation` is not given, as the "required fields only" example shows),
//!   and the contact given, which is the main contact (the "Create a company" example) and has
//!   the "Ordering only" role at the location ("The main contact is assigned ordering only
//!   permission by default", Help Center). Without a name: `["input", "company", "name"]` "Name
//!   must exist", `REQUIRED` (the example).
//! - `billingSameAsShipping` makes one address both, with one id (the `company_locations/create`
//!   example's addresses share theirs).
//! - Deleting a company deletes its locations, contacts and roles; its customers stay.
//! - Events: `companies/create`, `company_locations/create`, then the contact's (see
//!   [`super::contacts`]); `companies/update`; `company_locations/update` for an address or tax
//!   settings; the `…/delete` topics, a company's contacts and locations before it. Their order
//!   is lsf's: Shopify does not document it.
//!
//! Messages Shopify does not publish are marked where they are written; "Resource requested
//! does not exist." (`RESOURCE_NOT_FOUND`) is the one lsf gives every id that names nothing.

use lsf_core::store::Address;
use lsf_core::store::reference::country_name;
use serde_json::{Map, Value, json};

use super::contacts::{self, delete_contact};
use super::{
    CONTACT_ROLES, contact_role_gid_of, field_path, gids_of, given_text, primary_locale,
    shop_currency,
};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, parse_gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("companyCreate", company_create)
        .mutation("companyUpdate", company_update)
        .mutation("companyDelete", company_delete)
        .mutation("companiesDelete", companies_delete)
        .mutation("companyLocationCreate", location_create)
        .mutation("companyLocationUpdate", location_update)
        .mutation("companyLocationDelete", location_delete)
        .mutation("companyLocationsDelete", locations_delete)
        .mutation("companyLocationAssignAddress", location_assign_address)
        .mutation("companyAddressDelete", address_delete)
        .mutation("companyLocationTaxSettingsUpdate", tax_settings_update)
        .mutation("companyLocationAssignTaxExemptions", assign_tax_exemptions)
        .mutation("companyLocationRevokeTaxExemptions", revoke_tax_exemptions);
}

/// The error of an id that names nothing of the type expected.
pub fn not_found(field: &[&str]) -> UserError {
    UserError::new(field, "Resource requested does not exist.").code("RESOURCE_NOT_FOUND")
}

/// The id argument `name` when it names a record of `type_name`.
pub fn record_arg(
    state: &ShopState,
    ctx: &FieldContext<'_>,
    name: &str,
    type_name: &str,
) -> Result<String, UserError> {
    let id = ctx.arg_str(name).unwrap_or_default();
    if state.type_of(id) == Some(type_name) {
        Ok(id.to_string())
    } else {
        Err(not_found(&[name]))
    }
}

/// The object argument `name`.
fn object_arg(ctx: &FieldContext<'_>, name: &str) -> Map<String, Value> {
    ctx.arg(name)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The company whose `externalId` is `external`, other than `except`.
fn external_id_taken(state: &ShopState, external: &str, except: Option<&str>) -> bool {
    state.keys("Company").any(|key| {
        Some(key) != except
            && state
                .get(key)
                .and_then(|company| company.get("externalId"))
                .and_then(Value::as_str)
                == Some(external)
    })
}

/// Checks a `CompanyInput` (`path` is where it is in the arguments). `creating`: the name is
/// required.
fn check_company(
    state: &ShopState,
    input: &Map<String, Value>,
    path: &[&str],
    except: Option<&str>,
    creating: bool,
    errors: &mut Vec<UserError>,
) {
    match given_text(input, "name") {
        Some(Some(name)) if name.chars().count() > 255 => errors.push(
            // Rails' wording, with the payload's code.
            UserError::new(
                &field_path(path, "name"),
                "Name is too long (maximum is 255 characters)",
            )
            .code("TOO_LONG"),
        ),
        Some(Some(_)) => {}
        // The `companyCreate` example's error.
        None if creating => errors
            .push(UserError::new(&field_path(path, "name"), "Name must exist").code("REQUIRED")),
        Some(None) => errors
            .push(UserError::new(&field_path(path, "name"), "Name must exist").code("REQUIRED")),
        None => {}
    }
    if let Some(Some(external)) = given_text(input, "externalId")
        && external_id_taken(state, &external, except)
    {
        // Not published: Rails' wording, with the payload's code.
        errors.push(
            UserError::new(
                &field_path(path, "externalId"),
                "External ID has already been taken",
            )
            .code("TAKEN"),
        );
    }
}

/// Sets the fields of a company from a `CompanyInput`: those given.
fn apply_company(record: &mut Record, input: &Map<String, Value>) {
    for field in ["name", "externalId", "note"] {
        if let Some(text) = given_text(input, field) {
            record.insert(field.into(), json!(text));
        }
    }
    if let Some(since) = input.get("customerSince").and_then(Value::as_str) {
        record.insert("customerSince".into(), json!(since));
    }
}

/// A `CompanyAddressInput`, checked: `countryCode` is required (`CompanyAddress.countryCode` is
/// non-null).
fn check_address(input: &Map<String, Value>, path: &[&str], errors: &mut Vec<UserError>) {
    if given_text(input, "countryCode").flatten().is_none() {
        let field: Vec<&str> = path.iter().copied().chain(["countryCode"]).collect();
        // Not published: Rails' wording, with the payload's code.
        errors.push(UserError::new(&field, "Country code can't be blank").code("BLANK"));
    }
}

/// A `CompanyAddress` from its input, with the id `id`, on top of `base` (the address it
/// replaces), in the seed's shape (`company_address`).
fn company_address(
    input: &Map<String, Value>,
    id: &str,
    base: Option<&Value>,
    company: &str,
    now: &str,
) -> Value {
    let field = |name: &str| {
        given_text(input, name).unwrap_or_else(|| {
            base.and_then(|base| base.get(name))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
    };
    let code = field("countryCode")
        .unwrap_or_default()
        .to_ascii_uppercase();
    let zone = field("zoneCode").map(|zone| zone.to_ascii_uppercase());
    let address = Address {
        first_name: field("firstName").unwrap_or_default(),
        last_name: field("lastName").unwrap_or_default(),
        address1: field("address1").unwrap_or_default(),
        address2: field("address2").unwrap_or_default(),
        city: field("city").unwrap_or_default(),
        // Shopify writes the zone's name (`QC` is `Quebec`): lsf has no table of zones and
        // keeps the code *(chosen)*.
        province: zone.clone().unwrap_or_default(),
        province_code: zone.clone().unwrap_or_default(),
        country: country_name(&code).unwrap_or(&code).to_string(),
        country_code: code.clone(),
        zip: field("zip").unwrap_or_default(),
        phone: field("phone").unwrap_or_default(),
        ..Address::default()
    };
    let recipient = field("recipient").map_or_else(
        || value::full_name(&address.first_name, &address.last_name),
        |recipient| json!(recipient),
    );
    let created = base
        .and_then(|base| base.get("createdAt").cloned())
        .unwrap_or_else(|| json!(now));
    json!({
        "id": id,
        "address1": address.address1,
        "address2": value::text(&address.address2),
        "city": value::text(&address.city),
        "province": value::text(&address.province),
        "zoneCode": zone,
        "country": value::text(&address.country),
        "countryCode": code,
        "zip": value::text(&address.zip),
        "phone": value::text(&address.phone),
        "firstName": value::text(&address.first_name),
        "lastName": value::text(&address.last_name),
        "recipient": recipient,
        "companyName": company,
        "formattedAddress": value::formatted(&address),
        "formattedArea": value::formatted_area(&address),
        "createdAt": created,
        "updatedAt": now,
    })
}

/// The name of a company.
fn company_name(state: &ShopState, company: &str) -> String {
    state
        .get(company)
        .and_then(|record| record.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Checks a `CompanyLocationInput` (`path` is where it is in the arguments). `named`: the name
/// is required.
fn check_location(
    input: &Map<String, Value>,
    path: &[&str],
    named: bool,
    errors: &mut Vec<UserError>,
) {
    if named && given_text(input, "name").flatten().is_none() {
        // As `companyCreate`'s name.
        errors.push(UserError::new(&field_path(path, "name"), "Name must exist").code("REQUIRED"));
    }
    for field in ["shippingAddress", "billingAddress"] {
        if let Some(address) = input.get(field).and_then(Value::as_object) {
            check_address(address, &field_path(path, field), errors);
        }
    }
}

/// The `buyerExperienceConfiguration` of an input, as `BuyerExperienceConfiguration`.
fn buyer_experience(input: &Map<String, Value>) -> Value {
    let flag = |name: &str| input.get(name).and_then(Value::as_bool).unwrap_or(false);
    let terms = input.get("paymentTermsTemplateId").and_then(Value::as_str);
    json!({
        "checkoutToDraft": flag("checkoutToDraft"),
        "editableShippingAddress": flag("editableShippingAddress"),
        "payNowOnly": !flag("checkoutToDraft") && terms.is_none(),
        "paymentTermsTemplate": terms,
        "deposit": null,
    })
}

/// Sets the plain fields of a location from an input: those given.
fn apply_location(record: &mut Record, input: &Map<String, Value>) {
    for field in ["name", "externalId", "note", "phone", "locale"] {
        if let Some(text) = given_text(input, field) {
            record.insert(field.into(), json!(text));
        }
    }
    if let Some(configuration) = input
        .get("buyerExperienceConfiguration")
        .and_then(Value::as_object)
    {
        record.insert(
            "buyerExperienceConfiguration".into(),
            buyer_experience(configuration),
        );
    }
}

/// Creates a location of `company` from a checked `CompanyLocationInput`, named `name` when the
/// input names none. Emits `company_locations/create`. Returns its GID.
fn create_location(
    state: &mut ShopState,
    company: &str,
    input: &Map<String, Value>,
    name: &str,
) -> String {
    let id = state.new_gid("CompanyLocation");
    let now = state.timestamp();
    let company_name = company_name(state, company);
    let address = |state: &mut ShopState, field: &str| {
        input.get(field).and_then(Value::as_object).map(|address| {
            let id = state.new_gid("CompanyAddress");
            company_address(address, &id, None, &company_name, &now)
        })
    };
    let shipping = address(state, "shippingAddress");
    let billing = if input.get("billingSameAsShipping").and_then(Value::as_bool) == Some(true) {
        shipping.clone()
    } else {
        address(state, "billingAddress")
    };
    let registration = given_text(input, "taxRegistrationId").flatten();
    let exemptions = input
        .get("taxExemptions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let exempt = input
        .get("taxExempt")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut record = value::object(json!({
        "__typename": "CompanyLocation",
        "id": id,
        "name": name,
        "externalId": null,
        "company": company,
        "locale": primary_locale(state),
        "currency": shop_currency(state),
        "phone": null,
        "note": null,
        "buyerExperienceConfiguration": null,
        "shippingAddress": shipping,
        "billingAddress": billing,
        "taxSettings": {
            "taxRegistrationId": registration,
            "taxExempt": exempt,
            "taxExemptions": exemptions,
        },
        "taxRegistrationId": registration,
        "taxExemptions": exemptions,
        "ordersCount": {"count": 0, "precision": "EXACT"},
        "orderCount": 0,
        "totalSpent": value::money_v2(0, &shop_currency(state)),
        "roleAssignments": [],
        "createdAt": now,
        "updatedAt": now,
        "metafields": [],
    }));
    apply_location(&mut record, input);
    let location = state.insert(record);
    if let Some(Value::Array(locations)) = state
        .get_mut(company)
        .and_then(|record| record.get_mut("locations"))
    {
        locations.push(json!(location));
    }
    state.emit("company_locations/create", &location);
    location
}

/// `companyCreate`.
fn company_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = object_arg(ctx, "input");
    let object = |name: &str| input.get(name).and_then(Value::as_object).cloned();
    let company_input = object("company").unwrap_or_default();
    let location_input = object("companyLocation");
    let contact_input = object("companyContact");
    let state = mutation.state();
    let mut errors = Vec::new();
    check_company(
        state,
        &company_input,
        &["input", "company"],
        None,
        true,
        &mut errors,
    );
    if let Some(location) = &location_input {
        check_location(location, &["input", "companyLocation"], false, &mut errors);
        if location.contains_key("buyerExperienceConfiguration") {
            mutation.note(
                "companyCreate: lsf keeps `buyerExperienceConfiguration` but does not apply it \
                 at the checkout",
            );
        }
    }
    if let Some(contact) = &contact_input {
        contacts::check_contact(
            mutation.state(),
            contact,
            &["input", "companyContact"],
            None,
            true,
            &mut errors,
        );
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"company": null}), &errors));
    }

    let state = mutation.state_mut();
    let company = state.new_gid("Company");
    let number = parse_gid(&company).map_or(0, |(_, id)| id);
    let now = state.timestamp();
    let roles: Vec<String> = CONTACT_ROLES
        .iter()
        .map(|(key, name)| {
            state.insert(value::object(json!({
                "__typename": "CompanyContactRole",
                "id": contact_role_gid_of(number, key),
                "name": name,
                "note": null,
            })))
        })
        .collect();
    let mut record = value::object(json!({
        "__typename": "Company",
        "id": company,
        "name": "",
        "externalId": null,
        "note": null,
        "customerSince": now,
        "createdAt": now,
        "updatedAt": now,
        "locations": [],
        "contacts": [],
        "mainContact": null,
        "contactRoles": roles,
        "defaultRole": contact_role_gid_of(number, CONTACT_ROLES[1].0),
        "totalSpent": value::money_v2(0, &shop_currency(state)),
        "metafields": [],
    }));
    apply_company(&mut record, &company_input);
    let name = record
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    state.insert(record);
    state.emit("companies/create", &company);
    let location_input = location_input.unwrap_or_default();
    let location_name = given_text(&location_input, "name")
        .flatten()
        .unwrap_or(name);
    let location = create_location(state, &company, &location_input, &location_name);
    if let Some(contact_input) = &contact_input {
        let contact = contacts::create_contact(state, &company, contact_input);
        contacts::set_main_contact(state, &company, Some(&contact));
        let role = contact_role_gid_of(number, CONTACT_ROLES[1].0);
        contacts::assign_role(state, &contact, &role, &location);
    }
    Ok(payload(json!({"company": company}), &[]))
}

/// `companyUpdate`.
fn company_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let company = match record_arg(state, ctx, "companyId", "Company") {
        Ok(company) => company,
        Err(error) => return Ok(payload(json!({"company": null}), &[error])),
    };
    let input = object_arg(ctx, "input");
    let mut errors = Vec::new();
    check_company(
        state,
        &input,
        &["input"],
        Some(&company),
        false,
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"company": null}), &errors));
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&company) {
        apply_company(record, &input);
    }
    // The company's name is in its addresses (`companyName`).
    if input.contains_key("name") {
        let name = company_name(state, &company);
        for location in gids_of(state.get(&company), "locations") {
            if let Some(record) = state.get_mut(&location) {
                for field in ["shippingAddress", "billingAddress"] {
                    if let Some(Value::Object(address)) = record.get_mut(field) {
                        address.insert("companyName".into(), json!(name));
                    }
                }
            }
        }
    }
    state.touch(&company);
    state.emit("companies/update", &company);
    Ok(payload(json!({"company": company}), &[]))
}

/// A record with its `company` embedded, for the payload of a deletion: when the company goes
/// too, it is gone by the time the webhook is built.
pub fn with_company(state: &ShopState, key: &str) -> Option<Record> {
    let mut record = state.get(key).cloned()?;
    let company = record
        .get("company")
        .and_then(Value::as_str)
        .and_then(|company| state.get(company).cloned());
    if let Some(company) = company {
        record.insert("company".into(), Value::Object(company));
    }
    Some(record)
}

/// Deletes a company: its contacts, its locations, its roles, then itself, with the events of
/// each.
fn delete_company(state: &mut ShopState, company: &str) {
    let record = state.get(company).cloned().unwrap_or_default();
    for contact in gids_of(Some(&record), "contacts") {
        delete_contact(state, &contact);
    }
    for location in gids_of(Some(&record), "locations") {
        delete_location(state, &location);
    }
    for role in gids_of(Some(&record), "contactRoles") {
        state.remove(&role);
    }
    super::metafields::remove_all(state, &record);
    state.delete(company, "companies/delete");
}

/// `companyDelete`.
fn company_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let company = match record_arg(mutation.state(), ctx, "id", "Company") {
        Ok(company) => company,
        Err(error) => return Ok(payload(json!({"deletedCompanyId": null}), &[error])),
    };
    delete_company(mutation.state_mut(), &company);
    Ok(payload(json!({"deletedCompanyId": company}), &[]))
}

/// The ids of a list argument, each checked to name a record of `type_name`.
fn ids_arg(
    state: &ShopState,
    ctx: &FieldContext<'_>,
    name: &str,
    type_name: &str,
    errors: &mut Vec<UserError>,
) -> Vec<String> {
    let ids: Vec<String> = ctx
        .arg(name)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    for (index, id) in ids.iter().enumerate() {
        if state.type_of(id) != Some(type_name) {
            errors.push(not_found(&[name, &index.to_string()]));
        }
    }
    ids
}

/// `companiesDelete`.
fn companies_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let mut errors = Vec::new();
    let ids = ids_arg(mutation.state(), ctx, "companyIds", "Company", &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"deletedCompanyIds": null}), &errors));
    }
    let state = mutation.state_mut();
    for company in &ids {
        delete_company(state, company);
    }
    Ok(payload(json!({"deletedCompanyIds": ids}), &[]))
}

/// `companyLocationCreate`.
fn location_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let company = match record_arg(state, ctx, "companyId", "Company") {
        Ok(company) => company,
        Err(error) => return Ok(payload(json!({"companyLocation": null}), &[error])),
    };
    let input = object_arg(ctx, "input");
    let mut errors = Vec::new();
    check_location(&input, &["input"], true, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"companyLocation": null}), &errors));
    }
    let name = given_text(&input, "name").flatten().unwrap_or_default();
    let location = create_location(mutation.state_mut(), &company, &input, &name);
    Ok(payload(json!({"companyLocation": location}), &[]))
}

/// `companyLocationUpdate`.
fn location_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let location = match record_arg(state, ctx, "companyLocationId", "CompanyLocation") {
        Ok(location) => location,
        Err(error) => return Ok(payload(json!({"companyLocation": null}), &[error])),
    };
    let input = object_arg(ctx, "input");
    if let Some(None) = given_text(&input, "name") {
        let error = UserError::new(&["input", "name"], "Name must exist").code("REQUIRED");
        return Ok(payload(json!({"companyLocation": null}), &[error]));
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&location) {
        apply_location(record, &input);
    }
    state.touch(&location);
    state.emit("company_locations/update", &location);
    Ok(payload(json!({"companyLocation": location}), &[]))
}

/// Removes a location, its role assignments (from their contacts too), and takes it out of its
/// company. No event: the caller emits the deletion.
fn remove_location(state: &mut ShopState, location: &str) {
    let record = state.get(location).cloned().unwrap_or_default();
    for assignment in gids_of(Some(&record), "roleAssignments") {
        contacts::unlink_assignment(state, &assignment);
        state.remove(&assignment);
    }
    if let Some(company) = record.get("company").and_then(Value::as_str)
        && let Some(Value::Array(locations)) = state
            .get_mut(company)
            .and_then(|company| company.get_mut("locations"))
    {
        locations.retain(|item| item.as_str() != Some(location));
    }
    super::metafields::remove_all(state, &record);
}

/// Deletes a location and what is its own, and emits `company_locations/delete` with the
/// location's company embedded (it may be going too).
fn delete_location(state: &mut ShopState, location: &str) {
    let Some(snapshot) = with_company(state, location) else {
        return;
    };
    remove_location(state, location);
    state.emit_with("company_locations/delete", location, snapshot);
    state.remove(location);
}

/// `companyLocationDelete`.
fn location_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let location = match record_arg(
        mutation.state(),
        ctx,
        "companyLocationId",
        "CompanyLocation",
    ) {
        Ok(location) => location,
        Err(error) => return Ok(payload(json!({"deletedCompanyLocationId": null}), &[error])),
    };
    delete_location(mutation.state_mut(), &location);
    Ok(payload(json!({"deletedCompanyLocationId": location}), &[]))
}

/// `companyLocationsDelete`.
fn locations_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let mut errors = Vec::new();
    let ids = ids_arg(
        mutation.state(),
        ctx,
        "companyLocationIds",
        "CompanyLocation",
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"deletedCompanyLocationIds": null}), &errors));
    }
    let state = mutation.state_mut();
    for location in &ids {
        delete_location(state, location);
    }
    Ok(payload(json!({"deletedCompanyLocationIds": ids}), &[]))
}

/// `companyLocationAssignAddress`: sets the billing or shipping address (or both, one address)
/// of a location; an address of that type already there keeps its id.
fn location_assign_address(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let location = match record_arg(state, ctx, "locationId", "CompanyLocation") {
        Ok(location) => location,
        Err(error) => return Ok(payload(json!({"addresses": null}), &[error])),
    };
    let input = object_arg(ctx, "address");
    let types: Vec<String> = ctx
        .arg("addressTypes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let mut errors = Vec::new();
    check_address(&input, &["address"], &mut errors);
    if types.is_empty() {
        // Not published: Rails' wording, with the payload's code.
        errors
            .push(UserError::new(&["addressTypes"], "Address types can't be blank").code("BLANK"));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"addresses": null}), &errors));
    }
    let record = state.get(&location).cloned().unwrap_or_default();
    let company = record
        .get("company")
        .and_then(Value::as_str)
        .map(|company| company_name(state, company))
        .unwrap_or_default();
    let field = |kind: &str| match kind {
        "BILLING" => "billingAddress",
        _ => "shippingAddress",
    };
    let existing = types
        .iter()
        .find_map(|kind| {
            record
                .get(field(kind))
                .filter(|address| address.is_object())
        })
        .cloned();
    let state = mutation.state_mut();
    let id = existing
        .as_ref()
        .and_then(|address| address.get("id").and_then(Value::as_str))
        .map_or_else(|| state.new_gid("CompanyAddress"), str::to_string);
    let now = state.timestamp();
    let address = company_address(&input, &id, existing.as_ref(), &company, &now);
    if let Some(record) = state.get_mut(&location) {
        for kind in &types {
            record.insert(field(kind).into(), address.clone());
        }
    }
    state.touch(&location);
    state.emit("company_locations/update", &location);
    let addresses: Vec<&Value> = types.iter().map(|_| &address).collect();
    Ok(payload(json!({"addresses": addresses}), &[]))
}

/// `companyAddressDelete`: the address leaves the locations that have it.
fn address_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("addressId").unwrap_or_default().to_string();
    let state = mutation.state();
    let holders: Vec<String> = state
        .keys("CompanyLocation")
        .filter(|location| {
            state.get(location).is_some_and(|record| {
                ["shippingAddress", "billingAddress"].iter().any(|field| {
                    record
                        .get(*field)
                        .and_then(|address| address.get("id"))
                        .and_then(Value::as_str)
                        == Some(id.as_str())
                })
            })
        })
        .map(str::to_string)
        .collect();
    if holders.is_empty() {
        return Ok(payload(
            json!({"deletedAddressId": null}),
            &[not_found(&["addressId"])],
        ));
    }
    let state = mutation.state_mut();
    for location in &holders {
        if let Some(record) = state.get_mut(location) {
            for field in ["shippingAddress", "billingAddress"] {
                let held = record
                    .get(field)
                    .and_then(|address| address.get("id"))
                    .and_then(Value::as_str)
                    == Some(id.as_str());
                if held {
                    record.insert(field.into(), Value::Null);
                }
            }
        }
        state.touch(location);
        state.emit("company_locations/update", location);
    }
    Ok(payload(json!({"deletedAddressId": id}), &[]))
}

/// Changes the tax settings of a location (`taxSettings` and the deprecated `taxRegistrationId`
/// and `taxExemptions` beside it) and emits `company_locations/update`.
pub(crate) fn change_tax_settings(
    state: &mut ShopState,
    location: &str,
    change: impl FnOnce(&mut Map<String, Value>),
) {
    if let Some(record) = state.get_mut(location) {
        let mut settings = record
            .get("taxSettings")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        change(&mut settings);
        record.insert(
            "taxRegistrationId".into(),
            settings
                .get("taxRegistrationId")
                .cloned()
                .unwrap_or(Value::Null),
        );
        record.insert(
            "taxExemptions".into(),
            settings
                .get("taxExemptions")
                .cloned()
                .unwrap_or_else(|| json!([])),
        );
        record.insert("taxSettings".into(), Value::Object(settings));
    }
    state.touch(location);
    state.emit("company_locations/update", location);
}

/// The exemptions of a list argument.
fn exemptions_arg(ctx: &FieldContext<'_>, name: &str) -> Vec<Value> {
    ctx.arg(name)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Adds and removes exemptions from a list of them.
fn exemptions_changed(current: Option<&Value>, add: &[Value], remove: &[Value]) -> Value {
    let mut list: Vec<Value> = current
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for exemption in add {
        if !list.contains(exemption) {
            list.push(exemption.clone());
        }
    }
    list.retain(|exemption| !remove.contains(exemption));
    json!(list)
}

/// `companyLocationTaxSettingsUpdate`.
fn tax_settings_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let location = match record_arg(
        mutation.state(),
        ctx,
        "companyLocationId",
        "CompanyLocation",
    ) {
        Ok(location) => location,
        Err(error) => return Ok(payload(json!({"companyLocation": null}), &[error])),
    };
    let add = exemptions_arg(ctx, "exemptionsToAssign");
    let remove = exemptions_arg(ctx, "exemptionsToRemove");
    let exempt = ctx.arg("taxExempt").and_then(Value::as_bool);
    let registration = ctx
        .arg("taxRegistrationId")
        .map(|value| value.as_str().map(str::trim).filter(|id| !id.is_empty()));
    change_tax_settings(mutation.state_mut(), &location, |settings| {
        if let Some(registration) = registration {
            settings.insert("taxRegistrationId".into(), json!(registration));
        }
        if let Some(exempt) = exempt {
            settings.insert("taxExempt".into(), json!(exempt));
        }
        let exemptions = exemptions_changed(settings.get("taxExemptions"), &add, &remove);
        settings.insert("taxExemptions".into(), exemptions);
    });
    Ok(payload(json!({"companyLocation": location}), &[]))
}

fn assign_tax_exemptions(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    tax_exemptions(mutation, ctx, true)
}

fn revoke_tax_exemptions(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    tax_exemptions(mutation, ctx, false)
}

/// `companyLocationAssignTaxExemptions` and `companyLocationRevokeTaxExemptions`.
fn tax_exemptions(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    assign: bool,
) -> Result<Value, FieldError> {
    let location = match record_arg(
        mutation.state(),
        ctx,
        "companyLocationId",
        "CompanyLocation",
    ) {
        Ok(location) => location,
        Err(error) => return Ok(payload(json!({"companyLocation": null}), &[error])),
    };
    let given = exemptions_arg(ctx, "taxExemptions");
    change_tax_settings(mutation.state_mut(), &location, |settings| {
        let (add, remove): (&[Value], &[Value]) =
            if assign { (&given, &[]) } else { (&[], &given) };
        let exemptions = exemptions_changed(settings.get("taxExemptions"), add, remove);
        settings.insert("taxExemptions".into(), exemptions);
    });
    Ok(payload(json!({"companyLocation": location}), &[]))
}
