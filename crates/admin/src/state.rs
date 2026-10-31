//! The state of a shop: its records, seeded from the store data, and the events of what changed.
//!
//! See `crates/admin/docs/design.md`.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

/// A record: a JSON object in the shape of its GraphQL type, with its GID in `id` and, for the
/// records whose type has an interface or a union, its type in `__typename`.
pub type Record = Map<String, Value>;

/// The GID of an entity: `gid://shopify/Product/1`.
pub fn gid(type_name: &str, id: u64) -> String {
    format!("gid://shopify/{type_name}/{id}")
}

/// The type and the numeric id of a GID, when it is one of the form `gid://shopify/<Type>/<id>`
/// (a query string after the id, as some GIDs have, is ignored).
pub fn parse_gid(gid: &str) -> Option<(&str, u64)> {
    let rest = gid.strip_prefix("gid://shopify/")?;
    let (type_name, id) = rest.split_once('/')?;
    let id = id.split_once('?').map_or(id, |(id, _)| id);
    Some((type_name, id.parse().ok()?))
}

/// Something that changed in a shop, which webhooks report.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The webhook topic, as Shopify writes it in `X-Shopify-Topic`: `products/update`.
    pub topic: String,
    /// The shop it happened in: `my-shop.myshopify.com`.
    pub shop: String,
    /// The record it is about.
    pub gid: String,
    /// When it happened, by the clock of the shop.
    pub at: DateTime<Utc>,
    /// A unique id, `X-Shopify-Event-Id`.
    pub id: String,
    /// The record as it was when the event happened: what a `…/delete` topic reports, since the
    /// record is gone by the time the webhook is built.
    pub snapshot: Option<Record>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gids() {
        assert_eq!(gid("Product", 7), "gid://shopify/Product/7");
        assert_eq!(parse_gid("gid://shopify/Product/7"), Some(("Product", 7)));
        assert_eq!(
            parse_gid("gid://shopify/ProductImage/3?product_id=7"),
            Some(("ProductImage", 3))
        );
        assert_eq!(parse_gid("gid://other/Product/7"), None);
        assert_eq!(parse_gid("gid://shopify/Product/x"), None);
    }
}
