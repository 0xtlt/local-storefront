//! The visitor's session: their cart, who they are logged in as, the country they shop in.

use indexmap::IndexMap;

use crate::store::{CartLine, Store};

/// The outcome of a form the visitor just submitted, shown on the next render.
#[derive(Clone, Debug, Default)]
pub struct FormResult {
    /// `contact`, `customer_login`, ...
    pub form_type: String,
    pub posted_successfully: bool,
    /// `(field, message)`.
    pub errors: Vec<(String, String)>,
    /// The submitted values, to repopulate the form.
    pub values: IndexMap<String, String>,
}

/// The visitor's state.
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub cart_lines: Vec<CartLine>,
    pub cart_note: String,
    pub cart_attributes: IndexMap<String, String>,
    /// The id of the logged-in customer.
    pub customer_id: Option<u64>,
    /// ISO code of the selected country.
    pub country: Option<String>,
    pub form_result: Option<FormResult>,
    /// For a B2B customer: the id of the company location they are buying for.
    pub company_location: Option<u64>,
    /// The token of the visitor's cart, as `/cart.js` reports it and as the `cart` cookie
    /// holds it. Empty outside a server, where carts take a token derived from the host.
    pub cart_token: String,
}

impl Session {
    /// Whether the visitor has a cart: Shopify only creates one when something goes into it.
    pub fn has_cart(&self) -> bool {
        !self.cart_lines.is_empty()
            || !self.cart_note.is_empty()
            || !self.cart_attributes.is_empty()
    }

    /// The state a new visitor starts with, as configured in the store data.
    pub fn initial(store: &Store) -> Session {
        let defaults = &store.session_defaults;
        Session {
            cart_lines: defaults.cart_lines.clone(),
            cart_note: defaults.cart_note.clone(),
            cart_attributes: defaults.cart_attributes.clone(),
            customer_id: defaults
                .customer_email
                .as_deref()
                .and_then(|who| store.customer_named(who).ok().flatten())
                .map(|customer| customer.id),
            country: defaults.country.clone(),
            form_result: None,
            company_location: defaults.company_location,
            cart_token: String::new(),
        }
    }
}
