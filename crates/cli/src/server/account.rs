//! Customer accounts.
//!
//! New customer accounts are hosted by Shopify: they are not part of a theme, and nothing of
//! them can be rendered locally. `/account` stands in for them with a page of lsf's own, which
//! shows who is logged in and lets the visitor become anyone in the store data.

use lsf_core::drops::company::location_switch_path;
use lsf_core::filters::money::format_money;
use lsf_core::store::{Customer, Store};
use lsf_core::urls::encode_component;
use lsf_liquid::filters::escape_html;

use super::reply::Reply;
use super::storefront::Visit;

/// Whether the accounts are hosted by Shopify, in which case `/account` is lsf's page.
pub fn hosted(visit: &Visit<'_>) -> bool {
    lsf_core::site::hosted_accounts(&visit.state.app.theme, &visit.store)
}

/// A path of this storefront, from a `return_to` parameter. Anything else is ignored, so that
/// a link cannot send the visitor to another site.
pub fn local_path(value: Option<&str>) -> Option<String> {
    value
        .filter(|path| path.starts_with('/') && !path.starts_with("//") && !path.contains('\\'))
        .map(str::to_string)
}

fn return_to(visit: &Visit<'_>) -> Option<String> {
    local_path(
        visit
            .incoming
            .query_param("return_to")
            .or_else(|| visit.incoming.query_param("checkout_url")),
    )
}

/// Where a visitor who has to log in is sent: the stand-in, or the theme's login page.
pub fn entry(visit: &Visit<'_>) -> Reply {
    let page = visit.localized(if hosted(visit) {
        "/account"
    } else {
        "/account/login"
    });
    Reply::redirect(&match return_to(visit) {
        Some(target) => format!("{page}?return_to={}", encode_component(&target)),
        None => page,
    })
}

/// The link that logs the browser in as `who`: an email, `default` or `none`.
pub fn login_link(who: &str, return_to: &str) -> String {
    format!(
        "/__lsf/login?customer={}&return_to={}",
        encode_component(who),
        encode_component(return_to)
    )
}

/// `/company_location/update?location_id=…&return_to=…`: a B2B customer picks the location
/// they buy for.
pub fn switch_location(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    let wanted = super::params::text(&params, "location_id").and_then(|id| id.parse::<u64>().ok());
    let session = visit.session();
    let allowed = session
        .customer_id
        .and_then(|id| visit.store.customer_by_id(id))
        .zip(wanted)
        .is_some_and(|(customer, wanted)| {
            customer.company.is_some_and(|company| {
                customer
                    .company_locations
                    .iter()
                    .any(|index| visit.store.companies[company].locations[*index].id == wanted)
            })
        });
    if allowed {
        visit.update_session(|session| session.company_location = wanted);
    }
    let target = local_path(super::params::text(&params, "return_to").as_deref())
        .unwrap_or_else(|| visit.localized("/"));
    Reply::redirect(&target)
}

fn full_name(customer: &Customer) -> String {
    let name = [customer.first_name.as_str(), customer.last_name.as_str()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        customer.email.clone()
    } else {
        name
    }
}

/// What the logged-in customer is: their company, its locations, their orders and addresses.
fn current(visit: &Visit<'_>, customer: &Customer, here: &str) -> String {
    let store = &visit.store;
    let mut out = format!(
        "<section class=\"card\" data-lsf-current><h2>{}</h2><p class=\"muted\">{}</p>",
        escape_html(&full_name(customer)),
        escape_html(&customer.email)
    );
    if let Some(company) = customer.company.map(|index| &store.companies[index]) {
        let selected = customer.current_location(visit.session().company_location, store);
        let locations: String = customer
            .company_locations
            .iter()
            .map(|index| {
                let location = &company.locations[*index];
                let name = escape_html(&location.name);
                if Some(*index) == selected {
                    format!(
                        "<li data-lsf-location=\"{name}\" data-lsf-current-location><strong>{name}</strong> <span class=\"badge\">buying for</span></li>"
                    )
                } else {
                    format!(
                        "<li data-lsf-location=\"{name}\"><a href=\"{}\">{name}</a></li>",
                        escape_html(&location_switch_path(location.id, here))
                    )
                }
            })
            .collect();
        out.push_str(&format!(
            "<h3>Company</h3><p data-lsf-company>{}</p><h3>Locations</h3><ul>{locations}</ul>",
            escape_html(&company.name)
        ));
    }
    if !customer.orders.is_empty() {
        let format = &store.shop.money_format;
        let rows: String = customer
            .orders
            .iter()
            .map(|order| {
                let subtotal: i64 = order
                    .line_items
                    .iter()
                    .map(|line| {
                        store.products[line.variant.0].variants[line.variant.1].price
                            * i64::from(line.quantity)
                    })
                    .sum();
                format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape_html(&order.name),
                    order
                        .created_at
                        .with_timezone(&store.shop.timezone)
                        .format("%Y-%m-%d"),
                    escape_html(&order.financial_status),
                    format_money(subtotal + order.shipping_price + order.tax_price, format)
                )
            })
            .collect();
        out.push_str(&format!(
            "<h3>Orders</h3><table><thead><tr><th>Order</th><th>Date</th><th>Payment</th><th>Total</th></tr></thead><tbody>{rows}</tbody></table>"
        ));
    }
    if !customer.addresses.is_empty() {
        let items: String = customer
            .addresses
            .iter()
            .map(|address| {
                let parts = [
                    address.address1.as_str(),
                    address.zip.as_str(),
                    address.city.as_str(),
                    address.country.as_str(),
                ];
                let text = parts
                    .iter()
                    .filter(|part| !part.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("<li>{}</li>", escape_html(&text))
            })
            .collect();
        out.push_str(&format!("<h3>Addresses</h3><ul>{items}</ul>"));
    }
    out.push_str(&format!(
        "<p><a class=\"button\" href=\"{}\" data-lsf-logout>Log out</a></p></section>",
        escape_html(&visit.localized("/account/logout"))
    ));
    out
}

