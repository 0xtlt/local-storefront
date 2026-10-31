//! The HMACs Shopify signs with: the body of a webhook, the query of an OAuth redirect or of the
//! URL an embedded app is opened with, and the query an app proxy forwards. Each is an
//! HMAC-SHA256 keyed with the client secret; they differ in what is signed and how the digest is
//! written.
//!
//! The rules are those of Shopify's libraries, which a backend verifies with:
//! `@shopify/shopify-api` (`lib/utils/hmac-validator.ts`, `lib/utils/processed-query.ts`) and the
//! `shopify_api` gem (`Utils::HmacValidator`, `Auth::Oauth::AuthQuery#to_signable_string`), and the
//! test vectors of shopify.dev (see the tests).

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// The `X-Shopify-Hmac-Sha256` header of a webhook: the base64 HMAC-SHA256 of the body, as sent.
pub fn webhook_hmac(secret: &str, body: &[u8]) -> String {
    STANDARD.encode(hmac_sha256(secret.as_bytes(), body))
}

/// Whether `header` is the [`webhook_hmac`] of `body`, compared in constant time.
pub fn verify_webhook_hmac(secret: &str, body: &[u8], header: &str) -> bool {
    constant_time_eq(
        webhook_hmac(secret, body).as_bytes(),
        header.trim().as_bytes(),
    )
}

/// The `hmac` of a query Shopify's admin sends: the OAuth redirect (`code`, `host`, `shop`,
/// `state`, `timestamp`), the URL an embedded app is opened with, an admin link. Hex
/// HMAC-SHA256 of the other parameters, `hmac` and `signature` left out, sorted by name,
/// form-encoded and joined with `&`: `code=0907…&shop=some-shop.myshopify.com&state=…`.
///
/// `params` are decoded, in any order. A name given more than once has its values joined with
/// `,`, as `@shopify/shopify-api` does before it signs. Values are encoded as `URLSearchParams`
/// encodes them, a space as `%20`, which is what that library signs; the `shopify_api` gem
/// encodes a space as `+`, so the two only agree on values without one, as every value Shopify
/// sends is (hex codes, domains, base64 hosts, timestamps).
pub fn query_hmac<K, V>(secret: &str, params: impl IntoIterator<Item = (K, V)>) -> String
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let message = joined(params, ",")
        .into_iter()
        .map(|(name, value)| format!("{}={}", form_encode(&name), form_encode(&value)))
        .collect::<Vec<_>>()
        .join("&");
    hex(&hmac_sha256(secret.as_bytes(), message.as_bytes()))
}

/// Whether the `hmac` parameter of `params` is their [`query_hmac`], compared in constant time.
/// `false` without one.
pub fn verify_query_hmac<K, V>(secret: &str, params: impl IntoIterator<Item = (K, V)>) -> bool
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let params: Vec<(String, String)> = params
        .into_iter()
        .map(|(name, value)| (name.as_ref().to_string(), value.as_ref().to_string()))
        .collect();
    let Some(given) = params
        .iter()
        .find(|(name, _)| name == "hmac")
        .map(|(_, value)| value.clone())
    else {
        return false;
    };
    constant_time_eq(query_hmac(secret, params).as_bytes(), given.as_bytes())
}

/// The `signature` of the query an app proxy forwards to the app: hex HMAC-SHA256 of
/// `name=value` for each parameter but `signature`, the values of a name given more than once
/// joined with `,` (`extra=1,2`), sorted, and concatenated without a separator. Values are
/// decoded, not encoded again (`path_prefix=/apps/awesome_reviews`). The rule of shopify.dev
/// ("Authenticate app proxies"), which sorts the `name=value` strings.
pub fn app_proxy_signature<K, V>(secret: &str, params: impl IntoIterator<Item = (K, V)>) -> String
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut pairs: Vec<String> = joined(params, ",")
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    pairs.sort();
    hex(&hmac_sha256(secret.as_bytes(), pairs.concat().as_bytes()))
}

/// Whether the `signature` parameter of `params` is their [`app_proxy_signature`], compared in
/// constant time. `false` without one.
pub fn verify_app_proxy_signature<K, V>(
    secret: &str,
    params: impl IntoIterator<Item = (K, V)>,
) -> bool
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let params: Vec<(String, String)> = params
        .into_iter()
        .map(|(name, value)| (name.as_ref().to_string(), value.as_ref().to_string()))
        .collect();
    let Some(given) = params
        .iter()
        .find(|(name, _)| name == "signature")
        .map(|(_, value)| value.clone())
    else {
        return false;
    };
    constant_time_eq(
        app_proxy_signature(secret, params).as_bytes(),
        given.as_bytes(),
    )
}

