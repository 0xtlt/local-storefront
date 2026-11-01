//! Company contacts and their roles (B2B): `companyContactCreate`, `companyContactUpdate`,
//! `companyContactDelete`, `companyContactsDelete`, `companyContactRemoveFromCompany`,
//! `companyAssignCustomerAsContact`, `companyAssignMainContact`, `companyRevokeMainContact`,
//! and the role assignments: `companyContactAssignRole(s)`, `companyLocationAssignRoles`,
//! `companyContactRevokeRole(s)`, `companyLocationRevokeRoles`.
//!
//! A contact is a customer at a company: `companyContactCreate` "Creates a company contact and
//! the associated customer" (the reference), `companyAssignCustomerAsContact` makes an existing
//! customer one. Its id is the customer's, as the seed and the checkout's
//! `purchasingCompany.contact.id` have it, unless that id is taken (a customer who is a contact
//! of a second company). A role assignment gives the contact a role ("Location admin",
//! "Ordering only") at one location of its company: the locations a contact can order for,
//! which store sync writes back as the customer's `company_locations`.
//!
//! - A contact has one role per location: assigning a second is refused (Shopify's message is
//!   not published).
//! - Deleting a contact (or removing it from its company) leaves its customer; its role
//!   assignments go with it, and the company has no main contact when it was the main one.
//! - Events: `customers/create` (a new customer), `company_contacts/create`;
//!   `company_contacts/update` (and `customers/update` when the customer's fields change);
//!   `company_contacts/delete`; `companies/update` for the main contact;
//!   `company_contact_roles/assign` and `/revoke`. Their order is lsf's.

use serde_json::{Map, Value, json};

use super::companies::{ids_arg, not_found, record_arg, with_company};
use super::customer::{check_fields, create_customer, update_customer};
use super::{field_path, gids_of, given_text, object_arg};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, gid, parse_gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("companyContactCreate", contact_create)
        .mutation("companyContactUpdate", contact_update)
        .mutation("companyContactDelete", contact_delete)
        .mutation("companyContactsDelete", contacts_delete)
        .mutation("companyContactRemoveFromCompany", remove_from_company)
        .mutation("companyAssignCustomerAsContact", assign_customer_as_contact)
        .mutation("companyAssignMainContact", assign_main_contact)
        .mutation("companyRevokeMainContact", revoke_main_contact)
        .mutation("companyContactAssignRole", contact_assign_role)
        .mutation("companyContactAssignRoles", contact_assign_roles)
        .mutation("companyLocationAssignRoles", location_assign_roles)
        .mutation("companyContactRevokeRole", contact_revoke_role)
        .mutation("companyContactRevokeRoles", contact_revoke_roles)
        .mutation("companyLocationRevokeRoles", location_revoke_roles);
}

/// The customer fields of a `CompanyContactInput`.
fn customer_fields(input: &Map<String, Value>) -> Map<String, Value> {
    ["email", "firstName", "lastName", "phone", "locale"]
        .iter()
        .filter_map(|field| Some((field.to_string(), input.get(*field)?.clone())))
        .collect()
}

/// Checks a `CompanyContactInput` (`path` is where it is in the arguments): the customer's
/// fields as a customer's. `existing`: the contact it changes.
pub fn check_contact(
    state: &ShopState,
    input: &Map<String, Value>,
    path: &[&str],
    existing: Option<&str>,
    creating: bool,
    errors: &mut Vec<UserError>,
) {
    let customer = existing
        .and_then(|contact| state.get(contact))
        .and_then(|contact| contact.get("customer"))
        .and_then(Value::as_str);
    let fields = customer_fields(input);
    if creating || !fields.is_empty() {
        check_fields(state, &fields, customer, path, errors);
    }
}

