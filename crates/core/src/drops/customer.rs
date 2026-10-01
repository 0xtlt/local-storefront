//! `customer` and `order`.

use std::any::Any;

use slt_liquid::{Object, Value};

use super::metafield::MetafieldsDrop;
use super::product::{ProductDrop, VariantDrop, product_url};
use super::shop::AddressDrop;
use super::{Memo, PaginatedList, SiteRef, hash, strings, time_value};
use crate::store::{Customer, Order};

pub struct CustomerDrop {
    site: SiteRef,
    id: u64,
    memo: Memo,
}

impl CustomerDrop {
    pub fn value(site: &SiteRef, id: u64) -> Value {
        Value::object(CustomerDrop {
            site: site.clone(),
            id,
            memo: Memo::default(),
        })
    }

    fn customer(&self) -> Option<&Customer> {
        self.site.store.customer_by_id(self.id)
    }
}

fn order_total(site: &SiteRef, order: &Order) -> (i64, i64) {
    let subtotal: i64 = order
        .line_items
        .iter()
        .map(|line| {
            site.store.products[line.variant.0].variants[line.variant.1].price
                * i64::from(line.quantity)
        })
        .sum();
    (subtotal, subtotal + order.shipping_price + order.tax_price)
}

fn label(status: &str) -> String {
    crate::util::humanize(status)
}

fn order_value(site: &SiteRef, customer: &Customer, order: &Order) -> Value {
    let (subtotal, total) = order_total(site, order);
    let line_items: Vec<Value> = order
        .line_items
        .iter()
        .map(|line| {
            let product = &site.store.products[line.variant.0];
            let variant = &product.variants[line.variant.1];
            let quantity = i64::from(line.quantity);
            hash([
                ("id", Value::Int(variant.id as i64)),
                ("variant_id", Value::Int(variant.id as i64)),
                ("product_id", Value::Int(product.id as i64)),
                (
                    "title",
                    Value::from(if product.has_only_default_variant() {
                        product.title.clone()
                    } else {
                        format!("{} - {}", product.title, variant.title)
                    }),
                ),
                ("quantity", Value::Int(quantity)),
                ("price", Value::Int(variant.price)),
                ("final_price", Value::Int(variant.price)),
                ("original_price", Value::Int(variant.price)),
                ("line_price", Value::Int(variant.price * quantity)),
                ("final_line_price", Value::Int(variant.price * quantity)),
                ("original_line_price", Value::Int(variant.price * quantity)),
                ("sku", Value::from(&variant.sku)),
                ("vendor", Value::from(&product.vendor)),
                (
                    "url",
                    Value::from(format!(
                        "{}?variant={}",
                        product_url(site, product),
                        variant.id
                    )),
                ),
                ("product", ProductDrop::value(site, line.variant.0)),
                (
                    "variant",
                    VariantDrop::value(site, line.variant.0, line.variant.1),
                ),
                (
                    "image",
                    ProductDrop::value(site, line.variant.0).get("featured_image"),
                ),
                ("fulfillment", Value::Nil),
                ("line_level_discount_allocations", Value::array(Vec::new())),
                ("discount_allocations", Value::array(Vec::new())),
                ("properties", Value::array(Vec::new())),
            ])
        })
        .collect();
    let item_count: i64 = order
        .line_items
        .iter()
        .map(|line| i64::from(line.quantity))
        .sum();
    let address = |address: &Option<crate::store::Address>| {
        address.as_ref().map_or(Value::Nil, |address| {
            AddressDrop::value(site, address, false)
        })
    };
    hash([
        ("id", Value::Int(order.id as i64)),
        ("name", Value::from(&order.name)),
        ("order_number", Value::Int(order.order_number as i64)),
        (
            "confirmation_number",
            Value::from(format!("LOCAL{}", order.order_number)),
        ),
        ("created_at", time_value(site, order.created_at)),
        ("email", Value::from(&customer.email)),
        ("phone", Value::from(&customer.phone)),
        ("financial_status", Value::from(&order.financial_status)),
        (
            "financial_status_label",
            Value::from(label(&order.financial_status)),
        ),
        ("fulfillment_status", Value::from(&order.fulfillment_status)),
        (
            "fulfillment_status_label",
            Value::from(label(&order.fulfillment_status)),
        ),
        ("cancelled", Value::Bool(order.cancelled)),
        ("cancelled_at", Value::Nil),
        ("cancel_reason", Value::Nil),
        ("cancel_reason_label", Value::Nil),
        ("note", Value::from(&order.note)),
        ("line_items", Value::array(line_items.clone())),
        ("subtotal_line_items", Value::array(line_items)),
        ("item_count", Value::Int(item_count)),
        ("line_items_subtotal_price", Value::Int(subtotal)),
        ("subtotal_price", Value::Int(subtotal)),
        ("shipping_price", Value::Int(order.shipping_price)),
        ("tax_price", Value::Int(order.tax_price)),
        ("total_price", Value::Int(total)),
        ("total_net_amount", Value::Int(total)),
        ("total_discounts", Value::Int(0)),
        ("total_refunded_amount", Value::Int(0)),
        ("total_duties", Value::Nil),
        ("shipping_address", address(&order.shipping_address)),
        ("billing_address", address(&order.billing_address)),
        (
            "customer_url",
            Value::from(
                site.request
                    .localized(&format!("/account/orders/{}", order.id)),
            ),
        ),
        (
            "customer_order_url",
            Value::from(
                site.request
                    .localized(&format!("/account/orders/{}", order.id)),
            ),
        ),
        (
            "order_status_url",
            Value::from(
                site.request
                    .localized(&format!("/account/orders/{}", order.id)),
            ),
        ),
        ("discount_applications", Value::array(Vec::new())),
        ("cart_level_discount_applications", Value::array(Vec::new())),
        ("tax_lines", Value::array(Vec::new())),
        ("transactions", Value::array(Vec::new())),
        ("shipping_methods", Value::array(Vec::new())),
        ("tags", Value::array(Vec::new())),
        ("attributes", Value::hash(slt_liquid::Hash::new())),
        ("pickup_in_store?", Value::Bool(false)),
    ])
}

