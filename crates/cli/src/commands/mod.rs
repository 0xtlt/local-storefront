pub mod app;
pub mod app_init;
pub mod check;
pub mod docs;
pub mod fmt;
pub mod function;
pub mod init;
pub mod profile;
pub mod render;
pub mod routes;
pub mod schema;
pub mod serve;
pub mod validate;
pub mod webhook;

/// The value of `--app-url`: an `http://` or `https://` address with a host, where webhooks, OAuth
/// redirects and the app proxy go. Refused when the option is read, rather than by every delivery
/// failing later (`localhost:3000` without its scheme).
pub fn app_url(text: &str) -> Result<String, String> {
    let host = text
        .strip_prefix("http://")
        .or_else(|| text.strip_prefix("https://"));
    match host {
        Some(host) if !host.is_empty() && !host.starts_with('/') => Ok(text.to_string()),
        _ => Err(format!(
            "`{text}` is not an http:// or https:// address: give the backend's, like http://localhost:3000"
        )),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_app_url_is_an_http_address() {
        assert_eq!(
            super::app_url("http://localhost:3000").as_deref(),
            Ok("http://localhost:3000")
        );
        assert!(super::app_url("https://app.example.test/base").is_ok());
        for bad in ["localhost:3000", "http://", "ftp://x", "https:///path", ""] {
            assert!(super::app_url(bad).is_err(), "{bad}");
        }
    }
}
