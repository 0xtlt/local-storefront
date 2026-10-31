//! What makes a delivery Shopify's: the HMAC of its body, its headers, and its ids.
//!
//! The headers are those of the 2026-10 webhook reference
//! (<https://shopify.dev/docs/api/webhooks/2026-10#headers>), in the order of its example request.
//! `User-Agent: Shopify-Captain-Hook` is what Shopify's delivery service sends; Shopify's docs do
//! not name it (Cloudflare Radar's directory of verified bots does).

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// The `User-Agent` of Shopify's webhook deliveries.
pub const USER_AGENT: &str = "Shopify-Captain-Hook";

/// `X-Shopify-Hmac-Sha256`: the base64 HMAC-SHA256 of the raw body, keyed with the client
/// secret of the app.
pub fn hmac_sha256_base64(secret: &str, body: &[u8]) -> String {
    // An HMAC takes a key of any length: this cannot fail.
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes())
        .unwrap_or_else(|_| unreachable!("HMAC accepts keys of any length"));
    mac.update(body);
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// Whether `header` is the signature of `body` with `secret`: what a backend checks.
pub fn verify(secret: &str, body: &[u8], header: &str) -> bool {
    let Ok(given) = base64::engine::general_purpose::STANDARD.decode(header.trim()) else {
        return false;
    };
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes())
        .unwrap_or_else(|_| unreachable!("HMAC accepts keys of any length"));
    mac.update(body);
    mac.verify_slice(&given).is_ok()
}

/// What the headers of a delivery say.
#[derive(Clone, Debug)]
pub struct HeaderValues<'a> {
    pub topic: &'a str,
    pub shop: &'a str,
    pub api_version: &'a str,
    /// `X-Shopify-Webhook-Id`: one per delivery.
    pub webhook_id: &'a str,
    /// `X-Shopify-Event-Id`: shared by the deliveries of one change.
    pub event_id: &'a str,
    pub triggered_at: DateTime<Utc>,
    /// The `name` of the subscription, for `X-Shopify-Name`.
    pub name: Option<&'a str>,
}

/// The headers of a delivery of `body`, signed with `secret`, as Shopify writes their names.
pub fn headers(values: &HeaderValues, secret: &str, body: &[u8]) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Content-Type", "application/json".to_string()),
        ("X-Shopify-Topic", values.topic.to_string()),
        ("X-Shopify-Hmac-Sha256", hmac_sha256_base64(secret, body)),
        ("X-Shopify-Shop-Domain", values.shop.to_string()),
        ("X-Shopify-API-Version", values.api_version.to_string()),
        ("X-Shopify-Webhook-Id", values.webhook_id.to_string()),
        ("X-Shopify-Triggered-At", triggered_at(values.triggered_at)),
        ("X-Shopify-Event-Id", values.event_id.to_string()),
    ];
    if let Some(name) = values.name.filter(|name| !name.is_empty()) {
        headers.push(("X-Shopify-Name", name.to_string()));
    }
    headers.push(("User-Agent", USER_AGENT.to_string()));
    headers
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect()
}

/// `X-Shopify-Triggered-At`, as Shopify writes it: `2025-07-10T16:15:47.000000Z`.
pub fn triggered_at(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

/// A new random id in the form of a UUID (version 4), for `X-Shopify-Webhook-Id` and
/// `X-Shopify-Event-Id`. Unique across runs, so that a backend that remembers the deliveries it
/// processed does not take those of a new run for duplicates.
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    // `RandomState` is seeded from the system's randomness.
    let random = RandomState::new();
    let mut first = random.build_hasher();
    first.write_u64(count);
    first.write_u128(nanos);
    first.write_u32(std::process::id());
    let high = first.finish();
    let mut second = random.build_hasher();
    second.write_u64(high);
    second.write_u64(count.rotate_left(32));
    let low = second.finish();
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&high.to_be_bytes());
    bytes[8..].copy_from_slice(&low.to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn the_signature_is_the_base64_hmac_sha256_of_the_body() {
        // Computed with Python's `hmac` and `hashlib`.
        let body = br#"{"id":788032119674292922,"title":"Example T-Shirt"}"#;
        assert_eq!(
            hmac_sha256_base64("hush", body),
            "PChQV/s9qVvLuYZx4s3CvtPkNn5M2k1OrvTkc0qHHSM="
        );
        assert_eq!(
            hmac_sha256_base64("lsf-local-client-secret", b"{}"),
            "2DlNECN0cNPkVdkdyVuKNhnEDE+ErFPfpA64hhUdIzs="
        );
        assert!(verify(
            "hush",
            body,
            "PChQV/s9qVvLuYZx4s3CvtPkNn5M2k1OrvTkc0qHHSM="
        ));
        assert!(!verify(
            "other",
            body,
            "PChQV/s9qVvLuYZx4s3CvtPkNn5M2k1OrvTkc0qHHSM="
        ));
        assert!(!verify("hush", body, "not base64!"));
    }

    #[test]
    fn the_headers_are_shopifys() {
        let at = Utc.with_ymd_and_hms(2025, 7, 10, 16, 15, 47).unwrap();
        let values = HeaderValues {
            topic: "products/update",
            shop: "snowdevil.myshopify.com",
            api_version: "2026-10",
            webhook_id: "545ebdd4-b7be-4d3d-b14e-54aa78e0e0f9",
            event_id: "59d6d79c-595f-4e7e-bc6f-72f5c3d0c66f",
            triggered_at: at,
            name: Some("product-sync"),
        };
        let headers = headers(&values, "hush", b"{}");
        let names: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Content-Type",
                "X-Shopify-Topic",
                "X-Shopify-Hmac-Sha256",
                "X-Shopify-Shop-Domain",
                "X-Shopify-API-Version",
                "X-Shopify-Webhook-Id",
                "X-Shopify-Triggered-At",
                "X-Shopify-Event-Id",
                "X-Shopify-Name",
                "User-Agent",
            ]
        );
        let header = |name: &str| {
            headers
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(
            header("X-Shopify-Triggered-At"),
            Some("2025-07-10T16:15:47.000000Z")
        );
        assert_eq!(header("User-Agent"), Some("Shopify-Captain-Hook"));
        assert_eq!(header("Content-Type"), Some("application/json"));
        // No name, no `X-Shopify-Name`.
        let unnamed = super::headers(
            &HeaderValues {
                name: None,
                ..values
            },
            "hush",
            b"{}",
        );
        assert!(unnamed.iter().all(|(name, _)| name != "X-Shopify-Name"));
    }

    #[test]
    fn ids_look_like_uuids_and_do_not_repeat() {
        let first = new_id();
        let second = new_id();
        assert_ne!(first, second);
        assert_eq!(first.len(), 36);
        assert_eq!(first.matches('-').count(), 4);
        assert_eq!(&first[14..15], "4");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
    }
}
