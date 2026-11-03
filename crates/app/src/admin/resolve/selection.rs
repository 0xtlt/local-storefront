//! What a bulk mutation acts on, when it takes exactly one of `ids`, `search` and
//! `savedSearchId` (the discount bulk mutations, `discountBulkTagsAdd`, the draft order bulk
//! mutations, `discountCodeRedeemCodeBulkDelete`): Shopify refuses none and more than one
//! ("Only one of …", "One of … is required"), and a saved search is its query, run as `search`.
//!
//! Each family then checks the ids its way and runs the search over its records; the words of
//! the errors are each mutation's ([`Wording`]).

use serde_json::Value;

use super::UserError;
use crate::admin::graphql::FieldContext;
use crate::admin::state::ShopState;

/// What the arguments choose.
#[derive(Debug, PartialEq)]
pub enum Selection<'c> {
    /// The ids given, as given.
    Ids(&'c [Value]),
    /// A search query: `search`, or the query of the saved search `savedSearchId` names.
    Search(String),
}

/// Why the arguments choose nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionError {
    /// More than one of the three.
    TooMany,
    /// None of them.
    Missing,
    /// `savedSearchId` names no saved search of the resource.
    NoSavedSearch,
}

/// How a mutation names the three arguments in its errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wording {
    /// "IDs, search argument or saved search ID" (the automatic discounts, the discount tags).
    Titles,
    /// "'ids', 'search' or 'saved_search_id'" (the code discounts, the draft orders).
    Arguments,
}

impl SelectionError {
    /// The user error, with Shopify's code (`TOO_MANY_ARGUMENTS`, `MISSING_ARGUMENT`; `INVALID`
    /// for a saved search, lsf's) when the payload's error type has one (`coded`).
    pub fn user_error(self, wording: Wording, coded: bool) -> UserError {
        let arguments = match wording {
            Wording::Titles => "IDs, search argument or saved search ID",
            Wording::Arguments => "'ids', 'search' or 'saved_search_id'",
        };
        let (error, code) = match self {
            SelectionError::TooMany => (
                UserError::new(&[], format!("Only one of {arguments} is allowed.")),
                "TOO_MANY_ARGUMENTS",
            ),
            SelectionError::Missing => (
                UserError::new(&[], format!("One of {arguments} is required.")),
                "MISSING_ARGUMENT",
            ),
            // lsf's wording, as `savedSearchUpdate`'s.
            SelectionError::NoSavedSearch => (
                UserError::new(&["savedSearchId"], "Saved search does not exist"),
                "INVALID",
            ),
        };
        if coded { error.code(code) } else { error }
    }
}

/// What `ids`, `search` and `savedSearchId` choose: exactly one, a saved search of
/// `resource_type` (its `SearchResultType`: `PRICE_RULE`, `DRAFT_ORDER`, …).
pub fn selection<'c>(
    state: &ShopState,
    ctx: &'c FieldContext<'_>,
    resource_type: &str,
) -> Result<Selection<'c>, SelectionError> {
    let given = |name: &str| ctx.arg(name).filter(|value| !value.is_null());
    let (ids, search, saved) = (given("ids"), given("search"), given("savedSearchId"));
    match [ids, search, saved].iter().flatten().count() {
        0 => return Err(SelectionError::Missing),
        1 => {}
        _ => return Err(SelectionError::TooMany),
    }
    if let Some(ids) = ids {
        return Ok(Selection::Ids(
            ids.as_array().map(Vec::as_slice).unwrap_or_default(),
        ));
    }
    if let Some(saved) = saved {
        return saved
            .as_str()
            .and_then(|id| {
                crate::admin::domains::segments::saved_searches::query_for(state, id, resource_type)
            })
            .map(Selection::Search)
            .ok_or(SelectionError::NoSavedSearch);
    }
    Ok(Selection::Search(
        search
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_errors_take_each_mutations_words() {
        let titles = SelectionError::TooMany.user_error(Wording::Titles, true);
        assert_eq!(
            titles.message,
            "Only one of IDs, search argument or saved search ID is allowed."
        );
        assert_eq!(titles.code.as_deref(), Some("TOO_MANY_ARGUMENTS"));
        let arguments = SelectionError::Missing.user_error(Wording::Arguments, false);
        assert_eq!(
            arguments.message,
            "One of 'ids', 'search' or 'saved_search_id' is required."
        );
        assert_eq!(arguments.code, None);
        let saved = SelectionError::NoSavedSearch.user_error(Wording::Titles, true);
        assert_eq!(saved.field, Some(vec!["savedSearchId".to_string()]));
    }
}
