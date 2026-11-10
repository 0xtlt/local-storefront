//! Fixed bundles: `productBundleCreate`, `productBundleUpdate` (a product fixed bundle) and
//! `productVariantRelationshipBulkUpdate` (a variant fixed bundle), the components of a bundle
//! (`ProductVariant.productVariantComponents`, `Product.bundleComponents`), what derives from
//! them (`requiresComponents`, the bundle's inventory), and the fields that read the relation
//! both ways (`productParents`, `productComponents`).
//!
//! As shopify.dev describes them ("Add a product fixed bundle", "Add a variant fixed bundle",
//! "Start building bundles", read on 2026-10-31, and the reference of 2026-10):
//!
//! - "In a variant fixed bundle, all components are linked to the main variant through the
//!   `productVariantComponents` relationship. The bundle's price is set by the parent variant,
//!   while the bundle's inventory is determined by the inventory of its component variants";
//!   "The parent inventory quantity is the minimum of the component's inventory quantities"
//!   (each divided by the quantity the bundle takes of it). "If the variant is associated with
//!   components, then the attribute `productVariant.requiresComponents` returns `true`."
//! - `productVariantRelationshipBulkUpdate` creates, updates and removes the components of
//!   parent variants (`parentProductVariantId`, or `parentProductId` for a product of one
//!   variant), and prices the parent (`priceInput`: `COMPONENTS_SUM`, "the sum of the components
//!   price times their quantity"; `FIXED`; `NONE`). Its errors are its codes, the codes'
//!   descriptions their messages.
//! - `productBundleCreate` "creates a product bundle that groups multiple Product objects
//!   together as components": a product whose options are the components' option selections (or
//!   their `consolidatedOptions`, "a single bundle option based on the consolidatedOptions
//!   mapping", and a component's `quantityOption`), and whose variants are their combinations,
//!   each with the component variants it selects. "The mutation runs asynchronously and returns
//!   a ProductBundleOperation": lsf's is `COMPLETE` at once, with its `product`, as its
//!   `productSet` operations are. `productBundleUpdate` changes the title and the components
//!   (the variants are made again). The parent's price is the sum of its components' (lsf's
//!   choice: shopify.dev does not say what `productBundleCreate` prices a bundle at).
//! - Limits: "A bundle can have up to 30 components", "Bundles can have up to three options",
//!   "You can't combine quantity options", "Nested bundles aren't supported. A bundle can't have
//!   components and be part of another bundle simultaneously."
//! - Events: `products/create` (`productBundleCreate`) or `products/update`, and the inventory
//!   events of the variants made.
//!
//! The parent's own inventory item is not tracked: its quantity is its components'. A bundle
//! reaches the storefront as a product (the store data has no components): it is available while
//! its components are, and the local checkout sells it as one line, unless a cart transform
//! Function expands it (see "Bundles" in `crates/app/docs/admin/design.md`).

use serde_json::{Map, Value, json};

use super::products::{create_options, emit_new_inventory, free_handle, new_record};
use super::records::{self, error_at, field, objects, text};
use super::variants::{self, Choice};
use super::{cents_of, gids, product_changed};
use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::admin::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("productBundleCreate", bundle_create)
        .mutation("productBundleUpdate", bundle_update)
        .mutation(
            "productVariantRelationshipBulkUpdate",
            relationship_bulk_update,
        )
        .field("ProductVariant", "productParents", variant_parents)
        .field("Product", "productParents", product_parents)
        .field("Product", "productComponents", product_components)
        .field(
            "Product",
            "productComponentsCount",
            product_components_count,
        );
}

/// The components of a bundle variant: `(component variant, quantity)`.
type Components = Vec<(String, i64)>;

/// The record type of a component of a bundle variant.
const COMPONENT: &str = "ProductVariantComponent";

/// The field of a parent variant that lists its components.
const COMPONENTS: &str = "productVariantComponents";

/// The most components a bundle has.
const MAX_COMPONENTS: usize = 30;

/// The most options a bundle has.
const MAX_OPTIONS: usize = 3;

/// The record field of a product fixed bundle that keeps what it was made from: its
/// `components` and `consolidatedOptions` inputs, which `productBundleUpdate` makes it again from.
const BUNDLE_INPUT: &str = "lsfBundle";

// --- Reading ---

/// The components of a variant: `(component variant, quantity)`.
pub fn components_of(state: &ShopState, variant: &str) -> Vec<(String, i64)> {
    state
        .get(variant)
        .map(|record| gids(record.get(COMPONENTS)))
        .unwrap_or_default()
        .iter()
        .filter_map(|component| state.get(component))
        .filter_map(|component| {
            let variant = field(component, "productVariant")?.to_string();
            let quantity = component
                .get("quantity")
                .and_then(Value::as_i64)
                .unwrap_or(1);
            state.contains(&variant).then_some((variant, quantity))
        })
        .collect()
}

/// Whether a variant is a component of a bundle.
fn is_component(state: &ShopState, variant: &str) -> bool {
    state
        .all(COMPONENT)
        .any(|component| field(component, "productVariant") == Some(variant))
}

/// The inventory of a bundle variant, from its components: the fewest bundles their quantities
/// make (an untracked component, or one that sells when out of stock, does not limit it), and
/// whether it can be sold (every component can). `None` for a variant without components.
pub fn bundle_inventory(state: &ShopState, variant: &str) -> Option<(Option<i64>, bool)> {
    let components = components_of(state, variant);
    if components.is_empty() {
        return None;
    }
    let mut quantity: Option<i64> = None;
    let mut available = true;
    for (component, per_bundle) in &components {
        let record = state.get(component)?;
        let stock = record
            .get("inventoryQuantity")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let continues = field(record, "inventoryPolicy") == Some("CONTINUE");
        if records::tracked(state, component) && !continues {
            let bundles = stock.max(0) / (*per_bundle).max(1);
            quantity = Some(quantity.map_or(bundles, |known| known.min(bundles)));
        }
        available &= records::available_for_sale(state, component);
    }
    Some((
        quantity,
        available && quantity.is_none_or(|quantity| quantity > 0),
    ))
}

