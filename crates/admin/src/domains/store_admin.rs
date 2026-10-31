//! What a merchant manages in the admin of the online store, as the Admin API does it: URL
//! redirects in bulk and their CSV imports, saved searches, the shop's policies, the standard
//! metafield and metaobject definitions, and the acknowledgement of a failed file update.
//!
//! | Module | What |
//! |---|---|
//! | `redirects` | `urlRedirectBulkDeleteAll`, `…ByIds`, `…BySearch`, `…BySavedSearch`, `urlRedirectImportCreate`, `urlRedirectImportSubmit`; `urlRedirectImport`, `urlRedirectSavedSearches` |
//! | `saved_searches` | `savedSearchCreate`, `savedSearchUpdate`, `savedSearchDelete`; the `…SavedSearches` queries of every resource |
//! | `policies` | `shopPolicyUpdate` |
//! | `standard` | `standardMetafieldDefinitionEnable`, `standardMetaobjectDefinitionEnable`; `standardMetafieldDefinitionTemplates` |
//! | `files` | `fileAcknowledgeUpdateFailed` |
//!
//! A mutation Shopify answers with a `Job` is done within the request, as the other families
//! do it: its job is `done` ([`done_job`]), and what it changed is there for the next query.

pub mod files;
pub mod policies;
pub mod redirects;
pub mod saved_searches;
pub mod standard;

use serde_json::json;

use crate::resolve::Registry;
use crate::state::{Record, ShopState, value};

/// A `Job` that is already done: what lsf answers for the work Shopify does in the background
/// (a record, so that `node(id:)` and `job(id:)` find it).
pub fn done_job(state: &mut ShopState, kind: &str) -> String {
    let id = format!(
        "gid://shopify/Job/{}",
        value::uuid(&format!(
            "job/{}/{kind}/{}",
            state.domain(),
            state.count("Job")
        ))
    );
    let mut record = Record::new();
    record.insert("__typename".into(), json!("Job"));
    record.insert("id".into(), json!(id));
    record.insert("done".into(), json!(true));
    state.insert_as(id.clone(), record);
    id
}

pub fn register(registry: &mut Registry) {
    files::register(registry);
    policies::register(registry);
    redirects::register(registry);
    saved_searches::register(registry);
    standard::register(registry);
}

#[cfg(test)]
mod tests;
