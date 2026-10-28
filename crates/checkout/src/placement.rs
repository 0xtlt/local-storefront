//! Where the checkout page places the targets of checkout UI extensions: the `placements` of
//! `assets/api.json`. The page (`assets/views.js`) renders a slot at each; any other target of
//! the API needs something a local checkout lacks (pickup points, the chat), and no extension
//! renders there.

/// Where the page renders `target`, or `None` when it has no place on the page.
pub fn placement(target: &str) -> Option<&'static str> {
    crate::metadata::get()
        .placements
        .iter()
        .find(|placement| placement.target == target)
        .map(|placement| placement.place.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_of_the_views_is_a_place() {
        let views = String::from_utf8(crate::assets::get("views.js").unwrap().to_vec()).unwrap();
        for crate::metadata::Placement { target, .. } in &crate::metadata::get().placements {
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
