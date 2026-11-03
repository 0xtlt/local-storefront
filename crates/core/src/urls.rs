//! The pieces of URLs: percent-encoding and decoding, query strings, and the `?v=` versions and
//! size suffixes of the URLs Shopify serves from its CDN.

use crate::util::short_hash;

/// The id of the theme directory in asset URLs, when the Admin API says nothing of the theme
/// served (`Theme::id`).
pub const THEME_ID: u64 = 1;

/// Percent-encodes a query component like Ruby's `CGI.escape`, with spaces as `+`.
pub fn encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
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

/// A stable numeric version for `?v=`, in the range of a Unix timestamp like Shopify's.
pub fn version_of(key: &str) -> u64 {
    let hash = u64::from_str_radix(&short_hash(key)[..12], 16).unwrap_or(0);
    1_600_000_000 + hash % 100_000_000
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

/// Whether `text` is an `http://` or `https://` URL with a host (`https:///path` has none).
pub fn is_http_url(text: &str) -> bool {
    ["http://", "https://"].iter().any(|scheme| {
        text.strip_prefix(scheme)
            .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
    })
}

/// Decodes the `%XX` escapes of a URL or a path (a `%` not followed by two hex digits stays as
/// it is), the bytes read as UTF-8, an invalid sequence replaced by `\u{FFFD}`.
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let hex = |at: usize| {
        bytes
            .get(at)
            .and_then(|byte| char::from(*byte).to_digit(16))
    };
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match (bytes[index], hex(index + 1), hex(index + 2)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                index += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
    fn decodes_the_percent_escapes() {
        assert_eq!(percent_decode("caf%C3%A9%20au%2flait"), "café au/lait");
        // An escape needs two hex digits; a broken sequence is replaced.
        assert_eq!(percent_decode("100%25 %zz %+1 %4"), "100% %zz %+1 %4");
        assert_eq!(percent_decode("%FF."), "\u{FFFD}.");
    }

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
