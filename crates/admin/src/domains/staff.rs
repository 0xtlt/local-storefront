//! Staff members and the company locations they are assigned to:
//! `companyLocationAssignStaffMembers`, `companyLocationRemoveStaffMembers`, and
//! `CompanyLocation.staffMemberAssignments`.
//!
//! The store data has no staff: a shop has its owner, a `StaffMember` made from the shop (its
//! email, its name; `isShopOwner: true`, `accountType: REGULAR`, `active`), which
//! `Shop.accountOwner` names *(chosen: every Shopify shop has an owner)*. When another family
//! seeds staff members first, they are kept and none is added.
//!
//! As shopify.dev's reference of 2026-10 describes them: `companyLocationAssignStaffMembers`
//! "creates one or more mappings between a staff member at a shop and a company location" (a
//! `CompanyLocationStaffMemberAssignment`), `companyLocationRemoveStaffMembers` deletes them by
//! their ids. A location or a staff member that does not exist is `RESOURCE_NOT_FOUND`
//! ("Resource requested does not exist.", the message the companies family answers), a staff
//! member already assigned to the location `TAKEN` (lsf's message). No webhook topic covers the
//! assignments: they emit `lsf/changed`. They are not store data: they do not sync.
//!
//! The deprecated tax registration mutations of a company location are here too, on the logic
//! of `companyLocationTaxSettingsUpdate` that replaced them (`domains::customers::companies`):
//! `companyLocationCreateTaxRegistration(locationId:, taxId:)` sets the location's
//! `taxSettings.taxRegistrationId` (a blank id is `BLANK`, Rails' "Tax ID can't be blank"),
//! `companyLocationRevokeTaxRegistration(companyLocationId:)` clears it; both answer the
//! location and fire `company_locations/update`. The tax registration syncs with the company,
//! as `companyLocationTaxSettingsUpdate`'s does.

use lsf_core::util::stable_id;
use serde_json::{Value, json};

use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::seed::Seed;
use crate::state::{ShopState, gid};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("companyLocationAssignStaffMembers", assign_staff_members)
        .mutation("companyLocationRemoveStaffMembers", remove_staff_members)
        .mutation(
            "companyLocationCreateTaxRegistration",
            create_tax_registration,
        )
        .mutation(
            "companyLocationRevokeTaxRegistration",
            revoke_tax_registration,
        );
}

/// The GID of the shop's owner, the one staff member the seed makes.
pub fn owner_gid() -> String {
    gid("StaffMember", stable_id("staff_member", "account-owner"))
}

/// The shop's owner, when no staff member is seeded yet: the shop's `accountOwner`, and the staff
/// member point of sale opens sessions with.
pub fn seed(seed: &mut Seed) {
    if seed.state.count("StaffMember") > 0 {
        return;
    }
    let shop = &seed.store.shop;
    let id = owner_gid();
    let handle = seed
        .state
        .shop()
        .and_then(|record| record.get("myshopifyDomain")?.as_str())
        .and_then(|domain| domain.split('.').next())
        .unwrap_or_default()
        .to_string();
    let name = if shop.name.is_empty() {
        "Shop owner".to_string()
    } else {
        format!("{} owner", shop.name)
    };
    let locale = seed.store.primary_language().iso_code.clone();
    seed.insert(json!({
        "__typename": "StaffMember",
        "id": id,
        "name": name,
        "firstName": null,
        "lastName": null,
        "email": shop.email,
        "phone": null,
        "isShopOwner": true,
        "active": true,
        "exists": true,
        "accountType": "REGULAR",
        "locale": locale,
        "initials": [name.chars().next().map(|c| c.to_string()).unwrap_or_default()],
        "privateData": {
            "accountSettingsUrl": format!("https://admin.shopify.com/store/{handle}/settings/account"),
            "createdAt": Seed::epoch_string(),
            "permissions": [],
        },
    }));
    if let Some(shop_gid) = seed.state.shop_gid().map(str::to_string)
        && let Some(record) = seed.state.get_mut(&shop_gid)
    {
        record.insert("accountOwner".into(), json!(id));
    }
}

fn is(state: &ShopState, id: &str, type_name: &str) -> bool {
    state.type_of(id) == Some(type_name)
}

fn not_found(field: &[&str]) -> UserError {
    UserError::new(field, "Resource requested does not exist.").code("RESOURCE_NOT_FOUND")
}

