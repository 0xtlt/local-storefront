//! The customer account of an app: the pages of Shopify's new customer accounts where its
//! customer account UI extensions render.
//!
//! - `/account`: the orders of the customer logged in;
//! - `/account/orders/<id>`: the status of one of them;
//! - `/account/profile`: their name, contact, addresses and, for a B2B customer, their company
//!   and its locations;
//! - `/account/pages/<handle>/…`: the full page of an extension (`customer-account.page.render`);
//! - `/account/orders/<id>/pages/<handle>/…`: the full page of an extension for an order
//!   (`customer-account.order.page.render`).
//!
//! These pages replace lsf's stand-in for hosted accounts (`account.rs`) when the server has an
//! app, and only then: a theme that renders its accounts (`templates/customers`) keeps them, as
//! Shopify does not run customer account UI extensions with them. Logged out, the page lists
//! the customers of the store data to log in as.
//!
//! The orders are those of the customer in the store data, and the one the session placed last
//! in the local checkout. An order is written like the state of a checkout (lines, cost,
//! addresses), so that the page and the sandbox read it the same way; shipments come from its
//! `fulfillment_status`, returns from its `returns`.

use lsf_checkout::app::Selection;
use lsf_core::store::{Customer, Order, Store};
use serde_json::{Value as Json, json};

use super::checkout::{self, origin_of};
use super::checkout_account::{self, Identity, checkout_address};
use super::reply::Reply;
use super::{Incoming, ServerState};

/// The page a path of the customer account is.
#[derive(Debug, PartialEq, Eq)]
enum Page<'a> {
    Orders,
    Order(&'a str),
    Profile,
    /// A full page: the handle of the extension, the order it is for, and the path in it.
    Full {
        handle: &'a str,
        order: Option<&'a str>,
        rest: String,
    },
}

