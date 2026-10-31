//! Customers, and the companies they buy for (B2B): companies, their locations, contacts and
//! contact roles.
//!
//! The records and their ids: the "Records" section of `crates/admin/docs/design.md`. The
//! mutations, by what they change:
//!
//! - [`customer`]: `customerCreate`, `customerUpdate`, `customerSet`, `customerDelete`, the
//!   account invitation and activation URL, tax exemptions;
//! - [`addresses`]: a customer's address book;
//! - [`consent`]: email and SMS marketing consent;
//! - [`companies`]: companies and their locations, addresses and tax settings;
//! - [`contacts`]: company contacts, the main contact, and the roles contacts have at locations.
//!
//! [`queries`] has the custom resolvers, sort keys and search filters of the family.

pub mod addresses;
pub mod companies;
pub mod consent;
pub mod contacts;
pub mod customer;
mod metafields;
mod queries;
#[cfg(test)]
mod tests;

use lsf_core::store::{Address, Company, CompanyLocation, Customer};
use lsf_core::util::stable_id;
use serde_json::{Map, Value, json};

use super::catalog::cents_of;
use crate::resolve::Registry;
use crate::state::seed::Seed;
use crate::state::value::{self, date_time, text};
use crate::state::{Record, ShopState, gid};

/// The roles every company has on Shopify, with the key of their ids: buyers order with the
/// second, the default one.
pub const CONTACT_ROLES: [(&str, &str); 2] = [
    ("location-admin", "Location admin"),
    ("ordering-only", "Ordering only"),
];

pub fn customer_gid(customer: &Customer) -> String {
    gid("Customer", customer.id)
}

pub fn company_gid(company: &Company) -> String {
    gid("Company", company.id)
}

pub fn company_location_gid(location: &CompanyLocation) -> String {
    gid("CompanyLocation", location.id)
}

/// The contact of a customer at their company: its id is the customer's, as the checkout's
/// `purchasingCompany.contact.id` has it.
pub fn company_contact_gid(customer: &Customer) -> String {
    gid("CompanyContact", customer.id)
}

/// One of the [`CONTACT_ROLES`] of a company.
pub fn contact_role_gid(company: &Company, role: &str) -> String {
    contact_role_gid_of(company.id, role)
}

/// One of the [`CONTACT_ROLES`] of the company of numeric id `company`: the seed's ids, which a
/// company the Admin API creates gets too.
pub fn contact_role_gid_of(company: u64, role: &str) -> String {
    gid(
        "CompanyContactRole",
        stable_id("company_contact_role", &format!("{company}/{role}")),
    )
}

pub fn role_assignment_gid(customer: &Customer, location: &CompanyLocation) -> String {
    gid(
        "CompanyContactRoleAssignment",
        stable_id(
            "company_contact_role_assignment",
            &format!("{}/{}", customer.id, location.id),
        ),
    )
}

pub fn company_address_gid(location: &CompanyLocation, address: &Address) -> String {
    gid(
        "CompanyAddress",
        stable_id(
            "company_address",
            &format!("{}/{}", location.id, address.id),
        ),
    )
}

pub fn seed(seed: &mut Seed) {
    let store = seed.store;
    for customer in &store.customers {
        seed_customer(seed, customer);
    }
    for company in &store.companies {
        seed_company(seed, company);
    }
}

