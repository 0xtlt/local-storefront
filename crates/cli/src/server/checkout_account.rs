//! Who the checkout is logged in as.
//!
//! The customers of the store data are the login profiles of the checkout: the page lists them,
//! and the `customer` operation logs the checkout in as one of them, or out. A login changes the
//! session the way `session.customer` of the store data, a scenario or `PUT /__lsf/session` set
//! it at the start: the Functions, the extensions and the theme all read the same customer. The
//! cart, its token and the scenario stay.
//!
//! What a customer brings to the checkout follows Shopify's checkout: their email as the
//! contact, their default address as the shipping address (the address of the location a B2B
//! customer buys for), and the other addresses they saved to choose from. Whatever changes the
//! customer of a session (the page, `/__lsf/login`, the control API, the login form of the
//! theme), the checkout catches up the next time it is read: see [`prepare`].

use lsf_checkout::{Address, CheckoutState};
use lsf_core::store::{Address as StoreAddress, Customer};
use lsf_core::{Session, Store};
use serde_json::{Value as Json, json};

use super::account::full_name;

/// Who a checkout is for: the customer, and the location a B2B customer buys for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub customer: Option<u64>,
    /// The id of the company location, the one the customer actually buys for (their first one
    /// when the session names none).
    pub location: Option<u64>,
}

impl Identity {
    pub fn of(store: &Store, session: &Session) -> Identity {
        let customer = session.customer_id.and_then(|id| store.customer_by_id(id));
        Identity {
            customer: customer.map(|customer| customer.id),
            location: customer.and_then(|customer| {
                let company = &store.companies[customer.company?];
                customer
                    .current_location(session.company_location, store)
                    .map(|index| company.locations[index].id)
            }),
        }
    }
}

/// An address the customer can ship to without typing it.
pub struct SavedAddress<'a> {
    /// What the page tells the addresses apart by.
    pub id: String,
    /// The location whose address it is, for a B2B customer.
    pub location: Option<&'a str>,
    pub address: &'a StoreAddress,
}

/// The addresses a customer chooses from at checkout, the default one first. A B2B customer
/// ships to the location they buy for, as on Shopify; another customer to the addresses they
/// saved, the first one being their default.
pub fn saved_addresses<'a>(
    customer: &'a Customer,
    store: &'a Store,
    selected_location: Option<u64>,
) -> Vec<SavedAddress<'a>> {
    if let Some(company) = customer.company {
        return customer
            .current_location(selected_location, store)
            .map(|index| &store.companies[company].locations[index])
            .and_then(|location| {
                location
                    .shipping_address
                    .as_ref()
                    .map(|address| SavedAddress {
                        id: format!("location-{}", location.id),
                        location: Some(location.name.as_str()),
                        address,
                    })
            })
            .into_iter()
            .collect();
    }
    customer
        .addresses
        .iter()
        .map(|address| SavedAddress {
            id: format!("address-{}", address.id),
            location: None,
            address,
        })
        .collect()
}

/// An address of the store data as the checkout holds it: an empty field is no field.
pub fn checkout_address(address: &StoreAddress) -> Address {
    // Trimmed, unlike what a scenario or the buyer enters.
    let text = |value: &str| Some(value.trim().to_string());
    Address {
        first_name: text(&address.first_name),
        last_name: text(&address.last_name),
        company: text(&address.company),
        address1: text(&address.address1),
        address2: text(&address.address2),
        city: text(&address.city),
        country_code: text(&address.country_code),
        province_code: text(&address.province_code),
        zip: text(&address.zip),
        phone: text(&address.phone),
    }
    .normalized()
}

/// The addresses of `identity`, as the checkout holds them.
fn addresses_of(store: &Store, identity: Identity) -> Vec<Address> {
    identity
        .customer
        .and_then(|id| store.customer_by_id(id))
        .map(|customer| {
            saved_addresses(customer, store, identity.location)
                .iter()
                .map(|saved| checkout_address(saved.address))
                .collect()
        })
        .unwrap_or_default()
}

