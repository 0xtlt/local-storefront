//! What the queries of the family need beyond the generic rules: the search filters of
//! `customers` that are not a field of the record (`accepts_marketing`, `country`,
//! `orders_count`, `total_spent`, `customer_date`, `order_date`), the sort keys that are not a
//! field (`CustomerSortKeys.NAME` and `LOCATION`, `CompanySortKeys.ORDER_COUNT` and
//! `SINCE_DATE`, …), and the counts of a company.
//!
//! The filters are those `customers` documents in 2026-10 (`schemas/2026-10/search.json`); the
//! others (`email`, `phone`, `first_name`, `last_name`, `state`, `tag`, `updated_at`, `id`) the
//! generic search reads from the record. `last_abandoned_order_date` reads the customer's last
//! abandoned checkout of the local checkout (`domains::checkouts`).

use serde_json::Value;

use super::gids_of;
use crate::graphql::Output;
use crate::resolve::sort::SortValue;
use crate::resolve::{Registry, ShopResolver};
use crate::search::SearchValue;
use crate::state::{Record, parse_gid};

pub fn register(registry: &mut Registry) {
    registry
        .search_field("Customer", "accepts_marketing", |_r, _item, customer| {
            Some(vec![SearchValue::Bool(
                customer
                    .get("emailMarketingConsent")
                    .and_then(|consent| consent.get("marketingState"))
                    .and_then(Value::as_str)
                    == Some("SUBSCRIBED"),
            )])
        })
        .search_field("Customer", "country", |_r, _item, customer| {
            Some(
                addresses(customer)
                    .iter()
                    .flat_map(|address| ["country", "countryCodeV2"].map(|f| address.get(f)))
                    .filter_map(|value| value.and_then(Value::as_str))
                    .map(|country| SearchValue::Keyword(country.to_string()))
                    .collect(),
            )
        })
        .search_field("Customer", "customer_date", |_r, _item, customer| {
            Some(dates([customer.get("createdAt")]))
        })
        .search_field("Customer", "order_date", |r, _item, customer| {
            let orders = gids_of(Some(customer), "orders");
            Some(dates(orders.iter().map(|order| {
                r.state()
                    .get(order)
                    .and_then(|order| order.get("processedAt").or_else(|| order.get("createdAt")))
            })))
        })
        .search_field("Customer", "orders_count", |_r, _item, customer| {
            Some(number(customer.get("numberOfOrders")))
        })
        .search_field("Customer", "total_spent", |_r, _item, customer| {
            Some(number(
                customer
                    .get("amountSpent")
                    .and_then(|amount| amount.get("amount")),
            ))
        })
        // The customer's last abandoned checkout (`domains::checkouts`).
        .search_field(
            "Customer",
            "last_abandoned_order_date",
            |r, _item, customer| {
                let id = customer
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Some(
                    crate::domains::checkouts::last_abandonment(r.state(), id)
                        .map(SearchValue::Date)
                        .into_iter()
                        .collect(),
                )
            },
        )
        .default_text("Customer", |_r, customer| {
            let mut texts: Vec<String> = ["displayName", "firstName", "lastName", "email", "phone"]
                .iter()
                .filter_map(|field| customer.get(*field).and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            texts.extend(crate::domains::catalog::tags_of(customer.get("tags")));
            texts
        })
        // "Sort by the `name` value": the name the customer is shown by.
        .sort_key("Customer", "NAME", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|customer| customer.get("displayName")),
            )
        })
        // "Sort by the `location` value": the country, province and city of the default address
        // *(chosen: Shopify does not say which parts)*.
        .sort_key("Customer", "LOCATION", |r, item| {
            let Some(address) = r
                .item_map(item)
                .and_then(|customer| customer.get("defaultAddress"))
                .filter(|address| address.is_object())
            else {
                return SortValue::Missing;
            };
            let text = |field: &str| address.get(field).and_then(Value::as_str).unwrap_or("");
            SortValue::Text(
                format!("{} {} {}", text("country"), text("province"), text("city")).to_lowercase(),
            )
        })
        .sort_key("Company", "ORDER_COUNT", |r, item| {
            r.item_map(item).map_or(SortValue::Missing, |company| {
                SortValue::Number(orders_count(r, company) as f64)
            })
        })
        .sort_key("Company", "SINCE_DATE", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|company| company.get("customerSince")),
            )
        })
        .sort_key("CompanyLocation", "COMPANY_ID", |r, item| {
            company_id(r, item)
        })
        .sort_key("CompanyLocation", "COMPANY_AND_LOCATION_NAME", |r, item| {
            let Some(location) = r.item_map(item) else {
                return SortValue::Missing;
            };
            let company = location
                .get("company")
                .and_then(Value::as_str)
                .and_then(|company| r.state().get(company))
                .and_then(|company| company.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let name = location.get("name").and_then(Value::as_str).unwrap_or("");
            SortValue::Text(format!("{company} {name}").to_lowercase())
        })
        .sort_key("CompanyContact", "COMPANY_ID", |r, item| {
            company_id(r, item)
        })
        .sort_key("CompanyContact", "EMAIL", |r, item| {
            SortValue::of(contact_customer(r, item).and_then(|customer| customer.get("email")))
        })
        .sort_key("CompanyContact", "NAME", |r, item| {
            SortValue::of(
                contact_customer(r, item).and_then(|customer| customer.get("displayName")),
            )
        })
        .sort_key("CompanyContact", "NAME_EMAIL", |r, item| {
            let Some(customer) = contact_customer(r, item) else {
                return SortValue::Missing;
            };
            let text = |field: &str| customer.get(field).and_then(Value::as_str).unwrap_or("");
            SortValue::Text(format!("{} {}", text("displayName"), text("email")).to_lowercase())
        })
        // The orders of a company: those of its locations (the data counts them per location).
        .field("Company", "ordersCount", |r, ctx, object| {
            match r.map_of(object) {
                Some(company) => Ok(r.count(ctx, orders_count(r, company))),
                None => r.generic(ctx, object),
            }
        })
        // Deprecated for `contactsCount`.
        .field("Company", "contactCount", |r, _ctx, object| {
            Ok(Output::value(r.items_of(object, "contacts").len()))
        });
}

