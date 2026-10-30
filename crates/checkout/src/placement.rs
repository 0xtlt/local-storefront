//! Where the pages place the targets of checkout and customer account UI extensions: the
//! `placements` of `assets/api.json`, every target of Shopify's. The checkout page
//! (`assets/views.js`) and the customer account (`assets/account-views.js`) render a slot at each,
//! but the address autocomplete, which the checkout page runs as the buyer types.

/// Where the page renders `target`, or `None` when it has no place on the page.
pub fn placement(target: &str) -> Option<&'static str> {
    crate::metadata::get()
        .placements
        .iter()
        .find(|placement| placement.target == target)
        .map(|placement| placement.place.as_str())
}

/// The surface of `target`: `checkout` or `customer-account`, `None` when Shopify has no such
/// target.
pub fn surface(target: &str) -> Option<&'static str> {
    crate::metadata::get()
        .placements
        .iter()
        .find(|placement| placement.target == target)
        .map(|placement| placement.surface.as_str())
}

/// Whether `target` renders nothing: the page runs it (the address autocomplete).
pub fn runs(target: &str) -> bool {
    crate::metadata::get()
        .placements
        .iter()
        .any(|placement| placement.target == target && placement.runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_is_a_slot_of_the_views() {
        let file = |name| String::from_utf8(crate::assets::get(name).unwrap().to_vec()).unwrap();
        let (checkout, account, page) = (
            file("views.js"),
            file("account-views.js"),
            file("checkout.js"),
        );
        for placement in &crate::metadata::get().placements {
            let quoted = format!("'{}'", placement.target);
            let found = match (placement.surface.as_str(), placement.runs) {
                ("checkout", false) => checkout.contains(&quoted),
                ("checkout", true) => page.contains(&quoted),
                _ => account.contains(&quoted),
            };
            assert!(found, "{} has no slot", placement.target);
        }
    }

    #[test]
    fn every_target_of_shopify_has_a_place() {
        assert!(placement("purchase.checkout.block.render").is_some());
        assert!(placement("purchase.checkout.chat.render").is_some());
        assert!(placement("customer-account.order-status.block.render").is_some());
        assert!(placement("purchase.checkout.blocks.render").is_none());
    }
}
