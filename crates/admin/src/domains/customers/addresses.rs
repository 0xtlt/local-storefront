//! A customer's address book: `customerAddressCreate`, `customerAddressUpdate`,
//! `customerAddressDelete`, `customerUpdateDefaultAddress`, and the deprecated `addresses` of
//! `customerUpdate` and `customerSet`.
//!
//! The addresses are inline `MailingAddress`es of the customer (`addresses` and `addressesV2`,
//! the default first, as the seed has them; `defaultAddress` a copy of it), with the ids Shopify
//! gives them (`gid://shopify/MailingAddress/<n>?model_name=CustomerAddress`, as in
//! shopify.dev's examples), the numbers of a counter of the shop. Store sync writes them back
//! with these ids, the default first, so the checkout offers them.
//!
//! As shopify.dev describes them (reference of 2026-10, and the REST reference of the same
//! resource for what GraphQL does not say):
//!
//! - An address takes only the fields given; `countryCode` names the country (`country` and
//!   `province`, deprecated, are still read). An address without a country is accepted: the
//!   `customerUpdate` example creates `{address1, city}` alone. Shopify writes the province's
//!   name for its code (`ON` → `Ontario`); lsf has no table of provinces and keeps the code
//!   when the name is not given *(chosen)*.
//! - The first address of a customer is their default one; `setAsDefault: true` makes the
//!   address the default.
//! - The default address cannot be deleted: "Cannot delete the customer’s default address" (the
//!   REST reference's error; GraphQL's message is not published).
//! - `addresses` of `customerUpdate` and `customerSet` replace the address book: "all included
//!   entries are either created or updated, and all existing entries not included will be
//!   deleted" (`customerSet`'s description; `customerUpdate`'s examples do the same).
//! - Events: `customers/update`, whose payload has the addresses.

use lsf_core::store::Address;
use lsf_core::store::reference::{country_code_from_name, country_name, is_country_code};
use serde_json::{Map, Value, json};

use super::given_text;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, parse_gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("customerAddressCreate", address_create)
        .mutation("customerAddressUpdate", address_update)
        .mutation("customerAddressDelete", address_delete)
        .mutation("customerUpdateDefaultAddress", update_default_address);
}

/// The model name of the addresses of a customer, in their GIDs.
const MODEL: &str = "CustomerAddress";

/// The fields of an address as lsf-core has them, from a `MailingAddress` value.
fn address_struct(address: &Value, id: u64) -> Address {
    let text = |name: &str| {
        address
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Address {
        id,
        first_name: text("firstName"),
        last_name: text("lastName"),
        company: text("company"),
        address1: text("address1"),
        address2: text("address2"),
        city: text("city"),
        province: text("province"),
        province_code: text("provinceCode"),
        country: text("country"),
        country_code: text("countryCodeV2"),
        zip: text("zip"),
        phone: text("phone"),
    }
}

/// The address `base` (a `MailingAddress` value, or none for a new one) with the fields of a
/// `MailingAddressInput`, as a `MailingAddress` of id `id`.
pub fn address_of(input: &Map<String, Value>, id: u64, base: Option<&Value>) -> Value {
    let mut address = base.map_or_else(
        || Address {
            id,
            ..Address::default()
        },
        |base| address_struct(base, id),
    );
    let set = |field: &mut String, name: &str| {
        if let Some(given) = given_text(input, name) {
            *field = given.unwrap_or_default();
        }
    };
    set(&mut address.first_name, "firstName");
    set(&mut address.last_name, "lastName");
    set(&mut address.company, "company");
    set(&mut address.address1, "address1");
    set(&mut address.address2, "address2");
    set(&mut address.city, "city");
    set(&mut address.zip, "zip");
    set(&mut address.phone, "phone");
    if let Some(code) = given_text(input, "countryCode") {
        let code = code.unwrap_or_default().to_ascii_uppercase();
        address.country = country_name(&code).map_or_else(|| code.clone(), str::to_string);
        address.country_code = code;
    } else if let Some(country) = given_text(input, "country") {
        let country = country.unwrap_or_default();
        let upper = country.to_ascii_uppercase();
        if upper.len() == 2 && is_country_code(&upper) {
            address.country = country_name(&upper).map_or_else(|| upper.clone(), str::to_string);
            address.country_code = upper;
        } else {
            address.country_code = country_code_from_name(&country)
                .unwrap_or_default()
                .to_string();
            address.country = country;
        }
    }
    let province = given_text(input, "province");
    if let Some(code) = given_text(input, "provinceCode") {
        let code = code.unwrap_or_default().to_ascii_uppercase();
        if province.is_none() {
            address.province = code.clone();
        }
        address.province_code = code;
    }
    if let Some(name) = province {
        let name = name.unwrap_or_default();
        // A province given as its code (`ON`), as the deprecated field often is.
        if given_text(input, "provinceCode").is_none()
            && (2..=3).contains(&name.len())
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            address.province_code = name.clone();
        }
        address.province = name;
    }
    value::mailing_address(&address, MODEL)
}