/// A free id for the contact of `customer`: the customer's number, as the seed gives it, else a
/// new one.
fn contact_id(state: &mut ShopState, customer: &str) -> String {
    let own = parse_gid(customer).map(|(_, id)| gid("CompanyContact", id));
    match own {
        Some(id) if !state.contains(&id) => id,
        _ => state.new_gid("CompanyContact"),
    }
}

/// Makes `customer` a contact of `company`, and emits `company_contacts/create`. Returns the
/// contact's GID.
fn link_customer(
    state: &mut ShopState,
    company: &str,
    customer: &str,
    input: &Map<String, Value>,
) -> String {
    let contact = contact_id(state, customer);
    let now = state.timestamp();
    let locale = given_text(input, "locale").flatten().or_else(|| {
        state
            .get(customer)
            .and_then(|record| record.get("locale"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    state.insert(value::object(json!({
        "__typename": "CompanyContact",
        "id": contact,
        "company": company,
        "customer": customer,
        "title": given_text(input, "title").flatten(),
        "locale": locale,
        "isMainContact": false,
        "roleAssignments": [],
        "createdAt": now,
        "updatedAt": now,
    })));
    let push = |record: Option<&mut Record>, field: &str| {
        if let Some(Value::Array(items)) = record.and_then(|record| record.get_mut(field)) {
            items.push(json!(contact));
        }
    };
    push(state.get_mut(company), "contacts");
    push(state.get_mut(customer), "companyContactProfiles");
    state.emit("company_contacts/create", &contact);
    contact
}

/// Creates the customer of a `CompanyContactInput` and its contact at `company`, with their
/// events. Returns the contact's GID.
pub fn create_contact(state: &mut ShopState, company: &str, input: &Map<String, Value>) -> String {
    let customer = create_customer(state, &customer_fields(input));
    link_customer(state, company, &customer, input)
}

/// Makes `contact` (or nobody) the main contact of `company`. No event: the caller emits.
pub fn set_main_contact(state: &mut ShopState, company: &str, contact: Option<&str>) {
    for other in gids_of(state.get(company), "contacts") {
        if let Some(record) = state.get_mut(&other) {
            let main = Some(other.as_str()) == contact;
            if record.get("isMainContact").and_then(Value::as_bool) != Some(main) {
                record.insert("isMainContact".into(), json!(main));
            }
        }
    }
    if let Some(record) = state.get_mut(company) {
        record.insert("mainContact".into(), json!(contact));
    }
}

/// Gives `contact` the role `role` at `location`, and emits `company_contact_roles/assign`.
/// Returns the assignment's GID.
pub fn assign_role(state: &mut ShopState, contact: &str, role: &str, location: &str) -> String {
    let assignment = state.new_gid("CompanyContactRoleAssignment");
    let now = state.timestamp();
    let company = state
        .get(contact)
        .and_then(|record| record.get("company"))
        .cloned()
        .unwrap_or(Value::Null);
    state.insert(value::object(json!({
        "__typename": "CompanyContactRoleAssignment",
        "id": assignment,
        "company": company,
        "companyContact": contact,
        "companyLocation": location,
        "role": role,
        "createdAt": now,
        "updatedAt": now,
    })));
    for owner in [contact, location] {
        if let Some(Value::Array(items)) = state
            .get_mut(owner)
            .and_then(|record| record.get_mut("roleAssignments"))
        {
            items.push(json!(assignment));
        }
    }
    state.emit("company_contact_roles/assign", &assignment);
    assignment
}

/// Takes an assignment out of its contact's and its location's `roleAssignments`; the record
/// stays.
fn unlink_assignment(state: &mut ShopState, assignment: &str) {
    let Some(record) = state.get(assignment).cloned() else {
        return;
    };
    for field in ["companyContact", "companyLocation"] {
        let owner = record.get(field).and_then(Value::as_str);
        if let Some(Value::Array(items)) = owner
            .and_then(|owner| state.get_mut(owner))
            .and_then(|owner| owner.get_mut("roleAssignments"))
        {
            items.retain(|item| item.as_str() != Some(assignment));
        }
    }
}

/// Removes the role assignments of a contact or a location, from their other owner too. No
/// event.
pub fn remove_assignments(state: &mut ShopState, owner: &Record) {
    for assignment in gids_of(Some(owner), "roleAssignments") {
        unlink_assignment(state, &assignment);
        state.remove(&assignment);
    }
}

/// Revokes an assignment and emits `company_contact_roles/revoke` (its payload built from the
/// assignment as it was).
fn revoke(state: &mut ShopState, assignment: &str) {
    let Some(snapshot) = state.get(assignment).cloned() else {
        return;
    };
    unlink_assignment(state, assignment);
    state.emit_with("company_contact_roles/revoke", assignment, snapshot);
    state.remove(assignment);
}

/// Deletes a contact: its role assignments, its place in its company (the main contact no
/// more) and in its customer's profiles. Emits `company_contacts/delete`, the company embedded.
pub fn delete_contact(state: &mut ShopState, contact: &str) {
    let Some(snapshot) = with_company(state, contact) else {
        return;
    };
    let record = state.get(contact).cloned().unwrap_or_default();
    remove_assignments(state, &record);
    let text = |field: &str| {
        record
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    if let Some(company) = text("company") {
        state.pull_key(&company, "contacts", contact);
        let main = state
            .get(&company)
            .and_then(|record| record.get("mainContact"))
            .and_then(Value::as_str)
            == Some(contact);
        if main && let Some(record) = state.get_mut(&company) {
            record.insert("mainContact".into(), Value::Null);
        }
    }
    if let Some(customer) = text("customer")
        && let Some(Value::Array(profiles)) = state
            .get_mut(&customer)
            .and_then(|record| record.get_mut("companyContactProfiles"))
    {
        profiles.retain(|item| item.as_str() != Some(contact));
    }
    state.emit_with("company_contacts/delete", contact, snapshot);
    state.remove(contact);
}

/// The company of a contact.
fn company_of(state: &ShopState, contact: &str) -> Option<String> {
    state
        .get(contact)?
        .get("company")?
        .as_str()
        .map(str::to_string)
}

fn contact_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let company = match record_arg(state, ctx, "companyId", "Company") {
        Ok(company) => company,
        Err(error) => return Ok(payload(json!({"companyContact": null}), &[error])),
    };
    let input = object_arg(ctx, "input");
    let mut errors = Vec::new();
    check_contact(state, &input, &["input"], None, true, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"companyContact": null}), &errors));
    }
    let contact = create_contact(mutation.state_mut(), &company, &input);
    Ok(payload(json!({"companyContact": contact}), &[]))
}