/// The orders of a customer, newest first, as Shopify lists them.
fn orders(site: &SiteRef, customer: &Customer) -> Vec<Value> {
    let mut sorted: Vec<&Order> = customer.orders.iter().collect();
    sorted.sort_by_key(|order| std::cmp::Reverse(order.created_at));
    sorted
        .into_iter()
        .map(|order| order_value(site, customer, order))
        .collect()
}

/// A single order of the logged-in customer, for the order page.
pub fn find_order(site: &SiteRef, order_id: u64) -> Option<Value> {
    let customer = site.customer()?;
    customer
        .orders
        .iter()
        .find(|order| order.id == order_id)
        .map(|order| order_value(site, customer, order))
}

impl Object for CustomerDrop {
    fn type_name(&self) -> &str {
        "customer"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let customer = self.customer()?;
        Some(match key {
            "id" => Value::Int(customer.id as i64),
            "email" => Value::from(&customer.email),
            "first_name" => Value::from(&customer.first_name),
            "last_name" => Value::from(&customer.last_name),
            "name" => Value::from(
                [customer.first_name.as_str(), customer.last_name.as_str()]
                    .iter()
                    .filter(|part| !part.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            "phone" => Value::from(&customer.phone),
            "tags" => strings(&customer.tags),
            "accepts_marketing" => Value::Bool(customer.accepts_marketing),
            "has_account" => Value::Bool(customer.has_account),
            "tax_exempt" => Value::Bool(customer.tax_exempt),
            "addresses" => self.memo.get("addresses", || {
                PaginatedList::value(
                    customer
                        .addresses
                        .iter()
                        .map(|address| AddressDrop::value(site, address, true))
                        .collect(),
                )
            }),
            "addresses_count" => Value::from(customer.addresses.len()),
            "default_address" => customer.addresses.first().map_or(Value::Nil, |address| {
                AddressDrop::value(site, address, true)
            }),
            "orders" => self
                .memo
                .get("orders", || PaginatedList::value(orders(site, customer))),
            "orders_count" => Value::from(customer.orders.len()),
            "last_order" => orders(site, customer)
                .into_iter()
                .next()
                .unwrap_or(Value::Nil),
            "total_spent" => Value::Int(
                customer
                    .orders
                    .iter()
                    .map(|order| order_total(site, order).1)
                    .sum(),
            ),
            "metafields" => MetafieldsDrop::value(site, &customer.metafields),
            "b2b?" | "has_avatar?" => Value::Bool(false),
            "payment_methods" | "company_available_locations" => Value::array(Vec::new()),
            "company_available_locations_count" => Value::Int(0),
            "current_location" | "current_company" | "store_credit_account" => Value::Nil,
            _ => return None,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("customer:{}", self.id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