/// A new address id, from the shop's counter of `MailingAddress`.
pub fn new_address_id(state: &mut ShopState) -> u64 {
    parse_gid(&state.new_gid("MailingAddress")).map_or(1, |(_, id)| id)
}

/// The number of an address GID, with or without its `?model_name=`.
fn address_number(gid: &str) -> Option<u64> {
    parse_gid(gid)
        .filter(|(type_name, _)| *type_name == "MailingAddress")
        .map(|(_, id)| id)
}

/// The addresses of a customer record.
pub fn addresses(record: &Record) -> Vec<Value> {
    record
        .get("addressesV2")
        .or_else(|| record.get("addresses"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The position of the address `id` in a customer's address book.
fn position(record: &Record, id: &str) -> Option<usize> {
    let number = address_number(id)?;
    addresses(record).iter().position(|address| {
        address
            .get("id")
            .and_then(Value::as_str)
            .and_then(address_number)
            == Some(number)
    })
}

/// Whether the address at `index` is the customer's default.
fn is_default(record: &Record, address: &Value) -> bool {
    let id = |value: &Value| {
        value
            .get("id")
            .and_then(Value::as_str)
            .and_then(address_number)
    };
    record
        .get("defaultAddress")
        .is_some_and(|default| id(default).is_some() && id(default) == id(address))
}

/// Writes a customer's address book: `addresses` and `addressesV2`, the default (`default`, an
/// index into `list`; the first when `None`) first, and `defaultAddress`.
pub fn write_addresses(record: &mut Record, mut list: Vec<Value>, default: Option<usize>) {
    if let Some(index) = default.filter(|index| *index < list.len()) {
        let address = list.remove(index);
        list.insert(0, address);
    }
    record.insert("defaultAddress".into(), json!(list.first()));
    record.insert("addresses".into(), json!(list));
    record.insert("addressesV2".into(), json!(list));
}

/// Replaces a customer's address book with the `addresses` of an input (`customerUpdate`,
/// `customerSet`): an entry with the `id` of an address updates it, another is created, the
/// addresses not named are deleted. The default stays when it is kept, the first otherwise.
pub fn replace_addresses(state: &mut ShopState, customer: &str, inputs: &[Value]) {
    let Some(record) = state.get(customer).cloned() else {
        return;
    };
    let current = addresses(&record);
    let default = current
        .iter()
        .find(|address| is_default(&record, address))
        .and_then(|address| address.get("id").and_then(Value::as_str))
        .and_then(address_number);
    let mut list = Vec::new();
    for input in inputs.iter().filter_map(Value::as_object) {
        let existing = input
            .get("id")
            .and_then(Value::as_str)
            .and_then(address_number)
            .and_then(|number| {
                current.iter().find(|address| {
                    address
                        .get("id")
                        .and_then(Value::as_str)
                        .and_then(address_number)
                        == Some(number)
                })
            });
        let address = match existing {
            Some(base) => {
                let id = base
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(address_number)
                    .unwrap_or_default();
                address_of(input, id, Some(base))
            }
            None => {
                let id = new_address_id(state);
                address_of(input, id, None)
            }
        };
        list.push(address);
    }
    let kept_default = list.iter().position(|address| {
        address
            .get("id")
            .and_then(Value::as_str)
            .and_then(address_number)
            == default
    });
    if let Some(record) = state.get_mut(customer) {
        write_addresses(record, list, kept_default);
    }
}

/// The customer a mutation names, or the error of one that does not exist.
fn customer_of(state: &ShopState, ctx: &FieldContext<'_>) -> Result<String, UserError> {
    let id = ctx.arg_str("customerId").unwrap_or_default();
    if state.type_of(id) == Some("Customer") {
        Ok(id.to_string())
    } else {
        Err(UserError::new(&["customerId"], "Customer does not exist"))
    }
}

/// The address of a customer an `addressId` names, or its error (not published: lsf's words).
fn address_index(record: &Record, ctx: &FieldContext<'_>) -> Result<usize, UserError> {
    let id = ctx.arg_str("addressId").unwrap_or_default();
    position(record, id).ok_or_else(|| UserError::new(&["addressId"], "Address does not exist"))
}

/// `customerAddressCreate`.
fn address_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let customer = match customer_of(mutation.state(), ctx) {
        Ok(customer) => customer,
        Err(error) => return Ok(payload(json!({"address": null}), &[error])),
    };
    let input = ctx
        .arg("address")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let set_as_default = ctx.arg("setAsDefault").and_then(Value::as_bool) == Some(true);
    let state = mutation.state_mut();
    let id = new_address_id(state);
    let address = address_of(&input, id, None);
    if let Some(record) = state.get_mut(&customer) {
        let mut list = addresses(record);
        let current_default = list.iter().position(|item| is_default(record, item));
        list.push(address.clone());
        let default = if set_as_default || current_default.is_none() {
            Some(list.len() - 1)
        } else {
            current_default
        };
        write_addresses(record, list, default);
    }
    state.touch(&customer);
    state.emit("customers/update", &customer);
    Ok(payload(json!({"address": address}), &[]))
}

/// `customerAddressUpdate`.
fn address_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let found = customer_of(state, ctx).and_then(|customer| {
        let record = state.get(&customer).cloned().unwrap_or_default();
        address_index(&record, ctx).map(|index| (customer, record, index))
    });
    let (customer, record, index) = match found {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"address": null}), &[error])),
    };
    let input = ctx
        .arg("address")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut list = addresses(&record);
    let base = list[index].clone();
    let id = base
        .get("id")
        .and_then(Value::as_str)
        .and_then(address_number)
        .unwrap_or_default();
    let address = address_of(&input, id, Some(&base));
    list[index] = address.clone();
    let default = if ctx.arg("setAsDefault").and_then(Value::as_bool) == Some(true) {
        Some(index)
    } else {
        list.iter().position(|item| is_default(&record, item))
    };
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&customer) {
        write_addresses(record, list, default);
    }
    state.touch(&customer);
    state.emit("customers/update", &customer);
    Ok(payload(json!({"address": address}), &[]))
}