impl<'a> Page<'a> {
    fn parse(segments: &[&'a str]) -> Option<Page<'a>> {
        let rest = |parts: &[&str]| format!("/{}", parts.join("/"));
        match segments {
            [] | ["orders"] => Some(Page::Orders),
            ["orders", id] => Some(Page::Order(id)),
            ["profile"] => Some(Page::Profile),
            ["pages", handle, more @ ..] => Some(Page::Full {
                handle,
                order: None,
                rest: rest(more),
            }),
            ["orders", id, "pages", handle, more @ ..] => Some(Page::Full {
                handle,
                order: Some(id),
                rest: rest(more),
            }),
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Page::Orders => "orders",
            Page::Order(_) => "order",
            Page::Profile => "profile",
            Page::Full { order: None, .. } => "page",
            Page::Full { order: Some(_), .. } => "order-page",
        }
    }
}

/// Whether `/account` is the customer account of the app: the store's accounts are Shopify's
/// (hosted), not the theme's.
pub fn serves(state: &ServerState, store: &Store) -> bool {
    state.options.checkout.is_some() && lsf_core::site::hosted_accounts(&state.app.theme, store)
}

/// A page of the customer account, for the segments after `account`; `None` for a path that is
/// not one, which the storefront answers.
pub fn route(state: &ServerState, incoming: &Incoming, segments: &[&str]) -> Option<Reply> {
    let page = Page::parse(segments)?;
    let (id, is_new) = state.session_id(incoming);
    let reply = match state_json(state, incoming, &id, &page) {
        Ok(json) => checkout::shell_of(state, &json, "customer-account.html"),
        Err(reply) => reply,
    };
    Some(checkout::with_cookie(reply, &id, is_new))
}

/// What the page draws, and the extensions read.
fn state_json(
    state: &ServerState,
    incoming: &Incoming,
    session_id: &str,
    page: &Page<'_>,
) -> Result<Json, Reply> {
    let (session, store) = state.snapshot(session_id);
    let options = checkout::options(state);
    let app = state
        .load_app(&options.app_dir)
        .map_err(|problem| Reply::text(500, problem))?;
    let selection = Selection::resolve(&app, &options.extensions)
        .map_err(|problem| Reply::text(500, problem))?;
    let language = checkout::language_of(state, &store, incoming);
    let root = checkout::root_of(language);
    let identity = Identity::of(&store, &session);
    let customer = identity.customer.and_then(|id| store.customer_by_id(id));
    let placed = state
        .with_session(session_id, |entry| entry.checkout.completed.clone())
        .filter(|order| belongs_to(order, customer));
    let mut orders: Vec<Json> = placed.into_iter().map(placed_order_json).collect();
    if let Some(customer) = customer {
        orders.extend(
            customer
                .orders
                .iter()
                .map(|order| order_json(&store, order)),
        );
    }

    let order = match page {
        Page::Order(id)
        | Page::Full {
            order: Some(id), ..
        } => {
            let found = orders
                .iter()
                .find(|order| id.parse::<u64>().ok() == order["legacyId"].as_u64());
            match (found, customer) {
                (Some(order), _) => Some(order.clone()),
                // Logged out: the page asks to log in first.
                (None, None) => None,
                (None, Some(_)) => {
                    return Err(Reply::text(
                        404,
                        format!("The customer logged in has no order {id}."),
                    ));
                }
            }
        }
        _ => None,
    };
    let extension_page = match page {
        Page::Full { handle, rest, .. } => {
            let declared = selection.ui_extensions(&app).any(|extension| {
                extension.handle == *handle
                    && extension.targets.iter().any(|target| {
                        target.target
                            == if matches!(page, Page::Full { order: Some(_), .. }) {
                                "customer-account.order.page.render"
                            } else {
                                "customer-account.page.render"
                            }
                    })
            });
            if !declared {
                return Err(Reply::text(
                    404,
                    format!("No extension of the app has a full page named {handle}."),
                ));
            }
            Some(json!({"handle": handle, "path": rest}))
        }
        _ => None,
    };

    let company = customer.and_then(|customer| company_json(&store, customer, identity.location));
    let profiles: Vec<Json> = store
        .customers
        .iter()
        .map(|candidate| {
            checkout_account::profile_json(
                candidate,
                &store,
                Some(candidate.id) == identity.customer,
            )
        })
        .collect();
    let mut json = json!({
        "surface": "customer-account",
        "page": page.name(),
        "root": format!("{root}/account"),
        "extensionPage": extension_page,
        "home": checkout::home_path(state, root),
        "shop": {
            "name": store.shop.name,
            "currencyCode": store.shop.currency,
            "moneyFormat": store.shop.money_format,
            "locale": store.primary_language().iso_code,
            "storefrontUrl": origin_of(incoming),
            "myshopifyDomain": store.shop.permanent_domain,
            "countries": store.countries.iter()
                .map(|country| json!({"code": country.iso_code, "name": country.name}))
                .collect::<Vec<_>>(),
        },
        "customer": customer.map(|customer| customer_json(&store, customer)),
        "buyer": {
            "customer": customer.map(checkout::customer_json),
            "purchasingCompany": customer.and_then(|customer| {
                checkout::purchasing_company_json(customer, &store, session.company_location)
            }),
        },
        "company": company,
        "orders": orders,
        "order": order,
        "localization": checkout::localization_json(
            &store,
            language,
            &session,
            &lsf_checkout::CheckoutState::default(),
        ),
        "profiles": profiles,
        // The checkout's extensions have nothing to show here.
        "extensions": checkout::extensions_json(&app, &selection)
            .as_array()
            .into_iter()
            .flatten()
            .filter(|extension| has_account_target(extension))
            .cloned()
            .collect::<Vec<_>>(),
        "app": {"clientId": app.client_id},
        "diagnostics": app.diagnostics.iter()
            .filter(|diagnostic| match diagnostic.extension.as_deref() {
                None => true,
                Some(handle) => selection.contains(handle) && app.ui_extension(handle).is_some_and(|extension| {
                    extension.targets.iter().any(|target| target.target.starts_with("customer-account."))
                }),
            })
            .map(|diagnostic| json!(diagnostic.to_string()))
            .collect::<Vec<_>>(),
    });
    json["builds"] = state.builds.to_json();
    Ok(json)
}

/// Whether an extension of the state has a target of the customer account.
fn has_account_target(extension: &Json) -> bool {
    extension["targets"].as_array().is_some_and(|targets| {
        targets.iter().any(|target| {
            target
                .as_str()
                .is_some_and(|target| target.starts_with("customer-account."))
        })
    })
}

/// Whether an order the session placed is the customer's: the checkout was logged in as them.
fn belongs_to(order: &Json, customer: Option<&Customer>) -> bool {
    let Some(customer) = customer else {
        return false;
    };
    order["buyer"]["customer"]["id"] == json!(format!("gid://shopify/Customer/{}", customer.id))
}

/// The customer of the account: who they are and the addresses they saved, the default first.
fn customer_json(store: &Store, customer: &Customer) -> Json {
    let mut json = checkout::customer_json(customer);
    json["addresses"] = Json::Array(
        customer
            .addresses
            .iter()
            .enumerate()
            .map(|(index, address)| {
                json!({
                    "id": format!("gid://shopify/CustomerAddress/{}{}", customer.id, index + 1),
                    "default": index == 0,
                    "address": checkout_address(address),
                })
            })
            .collect(),
    );
    json["tags"] = json!(customer.tags);
    json["company"] = json!(
        customer
            .company
            .map(|index| store.companies[index].name.as_str())
    );
    json
}

/// The company of a B2B customer, and the locations they buy for, each with its address and the
/// customers of the store data who buy for it (its staff).
fn company_json(store: &Store, customer: &Customer, current: Option<u64>) -> Option<Json> {
    let index = customer.company?;
    let company = &store.companies[index];
    let locations: Vec<Json> = customer
        .company_locations
        .iter()
        .map(|position| {
            let location = &company.locations[*position];
            let staff: Vec<Json> = store
                .customers
                .iter()
                .filter(|other| {
                    other.company == Some(index) && other.company_locations.contains(position)
                })
                .map(|other| {
                    json!({
                        "name": super::account::full_name(other),
                        "email": other.email,
                        "title": other.company_contact.as_ref().and_then(|contact| contact.title.as_deref()),
                    })
                })
                .collect();
            json!({
                "id": format!("gid://shopify/CompanyLocation/{}", location.id),
                "name": location.name,
                "externalId": location.external_id,
                "address": location.shipping_address.as_ref().map(checkout_address),
                "taxRegistrationId": location.tax_registration_id,
                "current": current == Some(location.id),
                "staff": staff,
            })
        })
        .collect();
    Some(json!({
        "id": format!("gid://shopify/Company/{}", company.id),
        "name": company.name,
        "externalId": company.external_id,
        "locations": locations,
    }))
}

/// An order of the store data, written as the state of a checkout is.
fn order_json(store: &Store, order: &Order) -> Json {
    let currency = &store.shop.currency;
    let lines: Vec<Json> = order
        .line_items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let variant = &store.products[item.variant.0].variants[item.variant.1];
            let mut line = lsf_checkout::pipeline::order_line(store, variant.id, item.quantity)?;
            line.id = format!("gid://shopify/LineItem/{}{:02}", order.id, index + 1);
            serde_json::to_value(line).ok()
        })
        .collect();
    let line_id = |index: usize| lines.get(index).map(|line| line["id"].clone());
    let subtotal: i64 = order
        .line_items
        .iter()
        .map(|item| {
            store.products[item.variant.0].variants[item.variant.1].price * i64::from(item.quantity)
        })
        .sum();
    let money = |cents: i64| json!({"amount": cents as f64 / 100.0});
    // A fulfilled order went out in one shipment; a partial one sent its first line.
    let shipped: Vec<usize> = match (order.cancelled, order.fulfillment_status.as_str()) {
        (true, _) => Vec::new(),
        (false, "fulfilled") => (0..lines.len()).collect(),
        (false, "partial") => (0..lines.len().min(1)).collect(),
        _ => Vec::new(),
    };
    let fulfillments: Vec<Json> = if shipped.is_empty() {
        Vec::new()
    } else {
        vec![json!({
            "id": format!("gid://shopify/Fulfillment/{}1", order.id),
            "status": "SUCCESS",
            "lineIds": shipped.iter().filter_map(|index| line_id(*index)).collect::<Vec<_>>(),
        })]
    };
    let unfulfilled: Vec<Json> = (0..lines.len())
        .filter(|index| !shipped.contains(index) && !order.cancelled)
        .filter_map(line_id)
        .collect();
    let returns: Vec<Json> = order
        .returns
        .iter()
        .enumerate()
        .map(|(position, sent)| {
            json!({
                "id": format!("gid://shopify/Return/{}{}", order.id, position + 1),
                "status": sent.status.to_ascii_uppercase(),
                "lines": sent.lines.iter().filter_map(|(index, quantity)| {
                    Some(json!({"lineId": line_id(*index)?, "quantity": quantity}))
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "id": format!("gid://shopify/Order/{}", order.id),
        "legacyId": order.id,
        "name": order.name,
        "confirmationNumber": confirmation_number(order.id),
        "processedAt": order.created_at.to_rfc3339(),
        "cancelledAt": order.cancelled.then(|| order.created_at.to_rfc3339()),
        "financialStatus": order.financial_status,
        "fulfillmentStatus": order.fulfillment_status,
        "lines": lines,
        "cost": {
            "currencyCode": currency,
            "subtotal": money(subtotal),
            "shipping": money(order.shipping_price),
            "tax": money(order.tax_price),
            "total": money(subtotal + order.shipping_price + order.tax_price),
        },
        "shippingAddress": order.shipping_address.as_ref().map(checkout_address),
        "billingAddress": order.billing_address.as_ref().map(checkout_address),
        "note": (!order.note.is_empty()).then_some(&order.note),
        "attributes": [],
        "discountCodes": [],
        "discounts": [],
        "metafields": [],
        "fulfillments": fulfillments,
        "unfulfilledLineIds": unfulfilled,
        "returns": returns,
    })
}

/// The order the session placed in the local checkout, as `order_json` writes the others.
fn placed_order_json(placed: Json) -> Json {
    let number = placed["order"]["number"]
        .as_str()
        .and_then(|number| number.trim_start_matches('#').parse::<u64>().ok())
        .unwrap_or(1000);
    let mut order = placed;
    let lines: Vec<Json> = order["lines"].as_array().cloned().unwrap_or_default();
    let ids: Vec<Json> = lines.iter().map(|line| line["id"].clone()).collect();
    let object = order.as_object_mut().expect("an order is an object");
    let name = object["order"]["number"].clone();
    object.insert("id".into(), json!(format!("gid://shopify/Order/{number}")));
    object.insert("legacyId".into(), json!(number));
    object.insert("name".into(), name);
    object.insert(
        "confirmationNumber".into(),
        json!(confirmation_number(number)),
    );
    let processed = object["order"]["processedAt"].clone();
    object.insert("processedAt".into(), processed);
    object.insert("cancelledAt".into(), Json::Null);
    object.insert("financialStatus".into(), json!("paid"));
    object.insert("fulfillmentStatus".into(), json!("unfulfilled"));
    object.insert("billingAddress".into(), object["shippingAddress"].clone());
    object.insert("fulfillments".into(), json!([]));
    object.insert("unfulfilledLineIds".into(), Json::Array(ids));
    object.insert("returns".into(), json!([]));
    object.insert("placedHere".into(), json!(true));
    for key in [
        "runs",
        "account",
        "extensions",
        "builds",
        "diagnostics",
        "variant",
        "scenario",
    ] {
        object.remove(key);
    }
    order
}

/// The code Shopify gives the buyer for an order: letters and digits, from its id here.
fn confirmation_number(id: u64) -> String {
    const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut value = id.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (0..9)
        .map(|_| {
            let letter = LETTERS[(value % LETTERS.len() as u64) as usize] as char;
            value /= LETTERS.len() as u64;
            letter
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paths_of_the_customer_account() {
        assert_eq!(Page::parse(&[]), Some(Page::Orders));
        assert_eq!(Page::parse(&["orders", "12"]), Some(Page::Order("12")));
        assert_eq!(Page::parse(&["profile"]), Some(Page::Profile));
        assert_eq!(
            Page::parse(&["pages", "loyalty", "rewards", "3"]),
            Some(Page::Full {
                handle: "loyalty",
                order: None,
                rest: "/rewards/3".into()
            })
        );
        assert_eq!(
            Page::parse(&["orders", "12", "pages", "returns"]),
            Some(Page::Full {
                handle: "returns",
                order: Some("12"),
                rest: "/".into()
            })
        );
        // The stand-in's own paths are not pages of the account.
        assert_eq!(Page::parse(&["login"]), None);
        assert_eq!(Page::parse(&["addresses"]), None);
    }

    #[test]
    fn a_confirmation_number_is_nine_letters_and_digits() {
        let number = confirmation_number(1001);
        assert_eq!(number.len(), 9);
        assert!(
            number
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        );
        assert_ne!(number, confirmation_number(1002));
    }
}
