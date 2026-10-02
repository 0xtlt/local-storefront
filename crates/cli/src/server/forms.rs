//! Form submissions: contact, comments, localization, password, customer login.
//!
//! Nothing is persisted: a submission validates its input, records the outcome in the
//! visitor's session and redirects, and the next render of the form shows it
//! (`form.posted_successfully?`, `form.errors`).

use indexmap::IndexMap;
use lsf_core::FormResult;
use lsf_core::drops::cart::resolve_lines;
use lsf_core::filters::money::format_money;
use lsf_liquid::filters::escape_html;
use serde_json::Value as Json;

use super::reply::Reply;
use super::storefront::Visit;

/// The fields of `contact[...]`, `comment[...]` or `customer[...]`.
fn fields(params: &Json, group: &str) -> IndexMap<String, String> {
    params
        .get(group)
        .and_then(Json::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    let value = match value {
                        Json::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    (key.clone(), value)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn is_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !value.contains(char::is_whitespace)
}

/// Adds or replaces a query parameter and the fragment of a path.
fn with_param(path: &str, name: &str, value: &str, fragment: &str) -> String {
    let path = path.split('#').next().unwrap_or(path);
    let (base, query) = path.split_once('?').unwrap_or((path, ""));
    let mut pairs: Vec<String> = query
        .split('&')
        .filter(|pair| !pair.is_empty() && !pair.starts_with(&format!("{name}=")))
        .map(str::to_string)
        .collect();
    pairs.push(format!("{name}={value}"));
    format!("{base}?{}#{fragment}", pairs.join("&"))
}

fn flash(visit: &Visit<'_>, result: FormResult) {
    visit.update_session(|session| session.form_result = Some(result));
}

pub fn contact(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    let form_type =
        super::params::text(&params, "form_type").unwrap_or_else(|| "contact".to_string());
    let mut values = fields(&params, "contact");
    // `form.message` is an alias of the body in contact forms.
    if let Some(body) = values.get("body").cloned() {
        values.entry("message".to_string()).or_insert(body);
    }
    let mut errors = Vec::new();
    match values.get("email") {
        Some(email) if is_email(email) => {}
        Some(email) if email.is_empty() => {
            errors.push(("email".to_string(), "can't be blank".to_string()))
        }
        Some(_) => errors.push(("email".to_string(), "is invalid".to_string())),
        None => errors.push(("email".to_string(), "can't be blank".to_string())),
    }
    let success = errors.is_empty();
    flash(
        visit,
        FormResult {
            form_type: form_type.clone(),
            posted_successfully: success,
            errors,
            values,
        },
    );
    let back = visit
        .incoming
        .referer_path()
        .unwrap_or_else(|| visit.localized("/"));
    let flag = if form_type == "customer" {
        "customer_posted"
    } else {
        "contact_posted"
    };
    if success {
        Reply::redirect(&with_param(&back, flag, "true", "contact_form"))
    } else {
        Reply::redirect(&format!(
            "{}#contact_form",
            back.split('#').next().unwrap_or(&back)
        ))
    }
}

pub fn comment(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    let values = fields(&params, "comment");
    let mut errors = Vec::new();
    for field in ["author", "email", "body"] {
        if values
            .get(field)
            .is_none_or(|value| value.trim().is_empty())
        {
            errors.push((field.to_string(), "can't be blank".to_string()));
        }
    }
    if values
        .get("email")
        .is_some_and(|email| !email.is_empty() && !is_email(email))
    {
        errors.push(("email".to_string(), "is invalid".to_string()));
    }
    let success = errors.is_empty();
    flash(
        visit,
        FormResult {
            form_type: "new_comment".to_string(),
            posted_successfully: success,
            errors,
            values,
        },
    );
    let article = visit.request.path.trim_end_matches("/comments").to_string();
    Reply::redirect(&format!("{}#comment_form", visit.localized(&article)))
}

pub fn localization(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    let text = |key: &str| super::params::text(&params, key).filter(|value| !value.is_empty());
    if let Some(country) = text("country_code")
        && visit.store.country(&country).is_some()
    {
        visit.update_session(|session| session.country = Some(country.to_uppercase()));
    }
    let return_to = text("return_to").unwrap_or_else(|| "/".to_string());
    // Changing the language moves the visitor to the same page under the new locale root.
    let language = text("locale_code").or_else(|| text("language_code"));
    let target = match language.and_then(|code| visit.store.language(&code).cloned()) {
        Some(language) => {
            let (_, _, path) = visit.state.localize(&visit.store, &return_to);
            if language.primary {
                path
            } else if path == "/" {
                language.root_url.clone()
            } else {
                format!("{}{path}", language.root_url)
            }
        }
        None => return_to,
    };
    Reply::redirect(&target)
}

pub fn password(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    let given = super::params::text(&params, "password").unwrap_or_default();
    // The storefront is never locked: the right password only leads to the home page.
    if visit.store.shop.password == given {
        return Reply::redirect(&visit.localized("/"));
    }
    flash(
        visit,
        FormResult {
            form_type: "storefront_password".to_string(),
            posted_successfully: false,
            errors: vec![(
                "form".to_string(),
                "Password incorrect, please try again.".to_string(),
            )],
            values: IndexMap::new(),
        },
    );
    Reply::redirect(&visit.localized("/password"))
}

pub fn login(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    let values = fields(&params, "customer");
    let email = values.get("email").cloned().unwrap_or_default();
    let password = values.get("password").cloned().unwrap_or_default();
    let customer = visit.store.customer_by_email(&email).filter(|customer| {
        customer
            .password
            .as_ref()
            .is_none_or(|expected| *expected == password)
    });
    match customer {
        Some(customer) => {
            let id = customer.id;
            visit.update_session(|session| session.customer_id = Some(id));
            let target = super::params::text(&params, "return_to")
                .or_else(|| super::params::text(&params, "checkout_url"))
                .unwrap_or_else(|| visit.localized("/account"));
            Reply::redirect(&target)
        }
        None => {
            let mut kept = IndexMap::new();
            kept.insert("email".to_string(), email);
            flash(
                visit,
                FormResult {
                    form_type: "customer_login".to_string(),
                    posted_successfully: false,
                    errors: vec![(
                        "form".to_string(),
                        "Incorrect email or password.".to_string(),
                    )],
                    values: kept,
                },
            );
            super::account::entry(visit)
        }
    }
}

pub fn logout(visit: &Visit<'_>) -> Reply {
    visit.update_session(|session| session.customer_id = None);
    Reply::redirect(&visit.localized("/"))
}

pub fn register(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    let values = fields(&params, "customer");
    let email = values.get("email").cloned().unwrap_or_default();
    let message = if visit.store.customer_by_email(&email).is_some() {
        "This email address is already associated with an account. If this account is yours, you can reset your password"
    } else {
        "Accounts cannot be created on the local server. Add the customer to the store data (customers) and log in."
    };
    flash(
        visit,
        FormResult {
            form_type: "create_customer".to_string(),
            posted_successfully: false,
            errors: vec![("form".to_string(), message.to_string())],
            values,
        },
    );
    Reply::redirect(&visit.localized("/account/register"))
}

/// Forms whose effect is not simulated: the form shows an error saying so.
pub fn unsupported(visit: &Visit<'_>, form_type: &str) -> Reply {
    flash(
        visit,
        FormResult {
            form_type: form_type.to_string(),
            posted_successfully: false,
            errors: vec![(
                "form".to_string(),
                "This action is not simulated by the local server.".to_string(),
            )],
            values: IndexMap::new(),
        },
    );
    let back = visit
        .incoming
        .referer_path()
        .unwrap_or_else(|| visit.localized("/account"));
    Reply::redirect(&back)
}

/// Checkout is hosted by Shopify and is not part of a theme: show what would be bought.
pub fn checkout(visit: &Visit<'_>) -> Reply {
    let site = visit.site();
    let format = &site.store.shop.money_format;
    let lines = resolve_lines(&site);
    let total: i64 = lines.iter().map(|line| line.line_price()).sum();
    let rows: String = lines
        .iter()
        .map(|line| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_html(&line.title()),
                line.line.quantity,
                format_money(line.line_price(), format)
            )
        })
        .collect();
    Reply::html(
        200,
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Checkout</title>\
             <style>body{{font:16px/1.5 system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1rem}}\
             table{{width:100%;border-collapse:collapse}}td,th{{text-align:left;padding:.5rem;border-bottom:1px solid #ddd}}</style></head>\
             <body data-lsf-checkout><h1>Checkout</h1>\
             <p>Checkout is hosted by Shopify and is not simulated locally. This is the cart that would be checked out.</p>\
             <table><thead><tr><th>Item</th><th>Quantity</th><th>Total</th></tr></thead><tbody>{rows}</tbody>\
             <tfoot><tr><th colspan=\"2\">Total</th><th data-lsf-checkout-total>{}</th></tr></tfoot></table>\
             <p><a href=\"{}\">Back to cart</a></p></body></html>",
            format_money(total, format),
            visit.localized("/cart")
        ),
    )
}