/// The products whose variants have a variant of `product` as a component.
fn parents_of_product(state: &ShopState, product: &str) -> Vec<String> {
    let variants = state
        .get(product)
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default();
    parents_of_variants(state, &variants)
}

/// The products whose variants have one of `variants` as a component, in creation order.
fn parents_of_variants(state: &ShopState, variants: &[String]) -> Vec<String> {
    let mut parents: Vec<String> = Vec::new();
    for parent in state.all("ProductVariant") {
        let has = components_of(state, field(parent, "id").unwrap_or_default())
            .iter()
            .any(|(component, _)| variants.contains(component));
        if has
            && let Some(product) = field(parent, "product")
            && !parents.iter().any(|known| known == product)
        {
            parents.push(product.to_string());
        }
    }
    parents
}

/// `products` and the products of the bundles their variants are components of: what an
/// inventory change of a component refreshes and compares, since a bundle's quantity is its
/// components'. `products` as given for a shop without bundles.
pub fn with_parents(state: &ShopState, mut products: Vec<String>) -> Vec<String> {
    if state.count(COMPONENT) == 0 {
        return products;
    }
    for product in products.clone() {
        for parent in parents_of_product(state, &product) {
            if !products.contains(&parent) {
                products.push(parent);
            }
        }
    }
    products
}

/// What `refresh_product` runs for a product: the products of which its variants are components
/// derive their inventory again. Nothing for a shop without bundles.
pub fn refresh_parents(state: &mut ShopState, product: &str) {
    if state.count(COMPONENT) == 0 {
        return;
    }
    for parent in parents_of_product(state, product) {
        if parent != product {
            super::refresh_product(state, &parent);
        }
    }
}

// --- Writing ---

/// What deleting variants runs before they go: their own components go with them (a bundle
/// deleted), and every other variant that had one of them as a component loses it, as a
/// component that no longer exists can be no part of a bundle; the products of those bundles
/// are refreshed (their quantity, `requiresComponents` when none is left) and emit
/// `products/update`. Nothing for a shop without bundles.
pub fn removing_variants(state: &mut ShopState, variants: &[String]) {
    if state.count(COMPONENT) == 0 {
        return;
    }
    for variant in variants {
        let own = state
            .get(variant)
            .map(|record| gids(record.get(COMPONENTS)))
            .unwrap_or_default();
        for component in own {
            state.remove(&component);
        }
    }
    let parents: Vec<(String, Components)> = state
        .all("ProductVariant")
        .filter_map(|parent| {
            let id = field(parent, "id")?.to_string();
            if variants.contains(&id) {
                return None;
            }
            let components = components_of(state, &id);
            components
                .iter()
                .any(|(component, _)| variants.contains(component))
                .then(|| {
                    let kept = components
                        .into_iter()
                        .filter(|(component, _)| !variants.contains(component))
                        .collect();
                    (id, kept)
                })
        })
        .collect();
    let mut products: Vec<String> = Vec::new();
    for (parent, kept) in parents {
        set_components(state, &parent, &kept);
        if let Some(product) = state
            .get(&parent)
            .and_then(|record| field(record, "product"))
            .map(str::to_string)
            && !products.contains(&product)
        {
            products.push(product);
        }
    }
    for product in products {
        state.touch(&product);
        product_changed(state, &product);
        state.emit("products/update", &product);
    }
}

/// Sets the components of a variant (`(component variant, quantity)`), reusing its component
/// records where it has them, with `requiresComponents` and the product's
/// `hasVariantsThatRequiresComponents`.
fn set_components(state: &mut ShopState, variant: &str, components: &[(String, i64)]) {
    let existing: Vec<String> = state
        .get(variant)
        .map(|record| gids(record.get(COMPONENTS)))
        .unwrap_or_default();
    let mut kept = Vec::new();
    for (component, quantity) in components {
        let found = existing.iter().find(|id| {
            state
                .get(id)
                .and_then(|record| field(record, "productVariant"))
                == Some(component)
        });
        let id = match found {
            Some(id) => {
                if let Some(record) = state.get_mut(id) {
                    record.insert("quantity".into(), json!(quantity));
                }
                id.clone()
            }
            None => {
                let id = state.new_gid(COMPONENT);
                state.insert(value::object(json!({
                    "__typename": COMPONENT,
                    "id": id,
                    "productVariant": component,
                    "quantity": quantity,
                })));
                id
            }
        };
        kept.push(id);
    }
    for id in existing.iter().filter(|id| !kept.contains(id)) {
        state.remove(id);
    }
    if let Some(record) = state.get_mut(variant) {
        record.insert(COMPONENTS.into(), json!(kept));
        record.insert("requiresComponents".into(), json!(!components.is_empty()));
    }
    if let Some(product) = state
        .get(variant)
        .and_then(|record| field(record, "product"))
        .map(str::to_string)
    {
        let any = state
            .get(&product)
            .map(|record| gids(record.get("variants")))
            .unwrap_or_default()
            .iter()
            .any(|variant| {
                state
                    .get(variant)
                    .and_then(|record| record.get("requiresComponents"))
                    .and_then(Value::as_bool)
                    == Some(true)
            });
        if let Some(record) = state.get_mut(&product) {
            record.insert("hasVariantsThatRequiresComponents".into(), json!(any));
        }
    }
}

/// The sum of the components' prices times their quantities, in cents.
fn components_sum(state: &ShopState, components: &[(String, i64)]) -> i64 {
    components
        .iter()
        .map(|(variant, quantity)| {
            state
                .get(variant)
                .and_then(|record| cents_of(record.get("price")))
                .unwrap_or(0)
                .saturating_mul(*quantity)
        })
        .fold(0i64, i64::saturating_add)
}

