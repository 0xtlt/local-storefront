//! The graph a Function input query runs against: the root `Input` type and everything
//! reachable from it, answered from the checkout context.

use chrono::{DateTime, NaiveDateTime, NaiveTime, Utc};
use lsf_core::store::{
    Company, CompanyLocation, Customer, Market, Metafields, Metaobject, Product, SellingPlan,
    Variant,
};
use serde_json::{Value as Json, json};

use super::exec::{Args, Graph, Resolved};
use super::{InputContext, OwnerKind, decimal, delivery_group_id, payment_method_id};
use crate::{
    Address, AppliedDiscount, DeliveryGroup, DeliveryOption, DiscountAllocation, DiscountClass,
    DiscountValue, Metafield, PaymentMethod, ResultLine,
};

pub(super) struct FunctionInput<'c> {
    ctx: &'c InputContext<'c>,
}

impl<'c> FunctionInput<'c> {
    pub(super) fn new(ctx: &'c InputContext<'c>) -> Self {
        FunctionInput { ctx }
    }
}

#[derive(Clone)]
pub(super) enum Node<'c> {
    Input,
    Owner,
    Cart,
    CartCost,
    Line(&'c ResultLine),
    /// A line that ships: `Cart.deliverableLines`.
    DeliverableLine(&'c ResultLine),
    /// A discount applied so far, by its index in `InputContext::discounts`.
    DiscountApplication(usize),
    DiscountAllocation(&'c DiscountAllocation),
    /// The percentage of a discount (`PricingPercentageValue`).
    Percentage(f64),
    LineCost(&'c ResultLine),
    Variant(&'c ResultLine),
    Product(&'c ResultLine),
    /// A selling plan the line is bought with, and the prices it gives each delivery.
    SellingPlanAllocation(&'c SellingPlan, Vec<i64>),
    SellingPlan(&'c SellingPlan),
    PriceAdjustment(i64),
    Money(i64),
    Attribute(&'c str, &'c str),
    BuyerIdentity,
    Customer(&'c Customer),
    PurchasingCompany(&'c Customer),
    /// The customer as a contact of their company.
    CompanyContact(&'c Customer, &'c Company),
    Company(&'c Company),
    CompanyLocation(&'c CompanyLocation),
    DeliveryGroup(&'c DeliveryGroup),
    DeliveryOption(&'c DeliveryOption),
    Address(&'c Address),
    PaymentMethod(&'c PaymentMethod),
    Localization,
    Country(String),
    Language(String),
    /// The market of a country (its upper-case ISO code).
    Market(String),
    MarketRegion(String),
    Shop,
    Metaobject(&'c Metaobject),
    MetaobjectField(&'c str, &'c lsf_core::store::Metafield),
    LocalTime,
    EnteredCode(&'c str),
    BuyerJourney,
    HasTag(String, bool),
    Membership(String, bool),
    Metafield(String, String),
}

/// Every type of the graph, for the type conditions of fragments. Unions and interfaces the
/// graph has a member of are listed in [`Graph::applies`].
const TYPES: &[&str] = &[
    "Input",
    "Discount",
    "DiscountNode",
    "CartTransform",
    "DeliveryCustomization",
    "PaymentCustomization",
    "Validation",
    "Cart",
    "CartCost",
    "CartLine",
    "DeliverableCartLine",
    "DiscountApplication",
    "DiscountAllocation",
    "PricingPercentageValue",
    "CartLineCost",
    "ProductVariant",
    "CustomProduct",
    "Product",
    "SellingPlanAllocation",
    "SellingPlan",
    "SellingPlanAllocationPriceAdjustment",
    "MoneyV2",
    "Attribute",
    "BuyerIdentity",
    "Customer",
    "PurchasingCompany",
    "Company",
    "CompanyContact",
    "CompanyLocation",
    "CartDeliveryGroup",
    "CartDeliveryOption",
    "MailingAddress",
    "PaymentCustomizationPaymentMethod",
    "Localization",
    "Country",
    "Language",
    "Market",
    "MarketRegionCountry",
    "Shop",
    "Metaobject",
    "MetaobjectField",
    "LocalTime",
    "EnteredDiscountCode",
    "BuyerJourney",
    "HasTagResponse",
    "CollectionMembership",
    "Metafield",
    // Unions.
    "Merchandise",
    "MarketRegion",
    "PricingValue",
];

impl<'c> Graph for FunctionInput<'c> {
    type Node = Node<'c>;

    fn root(&self) -> Node<'c> {
        Node::Input
    }

    fn type_name(&self, node: &Node<'c>) -> &'static str {
        match node {
            Node::Input => "Input",
            Node::Owner => match self.ctx.owner.kind {
                OwnerKind::Discount => "Discount",
                OwnerKind::DiscountNode => "DiscountNode",
                OwnerKind::CartTransform => "CartTransform",
                OwnerKind::DeliveryCustomization => "DeliveryCustomization",
                OwnerKind::PaymentCustomization => "PaymentCustomization",
                OwnerKind::Validation => "Validation",
            },
            Node::Cart => "Cart",
            Node::CartCost => "CartCost",
            Node::Line(_) => "CartLine",
            Node::DeliverableLine(_) => "DeliverableCartLine",
            Node::DiscountApplication(_) => "DiscountApplication",
            Node::DiscountAllocation(_) => "DiscountAllocation",
            Node::Percentage(_) => "PricingPercentageValue",
            Node::LineCost(_) => "CartLineCost",
            Node::Variant(_) => "ProductVariant",
            Node::Product(_) => "Product",
            Node::SellingPlanAllocation(..) => "SellingPlanAllocation",
            Node::SellingPlan(_) => "SellingPlan",
            Node::PriceAdjustment(_) => "SellingPlanAllocationPriceAdjustment",
            Node::Money(_) => "MoneyV2",
            Node::Attribute(..) => "Attribute",
            Node::BuyerIdentity => "BuyerIdentity",
            Node::Customer(_) => "Customer",
            Node::PurchasingCompany(_) => "PurchasingCompany",
            Node::CompanyContact(..) => "CompanyContact",
            Node::Company(_) => "Company",
            Node::CompanyLocation(_) => "CompanyLocation",
            Node::DeliveryGroup(_) => "CartDeliveryGroup",
            Node::DeliveryOption(_) => "CartDeliveryOption",
            Node::Address(_) => "MailingAddress",
            Node::PaymentMethod(_) => "PaymentCustomizationPaymentMethod",
            Node::Localization => "Localization",
            Node::Country(_) => "Country",
            Node::Language(_) => "Language",
            Node::Market(_) => "Market",
            Node::MarketRegion(_) => "MarketRegionCountry",
            Node::Shop => "Shop",
            Node::Metaobject(_) => "Metaobject",
            Node::MetaobjectField(..) => "MetaobjectField",
            Node::LocalTime => "LocalTime",
            Node::EnteredCode(_) => "EnteredDiscountCode",
            Node::BuyerJourney => "BuyerJourney",
            Node::HasTag(..) => "HasTagResponse",
            Node::Membership(..) => "CollectionMembership",
            Node::Metafield(..) => "Metafield",
        }
    }

    fn applies(&self, node: &Node<'c>, condition: &str) -> Option<bool> {
        if !TYPES.contains(&condition) {
            return None;
        }
        let name = self.type_name(node);
        Some(
            name == condition
                || (condition == "Merchandise" && name == "ProductVariant")
                || (condition == "MarketRegion" && name == "MarketRegionCountry")
                || (condition == "PricingValue"
                    && matches!(name, "MoneyV2" | "PricingPercentageValue")),
        )
    }

    fn defaults(&self, node: &Node<'c>, field: &str) -> Vec<(String, Json)> {
        self.ctx
            .version
            .schema(self.ctx.api)
            .argument_defaults(self.type_name(node), field)
            .into_iter()
            .map(|(arg, default)| (arg.to_string(), default))
            .collect()
    }

    fn field(
        &self,
        node: &Node<'c>,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Node<'c>>>, String> {
        if name == "metafield"
            && let Some(source) = self.metafields(node)
        {
            let namespace = args
                .string("namespace")?
                .unwrap_or_else(|| "$app".to_string());
            let key = args.required_string("key")?;
            return Ok(Some(match source.find(&namespace, &key) {
                Some((kind, value)) => Resolved::Object(Node::Metafield(kind, value)),
                None => Resolved::Null,
            }));
        }
        Ok(match node {
            Node::Input => self.input(name)?,
            Node::Owner => self.owner(name),
            Node::Cart => self.cart(name, args)?,
            Node::CartCost => self.cart_cost(name),
            Node::Line(line) => self.line(line, name, args)?,
            Node::DeliverableLine(line) => match name {
                "id" => leaf(&line.id),
                "quantity" => leaf(line.quantity),
                "merchandise" => object(Node::Variant(line)),
                "attribute" => self.line(line, name, args)?,
                _ => None,
            },
            Node::DiscountApplication(index) => self.application(*index, name),
            Node::DiscountAllocation(allocation) => match name {
                "discountApplication" => object(Node::DiscountApplication(allocation.discount)),
                "discountedAmount" => object(Node::Money(allocation.amount)),
                _ => None,
            },
            Node::Percentage(percentage) => match name {
                "value" => leaf(percentage_decimal(*percentage)),
                _ => None,
            },
            Node::LineCost(line) => self.line_cost(line, name),
            Node::Variant(line) => self.variant(line, name, args)?,
            Node::Product(line) => self.product(line, name, args)?,
            Node::SellingPlanAllocation(plan, prices) => match name {
                "sellingPlan" => object(Node::SellingPlan(plan)),
                "priceAdjustments" => Some(Resolved::List(
                    prices
                        .iter()
                        .map(|price| Node::PriceAdjustment(*price))
                        .collect(),
                )),
                _ => None,
            },
            Node::SellingPlan(plan) => match name {
                "id" => leaf(format!("gid://shopify/SellingPlan/{}", plan.id)),
                "name" => leaf(&plan.name),
                "description" => Some(nullable(plan.description.as_ref())),
                "recurringDeliveries" => leaf(plan.recurring_deliveries),
                _ => None,
            },
            Node::PriceAdjustment(price) => match name {
                "price" | "perDeliveryPrice" => object(Node::Money(*price)),
                _ => None,
            },
            Node::Money(cents) => match name {
                "amount" => leaf(decimal(*cents)),
                "currencyCode" => leaf(self.ctx.currency),
                _ => None,
            },
            Node::Attribute(key, value) => match name {
                "key" => leaf(*key),
                "value" => leaf(*value),
                _ => None,
            },
            Node::BuyerIdentity => self.buyer_identity(name),
            Node::Customer(customer) => self.customer(customer, name, args)?,
            Node::PurchasingCompany(customer) => match name {
                "company" => customer
                    .company
                    .map(|index| Node::Company(&self.ctx.store.companies[index]))
                    .map_or(Some(Resolved::Null), object),
                "contact" => customer
                    .company
                    .map(|index| Node::CompanyContact(customer, &self.ctx.store.companies[index]))
                    .map_or(Some(Resolved::Null), object),
                "location" => {
                    let session = self.ctx.session;
                    customer
                        .current_location(session.company_location, self.ctx.store)
                        .zip(customer.company)
                        .map(|(location, company)| {
                            Node::CompanyLocation(
                                &self.ctx.store.companies[company].locations[location],
                            )
                        })
                        .map_or(Some(Resolved::Null), object)
                }
                _ => None,
            },
            // Every buyer of a company is one of its contacts. What the data does not say
            // defaults to the company: its creation date, no title, no language.
            Node::CompanyContact(customer, company) => {
                let contact = customer.company_contact.as_ref();
                match name {
                    "id" => leaf(format!("gid://shopify/CompanyContact/{}", customer.id)),
                    "title" => Some(nullable(contact.and_then(|c| c.title.as_ref()))),
                    "locale" => Some(nullable(contact.and_then(|c| c.locale.as_ref()))),
                    "createdAt" => leaf(date_time(
                        contact.map_or(company.created_at, |contact| contact.created_at),
                    )),
                    "updatedAt" => leaf(date_time(
                        contact.map_or(company.created_at, |c| c.updated_at),
                    )),
                    _ => None,
                }
            }
            Node::Company(company) => match name {
                "id" => leaf(format!("gid://shopify/Company/{}", company.id)),
                "name" => leaf(&company.name),
                "externalId" => Some(nullable(company.external_id.as_ref())),
                "createdAt" => leaf(date_time(company.created_at)),
                "updatedAt" => leaf(date_time(company.updated_at)),
                _ => None,
            },
            Node::CompanyLocation(location) => match name {
                "id" => leaf(format!("gid://shopify/CompanyLocation/{}", location.id)),
                "name" => leaf(&location.name),
                "externalId" => Some(nullable(location.external_id.as_ref())),
                "locale" => Some(nullable(location.locale.as_ref())),
                "createdAt" => leaf(date_time(location.created_at)),
                "updatedAt" => leaf(date_time(location.updated_at)),
                "ordersCount" => leaf(location.orders_count),
                "totalSpent" => object(Node::Money(location.total_spent)),
                _ => None,
            },
            Node::DeliveryGroup(group) => self.delivery_group(group, name),
            Node::DeliveryOption(option) => match name {
                "handle" => leaf(&option.handle),
                "title" => leaf(&option.title),
                "description" => Some(nullable(option.description.as_ref())),
                "cost" => object(Node::Money(option.cost)),
                "deliveryMethodType" => Some(Resolved::Leaf(json!(option.kind))),
                "code" => Some(Resolved::Null),
                _ => None,
            },
            Node::Address(address) if name == "market" => {
                // The market that sells to the country of the address; none when the store
                // does not sell there.
                let code = address.country_code.as_deref().unwrap_or_default();
                Some(if self.ctx.store.country(code).is_some() {
                    Resolved::Object(Node::Market(code.to_ascii_uppercase()))
                } else {
                    Resolved::Null
                })
            }
            Node::Address(address) => address_field(address, name),
            Node::PaymentMethod(method) => match name {
                "id" => leaf(payment_method_id(&method.handle)),
                "name" => leaf(&method.name),
                "placements" => leaf(
                    method
                        .placements
                        .iter()
                        .map(|placement| placement.graphql())
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            },
            Node::Localization => match name {
                "country" => object(Node::Country(self.country())),
                "language" => object(Node::Language(self.language())),
                "market" => object(Node::Market(self.country())),
                _ => None,
            },
            Node::Country(code) => match name {
                "isoCode" => leaf(code),
                _ => None,
            },
            Node::Language(code) => match name {
                "isoCode" => leaf(code),
                _ => None,
            },
            Node::Market(code) => self.market(code, name),
            Node::MarketRegion(code) => match name {
                "code" => leaf(code),
                "name" => leaf(
                    self.ctx
                        .store
                        .country(code)
                        .map_or(code.as_str(), |c| &c.name),
                ),
                _ => None,
            },
            Node::Shop => match name {
                "localTime" => object(Node::LocalTime),
                "metaobject" => Some(self.metaobject(args)?),
                _ => None,
            },
            Node::Metaobject(entry) => match name {
                "handle" => leaf(&entry.handle),
                "type" => leaf(&entry.kind),
                "field" => {
                    let key = args.required_string("key")?;
                    Some(
                        entry
                            .fields
                            .get_key_value(&key)
                            .map_or(Resolved::Null, |(key, field)| {
                                Resolved::Object(Node::MetaobjectField(key, field))
                            }),
                    )
                }
                _ => None,
            },
            Node::MetaobjectField(key, field) => {
                let value = source_text(&field.value);
                match name {
                    "key" => leaf(*key),
                    "type" => leaf(&field.kind),
                    "value" => leaf(&value),
                    "jsonValue" => Some(Resolved::Leaf(json_value(&field.kind, &value))),
                    _ => None,
                }
            }
            Node::LocalTime => self.local_time(name, args)?,
            Node::EnteredCode(code) => match name {
                "code" => leaf(*code),
                "rejectable" => leaf(true),
                _ => None,
            },
            Node::BuyerJourney => match name {
                "step" => leaf(self.ctx.state.step.graphql()),
                _ => None,
            },
            Node::HasTag(tag, has) => match name {
                "tag" => leaf(tag),
                "hasTag" => leaf(*has),
                _ => None,
            },
            Node::Membership(id, member) => match name {
                "collectionId" => leaf(id),
                "isMember" => leaf(*member),
                _ => None,
            },
            Node::Metafield(kind, value) => match name {
                "type" => leaf(kind),
                "value" => leaf(value),
                "jsonValue" => Some(Resolved::Leaf(json_value(kind, value))),
                _ => None,
            },
        })
    }
}

/// What the order discounts took from a line.
fn order_discounts(line: &ResultLine) -> i64 {
    line.discount_allocations
        .iter()
        .filter(|allocation| allocation.class == DiscountClass::Order)
        .map(|allocation| allocation.amount)
        .sum()
}

fn leaf<'c>(value: impl serde::Serialize) -> Option<Resolved<Node<'c>>> {
    Some(Resolved::Leaf(
        serde_json::to_value(value).unwrap_or(Json::Null),
    ))
}

fn object<'c>(node: Node<'c>) -> Option<Resolved<Node<'c>>> {
    Some(Resolved::Object(node))
}

/// A leaf that is null when the value is missing or blank.
fn nullable<'c, S: AsRef<str>>(value: Option<&S>) -> Resolved<Node<'c>> {
    match value.map(AsRef::as_ref) {
        Some(text) if !text.is_empty() => Resolved::Leaf(Json::String(text.to_string())),
        _ => Resolved::Null,
    }
}

impl<'c> FunctionInput<'c> {
    fn input(&self, name: &str) -> Result<Option<Resolved<Node<'c>>>, String> {
        let ctx = self.ctx;
        if let Some(kind) = [
            OwnerKind::Discount,
            OwnerKind::DiscountNode,
            OwnerKind::CartTransform,
            OwnerKind::DeliveryCustomization,
            OwnerKind::PaymentCustomization,
            OwnerKind::Validation,
        ]
        .into_iter()
        .find(|kind| kind.field() == name)
        {
            if kind != ctx.owner.kind {
                return Err(format!(
                    "this Function's owner is the `{}` root field, not `{name}`",
                    ctx.owner.kind.field()
                ));
            }
            return Ok(object(Node::Owner));
        }
        Ok(match name {
            "cart" => object(Node::Cart),
            "localization" => object(Node::Localization),
            "shop" => object(Node::Shop),
            "buyerJourney" => object(Node::BuyerJourney),
            "presentmentCurrencyRate" => leaf("1.0"),
            "triggeringDiscountCode" => Some(match ctx.triggering_code {
                Some(code) => Resolved::Leaf(Json::String(code.to_string())),
                None => Resolved::Null,
            }),
            "enteredDiscountCodes" => Some(Resolved::List(
                ctx.entered_codes
                    .iter()
                    .map(|code| Node::EnteredCode(code))
                    .collect(),
            )),
            "paymentMethods" => Some(Resolved::List(
                ctx.payment_methods
                    .iter()
                    .map(Node::PaymentMethod)
                    .collect(),
            )),
            _ => None,
        })
    }

    fn owner(&self, name: &str) -> Option<Resolved<Node<'c>>> {
        let owner = &self.ctx.owner;
        match name {
            "discountClasses" if owner.kind == OwnerKind::Discount => {
                Some(Resolved::Leaf(json!(owner.discount_classes)))
            }
            _ => None,
        }
    }

    fn cart(&self, name: &str, args: &mut Args) -> Result<Option<Resolved<Node<'c>>>, String> {
        let ctx = self.ctx;
        Ok(match name {
            "lines" => Some(Resolved::List(ctx.lines.iter().map(Node::Line).collect())),
            // The lines that ship: the ones the delivery groups hold.
            "deliverableLines" => Some(Resolved::List(
                ctx.lines
                    .iter()
                    .filter(|line| line.requires_shipping)
                    .map(Node::DeliverableLine)
                    .collect(),
            )),
            "discountApplications" => Some(Resolved::List(
                (0..ctx.discounts.len())
                    .map(Node::DiscountApplication)
                    .collect(),
            )),
            "cost" => object(Node::CartCost),
            "buyerIdentity" => object(Node::BuyerIdentity),
            "deliveryGroups" => Some(Resolved::List(
                ctx.delivery_groups
                    .iter()
                    .map(Node::DeliveryGroup)
                    .collect(),
            )),
            // What checkout does not collect, or does not have locally: no billing address (the
            // shipping one is the only address), purchase order number or point of sale
            // location, and no localized fields (tax identifiers and the like).
            "billingAddress" | "poNumber" | "retailLocation" => Some(Resolved::Null),
            "localizedFields" => {
                args.take("keys");
                Some(Resolved::List(Vec::new()))
            }
            "attribute" => {
                let key = args.required_string("key")?;
                Some(
                    ctx.session
                        .cart_attributes
                        .get_key_value(&key)
                        .map_or(Resolved::Null, |(key, value)| {
                            Resolved::Object(Node::Attribute(key, value))
                        }),
                )
            }
            _ => None,
        })
    }

    fn cart_cost(&self, name: &str) -> Option<Resolved<Node<'c>>> {
        let lines = self.ctx.lines;
        match name {
            // Before taxes and cart-level (order) discounts, after the discounts on the lines.
            "subtotalAmount" => object(Node::Money(
                lines
                    .iter()
                    .map(|line| line.total + order_discounts(line))
                    .sum(),
            )),
            // What the buyer pays at this step: every discount applied so far and the selected
            // shipping, before taxes.
            "totalAmount" => {
                let items: i64 = lines.iter().map(|line| line.total).sum();
                let shipping: i64 = self
                    .ctx
                    .delivery_groups
                    .iter()
                    .map(DeliveryGroup::selected_cost)
                    .sum();
                object(Node::Money(items + shipping))
            }
            // Taxes and duties are only known once the checkout is complete.
            "totalTaxAmount" | "totalDutyAmount" => Some(Resolved::Null),
            _ => None,
        }
    }

    fn line(
        &self,
        line: &'c ResultLine,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Node<'c>>>, String> {
        Ok(match name {
            "id" => leaf(&line.id),
            "quantity" => leaf(line.quantity),
            "merchandise" => object(Node::Variant(line)),
            "cost" => object(Node::LineCost(line)),
            "attribute" => {
                let key = args.required_string("key")?;
                Some(
                    line.attributes
                        .iter()
                        .find(|attribute| attribute.key == key)
                        .map_or(Resolved::Null, |attribute| {
                            Resolved::Object(Node::Attribute(&attribute.key, &attribute.value))
                        }),
                )
            }
            "discountAllocations" => Some(Resolved::List(
                line.discount_allocations
                    .iter()
                    .map(Node::DiscountAllocation)
                    .collect(),
            )),
            "sellingPlanAllocation" => Some(self.selling_plan(line).unwrap_or(Resolved::Null)),
            // A bundle is one line: its components are not lines of the cart a Function sees.
            "parentRelationship" => Some(Resolved::Null),
            _ => None,
        })
    }

    /// The selling plan of the line: the session cart line the id (`CartLine/<n>`) points to.
    fn selling_plan(&self, line: &ResultLine) -> Option<Resolved<Node<'c>>> {
        let store = self.ctx.store;
        let position = line.id.rsplit('/').next()?.parse::<usize>().ok()?;
        let cart_line = self.ctx.session.cart_lines.get(position.checked_sub(1)?)?;
        if cart_line.variant_id != line.variant_id {
            return None;
        }
        let (product, variant) = store.variant(line.variant_id)?;
        let (_, plan) = store.selling_plan_of(product, cart_line.selling_plan?)?;
        Some(Resolved::Object(Node::SellingPlanAllocation(
            plan,
            plan.allocation(variant).adjusted_prices,
        )))
    }

    fn line_cost(&self, line: &'c ResultLine, name: &str) -> Option<Resolved<Node<'c>>> {
        match name {
            "amountPerQuantity" => object(Node::Money(line.unit_price)),
            "compareAtAmountPerQuantity" => Some(
                self.store_variant(line)
                    .and_then(|(_, variant)| variant.compare_at_price)
                    .filter(|price| *price > line.unit_price)
                    .map_or(Resolved::Null, |price| Resolved::Object(Node::Money(price))),
            ),
            // Before any discount, then after the ones applied so far.
            "subtotalAmount" => object(Node::Money(line.subtotal)),
            "totalAmount" => object(Node::Money(line.total)),
            _ => None,
        }
    }

    fn store_variant(&self, line: &ResultLine) -> Option<(&'c Product, &'c Variant)> {
        self.ctx.store.variant(line.variant_id)
    }

    fn variant(
        &self,
        line: &'c ResultLine,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Node<'c>>>, String> {
        let found = self.store_variant(line);
        Ok(match name {
            "id" => leaf(format!("gid://shopify/ProductVariant/{}", line.variant_id)),
            "sku" => Some(nullable(
                found
                    .map(|(_, variant)| &variant.sku)
                    .filter(|sku| !sku.is_empty())
                    .or(line.sku.as_ref()),
            )),
            "title" => leaf(
                found
                    .map(|(_, variant)| variant.title.as_str())
                    .or(line.variant_title.as_deref())
                    .unwrap_or(&line.title),
            ),
            "requiresShipping" => leaf(line.requires_shipping),
            "weight" => leaf(found.map_or(0.0, |(_, variant)| {
                lsf_core::drops::product::weight_in_unit(variant.weight, &variant.weight_unit)
            })),
            "weightUnit" => leaf(
                match found.map(|(_, variant)| variant.weight_unit.as_str()) {
                    Some("g") => "GRAMS",
                    Some("oz") => "OUNCES",
                    Some("lb") => "POUNDS",
                    _ => "KILOGRAMS",
                },
            ),
            "product" => object(Node::Product(line)),
            // The collections of a variant are the ones of its product.
            "inAnyCollection" | "inCollections" => {
                self.collections(found.map(|(product, _)| product), name, args)?
            }
            _ => None,
        })
    }

    fn product(
        &self,
        line: &'c ResultLine,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Node<'c>>>, String> {
        let product = self.store_variant(line).map(|(product, _)| product);
        Ok(match name {
            "id" => leaf(format!("gid://shopify/Product/{}", line.product_id)),
            "handle" => leaf(product.map_or("", |product| product.handle.as_str())),
            "title" => leaf(product.map_or(line.title.as_str(), |product| product.title.as_str())),
            "productType" => Some(nullable(product.map(|product| &product.product_type))),
            "vendor" => leaf(product.map_or("", |product| product.vendor.as_str())),
            "isGiftCard" => leaf(product.is_some_and(|product| product.gift_card)),
            "hasAnyTag" => {
                let tags = args.string_list("tags")?;
                leaf(
                    tags.iter()
                        .any(|tag| has_tag(product.map(|p| p.tags.as_slice()), tag)),
                )
            }
            "hasTags" => {
                let tags = args.string_list("tags")?;
                Some(Resolved::List(
                    tags.into_iter()
                        .map(|tag| {
                            let has = has_tag(product.map(|p| p.tags.as_slice()), &tag);
                            Node::HasTag(tag, has)
                        })
                        .collect(),
                ))
            }
            "inAnyCollection" | "inCollections" => self.collections(product, name, args)?,
            _ => None,
        })
    }

    /// `inAnyCollection` and `inCollections`, which a product and its variants share.
    fn collections(
        &self,
        product: Option<&Product>,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Node<'c>>>, String> {
        let ids = args.string_list("ids")?;
        Ok(match name {
            "inAnyCollection" => leaf(ids.iter().any(|id| self.in_collection(product, id))),
            "inCollections" => Some(Resolved::List(
                ids.into_iter()
                    .map(|id| {
                        let member = self.in_collection(product, &id);
                        Node::Membership(id, member)
                    })
                    .collect(),
            )),
            _ => None,
        })
    }

    /// Whether the product is in the collection an id (a GID, or a bare number) names.
    fn in_collection(&self, product: Option<&Product>, id: &str) -> bool {
        let Some(wanted) = id.rsplit('/').next().and_then(|id| id.parse::<u64>().ok()) else {
            return false;
        };
        product.is_some_and(|product| {
            product
                .collections
                .iter()
                .any(|index| self.ctx.store.collections[*index].id == wanted)
        })
    }

    fn buyer_identity(&self, name: &str) -> Option<Resolved<Node<'c>>> {
        let ctx = self.ctx;
        let customer = self.customer_of_session();
        match name {
            "email" => Some(nullable(
                ctx.state
                    .email
                    .as_ref()
                    .or_else(|| customer.map(|customer| &customer.email)),
            )),
            "phone" => Some(nullable(
                ctx.state
                    .phone
                    .as_ref()
                    .or_else(|| customer.map(|customer| &customer.phone)),
            )),
            "isAuthenticated" => leaf(customer.is_some()),
            "customer" => customer.map_or(Some(Resolved::Null), |customer| {
                object(Node::Customer(customer))
            }),
            // The Shop app account of the buyer: there is none locally.
            "shopUser" => Some(Resolved::Null),
            "purchasingCompany" => Some(
                customer
                    .filter(|customer| customer.company.is_some())
                    .map_or(Resolved::Null, |customer| {
                        Resolved::Object(Node::PurchasingCompany(customer))
                    }),
            ),
            _ => None,
        }
    }

    fn customer_of_session(&self) -> Option<&'c Customer> {
        self.ctx
            .session
            .customer_id
            .and_then(|id| self.ctx.store.customer_by_id(id))
    }

    fn customer(
        &self,
        customer: &'c Customer,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Node<'c>>>, String> {
        let store = self.ctx.store;
        Ok(match name {
            "id" => leaf(format!("gid://shopify/Customer/{}", customer.id)),
            "email" => Some(nullable(Some(&customer.email))),
            "firstName" => Some(nullable(Some(&customer.first_name))),
            "lastName" => Some(nullable(Some(&customer.last_name))),
            "displayName" => {
                let full = format!("{} {}", customer.first_name, customer.last_name);
                let full = full.trim();
                leaf(if full.is_empty() {
                    customer.email.as_str()
                } else {
                    full
                })
            }
            // Added in 2026-10. The store data has no creation date: the first order's, or now.
            "createdAt" => {
                let created = customer
                    .orders
                    .iter()
                    .map(|order| order.created_at.fixed_offset())
                    .min()
                    .unwrap_or(self.ctx.now);
                leaf(created.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            }
            "numberOfOrders" => leaf(customer.orders.len() as u64),
            "amountSpent" => {
                let spent: i64 = customer
                    .orders
                    .iter()
                    .filter(|order| !order.cancelled)
                    .map(|order| {
                        let lines: i64 = order
                            .line_items
                            .iter()
                            .map(|line| {
                                store.products[line.variant.0].variants[line.variant.1].price
                                    * i64::from(line.quantity)
                            })
                            .sum();
                        lines + order.shipping_price + order.tax_price
                    })
                    .sum();
                object(Node::Money(spent))
            }
            "hasAnyTag" => {
                let tags = args.string_list("tags")?;
                leaf(tags.iter().any(|tag| has_tag(Some(&customer.tags), tag)))
            }
            "hasTags" => {
                let tags = args.string_list("tags")?;
                Some(Resolved::List(
                    tags.into_iter()
                        .map(|tag| {
                            let has = has_tag(Some(&customer.tags), &tag);
                            Node::HasTag(tag, has)
                        })
                        .collect(),
                ))
            }
            _ => None,
        })
    }

    fn delivery_group(&self, group: &'c DeliveryGroup, name: &str) -> Option<Resolved<Node<'c>>> {
        let ctx = self.ctx;
        match name {
            "id" => leaf(delivery_group_id(&group.handle)),
            "groupType" => {
                let subscription = ctx
                    .lines
                    .iter()
                    .filter(|line| group.line_ids.contains(&line.id))
                    .any(|line| self.selling_plan(line).is_some());
                leaf(if subscription {
                    "SUBSCRIPTION"
                } else {
                    "ONE_TIME_PURCHASE"
                })
            }
            "cartLines" => Some(Resolved::List(
                ctx.lines
                    .iter()
                    .filter(|line| group.line_ids.contains(&line.id))
                    .map(Node::Line)
                    .collect(),
            )),
            "discountAllocations" => Some(Resolved::List(
                group
                    .discount_allocations
                    .iter()
                    .map(Node::DiscountAllocation)
                    .collect(),
            )),
            "deliveryAddress" => Some(
                ctx.state
                    .shipping_address
                    .as_ref()
                    .map_or(Resolved::Null, |address| {
                        Resolved::Object(Node::Address(address))
                    }),
            ),
            "deliveryOptions" => Some(Resolved::List(
                group.options.iter().map(Node::DeliveryOption).collect(),
            )),
            "selectedDeliveryOption" => Some(
                group
                    .selected
                    .as_ref()
                    .and_then(|handle| group.options.iter().find(|option| &option.handle == handle))
                    .map_or(Resolved::Null, |option| {
                        Resolved::Object(Node::DeliveryOption(option))
                    }),
            ),
            _ => None,
        }
    }

    /// The country of the market: the one the session selected, the shop's otherwise.
    fn country(&self) -> String {
        let store = self.ctx.store;
        self.ctx
            .session
            .country
            .clone()
            .filter(|code| !code.is_empty())
            .or_else(|| {
                Some(store.shop.address.country_code.clone()).filter(|code| !code.is_empty())
            })
            .unwrap_or_else(|| "US".to_string())
            .to_ascii_uppercase()
    }

    /// The language of the buyer, else the primary one of the store. `LanguageCode` is the
    /// upper-case code with `_` for `-`: `PT_BR`.
    fn language(&self) -> String {
        let store = self.ctx.store;
        self.ctx
            .state
            .language
            .as_deref()
            .or_else(|| {
                store
                    .languages
                    .iter()
                    .find(|language| language.primary)
                    .or_else(|| store.languages.first())
                    .map(|language| language.iso_code.as_str())
            })
            .unwrap_or("EN")
            .replace('-', "_")
            .to_ascii_uppercase()
    }

    /// The market that sells to a country: the one the data groups it in, else one of its own.
    fn market(&self, code: &str, name: &str) -> Option<Resolved<Node<'c>>> {
        let store = self.ctx.store;
        let declared = store.market_of(code);
        match name {
            "id" => leaf(format!(
                "gid://shopify/Market/{}",
                lsf_core::util::stable_id("market", declared.map_or(code, |m| m.handle.as_str()))
            )),
            "handle" => leaf(match (declared, store.country(code)) {
                (Some(market), _) => market.handle.clone(),
                (None, Some(country)) => lsf_core::util::handleize(&country.name),
                (None, None) => code.to_ascii_lowercase(),
            }),
            "regions" => Some(Resolved::List(match declared {
                Some(market) => market
                    .countries
                    .iter()
                    .cloned()
                    .map(Node::MarketRegion)
                    .collect(),
                None => vec![Node::MarketRegion(code.to_string())],
            })),
            _ => None,
        }
    }

    /// `Shop.metaobject(handle: {type, handle})` or `(id:)`.
    fn metaobject(&self, args: &mut Args) -> Result<Resolved<Node<'c>>, String> {
        let store = self.ctx.store;
        let handle = args.take("handle");
        let id = args.string("id")?;
        let found = match (handle, id) {
            (Some(handle), None) => {
                let part = |key: &str| {
                    handle
                        .get(key)
                        .and_then(Json::as_str)
                        .ok_or_else(|| format!("argument `handle` needs a string `{key}`"))
                };
                store.metaobject(part("type")?, part("handle")?)
            }
            (None, Some(id)) => store
                .metaobjects
                .iter()
                .find(|entry| metaobject_id(entry) == id),
            _ => {
                return Err(
                    "give the metaobject either as `handle: {type:, handle:}` or as `id:`".into(),
                );
            }
        };
        Ok(found.map_or(Resolved::Null, |entry| {
            Resolved::Object(Node::Metaobject(entry))
        }))
    }

    /// A discount applied so far, as `DiscountApplication`.
    fn application(&self, index: usize, name: &str) -> Option<Resolved<Node<'c>>> {
        let discount: &AppliedDiscount = &self.ctx.discounts[index];
        let details = &discount.details;
        match name {
            "allocationMethod" => leaf(details.allocation_method.graphql()),
            "targetSelection" => leaf(details.target_selection.graphql()),
            "targetType" => leaf(details.target_type.graphql()),
            "totalAllocatedAmount" => object(Node::Money(discount.amount)),
            "value" => object(match details.value {
                DiscountValue::Percentage(percentage) => Node::Percentage(percentage),
                DiscountValue::Fixed(cents) => Node::Money(cents),
            }),
            _ => None,
        }
    }

    fn local_time(
        &self,
        name: &str,
        args: &mut Args,
    ) -> Result<Option<Resolved<Node<'c>>>, String> {
        let local = self
            .ctx
            .now
            .with_timezone(&self.ctx.store.shop.timezone)
            .naive_local();
        let time = |args: &mut Args, key: &str| -> Result<NaiveTime, String> {
            let text = args.required_string(key)?;
            ["%H:%M:%S%.f", "%H:%M"]
                .iter()
                .find_map(|format| NaiveTime::parse_from_str(&text, format).ok())
                .ok_or_else(|| {
                    format!("argument `{key}` must be a time like \"14:30:00\", got \"{text}\"")
                })
        };
        let date_time = |args: &mut Args, key: &str| -> Result<NaiveDateTime, String> {
            let text = args.required_string(key)?;
            ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M"]
                .iter()
                .find_map(|format| NaiveDateTime::parse_from_str(&text, format).ok())
                .ok_or_else(|| {
                    format!("argument `{key}` must be a date and time like \"2026-01-31T14:30:00\", got \"{text}\"")
                })
        };
        // `After` and `Before` are strict; `Between` includes both ends.
        Ok(match name {
            "date" => leaf(local.date().format("%Y-%m-%d").to_string()),
            "timeAfter" => leaf(local.time() > time(args, "time")?),
            "timeBefore" => leaf(local.time() < time(args, "time")?),
            "timeBetween" => {
                let (start, end) = (time(args, "startTime")?, time(args, "endTime")?);
                leaf(start <= local.time() && local.time() <= end)
            }
            "dateTimeAfter" => leaf(local > date_time(args, "dateTime")?),
            "dateTimeBefore" => leaf(local < date_time(args, "dateTime")?),
            "dateTimeBetween" => {
                let (start, end) = (
                    date_time(args, "startDateTime")?,
                    date_time(args, "endDateTime")?,
                );
                leaf(start <= local && local <= end)
            }
            _ => None,
        })
    }

    /// Where the `metafield` of a node reads from, for the nodes that have one.
    fn metafields(&self, node: &Node<'c>) -> Option<Source<'c>> {
        let store = self.ctx.store;
        match node {
            Node::Owner => Some(Source::Owned(self.ctx.owner.metafields)),
            // The ones checkout UI extensions write (`applyMetafieldChange`).
            Node::Cart => Some(Source::Owned(&self.ctx.state.metafields)),
            Node::Product(line) => Some(Source::Core(
                self.store_variant(line)
                    .map(|(product, _)| &product.metafields),
            )),
            Node::Variant(line) => Some(Source::Core(
                self.store_variant(line)
                    .map(|(_, variant)| &variant.metafields),
            )),
            Node::Customer(customer) => Some(Source::Core(Some(&customer.metafields))),
            // Added in 2026-10. Shipping rates carry no metafields.
            Node::DeliveryOption(_) => Some(Source::Core(None)),
            // The selling plans of the store carry no metafields.
            Node::SellingPlan(_) => Some(Source::Core(None)),
            Node::Company(company) => Some(Source::Core(Some(&company.metafields))),
            Node::CompanyLocation(location) => Some(Source::Core(Some(&location.metafields))),
            Node::Shop => Some(Source::Core(Some(&store.shop.metafields))),
            // The metafields of the discount that made the application.
            Node::DiscountApplication(index) => Some(Source::Owned(
                &self.ctx.discounts[*index].details.metafields,
            )),
            Node::Market(code) => Some(Source::Core(
                store
                    .market_of(code)
                    .map(|market: &Market| &market.metafields),
            )),
            _ => None,
        }
    }
}

/// A date as GraphQL `DateTime`: `2024-01-01T12:00:00Z`.
fn date_time(date: DateTime<Utc>) -> String {
    date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// A percentage as the `Decimal` Shopify sends: `10.0`, `12.5`.
pub(super) fn percentage_decimal(percentage: f64) -> String {
    let text = percentage.to_string();
    if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    }
}

/// The id of a metaobject, which the store data does not give: stable for its type and handle.
fn metaobject_id(entry: &Metaobject) -> String {
    format!(
        "gid://shopify/Metaobject/{}",
        lsf_core::util::stable_id("metaobject", &format!("{}/{}", entry.kind, entry.handle))
    )
}

/// The text a metafield value is kept as: JSON for everything that is not a string.
fn source_text(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn has_tag(tags: Option<&[String]>, wanted: &str) -> bool {
    tags.is_some_and(|tags| tags.iter().any(|tag| tag.eq_ignore_ascii_case(wanted)))
}

fn address_field<'c>(address: &Address, name: &str) -> Option<Resolved<Node<'c>>> {
    let text = |value: &Option<String>| nullable(value.as_ref());
    Some(match name {
        "address1" => text(&address.address1),
        "address2" => text(&address.address2),
        "city" => text(&address.city),
        "company" => text(&address.company),
        "countryCode" => match &address.country_code {
            Some(code) if !code.is_empty() => Resolved::Leaf(json!(code.to_ascii_uppercase())),
            _ => Resolved::Null,
        },
        "firstName" => text(&address.first_name),
        "lastName" => text(&address.last_name),
        "name" => {
            let full = [address.first_name.as_deref(), address.last_name.as_deref()]
                .into_iter()
                .flatten()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            nullable(Some(&full))
        }
        "latitude" | "longitude" => Resolved::Null,
        "phone" => text(&address.phone),
        "provinceCode" => text(&address.province_code),
        "zip" => text(&address.zip),
        _ => return None,
    })
}

/// Where metafields are read from: the store's, or the ones the pipeline hands over.
enum Source<'c> {
    Core(Option<&'c Metafields>),
    Owned(&'c [Metafield]),
}

impl Source<'_> {
    /// The `(type, value)` of a metafield. The reserved namespace of the app is written
    /// `$app` in queries and data (`$app:config`) and `app--<id>--config` by Shopify: all three
    /// name the same one.
    fn find(&self, namespace: &str, key: &str) -> Option<(String, String)> {
        let wanted = canonical_namespace(namespace);
        match self {
            Source::Core(None) => None,
            Source::Core(Some(metafields)) => {
                let (_, entries) = metafields
                    .iter()
                    .find(|(candidate, _)| canonical_namespace(candidate) == wanted)?;
                let metafield = entries.get(key)?;
                Some((metafield.kind.clone(), source_text(&metafield.value)))
            }
            Source::Owned(metafields) => metafields
                .iter()
                .find(|metafield| {
                    metafield.key == key && canonical_namespace(&metafield.namespace) == wanted
                })
                .map(|metafield| (metafield.kind.clone(), metafield.value.clone())),
        }
    }
}

fn canonical_namespace(namespace: &str) -> String {
    if let Some(rest) = namespace.strip_prefix("app--") {
        let digits =
            |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
        match rest.split_once("--") {
            Some((id, name)) if digits(id) => return format!("$app:{name}"),
            None if digits(rest) => return "$app".to_string(),
            _ => {}
        }
    }
    namespace.to_string()
}

/// A metafield's `jsonValue`: its value as JSON, according to its type.
fn json_value(kind: &str, value: &str) -> Json {
    let text = || Json::String(value.to_string());
    match kind {
        "boolean" => Json::Bool(value == "true"),
        "number_integer" => value.parse::<i64>().map_or_else(|_| text(), Json::from),
        "single_line_text_field"
        | "multi_line_text_field"
        | "number_decimal"
        | "url"
        | "color"
        | "date"
        | "date_time"
        | "id" => text(),
        // json, rating, money, dimension, weight, volume, link, rich text, lists, references:
        // the string holds JSON.
        _ => serde_json::from_str(value).unwrap_or_else(|_| text()),
    }
}
