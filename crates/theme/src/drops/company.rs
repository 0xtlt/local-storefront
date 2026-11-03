//! `company`, `company_location` and `company_address`: what a B2B customer buys for.

use std::any::Any;

use lsf_liquid::{Object, Value};

use super::metafield::MetafieldsDrop;
use super::shop::CountryName;
use super::{Memo, PaginatedList, SiteRef, hash};
use crate::store::{Address, Company, CompanyLocation, Customer};

/// The locations of their company a customer can buy for.
pub fn available_locations(site: &SiteRef, customer: &Customer) -> Value {
    let Some(company) = customer.company else {
        return Value::array(Vec::new());
    };
    PaginatedList::value(
        customer
            .company_locations
            .iter()
            .map(|location| CompanyLocationDrop::value(site, customer.id, company, *location))
            .collect(),
    )
}

pub struct CompanyDrop {
    site: SiteRef,
    /// The customer the company is seen by: they decide which locations are available.
    customer: u64,
    company: usize,
    memo: Memo,
}

impl CompanyDrop {
    pub fn value(site: &SiteRef, customer: u64, company: usize) -> Value {
        Value::object(CompanyDrop {
            site: site.clone(),
            customer,
            company,
            memo: Memo::default(),
        })
    }

    fn company(&self) -> &Company {
        &self.site.store.companies[self.company]
    }
}

impl Object for CompanyDrop {
    fn type_name(&self) -> &str {
        "company"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let company = self.company();
        let customer = site.store.customer_by_id(self.customer);
        Some(match key {
            "id" => Value::Int(company.id as i64),
            "name" => Value::from(&company.name),
            "external_id" => company.external_id.as_ref().map_or(Value::Nil, Value::from),
            "metafields" => MetafieldsDrop::value(site, &company.metafields),
            "available_locations" => self.memo.get("available_locations", || {
                customer.map_or_else(
                    || Value::array(Vec::new()),
                    |customer| available_locations(site, customer),
                )
            }),
            "available_locations_count" => {
                Value::from(customer.map_or(0, |customer| customer.company_locations.len()))
            }
            _ => return None,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("company:{}", self.company().id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct CompanyLocationDrop {
    site: SiteRef,
    customer: u64,
    company: usize,
    location: usize,
}

impl CompanyLocationDrop {
    pub fn value(site: &SiteRef, customer: u64, company: usize, location: usize) -> Value {
        Value::object(CompanyLocationDrop {
            site: site.clone(),
            customer,
            company,
            location,
        })
    }

    fn location(&self) -> &CompanyLocation {
        &self.site.store.companies[self.company].locations[self.location]
    }
}

/// The path that makes a location the one a B2B customer buys for, then comes back.
pub fn location_switch_path(location: u64, return_to: &str) -> String {
    format!("/company_location/update?location_id={location}&return_to={return_to}")
}

/// A `company_address`: the address of a company location.
fn company_address(address: &Address, company: &Company) -> Value {
    let optional = |text: &str| {
        if text.is_empty() {
            Value::Nil
        } else {
            Value::from(text)
        }
    };
    let street = [address.address1.as_str(), address.address2.as_str()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    hash([
        ("id", Value::Int(address.id as i64)),
        (
            "attention",
            Value::from(if address.company.is_empty() {
                &company.name
            } else {
                &address.company
            }),
        ),
        ("first_name", optional(&address.first_name)),
        ("last_name", optional(&address.last_name)),
        ("address1", Value::from(&address.address1)),
        ("address2", optional(&address.address2)),
        ("street", Value::from(street)),
        ("city", Value::from(&address.city)),
        ("province", optional(&address.province)),
        ("province_code", optional(&address.province_code)),
        ("zip", Value::from(&address.zip)),
        (
            "country",
            Value::object(CountryName {
                name: address.country.clone(),
                iso_code: address.country_code.clone(),
            }),
        ),
        ("country_code", Value::from(&address.country_code)),
    ])
}

impl Object for CompanyLocationDrop {
    fn type_name(&self) -> &str {
        "company_location"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let location = self.location();
        Some(match key {
            "id" => Value::Int(location.id as i64),
            "name" => Value::from(&location.name),
            "external_id" => location
                .external_id
                .as_ref()
                .map_or(Value::Nil, Value::from),
            "company" => CompanyDrop::value(site, self.customer, self.company),
            "current?" => Value::Bool(
                site.store
                    .customer_by_id(self.customer)
                    .and_then(|customer| {
                        customer.current_location(site.session.company_location, &site.store)
                    })
                    == Some(self.location),
            ),
            "metafields" => MetafieldsDrop::value(site, &location.metafields),
            "shipping_address" => location
                .shipping_address
                .as_ref()
                .map_or(Value::Nil, |address| {
                    company_address(address, &site.store.companies[self.company])
                }),
            "tax_registration_id" => location
                .tax_registration_id
                .as_ref()
                .map_or(Value::Nil, Value::from),
            "url_to_set_as_current" => Value::from(format!(
                "{}{}",
                site.request.origin(),
                location_switch_path(location.id, &site.request.localized(&site.request.path))
            )),
            "store_credit_account" => Value::Nil,
            _ => return None,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("company_location:{}", self.location().id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