/// The list of everyone the visitor can become.
pub fn chooser(store: &Store, current: Option<u64>, target: &str) -> String {
    let mark = |active: bool| {
        if active {
            " <span class=\"badge on\">logged in</span>"
        } else {
            ""
        }
    };
    let mut items = format!(
        "<li><a href=\"{}\" data-lsf-login=\"none\">Nobody</a> <span class=\"muted\">not logged in</span>{}</li>",
        escape_html(&login_link("none", target)),
        mark(current.is_none())
    );
    for (position, customer) in store.customers.iter().enumerate() {
        let mut badges = String::new();
        if position == 0 {
            badges.push_str(" <span class=\"badge\">default</span>");
        }
        if let Some(company) = customer.company.map(|index| &store.companies[index]) {
            badges.push_str(&format!(
                " <span class=\"badge b2b\">B2B · {}</span>",
                escape_html(&company.name)
            ));
        }
        items.push_str(&format!(
            "<li><a href=\"{}\" data-lsf-login=\"{email}\">{}</a> <span class=\"muted\">{email}</span>{badges}{}</li>",
            escape_html(&login_link(&customer.email, target)),
            escape_html(&full_name(customer)),
            mark(current == Some(customer.id)),
            email = escape_html(&customer.email),
        ));
    }
    format!("<ul class=\"people\" data-lsf-customers>{items}</ul>")
}

pub const STYLE: &str = "body{font:16px/1.5 system-ui,sans-serif;max-width:42rem;margin:3rem auto;padding:0 1rem;color:#1a1a1a}\
h1{margin-bottom:.25rem}h3{margin:1.25rem 0 .25rem;font-size:1rem}\
.note{background:#f4f6f8;border-radius:8px;padding:.75rem 1rem;font-size:.9rem}\
.card{border:1px solid #ddd;border-radius:8px;padding:.25rem 1rem 1rem;margin:1.5rem 0}\
.muted{color:#666;font-size:.9rem}.badge{font-size:.75rem;border:1px solid #bbb;border-radius:99px;padding:.05rem .5rem;white-space:nowrap}\
.badge.b2b{border-color:#1a5fb4;color:#1a5fb4}.badge.on{background:#0a7a3c;border-color:#0a7a3c;color:#fff}\
.people{list-style:none;padding:0}.people li{padding:.6rem 0;border-bottom:1px solid #eee}\
.button{display:inline-block;border:1px solid #1a1a1a;border-radius:6px;padding:.3rem .8rem;text-decoration:none;color:inherit}\
table{width:100%;border-collapse:collapse}td,th{text-align:left;padding:.4rem;border-bottom:1px solid #eee}";

/// `/account` when accounts are hosted by Shopify.
pub fn page(visit: &Visit<'_>) -> Reply {
    let session = visit.session();
    let customer = session
        .customer_id
        .and_then(|id| visit.store.customer_by_id(id));
    let here = visit.localized("/account");
    // After choosing, the visitor goes back to where they came from, or stays here.
    let target = return_to(visit).unwrap_or_else(|| here.clone());
    let details = customer.map_or_else(String::new, |customer| current(visit, customer, &here));
    Reply::html(
        200,
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
             <title>Account</title><style>{STYLE}</style></head>\
             <body data-lsf-account data-lsf-customer=\"{}\"><h1>Account</h1>\
             <p class=\"note\">Customer accounts are hosted by Shopify and are not part of the theme. \
             This page stands in for them: choose who is logged in.</p>\
             {details}<h2>Log in as</h2>{}\
             <p><a href=\"{}\">Back to the store</a></p></body></html>",
            customer.map_or_else(String::new, |customer| escape_html(&customer.email)),
            chooser(&visit.store, session.customer_id, &target),
            escape_html(&visit.localized("/")),
        ),
    )
    .header("x-lsf-template", "account (hosted by Shopify)")
}
