//! What a page would cost a Shopify storefront, in points.
//!
//! This server answers a metafield as fast as a title: the time a render takes here says
//! nothing of what loading that metafield costs where it comes from a database. A profile
//! therefore also counts what a render asks for, and gives each kind of thing a cost.
//!
//! The costs are a model, not a measure: Shopify does not publish what it spends on what.
//! The unit is a tag rendered. What a storefront has to fetch is taken to weigh as much as a
//! hundred tags, a variant a tenth of that since they come together, and a search ten times
//! more. They can be changed to follow what `shopify theme profile` shows of a real store.

use std::hash::Hash;

pub const LIQUID: &str = lsf_liquid::profiler::NODE_KIND;
pub const PRODUCT: &str = "product";
pub const VARIANT: &str = "variant";
pub const COLLECTION: &str = "collection";
pub const METAFIELD: &str = "metafield";
pub const METAOBJECT: &str = "metaobject";
pub const PAGE: &str = "page";
pub const BLOG: &str = "blog";
pub const ARTICLE: &str = "article";
pub const MENU: &str = "menu";
pub const SEARCH: &str = "search";

/// Every kind of thing a render is charged for: its name, the points of one, and what one is.
/// What is loaded is charged once per render, however many times the templates read it.
pub const KINDS: [(&str, u64, &str); 11] = [
    (LIQUID, 1, "A tag or an output rendered."),
    (PRODUCT, 100, "A product loaded."),
    (VARIANT, 10, "A variant loaded."),
    (COLLECTION, 100, "A collection loaded."),
    (METAFIELD, 100, "A metafield read."),
    (METAOBJECT, 100, "A metaobject loaded."),
    (PAGE, 100, "A page loaded."),
    (BLOG, 100, "A blog loaded."),
    (ARTICLE, 100, "An article loaded."),
    (MENU, 100, "A menu loaded."),
    (
        SEARCH,
        1000,
        "A search, a predictive search or the recommendations of a product.",
    ),
];

/// The points of each kind of thing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Costs(Vec<(&'static str, u64)>);

impl Default for Costs {
    fn default() -> Costs {
        Costs(
            KINDS
                .iter()
                .map(|(kind, points, _)| (*kind, *points))
                .collect(),
        )
    }
}

impl Costs {
    /// Changes the cost of some kinds: `product=50,metafield=5`.
    pub fn with(mut self, rules: &str) -> Result<Costs, String> {
        for rule in rules.split(',').map(str::trim) {
            if rule.is_empty() {
                continue;
            }
            let (kind, points) = rule
                .split_once('=')
                .ok_or_else(|| format!("\"{rule}\" is not <kind>=<points>"))?;
            let (kind, points) = (kind.trim(), points.trim());
            let points: u64 = points
                .parse()
                .map_err(|_| format!("\"{points}\" is not a number of points"))?;
            let known = self.0.iter_mut().find(|(known, _)| *known == kind);
            let Some(slot) = known else {
                let kinds: Vec<&str> = KINDS.iter().map(|(kind, _, _)| *kind).collect();
                return Err(format!(
                    "\"{kind}\" is not a kind of cost. Kinds: {}",
                    kinds.join(", ")
                ));
            };
            slot.1 = points;
        }
        Ok(self)
    }

    /// Each kind and the points of one.
    pub fn pairs(&self) -> &[(&'static str, u64)] {
        &self.0
    }
}

/// What one thing of a kind is, for a report.
pub fn describe(kind: &str) -> &'static str {
    KINDS
        .iter()
        .find(|(known, _, _)| *known == kind)
        .map_or("", |(_, _, what)| what)
}

/// Charges the render being profiled for a thing it loads: once, however many times the
/// templates ask for it. `key` tells the thing from the others of its kind.
pub(crate) fn loaded(kind: &'static str, key: impl Hash) {
    lsf_liquid::profiler::charge_once(kind, &key);
}

/// Charges the render being profiled for what costs every time it is asked for.
pub(crate) fn asked(kind: &'static str) {
    lsf_liquid::profiler::charge(kind);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs_can_be_changed_by_kind() {
        let costs = Costs::default()
            .with("product=50, metafield = 5,,liquid=0")
            .unwrap();
        let of = |kind: &str| {
            let pair = costs.pairs().iter().find(|(known, _)| *known == kind);
            pair.unwrap().1
        };
        assert_eq!((of("product"), of("metafield"), of("liquid")), (50, 5, 0));
        // The others keep theirs.
        assert_eq!(of("metaobject"), 100);
        assert_eq!(Costs::default().with("").unwrap(), Costs::default());

        let error = |rules: &str| Costs::default().with(rules).unwrap_err();
        assert_eq!(error("product"), "\"product\" is not <kind>=<points>");
        assert_eq!(error("product=x"), "\"x\" is not a number of points");
        assert!(
            error("products=5").starts_with("\"products\" is not a kind of cost. Kinds: liquid, "),
            "{}",
            error("products=5")
        );
    }
}