/// `companyContactUpdate`: the contact's `title` and `locale`, its customer's other fields.
fn contact_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let contact = match record_arg(state, ctx, "companyContactId", "CompanyContact") {
        Ok(contact) => contact,
        Err(error) => return Ok(payload(json!({"companyContact": null}), &[error])),
    };
    let input = object_arg(ctx, "input");
    let mut errors = Vec::new();
    check_contact(
        state,
        &input,
        &["input"],
        Some(&contact),
        false,
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"companyContact": null}), &errors));
    }
    let customer = state
        .get(&contact)
        .and_then(|record| record.get("customer"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&contact) {
        for field in ["title", "locale"] {
            if let Some(text) = given_text(&input, field) {
                record.insert(field.into(), json!(text));
            }
        }
    }
    state.touch(&contact);
    state.emit("company_contacts/update", &contact);
    let fields: Map<String, Value> = customer_fields(&input)
        .into_iter()
        .filter(|(field, _)| field != "locale")
        .collect();
    if let Some(customer) = customer.filter(|_| !fields.is_empty()) {
        update_customer(state, &customer, &fields);
    }
    Ok(payload(json!({"companyContact": contact}), &[]))
}

fn contact_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let contact = match record_arg(mutation.state(), ctx, "companyContactId", "CompanyContact") {
        Ok(contact) => contact,
        Err(error) => return Ok(payload(json!({"deletedCompanyContactId": null}), &[error])),
    };
    delete_contact(mutation.state_mut(), &contact);
    Ok(payload(json!({"deletedCompanyContactId": contact}), &[]))
}

