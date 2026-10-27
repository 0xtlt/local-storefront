//! The pages of the checkout that are not a checkout: the home page that lists the extensions of
//! the app and the checkout scenarios, and the notices that say why a scenario did not open.
//! They are plain HTML in the look of the checkout (`checkout.css`), without a script.

use lsf_liquid::filters::escape_html;

use super::reply::{Reply, cache};

/// Where the files of the checkout page are served from.
pub const ASSETS: &str = "/__lsf/checkout/assets";

/// A page of the checkout's look around `body`.
pub fn document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"light\"><title>{}</title>\
         <link rel=\"stylesheet\" href=\"{ASSETS}/checkout.css\"></head>\
         <body><div class=\"lsf-app lsf-home\">{body}</div></body></html>",
        escape_html(title)
    )
}

/// A page that says why a request for a checkout cannot be answered: a scenario that does not
/// exist or does not validate. `home` is where the list of the scenarios is.
pub fn notice(
    status: u16,
    title: &str,
    paragraphs: &[String],
    details: &[String],
    home: &str,
) -> Reply {
    let mut body = format!(
        "<header class=\"lsf-header\"><a class=\"lsf-brand\" href=\"{}\">Checkout</a></header>\
         <main class=\"lsf-home-main\"><section class=\"lsf-section\"><h2>{}</h2>",
        escape_html(home),
        escape_html(title)
    );
    for paragraph in paragraphs {
        body.push_str(&format!("<p>{}</p>", escape_html(paragraph)));
    }
    for detail in details {
        body.push_str(&format!(
            "<pre class=\"lsf-home-problem\">{}</pre>",
            escape_html(detail)
        ));
    }
    body.push_str(&format!(
        "<p><a class=\"lsf-link\" href=\"{}\">All checkouts and extensions</a></p></section></main>",
        escape_html(home)
    ));
    Reply::html(status, document(title, &body)).cached(cache::PRIVATE)
}
