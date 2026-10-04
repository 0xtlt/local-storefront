//! URLs of the things Shopify would serve from its CDN, pointed at the local server instead.
//!
//! The paths keep the shape of Shopify's (`/cdn/shop/files/...`, `/cdn/shop/t/<id>/assets/...`),
//! so theme code that inspects or rewrites URLs behaves the same.

use crate::site::Site;
use crate::util::short_hash;

/// The id of the theme in asset URLs.
pub const THEME_ID: u64 = 1;

/// Percent-encodes a query component like Ruby's `CGI.escape`, with spaces as `+`.
pub fn encode_component(input: &str) -> String {
    lsf_liquid::filters::url_encode(input)
}

/// Percent-encodes a path, leaving `/` and the characters that are safe in a path.
pub fn encode_path(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// `//host`, the protocol-relative origin Shopify prefixes CDN URLs with.
pub fn cdn_origin(site: &Site) -> String {
    format!("//{}", site.request.host)
}

/// A stable numeric version for `?v=`, in the range of a Unix timestamp like Shopify's.
pub fn version_of(key: &str) -> u64 {
    let hash = u64::from_str_radix(&short_hash(key)[..12], 16).unwrap_or(0);
    1_600_000_000 + hash % 100_000_000
}

/// The version of `assets/<name>.liquid`, which is requested as `<name>` and rendered with
/// the theme settings: it changes with the file and with the settings, as the URL of the
/// compiled file does on Shopify.
fn liquid_asset_version(site: &Site, name: &str) -> Option<u64> {
    let files = site.theme.files();
    let source = files.version(&format!("assets/{name}.liquid"));
    if source == 0 {
        return None;
    }
    let overrides = serde_json::to_string(&site.store.theme_settings).unwrap_or_default();
    Some(version_of(&format!(
        "{source}/{}/{overrides}",
        files.version("config/settings_data.json")
    )))
}

/// The URL of a theme asset: `//host/cdn/shop/t/1/assets/base.css?v=123`.
pub fn asset_url(site: &Site, name: &str) -> String {
    let version = match site.theme.files().version(&format!("assets/{name}")) {
        0 => liquid_asset_version(site, name).unwrap_or_else(|| version_of(name)),
        version => version,
    };
    format!(
        "{}/cdn/shop/t/{THEME_ID}/assets/{}?v={version}",
        cdn_origin(site),
        encode_path(name),
    )
}

/// The URL of a file in the data directory's `files/`: `//host/cdn/shop/files/shirt.jpg?v=123`.
pub fn file_url(site: &Site, src: &str) -> String {
    format!(
        "{}/cdn/shop/files/{}?v={}",
        cdn_origin(site),
        encode_path(src),
        version_of(src)
    )
}

/// Builds a query string with the parameters sorted by name, as Shopify's image URLs have.
pub fn with_sorted_params(base: &str, params: &[(&str, String)]) -> String {
    let (path, existing) = base.split_once('?').unwrap_or((base, ""));
    let mut all: Vec<(String, String)> = existing
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.to_string(), value.to_string())
        })
        .collect();
    for (key, value) in params {
        all.retain(|(existing, _)| existing != key);
        all.push(((*key).to_string(), value.clone()));
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));
    if all.is_empty() {
        return path.to_string();
    }
    let query: Vec<String> = all
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    format!("{path}?{}", query.join("&"))
}

/// Inserts a size suffix before the extension, as the legacy `img_url` filter does:
/// `shirt.jpg` + `large` → `shirt_large.jpg`.
pub fn with_size_suffix(url: &str, suffix: &str) -> String {
    let (path, query) = match url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (url, None),
    };
    let slash = path.rfind('/').map_or(0, |index| index + 1);
    let renamed = match path[slash..].rfind('.') {
        Some(dot) => format!("{}_{suffix}{}", &path[..slash + dot], &path[slash + dot..]),
        None => format!("{path}_{suffix}"),
    };
    match query {
        Some(query) => format!("{renamed}?{query}"),
        None => renamed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_image_parameters() {
        assert_eq!(
            with_sorted_params(
                "//h/cdn/shop/files/a.jpg?v=1",
                &[
                    ("width", "400".into()),
                    ("crop", "bottom".into()),
                    ("height", "400".into())
                ]
            ),
            "//h/cdn/shop/files/a.jpg?crop=bottom&height=400&v=1&width=400"
        );
    }

    #[test]
    fn adds_size_suffixes() {
        assert_eq!(
            with_size_suffix("//h/cdn/shop/files/a.b.jpg?v=1", "large"),
            "//h/cdn/shop/files/a.b_large.jpg?v=1"
        );
        assert_eq!(
            with_size_suffix("//h/files/noext", "small"),
            "//h/files/noext_small"
        );
    }
}