/// The parameters but `hmac` and `signature`, sorted by name, the values of a repeated name
/// joined with `separator` in the order they came.
fn joined<K, V>(params: impl IntoIterator<Item = (K, V)>, separator: &str) -> Vec<(String, String)>
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut joined: std::collections::BTreeMap<String, String> = Default::default();
    for (name, value) in params {
        let (name, value) = (name.as_ref(), value.as_ref());
        if name == "hmac" || name == "signature" {
            continue;
        }
        joined
            .entry(name.to_string())
            .and_modify(|existing| {
                existing.push_str(separator);
                existing.push_str(value);
            })
            .or_insert_with(|| value.to_string());
    }
    joined.into_iter().collect()
}

/// A query value encoded as `URLSearchParams` encodes it, a space as `%20`
/// (`ProcessedQuery#stringify` of `@shopify/shopify-api`).
fn form_encode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// HMAC-SHA256 of `data` keyed with `key`.
pub(crate) fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// SHA-256 of `data`.
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Lowercase hexadecimal.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// Whether two byte strings are equal, in a time that depends on their length only.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_hmac_sha256_as_rfc_4231_says() {
        // RFC 4231, test case 2.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn signs_an_oauth_redirect_as_shopify_documents_it() {
        // The example of shopify.dev's former "Verify a request" (secret `hush`), which
        // `@shopify/shopify-api` and the `shopify_api` gem agree with.
        let params = [
            ("code", "0907a61c0c8d55e99db179b68161bc00"),
            ("shop", "some-shop.myshopify.com"),
            ("state", "0.6784241404160823"),
            ("timestamp", "1337178173"),
        ];
        let hmac = "700e2dadb827fcc8609e9d5ce208b2e9cdaab9df07390d2cbca10d7c328fc4bf";
        assert_eq!(query_hmac("hush", params), hmac);
        // The order of the query and its `hmac` do not matter.
        let mut signed: Vec<(&str, &str)> = params.iter().rev().copied().collect();
        signed.insert(1, ("hmac", hmac));
        assert!(verify_query_hmac("hush", signed.clone()));
        assert!(!verify_query_hmac("other", signed.clone()));
        signed[0].1 = "1337178174";
        assert!(!verify_query_hmac("hush", signed));
        assert!(!verify_query_hmac("hush", params));
    }

    #[test]
    fn signs_encoded_values_as_url_search_params_writes_them() {
        // `host` is base64: `/`, `+` and `=` are signed percent-encoded, as both libraries do.
        let encoded = "host=YWJj%2Fdef%2B%3D&shop=a.myshopify.com";
        assert_eq!(
            query_hmac("s", [("shop", "a.myshopify.com"), ("host", "YWJj/def+=")]),
            hex(&hmac_sha256(b"s", encoded.as_bytes()))
        );
        assert_eq!(
            query_hmac("s", [("state", "a b")]),
            hex(&hmac_sha256(b"s", b"state=a%20b"))
        );
        // A repeated name is signed once, its values joined with commas.
        assert_eq!(
            query_hmac("s", [("ids[]", "1"), ("ids[]", "2")]),
            hex(&hmac_sha256(b"s", b"ids%5B%5D=1%2C2"))
        );
    }

    #[test]
    fn signs_an_app_proxy_query_as_shopify_documents_it() {
        // shopify.dev, "Authenticate app proxies": secret `hush`, the query of a logged-in
        // customer and of an anonymous one. The page writes the shop as `{shop}`; the signatures
        // are those of `shop-name.myshopify.com`.
        let query = |customer: &'static str| {
            vec![
                ("extra", "1"),
                ("extra", "2"),
                ("shop", "shop-name.myshopify.com"),
                ("logged_in_customer_id", customer),
                ("path_prefix", "/apps/awesome_reviews"),
                ("timestamp", "1317327555"),
            ]
        };
        assert_eq!(
            app_proxy_signature("hush", query("1")),
            "4c68c8624d737112c91818c11017d24d334b524cb5c2b8ba08daa056f7395ddb"
        );
        let anonymous = "e072b6d7e6622d85912a5214b860d3100dc1e73d9bc29f43796ac8c9ff8093cb";
        assert_eq!(app_proxy_signature("hush", query("")), anonymous);
        let mut signed = query("");
        signed.push(("signature", anonymous));
        assert!(verify_app_proxy_signature("hush", signed.clone()));
        signed[0].1 = "3";
        assert!(!verify_app_proxy_signature("hush", signed));
    }

    #[test]
    fn signs_a_webhook_body_in_base64() {
        let body = br#"{"id":1,"title":"Shirt"}"#;
        // Computed with Python's `hmac` (OpenSSL), independently of this code.
        let expected = "ljDWDCGb1C1kZ5eJKB/g+fLAJMjASvlGaUU9tZ6bIWM=";
        assert_eq!(webhook_hmac("hush", body), expected);
        assert!(verify_webhook_hmac("hush", body, expected));
        assert!(!verify_webhook_hmac("hush", b"{}", expected));
        assert!(!verify_webhook_hmac("other", body, expected));
    }
}