fn assign_staff_members(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let location = ctx
        .arg_str("companyLocationId")
        .unwrap_or_default()
        .to_string();
    let staff: Vec<String> = ctx
        .arg("staffMemberIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let state = m.state();
    let mut errors = Vec::new();
    if !is(state, &location, "CompanyLocation") {
        errors.push(not_found(&["companyLocationId"]));
    }
    let assigned: Vec<String> = state
        .all("CompanyLocationStaffMemberAssignment")
        .filter(|assignment| {
            assignment.get("companyLocation").and_then(Value::as_str) == Some(location.as_str())
        })
        .filter_map(|assignment| assignment.get("staffMember").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    for (index, member) in staff.iter().enumerate() {
        let path = ["staffMemberIds".to_string(), index.to_string()];
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        if !is(state, member, "StaffMember") {
            errors.push(not_found(&path));
        } else if assigned.contains(member) || staff[..index].contains(member) {
            errors.push(
                UserError::new(
                    &path,
                    "The staff member is already assigned to the location.",
                )
                .code("TAKEN"),
            );
        }
    }
    if !errors.is_empty() {
        return Ok(payload(
            json!({"companyLocationStaffMemberAssignments": null}),
            &errors,
        ));
    }
    let state = m.state_mut();
    let mut created = Vec::new();
    for member in &staff {
        let id = state.new_gid("CompanyLocationStaffMemberAssignment");
        state.insert_as(
            id.clone(),
            crate::state::value::object(json!({
                "__typename": "CompanyLocationStaffMemberAssignment",
                "id": id,
                "companyLocation": location,
                "staffMember": member,
            })),
        );
        created.push(id);
    }
    if let Some(record) = state.get_mut(&location) {
        let mut list = record
            .get("staffMemberAssignments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        list.extend(created.iter().map(|id| json!(id)));
        record.insert("staffMemberAssignments".into(), json!(list));
    }
    state.emit(crate::domains::app::CHANGED, &location);
    Ok(payload(
        json!({"companyLocationStaffMemberAssignments": created}),
        &[],
    ))
}

fn remove_staff_members(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let ids: Vec<String> = ctx
        .arg("companyLocationStaffMemberAssignmentIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let mut errors = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        if !is(m.state(), id, "CompanyLocationStaffMemberAssignment") {
            errors.push(not_found(&[
                "companyLocationStaffMemberAssignmentIds",
                &index.to_string(),
            ]));
        }
    }
    if !errors.is_empty() {
        return Ok(payload(
            json!({"deletedCompanyLocationStaffMemberAssignmentIds": null}),
            &errors,
        ));
    }
    let state = m.state_mut();
    for id in &ids {
        let location = state
            .get(id)
            .and_then(|record| record.get("companyLocation"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(location) = location {
            if let Some(record) = state.get_mut(&location) {
                let list: Vec<Value> = record
                    .get("staffMemberAssignments")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|item| item.as_str() != Some(id.as_str()))
                    .cloned()
                    .collect();
                record.insert("staffMemberAssignments".into(), json!(list));
            }
            state.emit(crate::domains::app::CHANGED, &location);
        }
        state.remove(id);
    }
    Ok(payload(
        json!({"deletedCompanyLocationStaffMemberAssignmentIds": ids}),
        &[],
    ))
}

/// `companyLocationCreateTaxRegistration`, deprecated for `companyLocationTaxSettingsUpdate`.
fn create_tax_registration(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let location = ctx.arg_str("locationId").unwrap_or_default().to_string();
    let tax_id = ctx.arg_str("taxId").unwrap_or_default().trim().to_string();
    let mut errors = Vec::new();
    if !is(m.state(), &location, "CompanyLocation") {
        errors.push(not_found(&["locationId"]));
    }
    if tax_id.is_empty() {
        errors.push(UserError::new(&["taxId"], "Tax ID can't be blank").code("BLANK"));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"companyLocation": null}), &errors));
    }
    crate::domains::customers::companies::change_tax_settings(
        m.state_mut(),
        &location,
        |settings| {
            settings.insert("taxRegistrationId".into(), json!(tax_id));
        },
    );
    Ok(payload(json!({"companyLocation": location}), &[]))
}

