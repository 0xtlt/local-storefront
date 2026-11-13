//! Form submissions: contact, comments, localization, password, customer login.
//!
//! Nothing is persisted: a submission validates its input, records the outcome in the
//! visitor's session and redirects, and the next render of the form shows it
//! (`form.posted_successfully?`, `form.errors`).

use indexmap::IndexMap;
use lsf_core::money::format_money;
use lsf_core::session::FormResult;
use lsf_liquid::filters::escape_html;
use lsf_theme::drops::cart::resolve_lines;
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

/// The path of a `return_to`, on the storefront's own host. Shopify keeps the path of a URL
/// that names a host (`https://other.example/x`, `//other.example/x`) and redirects to it on
/// the shop's domain, so the localization form never leads off the shop.
fn own_path(return_to: &str) -> String {
    // Browsers drop tabs and line breaks from a URL and read `\` as `/` before its query:
    // `/\other.example` is `//other.example`.
    let url: String = return_to
        .trim()
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let query = url.find(['?', '#']).unwrap_or(url.len());
    let url = url[..query].replace('\\', "/") + &url[query..];
    // A scheme (`https:`, `javascript:`), then an authority (`//host`).
    let scheme_end = url
        .find(':')
        .filter(|&end| {
            url[..end].starts_with(|c: char| c.is_ascii_alphabetic())
                && url[..end]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        })
        .map_or(0, |end| end + 1);
    let rest = &url[scheme_end..];
    let rest = if rest.starts_with("//") {
        let after = rest.trim_start_matches('/');
        &after[after.find(['/', '?', '#']).unwrap_or(after.len())..]
    } else {
        rest
    };
    if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    }
}

/// `POST /localization`: switches the visitor's country and language, and redirects to the
/// page on the web presence of the country. The country picked goes in the `localization`
/// cookie, as Shopify keeps it ("Used to localize the cart to the correct country").
pub fn localization(visit: &Visit<'_>) -> Reply {
    let params = visit.incoming.all_params();
    let picked = super::params::text(&params, "country_code")
        .and_then(|code| visit.store.country(&code))
        .map(|country| country.iso_code.to_uppercase());
    let reply = localization_redirect(visit, &params);
    match picked {
        Some(code) => reply.header(
            "set-cookie",
            format!(
                "{}={code}; Path=/; SameSite=Lax",
                super::LOCALIZATION_COOKIE
            ),
        ),
        None => reply,
    }
}

fn localization_redirect(visit: &Visit<'_>, params: &Json) -> Reply {
    let text = |key: &str| super::params::text(params, key).filter(|value| !value.is_empty());
    if let Some(country) = text("country_code")
        && visit.store.country(&country).is_some()
    {
        visit.update_session(|session| session.country = Some(country.to_uppercase()));
    }
    let return_to = own_path(&text("return_to").unwrap_or_else(|| "/".to_string()));
    let store = &visit.store;
    let (current, root, path) = store.split_locale(&return_to);
    let page_presence = store.market_of_root(&root);
    // The web presence of the country picked: the subfolders of its market (`/en-be`), else `/`.
    // Without a country, the one of the page.
    let presence = match text("country_code").and_then(|code| store.country(&code)) {
        Some(country) => store
            .market_of(&country.iso_code)
            .filter(|market| market.subfolder_suffix.is_some()),
        None => page_presence,
    };
    let asked = text("locale_code")
        .or_else(|| text("language_code"))
        .and_then(|code| store.language(&code));
    // The language must be one the market of the visitor's country offers: "If the currently
    // selected language is not supported by the selected country, it will be updated to the
    // default language for that country."
    // (https://shopify.dev/docs/storefronts/themes/markets/multiple-currencies-languages)
    let session = visit.session();
    let country = store.country_at(session.country.as_deref(), presence);
    let language = store.language_for(asked.unwrap_or(current), &country.iso_code);
    let same_presence =
        presence.map(|market| &market.handle) == page_presence.map(|market| &market.handle);
    if asked.is_none() && language.iso_code == current.iso_code && same_presence {
        return Reply::redirect(&return_to);
    }
    // The same page under the root of the language on that presence, as Shopify answers
    // `country_code=BE` with `/en-be/...`.
    let root = store.locale_root(language, presence);
    let target = if root.is_empty() {
        path
    } else if path == "/" {
        root
    } else {
        format!("{root}{path}")
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

pub fn logout(visit: &Visit<'_>) -> Reply {
    visit.update_session(|session| session.customer_id = None);
    Reply::redirect(&visit.localized("/"))
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
            // A line bought with a selling plan names it, as the checkout does.
            let plan = line.selling_plan.map_or_else(String::new, |(_, plan)| {
                format!("<br><small>{}</small>", escape_html(&plan.name))
            });
            format!(
                "<tr><td>{}{plan}</td><td>{}</td><td>{}</td></tr>",
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

#[cfg(test)]
mod tests {
    use super::own_path;

    #[test]
    fn return_to_keeps_the_path_on_the_storefronts_host() {
        // Shopify answers both with `Location: https://<shop domain>/x`.
        assert_eq!(own_path("https://other.example/x"), "/x");
        assert_eq!(own_path("//other.example/x"), "/x");
        assert_eq!(
            own_path("HTTP://other.example:8080/fr/x?a=1#top"),
            "/fr/x?a=1#top"
        );
        assert_eq!(own_path("https://other.example"), "/");
        assert_eq!(own_path("https://other.example?a=1"), "/?a=1");
        // What a browser reads as another host too.
        assert_eq!(own_path("/\\other.example/x"), "/x");
        assert_eq!(own_path("///other.example/x"), "/x");
        assert_eq!(own_path(" //other.example/x"), "/x");
        assert_eq!(own_path("/\t/other.example/x"), "/x");
        assert_eq!(own_path("javascript:alert(1)"), "/alert(1)");
        // A path stays as it is.
        assert_eq!(
            own_path("/en-be/products/tee?variant=1"),
            "/en-be/products/tee?variant=1"
        );
        assert_eq!(own_path("/"), "/");
        assert_eq!(own_path("cart"), "/cart");
        assert_eq!(own_path("/a:b"), "/a:b");
        assert_eq!(own_path("/search?q=a\\b"), "/search?q=a\\b");
    }
}