/// `companyContactRemoveFromCompany`: as `companyContactDelete` (the customer stays either way).
fn remove_from_company(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let contact = match record_arg(mutation.state(), ctx, "companyContactId", "CompanyContact") {
        Ok(contact) => contact,
        Err(error) => return Ok(payload(json!({"removedCompanyContactId": null}), &[error])),
    };
    delete_contact(mutation.state_mut(), &contact);
    Ok(payload(json!({"removedCompanyContactId": contact}), &[]))
}

fn contacts_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let mut errors = Vec::new();
    let ids = ids_arg(
        mutation.state(),
        ctx,
        "companyContactIds",
        "CompanyContact",
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"deletedCompanyContactIds": null}), &errors));
    }
    let state = mutation.state_mut();
    for contact in &ids {
        delete_contact(state, contact);
    }
    Ok(payload(json!({"deletedCompanyContactIds": ids}), &[]))
}

fn assign_customer_as_contact(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let found = record_arg(state, ctx, "companyId", "Company").and_then(|company| {
        record_arg(state, ctx, "customerId", "Customer").map(|customer| (company, customer))
    });
    let (company, customer) = match found {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"companyContact": null}), &[error])),
    };
    let already = gids_of(state.get(&customer), "companyContactProfiles")
        .iter()
        .any(|contact| company_of(state, contact).as_deref() == Some(company.as_str()));
    if already {
        // Not published: lsf's words, with the payload's code.
        let error = UserError::new(
            &["customerId"],
            "Customer is already a contact of this company",
        )
        .code("TAKEN");
        return Ok(payload(json!({"companyContact": null}), &[error]));
    }
    let contact = link_customer(mutation.state_mut(), &company, &customer, &Map::new());
    Ok(payload(json!({"companyContact": contact}), &[]))
}

fn assign_main_contact(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let found = record_arg(state, ctx, "companyId", "Company").and_then(|company| {
        record_arg(state, ctx, "companyContactId", "CompanyContact")
            .map(|contact| (company, contact))
    });
    let (company, contact) = match found {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"company": null}), &[error])),
    };
    if company_of(state, &contact).as_deref() != Some(company.as_str()) {
        // Not published: lsf's words, with the payload's code.
        let error = UserError::new(
            &["companyContactId"],
            "The company contact does not belong to the company",
        )
        .code("INVALID");
        return Ok(payload(json!({"company": null}), &[error]));
    }
    let state = mutation.state_mut();
    set_main_contact(state, &company, Some(&contact));
    state.touch(&company);
    state.emit("companies/update", &company);
    Ok(payload(json!({"company": company}), &[]))
}

fn revoke_main_contact(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let company = match record_arg(mutation.state(), ctx, "companyId", "Company") {
        Ok(company) => company,
        Err(error) => return Ok(payload(json!({"company": null}), &[error])),
    };
    let state = mutation.state_mut();
    set_main_contact(state, &company, None);
    state.touch(&company);
    state.emit("companies/update", &company);
    Ok(payload(json!({"company": company}), &[]))
}

