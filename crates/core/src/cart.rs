//! Cart lines resolved against the store: what a line costs, its title, its image and its key.

use crate::store::{
    Allocation, CartLine, Image, MediaKind, Product, SellingPlan, SellingPlanGroup, Store, Variant,
};
use crate::util::short_hash;

/// A cart line resolved against the store. Everything the storefront cart and the checkout say
/// of what a line costs comes from here, so that they cannot drift apart.
pub struct ResolvedLine<'a> {
    pub line: &'a CartLine,
    pub product_index: usize,
    pub variant_index: usize,
    pub product: &'a Product,
    pub variant: &'a Variant,
    /// The selling plan the line is bought with, when the product is still sold with it.
    pub selling_plan: Option<(&'a SellingPlanGroup, &'a SellingPlan)>,
    /// What a cart transform of the shop made of the line, as the storefront cart shows it.
    pub transformed: Option<&'a TransformedLine>,
}

/// What a cart transform made of a line of the cart (`lineExpand`, `lineUpdate`), as the
/// storefront cart shows it: Shopify runs the cart transforms of a shop on its cart, which shows
/// a bundle with the price and title the transform gave it, and its components.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransformedLine {
    /// The title the transform gave the line, when it gave one.
    pub title: Option<String>,
    /// The price of a unit after the transform.
    pub unit_price: i64,
    /// The lines an expand made of it.
    pub components: Vec<LineComponent>,
}

/// A component of a [`TransformedLine`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineComponent {
    pub variant_id: u64,
    pub quantity: u32,
    /// What the component costs of the price of the line, for all its units.
    pub line_price: i64,
}

/// A component of the fixed bundle of a cart line: its variant, how many of it the line holds,
/// and the part of the line's price it carries.
pub struct LineComponent<'a> {
    pub product_index: usize,
    pub variant_index: usize,
    pub product: &'a Product,
    pub variant: &'a Variant,
    pub quantity: i64,
    pub line_price: i64,
}