/// Sets a variant's price, in cents.
fn set_price(state: &mut ShopState, variant: &str, cents: i64) {
    if let Some(record) = state.get_mut(variant) {
        record.insert("price".into(), json!(value::money(cents)));
    }
}

// --- `productVariantRelationshipBulkUpdate` ---

/// An error of `productVariantRelationshipBulkUpdate`: its code, whose description is the
/// message.
fn relationship_error(path: &[String], code: &str) -> UserError {
    let message = match code {
        "CIRCULAR_REFERENCE" => "A parent product variant cannot contain itself as a component.",
        "DUPLICATE_PRODUCT_VARIANT_RELATIONSHIP" => {
            "A parent product variant must not contain duplicate product variant relationships."
        }
        "EXCEEDED_PRODUCT_VARIANT_RELATIONSHIP_LIMIT" => {
            "Exceeded the maximum allowable product variant relationships in a parent product variant."
        }
        "INVALID_QUANTITY" => "Product variant relationships must have a quantity greater than 0.",
        "MUST_SPECIFY_COMPONENTS" => {
            "The product variant relationships to remove must be specified if all the parent product variant's components aren't being removed."
        }
        "NESTED_PARENT_PRODUCT_VARIANT" => "Nested parent product variants aren't supported.",
        "PARENT_PRODUCT_VARIANT_CANNOT_BE_COMBINED_LISTING" => {
            "Combined listing cannot be parent product variants."
        }
        "CHILD_PRODUCT_VARIANT_CANNOT_BE_COMBINED_LISTING" => {
            "Combined listing cannot be child product variants."
        }
        "PARENT_PRODUCT_VARIANT_CANNOT_BE_GIFT_CARD" => {
            "Gift cards cannot be parent product variants."
        }
        "PARENT_PRODUCT_VARIANT_CANNOT_REQUIRE_SELLING_PLAN" => {
            "Parent product variants cannot require a selling plan."
        }
        "PARENT_REQUIRED" => "A parent product variant ID or product ID must be provided.",
        "PRODUCT_VARIANTS_NOT_COMPONENTS" => {
            "Some of the provided product variants are not components of the specified parent product variant."
        }
        "PRODUCT_VARIANTS_NOT_FOUND" => "The product variants were not found.",
        "UPDATE_PARENT_VARIANT_PRICE_REQUIRED" => {
            "A price must be provided for a parent product variant if the price calculation is set to fixed."
        }
        _ => "Unexpected error.",
    };
    error_at(path, message).code(code)
}

/// The path of an input field of the `index`-th input.
fn at(index: usize, names: &[&str]) -> Vec<String> {
    let mut path = vec!["input".to_string(), index.to_string()];
    path.extend(names.iter().map(|name| name.to_string()));
    path
}

/// Whether a product is in a combined listing.
fn in_combined_listing(state: &ShopState, product: &str) -> bool {
    state
        .get(product)
        .and_then(|record| record.get("combinedListingRole"))
        .is_some_and(|role| !role.is_null())
}

/// The parent variant an input names, or the error that refuses it.
fn parent_of(
    state: &ShopState,
    input: &Map<String, Value>,
    index: usize,
) -> Result<String, UserError> {
    if let Some(variant) = text(input, "parentProductVariantId") {
        if state.is_of(variant, "ProductVariant") {
            return Ok(variant.to_string());
        }
        return Err(relationship_error(
            &at(index, &["parentProductVariantId"]),
            "PRODUCT_VARIANTS_NOT_FOUND",
        ));
    }
    let Some(product) = text(input, "parentProductId") else {
        return Err(relationship_error(&at(index, &[]), "PARENT_REQUIRED"));
    };
    let variants = state
        .get_of(product, "Product")
        .map(|record| gids(record.get("variants")))
        .unwrap_or_default();
    match variants.as_slice() {
        [variant] => Ok(variant.clone()),
        [] => Err(relationship_error(
            &at(index, &["parentProductId"]),
            "PRODUCT_VARIANTS_NOT_FOUND",
        )),
        _ => {
            // lsf's wording: a product of several variants does not say which is the parent.
            Err(error_at(
                &at(index, &["parentProductId"]),
                "The product has more than one variant: name the parent with parentProductVariantId",
            )
            .code("PARENT_REQUIRED"))
        }
    }
}

