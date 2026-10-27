//! Where the checkout page places the targets of checkout UI extensions. The page
//! (`assets/views.js`) renders a slot at each of these; any other target of the API needs
//! something a local checkout lacks (pickup points, the chat), and no extension renders there.

/// Every target the page renders and the place it renders it, as `(target, place)`.
pub const PLACEMENTS: &[(&str, &str)] = &[
    (
        "purchase.checkout.header.render-after",
        "Checkout: at the top, above the contact step",
    ),
    (
        "purchase.checkout.block.render",
        "Checkout: a block between the shipping method and the payment",
    ),
    (
        "purchase.checkout.contact.render-after",
        "Checkout: under the contact field",
    ),
    (
        "purchase.checkout.delivery-address.render-before",
        "Checkout: above the delivery address",
    ),
    (
        "purchase.checkout.delivery-address.render-after",
        "Checkout: under the delivery address",
    ),
    (
        "purchase.checkout.shipping-option-list.render-before",
        "Checkout: above the list of shipping methods",
    ),
    (
        "purchase.checkout.shipping-option-list.render-after",
        "Checkout: under the list of shipping methods",
    ),
    (
        "purchase.checkout.shipping-option-item.render-after",
        "Checkout: under each shipping method",
    ),
    (
        "purchase.checkout.shipping-option-item.details.render",
        "Checkout: inside the selected shipping method",
    ),
    (
        "purchase.checkout.payment-method-list.render-before",
        "Checkout: above the list of payment methods",
    ),
    (
        "purchase.checkout.payment-method-list.render-after",
        "Checkout: under the list of payment methods",
    ),
    (
        "purchase.checkout.cart-line-list.render-after",
        "Checkout summary: under the lines",
    ),
    (
        "purchase.checkout.cart-line-item.render-after",
        "Checkout summary: under each line",
    ),
    (
        "purchase.checkout.reductions.render-before",
        "Checkout summary: above the discount code field",
    ),
    (
        "purchase.checkout.reductions.render-after",
        "Checkout summary: under the discount code field",
    ),
    (
        "purchase.checkout.actions.render-before",
        "Checkout: above the pay button",
    ),
    (
        "purchase.checkout.footer.render-after",
        "Checkout: at the bottom, under the pay button",
    ),
    (
        "purchase.thank-you.announcement.render",
        "Thank-you page: at the top, above the confirmation",
    ),
    (
        "purchase.thank-you.header.render-after",
        "Thank-you page: under the confirmation",
    ),
    (
        "purchase.thank-you.block.render",
        "Thank-you page: a block under the confirmation message",
    ),
    (
        "purchase.thank-you.customer-information.render-after",
        "Thank-you page: under the order details",
    ),
    (
        "purchase.thank-you.cart-line-list.render-after",
        "Thank-you summary: under the lines",
    ),
    (
        "purchase.thank-you.cart-line-item.render-after",
        "Thank-you summary: under each line",
    ),
    (
        "purchase.thank-you.footer.render-after",
        "Thank-you page: at the bottom",
    ),
];

/// Where the page renders `target`, or `None` when it has no place on the page.
pub fn placement(target: &str) -> Option<&'static str> {
    PLACEMENTS
        .iter()
        .find(|(name, _)| *name == target)
        .map(|(_, place)| *place)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The quoted names between `RENDERED_TARGETS = [` and `];` of a script.
    fn rendered_targets(script: &str) -> Vec<String> {
        let list = script
            .split("RENDERED_TARGETS = [")
            .nth(1)
            .and_then(|rest| rest.split("];").next())
            .expect("the list of rendered targets");
        list.split('\'')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn the_places_are_the_targets_the_page_renders() {
        let script =
            String::from_utf8(crate::assets::get("extension-host.js").unwrap().to_vec()).unwrap();
        let rendered = rendered_targets(&script);
        assert_eq!(rendered.len(), PLACEMENTS.len());
        let described: Vec<&str> = PLACEMENTS.iter().map(|(target, _)| *target).collect();
        assert_eq!(rendered, described);
    }

    #[test]
    fn a_slot_of_the_views_is_a_place() {
        let views = String::from_utf8(crate::assets::get("views.js").unwrap().to_vec()).unwrap();
        for (target, _) in PLACEMENTS {
            assert!(
                views.contains(&format!("'{target}'")),
                "{target} has no slot in views.js"
            );
        }
    }

    #[test]
    fn a_target_without_a_slot_has_no_place() {
        assert!(placement("purchase.checkout.block.render").is_some());
        assert!(placement("purchase.checkout.chat.render").is_none());
        assert!(placement("purchase.checkout.pickup-location-list.render-before").is_none());
    }
}
