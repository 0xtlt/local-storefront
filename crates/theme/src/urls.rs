//! URLs of the things Shopify would serve from its CDN, pointed at the local server instead.
//!
//! The paths keep the shape of Shopify's (`/cdn/shop/files/...`, `/cdn/shop/t/<id>/assets/...`),
//! so theme code that inspects or rewrites URLs behaves the same.

use lsf_core::urls::{encode_path, version_of};

use crate::site::Site;

/// `//host`, the protocol-relative origin Shopify prefixes CDN URLs with.
pub fn cdn_origin(site: &Site) -> String {
    format!("//{}", site.request.host)
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
        "{}/cdn/shop/t/{}/assets/{}?v={version}",
        cdn_origin(site),
        site.theme.id(),
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

#[cfg(test)]
mod tests {
    use lsf_core::urls::encode_component;
    use lsf_liquid::filters::url_encode;

    /// lsf-core encodes the query of a URL without Liquid: it has to agree with `url_encode`,
    /// which a theme encodes the same values with.
    #[test]
    fn encodes_a_component_as_url_encode_does() {
        let every_ascii: String = (0u8..128).map(char::from).collect();
        let inputs = [
            "",
            " ",
            "a b",
            "  two  spaces ",
            "+",
            "%",
            "%20",
            "100% cotton",
            "&=?#/:;,@$!'()*[]",
            "~-._",
            "café au lait",
            "日本語",
            "🛍️ bag",
            "\u{0}\n\t\r",
            "ÿ\u{7f}\u{80}\u{ffff}",
            "a+b=c&d",
            &every_ascii,
        ];
        for input in inputs {
            assert_eq!(encode_component(input), url_encode(input), "{input:?}");
        }
    }
}