/// Checks an assignment to make: the contact, the role and the location are of one company,
/// and the contact has no role at the location yet (nor gets one from `pending`, the
/// `(contact, location)`s the same mutation assigns before it). `names` are the input fields
/// the errors point at, under `path`.
fn check_assignment(
    state: &ShopState,
    (contact, role, location): (&str, &str, &str),
    path: &[&str],
    names: (&str, &str, &str),
    pending: &[(String, String)],
    errors: &mut Vec<UserError>,
) {
    let (contact_name, role_name, location_name) = names;
    let company = company_of(state, contact);
    let mut found = true;
    if state.type_of(contact) != Some("CompanyContact") {
        errors.push(not_found(&field_path(path, contact_name)));
        found = false;
    }
    let company_roles = company
        .as_deref()
        .map(|company| gids_of(state.get(company), "contactRoles"))
        .unwrap_or_default();
    if state.type_of(role) != Some("CompanyContactRole")
        || (found && !company_roles.iter().any(|known| known == role))
    {
        errors.push(not_found(&field_path(path, role_name)));
        found = false;
    }
    let location_company = state
        .get_of(location, "CompanyLocation")
        .and_then(|record| record.get("company"))
        .and_then(Value::as_str);
    if location_company.is_none() || (company.is_some() && location_company != company.as_deref()) {
        errors.push(not_found(&field_path(path, location_name)));
        found = false;
    }
    if !found {
        return;
    }
    let assigned = gids_of(state.get(contact), "roleAssignments")
        .iter()
        .filter_map(|assignment| state.get(assignment))
        .any(|assignment| {
            assignment.get("companyLocation").and_then(Value::as_str) == Some(location)
        })
        || pending
            .iter()
            .any(|(other, at)| other == contact && at == location);
    if assigned {
        // Not published: lsf's words, with the payload's code.
        errors.push(
            UserError::new(
                &field_path(path, location_name),
                "Company contact has already been assigned a role in that company location",
            )
            .code("TAKEN"),
        );
    }
}

fn contact_assign_role(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let arg = |name: &str| ctx.arg_str(name).unwrap_or_default().to_string();
    let (contact, role, location) = (
        arg("companyContactId"),
        arg("companyContactRoleId"),
        arg("companyLocationId"),
    );
    let mut errors = Vec::new();
    check_assignment(
        mutation.state(),
        (&contact, &role, &location),
        &[],
        ROLE_FIELDS,
        &[],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(
            json!({"companyContactRoleAssignment": null}),
            &errors,
        ));
    }
    let assignment = assign_role(mutation.state_mut(), &contact, &role, &location);
    Ok(payload(
        json!({"companyContactRoleAssignment": assignment}),
        &[],
    ))
}

/// The input fields of an assignment's contact, role and location, which its errors name.
const ROLE_FIELDS: (&str, &str, &str) = (
    "companyContactId",
    "companyContactRoleId",
    "companyLocationId",
);

/// The `(contact, role, location)` of each item of `rolesToAssign`: the mutation's own contact
/// or location when it has one, else the item's.
fn roles_to_assign(
    ctx: &FieldContext<'_>,
    contact: Option<&str>,
    location: Option<&str>,
) -> Vec<(String, String, String)> {
    let (contact_field, role_field, location_field) = ROLE_FIELDS;
    ctx.arg("rolesToAssign")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|item| {
            let field = |name: &str, own: Option<&str>| {
                own.or_else(|| item.get(name).and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_string()
            };
            (
                field(contact_field, contact),
                field(role_field, None),
                field(location_field, location),
            )
        })
        .collect()
}

/// Makes the assignments `(contact, role, location)` of a plural mutation, checked first: all
/// or none.
fn assign_all(
    mutation: &mut Mutation<'_>,
    wanted: Vec<(String, String, String)>,
    path: &str,
    names: (&str, &str, &str),
) -> Result<Value, FieldError> {
    let mut errors = Vec::new();
    let mut pending = Vec::new();
    for (index, (contact, role, location)) in wanted.iter().enumerate() {
        let position = index.to_string();
        let path: Vec<&str> = if path.is_empty() {
            Vec::new()
        } else {
            vec![path, &position]
        };
        check_assignment(
            mutation.state(),
            (contact, role, location),
            &path,
            names,
            &pending,
            &mut errors,
        );
        pending.push((contact.clone(), location.clone()));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"roleAssignments": null}), &errors));
    }
    let state = mutation.state_mut();
    let made: Vec<String> = wanted
        .iter()
        .map(|(contact, role, location)| assign_role(state, contact, role, location))
        .collect();
    Ok(payload(json!({"roleAssignments": made}), &[]))
}

