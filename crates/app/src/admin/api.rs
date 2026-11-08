//! One HTTP request in, one answer out: the Admin API endpoints, OAuth, and the control
//! endpoints of the Admin API and of the webhooks. `lsf` decodes the request and sends the
//! answer; nothing here touches a socket, so every endpoint is tested without a network.
//!
//! See `crates/app/docs/admin/design.md`.

mod admin;
mod app;
mod audit;
mod embedded;
mod pricing;
pub mod samples;
mod simulate;
mod subscriptions;
mod sync;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

pub use admin::{AdminApi, AdminOptions, INVALID_TOKEN, Reseeded, ThrottleSetting};
pub use sync::Synced;

/// A request, as `lsf` decoded it.
#[derive(Clone, Debug, Default)]
pub struct HttpRequest {
    /// `GET`, `POST`, …
    pub method: String,
    /// The path, percent-decoded, without the query.
    pub path: String,
    /// The query string, decoded, in order.
    pub query: Vec<(String, String)>,
    /// The headers, names in lowercase.
    pub headers: HashMap<String, String>,
    /// The body, as sent.
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    pub fn query_param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// An answer.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    /// Header names in lowercase, in the order they are sent.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A JSON answer.
    pub fn json(status: u16, body: &serde_json::Value) -> HttpResponse {
        HttpResponse {
            status,
            headers: vec![(
                "content-type".to_string(),
                "application/json; charset=utf-8".to_string(),
            )],
            body: serde_json::to_vec(body).unwrap_or_default(),
        }
    }

    /// `{"error": message}`, the answer of lsf's endpoints to a request they refuse.
    pub(crate) fn error(status: u16, message: impl Into<String>) -> HttpResponse {
        HttpResponse::json(status, &serde_json::json!({"error": message.into()}))
    }

    /// The `405` of an endpoint that does not take `method`.
    pub(crate) fn not_allowed(method: &str) -> HttpResponse {
        HttpResponse::error(405, format!("{method} is not allowed here"))
    }

    /// An HTML page.
    pub(crate) fn html(status: u16, body: impl Into<String>) -> HttpResponse {
        HttpResponse {
            status,
            headers: vec![(
                "content-type".to_string(),
                "text/html; charset=utf-8".to_string(),
            )],
            body: body.into().into_bytes(),
        }
    }

    /// A redirect to `location`.
    pub fn redirect(location: &str) -> HttpResponse {
        HttpResponse {
            status: 302,
            headers: vec![("location".to_string(), location.to_string())],
            body: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> HttpResponse {
        self.headers.push((name.to_ascii_lowercase(), value.into()));
        self
    }
}

/// `&`, `<`, `>` and `"` escaped, for the text and the attributes of an HTML page.
pub(crate) fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