/// `customerAddressDelete`.
fn address_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let found = customer_of(state, ctx).and_then(|customer| {
        let record = state.get(&customer).cloned().unwrap_or_default();
        address_index(&record, ctx).map(|index| (customer, record, index))
    });
    let (customer, record, index) = match found {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"deletedAddressId": null}), &[error])),
    };
    let mut list = addresses(&record);
    if is_default(&record, &list[index]) {
        let error = UserError::new(
            &["addressId"],
            "Cannot delete the customer\u{2019}s default address",
        );
        return Ok(payload(json!({"deletedAddressId": null}), &[error]));
    }
    let removed = list.remove(index);
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&customer) {
        let default = list.iter().position(|item| is_default(record, item));
        write_addresses(record, list, default);
    }
    state.touch(&customer);
    state.emit("customers/update", &customer);
    Ok(payload(json!({"deletedAddressId": removed.get("id")}), &[]))
}

/// `customerUpdateDefaultAddress`.
fn update_default_address(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let found = customer_of(state, ctx).and_then(|customer| {
        let record = state.get(&customer).cloned().unwrap_or_default();
        address_index(&record, ctx).map(|index| (customer, record, index))
    });
    let (customer, record, index) = match found {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"customer": null}), &[error])),
    };
    let list = addresses(&record);
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&customer) {
        write_addresses(record, list, Some(index));
    }
    state.touch(&customer);
    state.emit("customers/update", &customer);
    Ok(payload(json!({"customer": customer}), &[]))
}
