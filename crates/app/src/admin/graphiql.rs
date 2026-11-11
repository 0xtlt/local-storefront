//! lsf's GraphiQL (`/__lsf/graphiql`): a page to write queries against the local Admin API in a
//! browser, the shops, their tokens and the served API versions filled in from `GET /__lsf/admin`,
//! with the schema's documentation from its introspection. It is lsf's own page, one file
//! embedded in the binary (`assets/graphiql/graphiql.html`): no script of another origin, so it
//! works offline, and no third-party code to license.

use super::api::{HttpRequest, HttpResponse};

/// Where the page is served.
pub const PATH: &str = "/__lsf/graphiql";

const PAGE: &[u8] = include_bytes!("../../assets/graphiql/graphiql.html");

/// `GET /__lsf/graphiql`.
pub fn page(request: &HttpRequest) -> HttpResponse {
    let method = request.method.to_ascii_uppercase();
    if method != "GET" && method != "HEAD" {
        return HttpResponse::not_allowed(&method);
    }
    HttpResponse {
        status: 200,
        headers: vec![
            (
                "content-type".to_string(),
                "text/html; charset=utf-8".to_string(),
            ),
            ("cache-control".to_string(), "no-store".to_string()),
        ],
        body: PAGE.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_loads_nothing_from_another_origin() {
        let html = String::from_utf8(PAGE.to_vec()).unwrap();
        assert!(html.contains("/__lsf/admin"));
        for remote in [
            "src=\"http",
            "href=\"http",
            "@import",
            "//cdn",
            "unpkg",
            "esm.sh",
        ] {
            assert!(!html.contains(remote), "{remote}");
        }
    }
}