/// Brings the checkout from `before` (`None`: nobody has prepared it yet) to `after`:
///
/// - the first time, a logged-in customer ships to their default address unless an address was
///   entered (a scenario's);
/// - another customer (or nobody): the contact is theirs, not what was typed before, and the
///   shipping address their default one. An address the buyer typed stays when the new
///   customer has none; one the customer before had saved does not;
/// - the same customer at another company location: the order ships to the new location when it
///   shipped to the old one.
///
/// The country of the session follows the address, as entering one does.
pub fn transition(
    store: &Store,
    session: &mut Session,
    checkout: &mut CheckoutState,
    before: Option<Identity>,
    after: Identity,
) {
    let default = addresses_of(store, after).into_iter().next();
    let changed = match before {
        None => {
            if checkout.shipping_address.is_none() && default.is_some() {
                checkout.shipping_address = default;
                true
            } else {
                false
            }
        }
        Some(before) if before.customer != after.customer => {
            checkout.email = None;
            checkout.phone = None;
            let theirs = checkout
                .shipping_address
                .as_ref()
                .is_some_and(|address| addresses_of(store, before).contains(address));
            if default.is_some() || theirs {
                checkout.shipping_address = default;
            }
            true
        }
        Some(before) if before.location != after.location => {
            let shipped_there = checkout
                .shipping_address
                .as_ref()
                .is_none_or(|address| addresses_of(store, before).first() == Some(address));
            if shipped_there && default.is_some() {
                checkout.shipping_address = default;
            }
            true
        }
        Some(_) => false,
    };
    if changed
        && let Some(country) = checkout
            .shipping_address
            .as_ref()
            .and_then(|address| address.country_code.as_deref())
            .and_then(|code| store.country(code))
    {
        session.country = Some(country.iso_code.clone());
    }
}

/// The checkout of a session, caught up with the customer of the session: what the page, the
/// control API or the login form of the theme did since it was last read. `prepared` is who the
/// checkout was last prepared for.
pub fn prepare(
    store: &Store,
    session: &mut Session,
    checkout: &mut CheckoutState,
    prepared: &mut Option<Identity>,
) {
    let now = Identity::of(store, session);
    if *prepared != Some(now) {
        transition(store, session, checkout, *prepared, now);
        *prepared = Some(now);
    }
}

/// `{"type": "customer", "customer": "<email>" | "default" | null, "location": "<name or id>"}`:
/// logs the checkout in as a customer of the store data, or out with `null`.
pub fn log_in(
    store: &Store,
    session: &mut Session,
    checkout: &mut CheckoutState,
    operation: &Json,
) -> Result<(), String> {
    let who = match operation.get("customer") {
        None | Some(Json::Null) => "none",
        Some(Json::String(who)) => who.as_str(),
        Some(_) => {
            return Err(
                "the `customer` of a `customer` operation is an email, or null to log out".into(),
            );
        }
    };
    let customer = store.customer_named(who)?;
    let location = location_param(operation)
        .map(|wanted| location_of(store, customer, &wanted))
        .transpose()?;
    switch_to(
        store,
        session,
        checkout,
        customer.map(|customer| customer.id),
        location,
    );
    Ok(())
}

/// Logs the checkout in as `customer` (`None`: out), at `location` for a B2B customer: what the
/// `customer` operation, `/checkout?customer=` and `/checkout?scenario=…&customer=` all do, so
/// that a login gives the same checkout wherever it comes from. A login from a URL is the login
/// of the menu made on the checkout the URL opened: the scenario's entries first, then the login.
pub fn switch_to(
    store: &Store,
    session: &mut Session,
    checkout: &mut CheckoutState,
    customer: Option<u64>,
    location: Option<u64>,
) {
    let before = Identity::of(store, session);
    if session.customer_id != customer {
        session.customer_id = customer;
        session.company_location = None;
    }
    if location.is_some() {
        session.company_location = location;
    }
    transition(
        store,
        session,
        checkout,
        Some(before),
        Identity::of(store, session),
    );
}