/// The components an input leaves a parent variant with, checked: `(variant, quantity)`.
fn planned_components(
    state: &ShopState,
    parent: &str,
    input: &Map<String, Value>,
    index: usize,
    errors: &mut Vec<UserError>,
) -> Vec<(String, i64)> {
    let mut components = components_of(state, parent);
    let remove_all = input
        .get("removeAllProductVariantRelationships")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let creates = objects(input, "productVariantRelationshipsToCreate");
    let updates = objects(input, "productVariantRelationshipsToUpdate");
    let removes = gids(input.get("productVariantRelationshipsToRemove"));
    if !remove_all && creates.is_empty() && updates.is_empty() && removes.is_empty() {
        errors.push(relationship_error(
            &at(index, &[]),
            "MUST_SPECIFY_COMPONENTS",
        ));
        return components;
    }
    if remove_all {
        components.clear();
    }
    for (position, id) in removes.iter().enumerate() {
        let before = components.len();
        components.retain(|(component, _)| component != id);
        if before == components.len() && !remove_all {
            errors.push(relationship_error(
                &at(
                    index,
                    &["productVariantRelationshipsToRemove", &position.to_string()],
                ),
                "PRODUCT_VARIANTS_NOT_COMPONENTS",
            ));
        }
    }
    for (position, update) in updates.iter().enumerate() {
        let id = text(update, "id").unwrap_or_default();
        let quantity = update.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        let path = at(
            index,
            &["productVariantRelationshipsToUpdate", &position.to_string()],
        );
        if quantity < 1 {
            errors.push(relationship_error(&path, "INVALID_QUANTITY"));
        }
        match components.iter_mut().find(|(component, _)| component == id) {
            Some(component) => component.1 = quantity,
            None => errors.push(relationship_error(&path, "PRODUCT_VARIANTS_NOT_COMPONENTS")),
        }
    }
    for (position, create) in creates.iter().enumerate() {
        let id = text(create, "id").unwrap_or_default().to_string();
        let quantity = create.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        let path = at(
            index,
            &["productVariantRelationshipsToCreate", &position.to_string()],
        );
        let code = if !state.is_of(&id, "ProductVariant") {
            Some("PRODUCT_VARIANTS_NOT_FOUND")
        } else if id == parent {
            Some("CIRCULAR_REFERENCE")
        } else if quantity < 1 {
            Some("INVALID_QUANTITY")
        } else if components.iter().any(|(component, _)| *component == id) {
            Some("DUPLICATE_PRODUCT_VARIANT_RELATIONSHIP")
        } else if !components_of(state, &id).is_empty() {
            Some("NESTED_PARENT_PRODUCT_VARIANT")
        } else if state
            .get(&id)
            .and_then(|record| field(record, "product"))
            .is_some_and(|product| in_combined_listing(state, product))
        {
            Some("CHILD_PRODUCT_VARIANT_CANNOT_BE_COMBINED_LISTING")
        } else {
            None
        };
        match code {
            Some(code) => errors.push(relationship_error(&path, code)),
            None => components.push((id, quantity)),
        }
    }
    if components.len() > MAX_COMPONENTS {
        errors.push(relationship_error(
            &at(index, &["productVariantRelationshipsToCreate"]),
            "EXCEEDED_PRODUCT_VARIANT_RELATIONSHIP_LIMIT",
        ));
    }
    components
}

/// Checks the parent variant itself: a bundle can't be a component, a gift card, a product that
/// requires a selling plan, or in a combined listing.
fn check_parent(state: &ShopState, parent: &str, index: usize, errors: &mut Vec<UserError>) {
    let product_id = state
        .get(parent)
        .and_then(|record| field(record, "product"));
    let product = product_id.and_then(|product| state.get(product));
    let flag = |name: &str| {
        product
            .and_then(|product| product.get(name))
            .and_then(Value::as_bool)
            == Some(true)
    };
    let path = at(index, &[]);
    if is_component(state, parent) {
        errors.push(relationship_error(&path, "NESTED_PARENT_PRODUCT_VARIANT"));
    } else if flag("isGiftCard") {
        errors.push(relationship_error(
            &path,
            "PARENT_PRODUCT_VARIANT_CANNOT_BE_GIFT_CARD",
        ));
    } else if flag("requiresSellingPlan") {
        errors.push(relationship_error(
            &path,
            "PARENT_PRODUCT_VARIANT_CANNOT_REQUIRE_SELLING_PLAN",
        ));
    } else if product_id.is_some_and(|product| in_combined_listing(state, product)) {
        errors.push(relationship_error(
            &path,
            "PARENT_PRODUCT_VARIANT_CANNOT_BE_COMBINED_LISTING",
        ));
    }
}

/// `productVariantRelationshipBulkUpdate`.
fn relationship_bulk_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let inputs = records::arg_objects(ctx, "input");
    let state = mutation.state();
    let mut errors = Vec::new();
    // Each parent, the components it is left with, its new price in cents.
    let mut plans: Vec<(String, Components, Option<i64>)> = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let parent = match parent_of(state, input, index) {
            Ok(parent) => parent,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        check_parent(state, &parent, index, &mut errors);
        let components = planned_components(state, &parent, input, index, &mut errors);
        let price = input.get("priceInput").and_then(Value::as_object);
        let calculation = price.and_then(|price| text(price, "calculation"));
        let cents = match calculation {
            Some("COMPONENTS_SUM") => Some(components_sum(state, &components)),
            Some("FIXED") => match price.and_then(|price| price.get("price")) {
                Some(price) if !price.is_null() => variants::money_cents(price),
                _ => {
                    errors.push(relationship_error(
                        &at(index, &["priceInput", "price"]),
                        "UPDATE_PARENT_VARIANT_PRICE_REQUIRED",
                    ));
                    None
                }
            },
            _ => None,
        };
        plans.push((parent, components, cents));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"parentProductVariants": null}), &errors));
    }
    let state = mutation.state_mut();
    let mut parents = Vec::new();
    let mut products: Vec<String> = Vec::new();
    for (parent, components, cents) in &plans {
        set_components(state, parent, components);
        if let Some(cents) = cents {
            set_price(state, parent, *cents);
        }
        state.touch(parent);
        if let Some(product) = state
            .get(parent)
            .and_then(|record| field(record, "product"))
            .map(str::to_string)
            && !products.contains(&product)
        {
            products.push(product);
        }
        parents.push(json!(parent));
    }
    for product in &products {
        state.touch(product);
        product_changed(state, product);
        state.emit("products/update", product);
    }
    Ok(payload(json!({"parentProductVariants": parents}), &[]))
}

// --- Product fixed bundles ---

/// What choosing a value of a bundle option does to a component: one of its options' values, or
/// its quantity.
#[derive(Clone, Debug)]
enum Pick {
    Value { option: String, value: String },
    Quantity(i64),
}

/// A value of a bundle option and what it picks, per component.
#[derive(Clone, Debug)]
struct BundleValue {
    name: String,
    picks: Vec<(usize, Pick)>,
}

/// An option of a bundle.
#[derive(Clone, Debug)]
struct BundleOption {
    name: String,
    values: Vec<BundleValue>,
    /// The component options it is made of: `(component, option)`; a quantity option has none.
    sources: Vec<(usize, Option<String>)>,
}

/// A component of a bundle, as its input gives it.
#[derive(Clone, Debug)]
struct Component {
    product: String,
    quantity: Option<i64>,
}