fn seed_customer(seed: &mut Seed, customer: &Customer) {
    let store = seed.store;
    let id = customer_gid(customer);
    // The store data gives no creation date: the first order's, as the checkout's
    // `Customer.createdAt` has it, else the fixed date of the seed.
    let created = customer
        .orders
        .iter()
        .map(|order| order.created_at)
        .min()
        .map_or_else(Seed::epoch_string, date_time);
    let addresses: Vec<Value> = customer
        .addresses
        .iter()
        .map(|address| value::mailing_address(address, "CustomerAddress"))
        .collect();
    let marketing_state = if customer.accepts_marketing {
        "SUBSCRIBED"
    } else {
        "NOT_SUBSCRIBED"
    };
    // Every buyer of a company is one of its contacts, as in the checkout.
    let contact = customer.company.map(|_| company_contact_gid(customer));
    let metafields = seed.metafields(&id, "CUSTOMER", &created, &customer.metafields);
    let mut record = value::object(json!({
        "__typename": "Customer",
        "id": id,
        "firstName": text(&customer.first_name),
        "lastName": text(&customer.last_name),
        "email": text(&customer.email),
        "phone": text(&customer.phone),
        "emailMarketingConsent": (!customer.email.is_empty()).then(|| json!({
            "marketingState": marketing_state,
            "marketingOptInLevel": "SINGLE_OPT_IN",
            "consentUpdatedAt": null,
        })),
        "smsMarketingConsent": (!customer.phone.is_empty()).then(|| json!({
            "marketingState": "NOT_SUBSCRIBED",
            "marketingOptInLevel": "SINGLE_OPT_IN",
            "consentUpdatedAt": null,
        })),
        "addresses": addresses,
        "addressesV2": addresses,
        "defaultAddress": addresses.first(),
        "state": if customer.has_account { "ENABLED" } else { "DISABLED" },
        "verifiedEmail": customer.has_account,
        "taxExempt": customer.tax_exempt,
        "taxExemptions": [],
        "dataSaleOptOut": false,
        "tags": customer.tags,
        "locale": store.primary_language().iso_code,
        "companyContactProfiles": contact.into_iter().collect::<Vec<_>>(),
        "orders": [],
        "createdAt": created,
        "updatedAt": created,
        "metafields": metafields,
    }));
    contact_fields(&mut record);
    seed.insert_record(record);
}