/// `{"type": "companyLocation", "location": "<name or id>"}`: the location the logged-in B2B
/// customer buys for, one of theirs.
pub fn choose_location(
    store: &Store,
    session: &mut Session,
    checkout: &mut CheckoutState,
    operation: &Json,
) -> Result<(), String> {
    let wanted = location_param(operation).ok_or("a `companyLocation` needs a `location`")?;
    let customer = session.customer_id.and_then(|id| store.customer_by_id(id));
    let before = Identity::of(store, session);
    session.company_location = Some(location_of(store, customer, &wanted)?);
    transition(
        store,
        session,
        checkout,
        Some(before),
        Identity::of(store, session),
    );
    Ok(())
}

fn location_param(operation: &Json) -> Option<String> {
    match operation.get("location")? {
        Json::String(text) => Some(text.trim().to_string()).filter(|text| !text.is_empty()),
        Json::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// A location the customer can buy for, by its id (`42`, `gid://shopify/CompanyLocation/42`)
/// or its name.
fn location_of(store: &Store, customer: Option<&Customer>, wanted: &str) -> Result<u64, String> {
    let id = wanted
        .strip_prefix("gid://shopify/CompanyLocation/")
        .unwrap_or(wanted)
        .parse::<u64>()
        .ok();
    if let (Some(id), Some(customer)) = (id, customer)
        && let Some(company) = customer.company
        && customer
            .company_locations
            .iter()
            .any(|index| store.companies[company].locations[*index].id == id)
    {
        return Ok(id);
    }
    lsf_core::store::build::resolve_company_location(store, customer, wanted)
}

/// One customer, as the picker of the page (and the home page) lists them.
pub fn profile_json(customer: &Customer, store: &Store, current: bool) -> Json {
    let company = customer.company.map(|index| &store.companies[index]);
    let country = saved_addresses(customer, store, None)
        .first()
        .map(|saved| saved.address.country_code.to_ascii_uppercase())
        .filter(|code| !code.is_empty());
    json!({
        "email": customer.email,
        "name": full_name(customer),
        "tags": customer.tags,
        "ordersCount": customer.orders.len(),
        "addressesCount": customer.addresses.len(),
        "company": company.map(|company| company.name.as_str()),
        "locationsCount": customer.company_locations.len(),
        "countryCode": country,
        "current": current,
    })
}

fn address_json(saved: &SavedAddress<'_>) -> Json {
    json!({
        "id": saved.id,
        "location": saved.location,
        "address": checkout_address(saved.address),
    })
}

/// `account` of the state: what the page needs to log in as someone, and what the logged-in
/// customer brings.
///
/// - `profiles`: every customer of the store data, for the picker;
/// - `addresses`: the addresses the logged-in customer chooses from, the default one first;
/// - `locations`: the locations a B2B customer can buy for, with the one they buy for.
pub fn account_json(store: &Store, session: &Session) -> Json {
    let identity = Identity::of(store, session);
    let customer = identity.customer.and_then(|id| store.customer_by_id(id));
    let profiles: Vec<Json> = store
        .customers
        .iter()
        .map(|candidate| profile_json(candidate, store, Some(candidate.id) == identity.customer))
        .collect();
    let addresses: Vec<Json> = customer
        .map(|customer| saved_addresses(customer, store, identity.location))
        .unwrap_or_default()
        .iter()
        .map(address_json)
        .collect();
    let locations: Vec<Json> = customer
        .and_then(|customer| Some((customer, &store.companies[customer.company?])))
        .map(|(customer, company)| {
            customer
                .company_locations
                .iter()
                .map(|index| {
                    let location = &company.locations[*index];
                    json!({
                        "id": format!("gid://shopify/CompanyLocation/{}", location.id),
                        "name": location.name,
                        "current": identity.location == Some(location.id),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({"profiles": profiles, "addresses": addresses, "locations": locations})
}
