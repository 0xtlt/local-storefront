//! The response type the handlers build, independent of the HTTP library.

use serde_json::Value as Json;

/// The `Cache-Control` of each kind of response, as Shopify sends it: read from what its own
/// demo stores of Dawn and Horizon answered in October 2026.
pub mod cache {
    /// Theme assets, the compiled scripts, images and files: a year of 365.25 days.
    pub const ASSET: &str = "public, max-age=31557600";
    /// Fonts and the compiled stylesheet.
    pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";
    /// Shopify's own files whose URL carries a hash: its scripts, the payment icons.
    pub const PLATFORM: &str = "public, max-age=31536000";
    /// Shopify's scripts at a URL that never changes (`standard-actions.js`).
    pub const PLATFORM_FIXED: &str = "public, max-age=600, must-revalidate";
    /// A theme asset that does not exist.
    pub const MISSING_ASSET: &str = "public, max-age=60";
    /// What the storefront renders: pages, sections, product and search JSON. Shopify sends
    /// this when the response does not come out of its page cache.
    pub const RENDERED: &str = "private, max-age=0, must-revalidate";
    /// Pages that are not found, and redirects.
    pub const PRIVATE: &str = "private, no-store";
    /// What changes the cart.
    pub const CART_WRITE: &str = "no-cache, no-store";
}

pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Reply {
        Reply {
            status,
            headers: vec![("content-type".to_string(), content_type.to_string())],
            body: body.into(),
        }
    }

    pub fn html(status: u16, body: impl Into<String>) -> Reply {
        Reply::new(status, "text/html; charset=utf-8", body.into().into_bytes())
    }

    pub fn json(status: u16, value: &Json) -> Reply {
        Reply::new(
            status,
            "application/json; charset=utf-8",
            value.to_string().into_bytes(),
        )
    }

    pub fn text(status: u16, body: impl Into<String>) -> Reply {
        Reply::new(
            status,
            "text/plain; charset=utf-8",
            body.into().into_bytes(),
        )
    }

    pub fn redirect(location: &str) -> Reply {
        let mut reply = Reply::new(302, "text/html; charset=utf-8", Vec::new());
        reply
            .headers
            .push(("location".to_string(), location.to_string()));
        reply
    }

    pub fn not_found() -> Reply {
        Reply::text(404, "Not found")
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Reply {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    /// Says how long the response can be kept: one of the policies of [`cache`].
    pub fn cached(self, policy: &str) -> Reply {
        self.header("cache-control", policy)
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case(name))
    }
}

/// The content type of a file, by extension.
pub fn content_type(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "m3u8" => "application/x-mpegURL",
        "mp3" => "audio/mpeg",
        "glb" => "model/gltf-binary",
        "usdz" => "model/vnd.usdz+zip",
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
}
