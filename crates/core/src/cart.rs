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
}

impl ResolvedLine<'_> {
    /// What the variant costs with the line's selling plan.
    pub fn allocation(&self) -> Option<Allocation> {
        self.selling_plan
            .map(|(_, plan)| plan.allocation(self.variant))
    }

    /// The unit price before discounts: the price of the variant, or the one its selling plan
    /// sets. A selling plan changes the price itself; it is not a discount.
    pub fn original_price(&self) -> i64 {
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

    /// The unit price, taking quantity price breaks into account.
    pub fn unit_price(&self) -> i64 {
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
        self.variant
            .media_index
            .and_then(|index| self.product.media.get(index))
            .filter(|media| matches!(media.kind, MediaKind::Image))
            .and_then(|media| media.preview.as_ref())
            .or_else(|| self.product.images().next())
    }
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
