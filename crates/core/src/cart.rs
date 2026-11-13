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

impl ResolvedLine<'_> {
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