/// Derives what a customer's names, email, phone and consents give, which the seed and every
/// mutation that changes them share: `displayName` ("based on the values for first_name and
/// last_name. If [they] are not available, then this falls back to the customer's email address,
/// and if that is not available, the customer's phone number", the schema),
/// `defaultEmailAddress` and `defaultPhoneNumber` (the same consents, in the shapes of the
/// fields that replace the deprecated ones), and no consent without the email or the phone it is
/// about.
pub fn contact_fields(record: &mut Record) {
    let get = |record: &Record, field: &str| {
        record
            .get(field)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let email = get(record, "email");
    let phone = get(record, "phone");
    let display = match value::full_name(
        &get(record, "firstName").unwrap_or_default(),
        &get(record, "lastName").unwrap_or_default(),
    ) {
        Value::String(name) => name,
        _ => email.clone().or_else(|| phone.clone()).unwrap_or_default(),
    };
    record.insert("displayName".into(), json!(display));

    let email_consent = email.as_ref().map(|_| {
        record
            .get("emailMarketingConsent")
            .filter(|consent| consent.is_object())
            .cloned()
            .unwrap_or_else(consent::default_email_consent)
    });
    let default_email = email
        .as_ref()
        .zip(email_consent.as_ref())
        .map(|(email, consent)| {
            let state = match consent.get("marketingState").and_then(Value::as_str) {
                // `CustomerEmailAddressMarketingState` has no `REDACTED`.
                Some("REDACTED") | None => "NOT_SUBSCRIBED",
                Some(state) => state,
            };
            json!({
                "emailAddress": email,
                "marketingState": state,
                "marketingOptInLevel": consent.get("marketingOptInLevel"),
                "marketingUpdatedAt": consent.get("consentUpdatedAt"),
                "sourceLocation": consent.get("sourceLocation"),
                "validFormat": customer::is_email(email),
            })
        });
    record.insert("emailMarketingConsent".into(), json!(email_consent));
    record.insert("defaultEmailAddress".into(), json!(default_email));

    let sms_consent = phone.as_ref().map(|_| {
        record
            .get("smsMarketingConsent")
            .filter(|consent| consent.is_object())
            .cloned()
            .unwrap_or_else(consent::default_sms_consent)
    });
    let default_phone = phone
        .as_ref()
        .zip(sms_consent.as_ref())
        .map(|(phone, consent)| {
            let field = |name: &str| consent.get(name).cloned().unwrap_or(Value::Null);
            // `CustomerMarketingConsentState` says `NEVER_SUBSCRIBED` where the SMS state says
            // `NOT_SUBSCRIBED`.
            let state = match consent.get("marketingState").and_then(Value::as_str) {
                Some("NOT_SUBSCRIBED") | None => "NEVER_SUBSCRIBED",
                Some(state) => state,
            };
            json!({
                "phoneNumber": phone,
                "marketingState": field("marketingState"),
                "marketingOptInLevel": field("marketingOptInLevel"),
                "marketingUpdatedAt": field("consentUpdatedAt"),
                "marketingCollectedFrom": field("consentCollectedFrom"),
                "sourceLocation": field("sourceLocation"),
                "smsMarketingConsent": {
                    "state": state,
                    "optInLevel": field("marketingOptInLevel"),
                    "updatedAt": field("consentUpdatedAt"),
                    "collectedFrom": field("consentCollectedFrom"),
                    "sourceLocation": field("sourceLocation"),
                },
            })
        });
    record.insert("smsMarketingConsent".into(), json!(sms_consent));
    record.insert("defaultPhoneNumber".into(), json!(default_phone));
}

/// The locale of the shop's primary language: what a customer or a contact created without one
/// gets.
pub fn primary_locale(state: &ShopState) -> String {
    state
        .all("ShopLocale")
        .find(|locale| locale.get("primary").and_then(Value::as_bool) == Some(true))
        .and_then(|locale| locale.get("locale").and_then(Value::as_str))
        .unwrap_or("en")
        .to_string()
}

/// The shop's currency: what a company location created gets.
pub fn shop_currency(state: &ShopState) -> String {
    state
        .shop()
        .and_then(|shop| shop.get("currencyCode"))
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string()
}

/// The GIDs of an array of a record (`locations`, `contacts`, `roleAssignments`).
pub fn gids_of(record: Option<&Record>, field: &str) -> Vec<String> {
    record
        .and_then(|record| record.get(field))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The path of `field` in an input at `path`: what a user error names.
pub fn field_path<'a>(path: &[&'a str], field: &'a str) -> Vec<&'a str> {
    path.iter().copied().chain([field]).collect()
}

/// A string of an input object: `None` when it is not given, `Some(None)` when it is given as
/// `null` or blank (to clear the field), `Some(Some(text))` trimmed otherwise.
pub fn given_text(input: &Map<String, Value>, field: &str) -> Option<Option<String>> {
    let value = input.get(field)?;
    Some(
        value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
    )
}

fn seed_company(seed: &mut Seed, company: &Company) {
    let store = seed.store;
    let id = company_gid(company);
    let created = date_time(company.created_at);
    let roles: Vec<String> = CONTACT_ROLES
        .iter()
        .map(|(key, name)| {
            let role = contact_role_gid(company, key);
            seed.insert(json!({
                "__typename": "CompanyContactRole",
                "id": role,
                "name": name,
                "note": null,
            }))
        })
        .collect();
    let ordering_only = contact_role_gid(company, CONTACT_ROLES[1].0);
    let company_index = store
        .companies
        .iter()
        .position(|candidate| candidate.id == company.id);
    let buyers: Vec<&Customer> = store
        .customers
        .iter()
        .filter(|customer| customer.company.is_some() && customer.company == company_index)
        .collect();

    // Each buyer can order for the locations the data gives them, with the default role.
    let mut assignments: Vec<(String, String, String)> = Vec::new();
    let mut contacts = Vec::new();
    for (rank, customer) in buyers.iter().enumerate() {
        let contact = company_contact_gid(customer);
        let mut own = Vec::new();
        for index in &customer.company_locations {
            let Some(location) = company.locations.get(*index) else {
                continue;
            };
            let assignment = role_assignment_gid(customer, location);
            seed.insert(json!({
                "__typename": "CompanyContactRoleAssignment",
                "id": assignment,
                "company": id,
                "companyContact": contact,
                "companyLocation": company_location_gid(location),
                "role": ordering_only,
                "createdAt": created,
                "updatedAt": created,
            }));
            own.push(assignment.clone());
            assignments.push((company_location_gid(location), assignment, contact.clone()));
        }
        let details = customer.company_contact.as_ref();
        let contact_created = details.map_or(created.clone(), |c| date_time(c.created_at));
        seed.insert(json!({
            "__typename": "CompanyContact",
            "id": contact,
            "company": id,
            "customer": customer_gid(customer),
            "title": details.and_then(|c| c.title.as_deref()).map_or(Value::Null, text),
            "locale": details.and_then(|c| c.locale.as_deref()).map_or(Value::Null, text),
            "isMainContact": rank == 0,
            "roleAssignments": own,
            "createdAt": contact_created,
            "updatedAt": details.map_or(created.clone(), |c| date_time(c.updated_at)),
        }));
        contacts.push(contact);
    }

    let mut locations = Vec::new();
    let mut spent = 0;
    for location in &company.locations {
        let location_id = company_location_gid(location);
        let location_created = date_time(location.created_at);
        let shipping = location
            .shipping_address
            .as_ref()
            .map(|address| company_address(location, address, &company.name, &location_created));
        let role_assignments: Vec<&String> = assignments
            .iter()
            .filter(|(owner, _, _)| *owner == location_id)
            .map(|(_, assignment, _)| assignment)
            .collect();
        let metafields = seed.metafields(
            &location_id,
            "COMPANY_LOCATION",
            &location_created,
            &location.metafields,
        );
        spent += location.total_spent;
        seed.insert(json!({
            "__typename": "CompanyLocation",
            "id": location_id,
            "name": location.name,
            "externalId": location.external_id,
            "company": id,
            "locale": location.locale,
            "currency": seed.currency,
            "shippingAddress": shipping,
            "billingAddress": null,
            "taxSettings": {
                "taxRegistrationId": location.tax_registration_id,
                "taxExempt": false,
                "taxExemptions": [],
            },
            "taxRegistrationId": location.tax_registration_id,
            "taxExemptions": [],
            "ordersCount": {"count": location.orders_count, "precision": "EXACT"},
            "orderCount": location.orders_count,
            "totalSpent": seed.money_v2(location.total_spent),
            // What the data counts without the orders behind them, which the orders placed
            // for the location add to (`refresh_company_location`).
            ORDERS_BASE: {"count": location.orders_count, "spent": location.total_spent},
            "roleAssignments": role_assignments,
            "createdAt": location_created,
            "updatedAt": date_time(location.updated_at),
            "metafields": metafields,
        }));
        locations.push(location_id);
    }

    let metafields = seed.metafields(&id, "COMPANY", &created, &company.metafields);
    seed.insert(json!({
        "__typename": "Company",
        "id": id,
        "name": company.name,
        "externalId": company.external_id,
        "note": null,
        "customerSince": created,
        "createdAt": created,
        "updatedAt": date_time(company.updated_at),
        "locations": locations,
        "contacts": contacts,
        "mainContact": contacts.first(),
        "contactRoles": roles,
        "defaultRole": ordering_only,
        "totalSpent": seed.money_v2(spent),
        "metafields": metafields,
    }));
}

/// The address of a company location: a `CompanyAddress`, which is an object of its own on
/// Shopify (with an id), held inline by the location.
fn company_address(
    location: &CompanyLocation,
    address: &Address,
    company: &str,
    created: &str,
) -> Value {
    json!({
        "id": company_address_gid(location, address),
        "address1": address.address1,
        "address2": text(&address.address2),
        "city": text(&address.city),
        "province": text(&address.province),
        "zoneCode": text(&address.province_code),
        "country": text(&address.country),
        "countryCode": address.country_code.to_ascii_uppercase(),
        "zip": text(&address.zip),
        "phone": text(&address.phone),
        "firstName": text(&address.first_name),
        "lastName": text(&address.last_name),
        "recipient": value::full_name(&address.first_name, &address.last_name),
        "companyName": if address.company.is_empty() { company } else { &address.company },
        "formattedAddress": value::formatted(address),
        "formattedArea": value::formatted_area(address),
        "createdAt": created,
        "updatedAt": created,
    })
}

/// Recomputes what a customer derives from their orders: `orders`, `numberOfOrders`,
/// `amountSpent` (what the orders that are not cancelled cost, as the checkout's
/// `Customer.amountSpent`), `lastOrder` and `canDelete`. The seed runs it once the orders
/// exist; a mutation that creates, cancels or deletes an order runs it after.
pub fn refresh_customer(state: &mut ShopState, customer: &str) {
    let currency = state
        .shop()
        .and_then(|shop| shop.get("currencyCode"))
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string();
    let orders: Vec<(String, i64, bool, String)> = state
        .all("Order")
        .filter(|order| order.get("customer").and_then(Value::as_str) == Some(customer))
        .map(|order| {
            let total = order
                .get("totalPriceSet")
                .and_then(|set| set.get("shopMoney"))
                .and_then(|money| cents_of(money.get("amount")))
                .unwrap_or(0);
            let cancelled = order.get("cancelledAt").is_some_and(|at| !at.is_null());
            let processed = order
                .get("processedAt")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            (
                order
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                total,
                cancelled,
                processed,
            )
        })
        .collect();
    let spent: i64 = orders
        .iter()
        .filter(|(_, _, cancelled, _)| !cancelled)
        .map(|(_, total, _, _)| total)
        .sum();
    let last = orders
        .iter()
        .max_by(|a, b| a.3.cmp(&b.3))
        .map(|(id, _, _, _)| id.clone());
    let ids: Vec<String> = orders.iter().map(|(id, _, _, _)| id.clone()).collect();
    let Some(record) = state.get_mut(customer) else {
        return;
    };
    record.insert("numberOfOrders".to_string(), json!(ids.len().to_string()));
    record.insert("amountSpent".to_string(), value::money_v2(spent, &currency));
    record.insert("lastOrder".to_string(), json!(last));
    record.insert("canDelete".to_string(), json!(ids.is_empty()));
    record.insert("orders".to_string(), json!(ids));
}

/// What a company location's counters start from, kept on it under this key, which GraphQL
/// never reads: `{"count", "spent"}` (cents), the store data's `orders_count` and `total_spent`,
/// which it gives without the orders behind them (0 for a location the Admin API creates).
pub const ORDERS_BASE: &str = "lsfOrdersBase";

/// The company location an order was placed for: its `purchasingEntity` when it is a
/// `PurchasingCompany`.
pub fn order_company_location(order: &Record) -> Option<&str> {
    let entity = order.get("purchasingEntity")?;
    (entity.get("__typename").and_then(Value::as_str) == Some("PurchasingCompany"))
        .then(|| entity.get("location").and_then(Value::as_str))
        .flatten()
}

/// Recomputes what a company location and its company derive from the orders placed for the
/// location, as [`refresh_customer`] does for a customer: the location's `ordersCount`,
/// `orderCount` and `totalSpent` (what the store data counts, plus the orders of the shop placed
/// for it, the cancelled ones costing nothing, as `amountSpent` counts them), and the company's
/// `totalSpent`, the sum of its locations'. `Company.ordersCount` is read from its locations.
pub fn refresh_company_location(state: &mut ShopState, location: &str) {
    let Some(record) = state.get(location) else {
        return;
    };
    let currency = state
        .shop()
        .and_then(|shop| shop.get("currencyCode"))
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string();
    let base = record.get(ORDERS_BASE);
    let base_count = base
        .and_then(|base| base.get("count"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let base_spent = base
        .and_then(|base| base.get("spent"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let company = record
        .get("company")
        .and_then(Value::as_str)
        .map(str::to_string);
    let orders: Vec<(i64, bool)> = state
        .all("Order")
        .filter(|order| order_company_location(order) == Some(location))
        .map(|order| {
            let total = order
                .get("totalPriceSet")
                .and_then(|set| set.get("shopMoney"))
                .and_then(|money| cents_of(money.get("amount")))
                .unwrap_or(0);
            let cancelled = order.get("cancelledAt").is_some_and(|at| !at.is_null());
            (total, cancelled)
        })
        .collect();
    let count = base_count + orders.len() as i64;
    let spent = orders
        .iter()
        .filter(|(_, cancelled)| !cancelled)
        .fold(base_spent, |spent, (total, _)| spent.saturating_add(*total));
    if let Some(record) = state.get_mut(location) {
        record.insert(
            "ordersCount".into(),
            json!({"count": count, "precision": "EXACT"}),
        );
        record.insert("orderCount".into(), json!(count));
        record.insert("totalSpent".into(), value::money_v2(spent, &currency));
    }
    let Some(company) = company else {
        return;
    };
    let locations = state
        .get(&company)
        .and_then(|record| record.get("locations"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total = locations
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|location| state.get(location))
        .filter_map(|location| {
            location
                .get("totalSpent")
                .and_then(|money| cents_of(money.get("amount")))
        })
        .fold(0i64, i64::saturating_add);
    if let Some(record) = state.get_mut(&company) {
        record.insert("totalSpent".into(), value::money_v2(total, &currency));
    }
}

/// The custom resolvers and the mutations of this family: see "Writing a resolver or a mutation"
/// in `crates/admin/docs/design.md`.
pub fn register(registry: &mut Registry) {
    customer::register(registry);
    addresses::register(registry);
    consent::register(registry);
    companies::register(registry);
    contacts::register(registry);
    queries::register(registry);
}
