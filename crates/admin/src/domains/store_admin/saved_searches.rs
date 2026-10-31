//! The saved search a bulk mutation of the online store names (`urlRedirectBulkDeleteBySavedSearch`):
//! the saved searches themselves (`savedSearchCreate` and the `…SavedSearches` queries) are the
//! segments family's (`domains::segments::saved_searches`).

use crate::state::{Record, ShopState};

/// The saved search of `id`, when it is one.
pub fn saved_search(state: &ShopState, id: &str) -> Option<Record> {
    (state.type_of(id) == Some("SavedSearch"))
        .then(|| state.get(id).cloned())
        .flatten()
}