/// The addresses of a customer.
fn addresses(customer: &Record) -> Vec<Value> {
    customer
        .get("addressesV2")
        .or_else(|| customer.get("addresses"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Dates written in RFC 3339, as search values.
fn dates<'v>(values: impl IntoIterator<Item = Option<&'v Value>>) -> Vec<SearchValue> {
    values
        .into_iter()
        .filter_map(|value| value.and_then(Value::as_str))
        .filter_map(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|date| SearchValue::Date(date.with_timezone(&chrono::Utc)))
        .collect()
}

/// A number, or a decimal string, as a search value.
fn number(value: Option<&Value>) -> Vec<SearchValue> {
    let number = match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => text.trim().parse().ok(),
        _ => None,
    };
    number.map(SearchValue::Number).into_iter().collect()
}

/// The orders of a company's locations.
fn orders_count(r: &ShopResolver<'_>, company: &Record) -> usize {
    gids_of(Some(company), "locations")
        .iter()
        .filter_map(|location| r.state().get(location))
        .filter_map(|location| {
            location
                .get("ordersCount")
                .and_then(|count| count.get("count"))
                .and_then(Value::as_u64)
        })
        .sum::<u64>() as usize
}

/// The number of the company of a location or a contact.
fn company_id(r: &ShopResolver<'_>, item: &Value) -> SortValue {
    r.item_map(item)
        .and_then(|record| record.get("company"))
        .and_then(Value::as_str)
        .and_then(parse_gid)
        .map_or(SortValue::Missing, |(_, id)| SortValue::Number(id as f64))
}

/// The customer of a contact.
fn contact_customer<'r>(r: &'r ShopResolver<'_>, item: &Value) -> Option<&'r Record> {
    let customer = r.item_map(item)?.get("customer")?.as_str()?.to_string();
    r.state().get(&customer)
}