/// `companyLocationRevokeTaxRegistration`, deprecated for `companyLocationTaxSettingsUpdate`.
fn revoke_tax_registration(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let location = ctx
        .arg_str("companyLocationId")
        .unwrap_or_default()
        .to_string();
    if !is(m.state(), &location, "CompanyLocation") {
        let error = not_found(&["companyLocationId"]);
        return Ok(payload(json!({"companyLocation": null}), &[error]));
    }
    crate::domains::customers::companies::change_tax_settings(
        m.state_mut(),
        &location,
        |settings| {
            settings.insert("taxRegistrationId".into(), Value::Null);
        },
    );
    Ok(payload(json!({"companyLocation": location}), &[]))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::domains::segments::tests::{data, demo, run};

    fn location(shop: &mut crate::state::ShopState) -> String {
        let created = run(
            shop,
            "mutation { companyCreate(input: {company: {name: \"Acme B2B\"}, companyLocation: {name: \"HQ\"}}) {
                company { locations(first: 1) { nodes { id } } } userErrors { message } } }",
            json!({}),
        );
        data(&created, "companyCreate")["company"]["locations"]["nodes"][0]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn staff_members_are_assigned_to_a_location_and_removed() {
        let mut shop = demo();
        let owner = run(
            &mut shop,
            "{ shop { accountOwner { id isShopOwner email } } }",
            json!({}),
        );
        let owner = data(&owner, "shop")["accountOwner"].clone();
        assert_eq!(owner["isShopOwner"], true);
        let location = location(&mut shop);
        let assign = "mutation($location: ID!, $staff: [ID!]!) { companyLocationAssignStaffMembers(companyLocationId: $location, staffMemberIds: $staff) {
            companyLocationStaffMemberAssignments { id staffMember { id } companyLocation { name } } userErrors { field message code } } }";
        let assigned = run(
            &mut shop,
            assign,
            json!({"location": location, "staff": [owner["id"]]}),
        );
        let assignment = &data(&assigned, "companyLocationAssignStaffMembers")["companyLocationStaffMemberAssignments"]
            [0];
        assert_eq!(assignment["staffMember"]["id"], owner["id"]);
        assert_eq!(assignment["companyLocation"]["name"], "HQ");
        let read = run(
            &mut shop,
            "query($id: ID!) { companyLocation(id: $id) { staffMemberAssignments(first: 5) { nodes { id } } } }",
            json!({"id": location}),
        );
        assert_eq!(
            data(&read, "companyLocation")["staffMemberAssignments"]["nodes"][0]["id"],
            assignment["id"]
        );
        let before = shop.picture();
        let again = run(
            &mut shop,
            assign,
            json!({"location": location, "staff": [owner["id"], "gid://shopify/StaffMember/1"]}),
        );
        let codes: Vec<&str> = data(&again, "companyLocationAssignStaffMembers")["userErrors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|error| error["code"].as_str().unwrap())
            .collect();
        assert_eq!(codes, ["TAKEN", "RESOURCE_NOT_FOUND"]);
        assert_eq!(shop.picture(), before);
        let removed = run(
            &mut shop,
            "mutation($ids: [ID!]!) { companyLocationRemoveStaffMembers(companyLocationStaffMemberAssignmentIds: $ids) { deletedCompanyLocationStaffMemberAssignmentIds userErrors { code } } }",
            json!({"ids": [assignment["id"]]}),
        );
        assert_eq!(
            data(&removed, "companyLocationRemoveStaffMembers")["deletedCompanyLocationStaffMemberAssignmentIds"],
            json!([assignment["id"]])
        );
        assert_eq!(shop.count("CompanyLocationStaffMemberAssignment"), 0);
    }

    #[test]
    fn the_deprecated_tax_registration_mutations_set_and_clear_it() {
        let mut shop = demo();
        let location = location(&mut shop);
        shop.take_events();
        let created = run(
            &mut shop,
            "mutation($id: ID!) { companyLocationCreateTaxRegistration(locationId: $id, taxId: \"FR123\") {
                companyLocation { taxSettings { taxRegistrationId } } userErrors { field message code } } }",
            json!({"id": location}),
        );
        assert_eq!(
            data(&created, "companyLocationCreateTaxRegistration")["companyLocation"]["taxSettings"]
                ["taxRegistrationId"],
            "FR123"
        );
        let topics: Vec<String> = shop
            .take_events()
            .into_iter()
            .map(|event| event.topic)
            .collect();
        assert_eq!(topics, ["company_locations/update"]);
        let before = shop.picture();
        let refused = run(
            &mut shop,
            "mutation($id: ID!) { companyLocationCreateTaxRegistration(locationId: $id, taxId: \" \") { userErrors { field code } } }",
            json!({"id": location}),
        );
        assert_eq!(
            data(&refused, "companyLocationCreateTaxRegistration")["userErrors"],
            json!([{"field": ["taxId"], "code": "BLANK"}])
        );
        let missing = run(
            &mut shop,
            "mutation { companyLocationRevokeTaxRegistration(companyLocationId: \"gid://shopify/CompanyLocation/1\") { userErrors { field code } } }",
            json!({}),
        );
        assert_eq!(
            data(&missing, "companyLocationRevokeTaxRegistration")["userErrors"],
            json!([{"field": ["companyLocationId"], "code": "RESOURCE_NOT_FOUND"}])
        );
        assert_eq!(shop.picture(), before);
        let revoked = run(
            &mut shop,
            "mutation($id: ID!) { companyLocationRevokeTaxRegistration(companyLocationId: $id) {
                companyLocation { taxSettings { taxRegistrationId } } userErrors { message } } }",
            json!({"id": location}),
        );
        assert_eq!(
            data(&revoked, "companyLocationRevokeTaxRegistration")["companyLocation"]["taxSettings"]
                ["taxRegistrationId"],
            Value::Null
        );
    }
}