/// A bundle to make: its components and its options.
struct Plan {
    components: Vec<Component>,
    options: Vec<BundleOption>,
}

fn below(path: &[String], names: &[&str]) -> Vec<String> {
    let mut path = path.to_vec();
    path.extend(names.iter().map(|name| name.to_string()));
    path
}

/// The values of an option of a product, by name.
fn option_values(state: &ShopState, option: &str) -> Vec<String> {
    state
        .get(option)
        .map(|record| {
            gids(record.get("optionValues"))
                .iter()
                .filter_map(|value| state.get(value))
                .filter_map(|value| field(value, "name"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the `components` and `consolidatedOptions` of a bundle input into a plan, checked.
/// `bundle` is the product being updated, which may not be one of its own components.
fn plan_of(
    state: &ShopState,
    components: &[&Map<String, Value>],
    consolidated: &[&Map<String, Value>],
    path: &[String],
    bundle: Option<&str>,
    errors: &mut Vec<UserError>,
) -> Option<Plan> {
    let components_path = below(path, &["components"]);
    if components.is_empty() {
        // lsf's wording.
        errors.push(error_at(&components_path, "Components can't be blank"));
        return None;
    }
    if components.len() > MAX_COMPONENTS {
        // The guide's limit, in its words.
        errors.push(error_at(
            &components_path,
            "A bundle can have up to 30 components.",
        ));
    }
    // The options a consolidated option takes over: (component option, value) → bundle value.
    let consolidated_options: Vec<String> = consolidated
        .iter()
        .flat_map(|option| objects(option, "optionSelections"))
        .flat_map(|selection| objects(selection, "components"))
        .filter_map(|component| text(component, "componentOptionId").map(str::to_string))
        .collect();
    let mut plan = Plan {
        components: Vec::new(),
        options: Vec::new(),
    };
    for (index, component) in components.iter().enumerate() {
        let at = below(&components_path, &[&index.to_string()]);
        let product = text(component, "productId").unwrap_or_default().to_string();
        if !state.is_of(&product, "Product") {
            errors.push(error_at(
                &below(&at, &["productId"]),
                "Product does not exist",
            ));
            continue;
        }
        if Some(product.as_str()) == bundle {
            errors.push(error_at(
                &below(&at, &["productId"]),
                "Nested bundles aren't supported.",
            ));
            continue;
        }
        let nested = state
            .get(&product)
            .and_then(|record| record.get("hasVariantsThatRequiresComponents"))
            .and_then(Value::as_bool)
            == Some(true);
        if nested {
            // The guide's rule, in its words.
            errors.push(error_at(
                &below(&at, &["productId"]),
                "Nested bundles aren't supported.",
            ));
        }
        let quantity = component.get("quantity").and_then(Value::as_i64);
        if quantity.is_some_and(|quantity| quantity < 1) {
            errors.push(error_at(
                &below(&at, &["quantity"]),
                "Quantity must be greater than 0",
            ));
        }
        let own_options = gids(state.get(&product).and_then(|record| record.get("options")));
        for (position, selection) in objects(component, "optionSelections").iter().enumerate() {
            let at = below(&at, &["optionSelections", &position.to_string()]);
            let option = text(selection, "componentOptionId")
                .unwrap_or_default()
                .to_string();
            if !own_options.contains(&option) {
                errors.push(error_at(
                    &below(&at, &["componentOptionId"]),
                    "Option does not exist",
                ));
                continue;
            }
            let known = option_values(state, &option);
            let values: Vec<String> = selection
                .get("values")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if values.is_empty() {
                errors.push(error_at(&below(&at, &["values"]), "Values can't be blank"));
            }
            for (number, value) in values.iter().enumerate() {
                if !known.contains(value) {
                    errors.push(error_at(
                        &below(&at, &["values", &number.to_string()]),
                        format!("Option value '{value}' does not exist"),
                    ));
                }
            }
            if consolidated_options.contains(&option) {
                continue;
            }
            let name = text(selection, "name")
                .unwrap_or_default()
                .trim()
                .to_string();
            if name.is_empty() {
                errors.push(error_at(
                    &below(&at, &["name"]),
                    "Option name can't be blank",
                ));
            }
            plan.options.push(BundleOption {
                name,
                values: values
                    .iter()
                    .map(|value| BundleValue {
                        name: value.clone(),
                        picks: vec![(
                            index,
                            Pick::Value {
                                option: option.clone(),
                                value: value.clone(),
                            },
                        )],
                    })
                    .collect(),
                sources: vec![(index, Some(option.clone()))],
            });
        }
        if let Some(quantity_option) = component.get("quantityOption").and_then(Value::as_object) {
            let values: Vec<BundleValue> = objects(quantity_option, "values")
                .iter()
                .map(|value| BundleValue {
                    name: text(value, "name").unwrap_or_default().to_string(),
                    picks: vec![(
                        index,
                        Pick::Quantity(value.get("quantity").and_then(Value::as_i64).unwrap_or(1)),
                    )],
                })
                .collect();
            if values.iter().any(|value| match value.picks[0].1 {
                Pick::Quantity(quantity) => quantity < 1,
                Pick::Value { .. } => false,
            }) {
                errors.push(error_at(
                    &below(&at, &["quantityOption", "values"]),
                    "Quantity must be greater than 0",
                ));
            }
            plan.options.push(BundleOption {
                name: text(quantity_option, "name")
                    .unwrap_or_default()
                    .to_string(),
                values,
                sources: vec![(index, None)],
            });
        }
        plan.components.push(Component { product, quantity });
    }
    // The consolidated options come first, in their order.
    let mut merged = Vec::new();
    for (position, option) in consolidated.iter().enumerate() {
        let at = below(path, &["consolidatedOptions", &position.to_string()]);
        let mut values = Vec::new();
        let mut sources: Vec<(usize, Option<String>)> = Vec::new();
        for selection in objects(option, "optionSelections") {
            let mut picks = Vec::new();
            for component in objects(selection, "components") {
                let option_id = text(component, "componentOptionId").unwrap_or_default();
                let Some(owner) = plan.components.iter().position(|planned| {
                    gids(
                        state
                            .get(&planned.product)
                            .and_then(|record| record.get("options")),
                    )
                    .iter()
                    .any(|known| known == option_id)
                }) else {
                    errors.push(error_at(
                        &below(&at, &["optionSelections"]),
                        format!("Option {option_id} is not an option of a component"),
                    ));
                    continue;
                };
                if !sources
                    .iter()
                    .any(|(_, known)| known.as_deref() == Some(option_id))
                {
                    sources.push((owner, Some(option_id.to_string())));
                }
                picks.push((
                    owner,
                    Pick::Value {
                        option: option_id.to_string(),
                        value: text(component, "componentOptionValue")
                            .unwrap_or_default()
                            .to_string(),
                    },
                ));
            }
            values.push(BundleValue {
                name: text(selection, "optionValue")
                    .unwrap_or_default()
                    .to_string(),
                picks,
            });
        }
        merged.push(BundleOption {
            name: text(option, "optionName").unwrap_or_default().to_string(),
            values,
            sources,
        });
    }
    merged.append(&mut plan.options);
    plan.options = merged;
    if plan.options.len() > MAX_OPTIONS {
        // The guide's limit, in its words.
        errors.push(error_at(path, "Bundles can have up to three options."));
    }
    let mut names: Vec<String> = Vec::new();
    for option in &plan.options {
        if names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&option.name))
        {
            errors.push(error_at(
                path,
                format!("Option '{}' already exists.", option.name),
            ));
        }
        names.push(option.name.clone());
    }
    Some(plan)
}

/// The combinations of the values of the options: one index per option.
fn combinations(options: &[BundleOption]) -> Vec<Vec<usize>> {
    let mut all: Vec<Vec<usize>> = vec![Vec::new()];
    for option in options {
        let mut next = Vec::new();
        for combination in &all {
            for index in 0..option.values.len() {
                let mut longer = combination.clone();
                longer.push(index);
                next.push(longer);
            }
        }
        all = next;
    }
    all
}

/// The variant of a product whose options have the values picked: `(option, value)`.
fn variant_matching(
    state: &ShopState,
    product: &str,
    picked: &[(String, String)],
) -> Option<String> {
    let options = variants::options_of(state, product);
    let option_of = |value: &str| {
        options
            .iter()
            .find(|option| option.values.iter().any(|(id, _)| id == value))
            .map(|option| option.id.clone())
    };
    let variants = gids(state.get(product).and_then(|record| record.get("variants")));
    variants.into_iter().find(|variant| {
        let Some(record) = state.get(variant) else {
            return false;
        };
        let selected: Vec<(String, String)> = record
            .get("selectedOptions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| {
                let value = option.get("value")?.as_str()?.to_string();
                let id = option_of(option.get("optionValue")?.as_str()?)?;
                Some((id, value))
            })
            .collect();
        picked.iter().all(|pick| selected.contains(pick))
    })
}

/// The components of a combination of a bundle's option values: `(variant, quantity)`, or the
/// component that no variant of its product matches.
fn components_of_combination(
    state: &ShopState,
    plan: &Plan,
    combination: &[usize],
) -> Result<Vec<(String, i64)>, usize> {
    let mut picked: Vec<Vec<(String, String)>> = vec![Vec::new(); plan.components.len()];
    let mut quantities: Vec<Option<i64>> = plan.components.iter().map(|c| c.quantity).collect();
    for (option, value) in plan.options.iter().zip(combination) {
        for (component, pick) in &option.values[*value].picks {
            match pick {
                Pick::Value { option, value } => {
                    picked[*component].push((option.clone(), value.clone()));
                }
                Pick::Quantity(quantity) => quantities[*component] = Some(*quantity),
            }
        }
    }
    let mut components = Vec::new();
    for (index, component) in plan.components.iter().enumerate() {
        let variant = variant_matching(state, &component.product, &picked[index]).ok_or(index)?;
        components.push((variant, quantities[index].unwrap_or(1)));
    }
    Ok(components)
}

/// Checks that every combination of the bundle's values selects a variant of each component.
fn check_combinations(
    state: &ShopState,
    plan: &Plan,
    path: &[String],
    errors: &mut Vec<UserError>,
) {
    let all = combinations(&plan.options);
    if all.len() > variants::VARIANT_LIMIT {
        errors.push(error_at(
            path,
            format!(
                "A product can have at most {} variants",
                variants::VARIANT_LIMIT
            ),
        ));
        return;
    }
    for combination in &all {
        if let Err(index) = components_of_combination(state, plan, combination) {
            let names: Vec<String> = plan
                .options
                .iter()
                .zip(combination)
                .map(|(option, value)| option.values[*value].name.clone())
                .collect();
            // lsf's wording.
            errors.push(error_at(
                &below(path, &["components", &index.to_string()]),
                format!(
                    "No variant of the component matches the bundle's values {}",
                    names.join(" / ")
                ),
            ));
            return;
        }
    }
}

/// Makes the options and the variants of a bundle product from its plan, and its
/// `bundleComponents` and `bundleConsolidatedOptions`. Emits the inventory events of the
/// variants made.
fn build(state: &mut ShopState, product: &str, plan: &Plan) {
    let options: Vec<(String, Vec<String>)> = if plan.options.is_empty() {
        vec![("Title".to_string(), vec!["Default Title".to_string()])]
    } else {
        plan.options
            .iter()
            .map(|option| {
                (
                    option.name.clone(),
                    option
                        .values
                        .iter()
                        .map(|value| value.name.clone())
                        .collect(),
                )
            })
            .collect()
    };
    create_options(state, product, &options);
    let made = variants::options_of(state, product);
    let stock = variants::default_stock(state);
    let combinations = if plan.options.is_empty() {
        vec![vec![0]]
    } else {
        combinations(&plan.options)
    };
    for combination in &combinations {
        let choices: Vec<Choice> = made
            .iter()
            .zip(combination)
            .map(|(option, value)| Choice {
                option: option.id.clone(),
                name: option.values[*value].1.clone(),
                value: Some(option.values[*value].0.clone()),
            })
            .collect();
        let variant = variants::insert_variant(state, product, &choices, &Map::new(), &stock);
        let components = if plan.options.is_empty() {
            plan.components
                .iter()
                .filter_map(|component| {
                    let variant = variant_matching(state, &component.product, &[])?;
                    Some((variant, component.quantity.unwrap_or(1)))
                })
                .collect()
        } else {
            components_of_combination(state, plan, combination).unwrap_or_default()
        };
        let cents = components_sum(state, &components);
        set_components(state, &variant, &components);
        set_price(state, &variant, cents);
        emit_new_inventory(state, &variant);
    }
    // `Product.bundleComponents`: each component, the options it was selected by, its variants.
    let variant_ids = gids(state.get(product).and_then(|record| record.get("variants")));
    let used: Vec<String> = variant_ids
        .iter()
        .flat_map(|variant| components_of(state, variant))
        .map(|(variant, _)| variant)
        .collect();
    let parent_option = |option: &BundleOption| {
        made.iter()
            .find(|made| made.name == option.name)
            .map(|made| made.id.clone())
    };
    let mut bundle_components = Vec::new();
    for (index, component) in plan.components.iter().enumerate() {
        let product_variants = gids(
            state
                .get(&component.product)
                .and_then(|record| record.get("variants")),
        );
        let component_variants: Vec<&String> = product_variants
            .iter()
            .filter(|variant| used.contains(variant))
            .collect();
        let mut selections = Vec::new();
        let mut quantity_option = Value::Null;
        for option in &plan.options {
            for (owner, source) in &option.sources {
                if *owner != index {
                    continue;
                }
                match source {
                    Some(component_option) => {
                        let chosen: Vec<String> = option
                            .values
                            .iter()
                            .flat_map(|value| value.picks.iter())
                            .filter_map(|(owner, pick)| match pick {
                                Pick::Value { option, value }
                                    if *owner == index && option == component_option =>
                                {
                                    Some(value.clone())
                                }
                                _ => None,
                            })
                            .collect();
                        let values: Vec<Value> = option_values(state, component_option)
                            .iter()
                            .map(|value| {
                                json!({"value": value, "selectionStatus": if chosen.contains(value) { "SELECTED" } else { "DESELECTED" }})
                            })
                            .collect();
                        selections.push(json!({
                            "componentOption": component_option,
                            "parentOption": parent_option(option),
                            "values": values,
                        }));
                    }
                    None => {
                        quantity_option = json!({
                            "name": option.name,
                            "parentOption": parent_option(option),
                            "values": option.values.iter().map(|value| json!({
                                "name": value.name,
                                "quantity": match value.picks.first() { Some((_, Pick::Quantity(q))) => *q, _ => 1 },
                            })).collect::<Vec<_>>(),
                        });
                    }
                }
            }
        }
        bundle_components.push(json!({
            "componentProduct": component.product,
            "quantity": if quantity_option.is_null() { json!(component.quantity.unwrap_or(1)) } else { Value::Null },
            "quantityOption": quantity_option,
            "optionSelections": selections,
            "componentVariants": component_variants,
        }));
    }
    let consolidated: Vec<Value> = plan
        .options
        .iter()
        .filter(|option| option.sources.len() > 1)
        .map(|option| {
            json!({
                "name": option.name,
                "selections": option.values.iter().map(|value| json!({
                    "value": value.name,
                    "components": value.picks.iter().filter_map(|(_, pick)| match pick {
                        Pick::Value { option, value } => Some(json!({"optionId": option, "value": value})),
                        Pick::Quantity(_) => None,
                    }).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    if let Some(record) = state.get_mut(product) {
        record.insert("bundleComponents".into(), json!(bundle_components));
        record.insert(
            "bundleConsolidatedOptions".into(),
            if consolidated.is_empty() {
                Value::Null
            } else {
                json!(consolidated)
            },
        );
    }
}

/// Removes a product's variants and options, to make them again.
fn clear(state: &mut ShopState, product: &str) {
    let variants = gids(state.get(product).and_then(|record| record.get("variants")));
    for variant in &variants {
        variants::remove_variant(state, product, variant);
    }
    let options = gids(state.get(product).and_then(|record| record.get("options")));
    for option in options {
        for value in gids(
            state
                .get(&option)
                .and_then(|record| record.get("optionValues")),
        ) {
            state.remove(&value);
        }
        state.remove(&option);
    }
    if let Some(record) = state.get_mut(product) {
        record.insert("options".into(), json!([]));
        record.insert("variants".into(), json!([]));
    }
}

/// A `ProductBundleOperation` that is complete, for a product.
fn operation(state: &mut ShopState, product: &str) -> String {
    let id = state.new_gid("ProductBundleOperation");
    state.insert(value::object(json!({
        "__typename": "ProductBundleOperation",
        "id": id,
        "product": product,
        "status": "COMPLETE",
        "userErrors": [],
    })));
    id
}

/// `productBundleCreate`.
fn bundle_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let path = vec!["input".to_string()];
    let state = mutation.state();
    let mut errors = Vec::new();
    let title = text(&input, "title").unwrap_or_default().trim().to_string();
    super::products::check_title(&title, &mut errors);
    for error in &mut errors {
        error.field = Some(vec!["input".to_string(), "title".to_string()]);
    }
    let components = objects(&input, "components");
    let consolidated = objects(&input, "consolidatedOptions");
    let plan = plan_of(state, &components, &consolidated, &path, None, &mut errors);
    if let Some(plan) = &plan
        && errors.is_empty()
    {
        check_combinations(state, plan, &path, &mut errors);
    }
    let Some(plan) = plan.filter(|_| errors.is_empty()) else {
        return Ok(payload(json!({"productBundleOperation": null}), &errors));
    };
    let state = mutation.state_mut();
    let handle = free_handle(state, &title);
    let (product, mut record) = new_record(state, &title, &handle);
    record.insert("hasVariantsThatRequiresComponents".into(), json!(true));
    record.insert(
        BUNDLE_INPUT.into(),
        json!({"components": input.get("components"), "consolidatedOptions": input.get("consolidatedOptions")}),
    );
    state.insert(record);
    build(state, &product, &plan);
    product_changed(state, &product);
    state.emit("products/create", &product);
    let operation = operation(state, &product);
    Ok(payload(json!({"productBundleOperation": operation}), &[]))
}

/// `productBundleUpdate`.
fn bundle_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let path = vec!["input".to_string()];
    let state = mutation.state();
    let product = text(&input, "productId").unwrap_or_default().to_string();
    let bundle = state
        .get_of(&product, "Product")
        .and_then(|record| record.get(BUNDLE_INPUT))
        .cloned();
    let Some(bundle) = bundle else {
        // lsf's wording, as productUpdate's.
        let error = UserError::new(&["input", "productId"], "Product does not exist");
        return Ok(payload(json!({"productBundleOperation": null}), &[error]));
    };
    let mut errors = Vec::new();
    if let Some(title) = text(&input, "title") {
        super::products::check_title(title, &mut errors);
        for error in &mut errors {
            error.field = Some(vec!["input".to_string(), "title".to_string()]);
        }
    }
    let rebuilt = input.contains_key("components") || input.contains_key("consolidatedOptions");
    let components_value = input
        .get("components")
        .filter(|value| !value.is_null())
        .or_else(|| bundle.get("components"))
        .cloned()
        .unwrap_or(Value::Null);
    let consolidated_value = if input.contains_key("consolidatedOptions") {
        input
            .get("consolidatedOptions")
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        bundle
            .get("consolidatedOptions")
            .cloned()
            .unwrap_or(Value::Null)
    };
    let as_objects = |value: &Value| -> Vec<Map<String, Value>> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .cloned()
            .collect()
    };
    let components = as_objects(&components_value);
    let consolidated = as_objects(&consolidated_value);
    let plan = if rebuilt {
        let components: Vec<&Map<String, Value>> = components.iter().collect();
        let consolidated: Vec<&Map<String, Value>> = consolidated.iter().collect();
        let plan = plan_of(
            state,
            &components,
            &consolidated,
            &path,
            Some(&product),
            &mut errors,
        );
        if let Some(plan) = &plan
            && errors.is_empty()
        {
            check_combinations(state, plan, &path, &mut errors);
        }
        plan
    } else {
        None
    };
    if !errors.is_empty() {
        return Ok(payload(json!({"productBundleOperation": null}), &errors));
    }
    let state = mutation.state_mut();
    if let Some(title) = text(&input, "title")
        && let Some(record) = state.get_mut(&product)
    {
        record.insert("title".into(), json!(title.trim()));
    }
    if let Some(plan) = &plan {
        clear(state, &product);
        build(state, &product, plan);
        if let Some(record) = state.get_mut(&product) {
            record.insert(
                BUNDLE_INPUT.into(),
                json!({"components": components_value, "consolidatedOptions": consolidated_value}),
            );
        }
    }
    state.touch(&product);
    product_changed(state, &product);
    state.emit("products/update", &product);
    let operation = operation(state, &product);
    Ok(payload(json!({"productBundleOperation": operation}), &[]))
}

// --- Queries ---

/// `ProductVariant.productParents`: the products with a variant that has this one as a component.
fn variant_parents(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let variant = object.key().unwrap_or_default().to_string();
    let items: Vec<Value> = parents_of_variants(r.state(), &[variant])
        .into_iter()
        .map(Value::String)
        .collect();
    r.connection(ctx, items)
}

/// `Product.productParents`: the products with a variant that has one of this product's
/// variants as a component.
fn product_parents(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let product = object.key().unwrap_or_default();
    let items: Vec<Value> = parents_of_product(r.state(), product)
        .into_iter()
        .map(Value::String)
        .collect();
    r.connection(ctx, items)
}

/// The products of the components of a product's variants, in order.
fn component_products(state: &ShopState, product: &str) -> Vec<String> {
    let mut products: Vec<String> = Vec::new();
    for variant in gids(state.get(product).and_then(|record| record.get("variants"))) {
        for (component, _) in components_of(state, &variant) {
            if let Some(owner) = state
                .get(&component)
                .and_then(|record| field(record, "product"))
                && !products.iter().any(|known| known == owner)
            {
                products.push(owner.to_string());
            }
        }
    }
    products
}

/// `Product.productComponents`: a `ProductComponentType` per product of the components, with
/// its variants that are components of this product's variants and those that are not.
fn product_components(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let product = object.key().unwrap_or_default();
    let used: Vec<String> = gids(state.get(product).and_then(|record| record.get("variants")))
        .iter()
        .flat_map(|variant| components_of(state, variant))
        .map(|(variant, _)| variant)
        .collect();
    let items: Vec<Value> = component_products(state, product)
        .into_iter()
        .map(|component| {
            let variants = gids(
                state
                    .get(&component)
                    .and_then(|record| record.get("variants")),
            );
            let (inside, outside): (Vec<String>, Vec<String>) = variants
                .into_iter()
                .partition(|variant| used.contains(variant));
            json!({
                "__typename": "ProductComponentType",
                "id": component,
                "product": component,
                "componentVariants": inside,
                "nonComponentVariants": outside,
            })
        })
        .collect();
    r.connection(ctx, items)
}

/// `Product.productComponentsCount`.
fn product_components_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let product = object.key().unwrap_or_default();
    Ok(r.count(ctx, component_products(r.state(), product).len()))
}