impl<'a> ResolvedLine<'a> {
    /// The components of the line's variant when it is a fixed bundle, each holding its quantity
    /// per bundle times the line's. The line's price is allocated across them "using a weighted
    /// price algorithm", which Shopify does not detail: lsf weighs each component by its price
    /// times its quantity, in whole cents, the remainder on the last one.
    /// <https://shopify.dev/docs/apps/build/product-merchandising/bundles/start-building>
    pub fn components(&self, store: &'a Store) -> Vec<LineComponent<'a>> {
        let found: Vec<(usize, usize, i64)> = self
            .variant
            .components
            .iter()
            .filter_map(|(id, per_bundle)| {
                let (product, variant) = store.variant_location(*id)?;
                Some((product, variant, i64::from(*per_bundle)))
            })
            .collect();
        let weight = |(product, variant, per_bundle): &(usize, usize, i64)| {
            i128::from(store.products[*product].variants[*variant].price.max(0))
                * i128::from(*per_bundle)
        };
        let total_weight: i128 = found.iter().map(weight).sum();
        let line_price = self.line_price();
        let mut allocated = 0;
        let last = found.len().saturating_sub(1);
        found
            .iter()
            .enumerate()
            .map(|(position, component)| {
                let (product, variant, per_bundle) = *component;
                let share = if position == last {
                    line_price - allocated
                } else if total_weight == 0 {
                    line_price / found.len() as i64
                } else {
                    (i128::from(line_price) * weight(component) / total_weight) as i64
                };
                allocated += share;
                LineComponent {
                    product_index: product,
                    variant_index: variant,
                    product: &store.products[product],
                    variant: &store.products[product].variants[variant],
                    quantity: per_bundle * i64::from(self.line.quantity),
                    line_price: share,
                }
            })
            .collect()
    }

    /// What the variant costs with the line's selling plan.
    pub fn allocation(&self) -> Option<Allocation> {
        self.selling_plan
            .map(|(_, plan)| plan.allocation(self.variant))
    }

    /// The unit price before discounts: the price of the variant, or the one its selling plan
    /// sets, or the one a cart transform sets. A selling plan or a transform changes the price
    /// itself; it is not a discount.
    pub fn original_price(&self) -> i64 {
        if let Some(transformed) = self.transformed {
            return transformed.unit_price;
        }
        self.allocation()
            .map_or(self.variant.price, |allocation| allocation.price)
    }

    /// The compare-at price of a unit: the one of its selling plan when it has one (the price of
    /// the variant when the plan lowers it), the compare-at price of the variant otherwise.
    /// `None` when there is none; a caller that shows it only when it is above the price checks
    /// that itself.
    pub fn compare_at_price(&self) -> Option<i64> {
        match self.allocation() {
            Some(allocation) => allocation.compare_at_price,
            None => self.variant.compare_at_price,
        }
    }

    /// The unit price, taking quantity price breaks and a cart transform into account.
    pub fn unit_price(&self) -> i64 {
        if let Some(transformed) = self.transformed {
            return transformed.unit_price;
        }
        if let Some(allocation) = self.allocation() {
            return allocation.price;
        }
        self.variant
            .quantity_price_breaks
            .iter()
            .filter(|(minimum, _)| self.line.quantity >= *minimum)
            .map(|(_, price)| *price)
            .min()
            .unwrap_or(self.variant.price)
    }

    pub fn line_price(&self) -> i64 {
        self.unit_price() * i64::from(self.line.quantity)
    }

    pub fn original_line_price(&self) -> i64 {
        self.original_price() * i64::from(self.line.quantity)
    }

    /// What is due at checkout for the line: less than its price when its selling plan takes
    /// a deposit.
    pub fn checkout_charge(&self) -> i64 {
        match self.allocation() {
            Some(allocation) => allocation.checkout_charge_amount * i64::from(self.line.quantity),
            None => self.line_price(),
        }
    }

    pub fn title(&self) -> String {
        if let Some(title) = self.transformed.and_then(|line| line.title.as_ref()) {
            return title.clone();
        }
        if self.product.has_only_default_variant() {
            self.product.title.clone()
        } else {
            format!("{} - {}", self.product.title, self.variant.title)
        }
    }

    pub fn key(&self) -> String {
        line_key(self.line)
    }

    /// The image of the line: the one of its variant when that is an image, the first one of
    /// its product otherwise.
    pub fn image(&self) -> Option<&'_ Image> {
        variant_image(self.product, self.variant)
    }
}

/// The image of a variant on a cart line: its own when that is an image, the first one of its
/// product otherwise.
pub fn variant_image<'a>(product: &'a Product, variant: &Variant) -> Option<&'a Image> {
    variant
        .media_index
        .and_then(|index| product.media.get(index))
        .filter(|media| matches!(media.kind, MediaKind::Image))
        .and_then(|media| media.preview.as_ref())
        .or_else(|| product.images().next())
}

/// Resolves a cart line against the store. `None` when the variant no longer exists.
pub fn resolve_line<'a>(store: &'a Store, line: &'a CartLine) -> Option<ResolvedLine<'a>> {
    let (product_index, variant_index) = store.variant_location(line.variant_id)?;
    let product = &store.products[product_index];
    Some(ResolvedLine {
        line,
        product_index,
        variant_index,
        product,
        variant: &product.variants[variant_index],
        selling_plan: line
            .selling_plan
            .and_then(|id| store.selling_plan_of(product, id)),
        transformed: None,
    })
}

/// The key identifying a line: the variant plus a hash of the line's properties and selling
/// plan, as lines that differ by those stay separate.
pub fn line_key(line: &CartLine) -> String {
    let mut properties: Vec<String> = line
        .properties
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    if let Some(selling_plan) = line.selling_plan {
        properties.push(format!("\u{0}selling_plan={selling_plan}"));
    }
    format!(
        "{}:{}",
        line.variant_id,
        &short_hash(&properties.join("\n"))[..16]
    )
}
