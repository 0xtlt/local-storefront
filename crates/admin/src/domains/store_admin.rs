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
//! do it: its job is `done` ([`crate::resolve::done_job`]), and what it changed is there for the next query.

pub mod files;
pub mod policies;
pub mod redirects;
pub mod standard;

use crate::resolve::Registry;

pub fn register(registry: &mut Registry) {
    files::register(registry);
    policies::register(registry);
    redirects::register(registry);
    standard::register(registry);
}

#[cfg(test)]
mod tests;