fn contact_assign_roles(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let contact = ctx
        .arg_str("companyContactId")
        .unwrap_or_default()
        .to_string();
    if mutation.state().type_of(&contact) != Some("CompanyContact") {
        let error = not_found(&["companyContactId"]);
        return Ok(payload(json!({"roleAssignments": null}), &[error]));
    }
    let wanted = roles_to_assign(ctx, Some(&contact), None);
    assign_all(mutation, wanted, "rolesToAssign", ROLE_FIELDS)
}

fn location_assign_roles(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let location = ctx
        .arg_str("companyLocationId")
        .unwrap_or_default()
        .to_string();
    if mutation.state().type_of(&location) != Some("CompanyLocation") {
        let error = not_found(&["companyLocationId"]);
        return Ok(payload(json!({"roleAssignments": null}), &[error]));
    }
    let wanted = roles_to_assign(ctx, None, Some(&location));
    assign_all(mutation, wanted, "rolesToAssign", ROLE_FIELDS)
}

/// Revokes the assignments named, each checked to be one of `owner`'s (`roleAssignments`): all
/// or none. `path`: the argument they are in.
fn revoke_all(
    mutation: &mut Mutation<'_>,
    owner: &str,
    assignments: Vec<String>,
    path: &str,
    single: bool,
) -> Result<Value, FieldError> {
    let empty = if single {
        json!({"revokedCompanyContactRoleAssignmentId": null})
    } else {
        json!({"revokedRoleAssignmentIds": null})
    };
    let own = gids_of(mutation.state().get(owner), "roleAssignments");
    let errors: Vec<UserError> = assignments
        .iter()
        .enumerate()
        .filter(|(_, assignment)| !own.contains(assignment))
        .map(|(index, _)| {
            if single {
                not_found(&[path])
            } else {
                not_found(&[path, &index.to_string()])
            }
        })
        .collect();
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    for assignment in &assignments {
        revoke(state, assignment);
    }
    Ok(payload(
        if single {
            json!({"revokedCompanyContactRoleAssignmentId": assignments.first()})
        } else {
            json!({"revokedRoleAssignmentIds": assignments})
        },
        &[],
    ))
}

fn contact_revoke_role(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let contact = match record_arg(mutation.state(), ctx, "companyContactId", "CompanyContact") {
        Ok(contact) => contact,
        Err(error) => {
            let empty = json!({"revokedCompanyContactRoleAssignmentId": null});
            return Ok(payload(empty, &[error]));
        }
    };
    let assignment = ctx
        .arg_str("companyContactRoleAssignmentId")
        .unwrap_or_default()
        .to_string();
    revoke_all(
        mutation,
        &contact,
        vec![assignment],
        "companyContactRoleAssignmentId",
        true,
    )
}

/// `companyContactRevokeRoles`: the assignments named, or all of them with `revokeAll`.
fn contact_revoke_roles(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let contact = match record_arg(mutation.state(), ctx, "companyContactId", "CompanyContact") {
        Ok(contact) => contact,
        Err(error) => return Ok(payload(json!({"revokedRoleAssignmentIds": null}), &[error])),
    };
    let assignments = if ctx.arg("revokeAll").and_then(Value::as_bool) == Some(true) {
        gids_of(mutation.state().get(&contact), "roleAssignments")
    } else {
        gids(ctx.arg("roleAssignmentIds"))
    };
    revoke_all(mutation, &contact, assignments, "roleAssignmentIds", false)
}

fn location_revoke_roles(
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
        Err(error) => return Ok(payload(json!({"revokedRoleAssignmentIds": null}), &[error])),
    };
    let assignments = gids(ctx.arg("rolesToRevoke"));
    revoke_all(mutation, &location, assignments, "rolesToRevoke", false)
}
