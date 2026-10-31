//! The API versions lsf serves, read from the path of a request.
//!
//! Shopify serves each supported version at `/admin/api/<version>/graphql.json`, and answers a
//! version it no longer supports with the oldest one it does. lsf serves 2026-10 only, and says
//! so for any other version (404): a local tool says what it does not do, rather than answering
//! with a schema the app did not ask for. The REST Admin API (`/admin/api/<version>/<resource>.json`)
//! is not served either.

use serde_json::{Value, json};

/// The versions lsf serves.
pub const SERVED: &[&str] = &["2026-10"];

/// The one version of the Admin API lsf serves.
pub const ADMIN_2026_10: &str = "2026-10";

/// What the path of a request under `/admin/api` is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdminPath<'a> {
    /// `/admin/api/2026-10/graphql.json`: the GraphQL Admin API of a version lsf serves.
    Graphql(&'static str),
    /// `/admin/api/<version>/graphql.json` of a version lsf does not serve (`2026-07`,
    /// `unstable`), or `/admin/api/graphql.json` without one.
    UnsupportedVersion(&'a str),
    /// `/admin/api/<version>/<resource>.json`: the REST Admin API, which lsf does not serve.
    Rest { version: &'a str, resource: &'a str },
}

/// Reads the path of a request (without its query string). `None` when it is not under
/// `/admin/api/`.
pub fn parse_path(path: &str) -> Option<AdminPath<'_>> {
    let rest = path.strip_prefix("/admin/api/")?;
    let rest = rest.trim_end_matches('/');
    if rest == "graphql.json" || rest == "graphql" {
        return Some(AdminPath::UnsupportedVersion(""));
    }
    let (version, resource) = rest.split_once('/')?;
    if resource == "graphql.json" || resource == "graphql" {
        return Some(match SERVED.iter().find(|served| **served == version) {
            Some(served) => AdminPath::Graphql(served),
            None => AdminPath::UnsupportedVersion(version),
        });
    }
    Some(AdminPath::Rest { version, resource })
}

/// Whether a string is an API version as Shopify names them: `2026-10`, or `unstable`.
pub fn is_version_name(version: &str) -> bool {
    if version == "unstable" {
        return true;
    }
    let bytes = version.as_bytes();
    version.len() == 7
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..].iter().all(u8::is_ascii_digit)
}

/// The body of the 404 for a version lsf does not serve: in the shape of Shopify's errors
/// (`{"errors": "..."}`), saying which versions it serves.
pub fn unsupported_version(version: &str) -> Value {
    let served = SERVED.join(", ");
    let message = if version.is_empty() {
        format!(
            "lsf serves the Admin API at /admin/api/<version>/graphql.json, for API version {served} only"
        )
    } else if is_version_name(version) {
        format!("lsf serves the Admin API for API version {served} only, not {version}")
    } else {
        format!(
            "{version} is not an API version: lsf serves the Admin API for API version {served}"
        )
    };
    json!({ "errors": message })
}

/// The body of the 404 for the REST Admin API, which lsf does not serve.
pub fn rest_not_served(resource: &str) -> Value {
    json!({
        "errors": format!(
            "lsf does not serve the REST Admin API ({resource}): use the GraphQL Admin API at /admin/api/{ADMIN_2026_10}/graphql.json"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_of_a_path() {
        assert_eq!(
            parse_path("/admin/api/2026-10/graphql.json"),
            Some(AdminPath::Graphql("2026-10"))
        );
        assert_eq!(
            parse_path("/admin/api/2027-01/graphql.json"),
            Some(AdminPath::UnsupportedVersion("2027-01"))
        );
        assert_eq!(
            parse_path("/admin/api/unstable/graphql.json"),
            Some(AdminPath::UnsupportedVersion("unstable"))
        );
        assert_eq!(
            parse_path("/admin/api/graphql.json"),
            Some(AdminPath::UnsupportedVersion(""))
        );
        assert_eq!(
            parse_path("/admin/api/2026-10/products.json"),
            Some(AdminPath::Rest {
                version: "2026-10",
                resource: "products.json"
            })
        );
        assert_eq!(
            parse_path("/admin/api/2026-10/products/1.json"),
            Some(AdminPath::Rest {
                version: "2026-10",
                resource: "products/1.json"
            })
        );
        assert_eq!(parse_path("/admin/oauth/authorize"), None);
    }

    #[test]
    fn says_which_versions_it_serves() {
        assert_eq!(
            unsupported_version("2026-07"),
            json!({"errors": "lsf serves the Admin API for API version 2026-10 only, not 2026-07"})
        );
        assert!(is_version_name("2027-01"));
        assert!(is_version_name("unstable"));
        assert!(!is_version_name("v1"));
        assert!(
            rest_not_served("products.json")["errors"]
                .as_str()
                .unwrap()
                .contains("/admin/api/2026-10/graphql.json")
        );
    }
}
