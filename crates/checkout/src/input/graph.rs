//! The graph a Function input query runs against: the root `Input` type and everything
//! reachable from it, answered from the checkout context.

use chrono::{NaiveDateTime, NaiveTime};
use lsf_core::store::{
    Company, CompanyLocation, Customer, Metafields, Product, SellingPlan, Variant,
};
use serde_json::{Value as Json, json};

use super::exec::{Args, Graph, Resolved};
use super::{InputContext, OwnerKind, decimal, delivery_group_id, payment_method_id};
use crate::{Address, DeliveryGroup, DeliveryOption, Metafield, PaymentMethod, ResultLine};

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
    Company(&'c Company),
    CompanyLocation(&'c CompanyLocation),
    DeliveryGroup(&'c DeliveryGroup),
    DeliveryOption(&'c DeliveryOption),
    Address(&'c Address),
    PaymentMethod(&'c PaymentMethod),
    Localization,
    Country(String),
    Language(String),
    Market,
    MarketRegion(String),
    Shop,
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
    "CartLineCost",
    "ProductVariant",
    "CustomProduct",
    "Product",
    "SellingPlanAllocation",
    "SellingPlan",
    "SellingPlanAllocationPriceAdjustment",
    "MoneyV2",
    "Attribute",
    "CartBuyerIdentity",
    "Customer",
    "PurchasingCompany",
    "Company",
    "CompanyLocation",
    "CartDeliveryGroup",
    "CartDeliveryOption",
    "MailingAddress",
    "PaymentMethod",
    "Localization",
    "Country",
    "Language",
    "Market",
    "MarketRegionCountry",
    "Shop",
    "LocalTime",
    "EnteredDiscountCode",
    "BuyerJourney",
    "HasTagResponse",
    "CollectionMembership",
    "Metafield",
    // Unions.
    "Merchandise",
    "MarketRegion",
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
            Node::LineCost(_) => "CartLineCost",
            Node::Variant(_) => "ProductVariant",
            Node::Product(_) => "Product",
            Node::SellingPlanAllocation(..) => "SellingPlanAllocation",
            Node::SellingPlan(_) => "SellingPlan",
            Node::PriceAdjustment(_) => "SellingPlanAllocationPriceAdjustment",
            Node::Money(_) => "MoneyV2",
            Node::Attribute(..) => "Attribute",
            Node::BuyerIdentity => "CartBuyerIdentity",
            Node::Customer(_) => "Customer",
            Node::PurchasingCompany(_) => "PurchasingCompany",
            Node::Company(_) => "Company",
            Node::CompanyLocation(_) => "CompanyLocation",
            Node::DeliveryGroup(_) => "CartDeliveryGroup",
            Node::DeliveryOption(_) => "CartDeliveryOption",
            Node::Address(_) => "MailingAddress",
            Node::PaymentMethod(_) => "PaymentMethod",
            Node::Localization => "Localization",
            Node::Country(_) => "Country",
            Node::Language(_) => "Language",
            Node::Market => "Market",
            Node::MarketRegion(_) => "MarketRegionCountry",
            Node::Shop => "Shop",
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
                || (condition == "MarketRegion" && name == "MarketRegionCountry"),
        )
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
            Node::LineCost(line) => self.line_cost(line, name),
            Node::Variant(line) => self.variant(line, name),
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
            Node::Company(company) => match name {
                "id" => leaf(format!("gid://shopify/Company/{}", company.id)),
                "name" => leaf(&company.name),
                "externalId" => Some(nullable(company.external_id.as_ref())),
                _ => None,
            },
            Node::CompanyLocation(location) => match name {
                "id" => leaf(format!("gid://shopify/CompanyLocation/{}", location.id)),
                "name" => leaf(&location.name),
                "externalId" => Some(nullable(location.external_id.as_ref())),
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
            Node::Address(address) => address_field(address, name),
            Node::PaymentMethod(method) => match name {
                "id" => leaf(payment_method_id(&method.handle)),
                "name" => leaf(&method.name),
                _ => None,
            },
            Node::Localization => match name {
                "country" => object(Node::Country(self.country())),
                "language" => object(Node::Language(self.language())),
                "market" => object(Node::Market),
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
            Node::Market => self.market(name),
            Node::MarketRegion(code) => match name {
                "code" => leaf(code),
                _ => None,
            },
            Node::Shop => match name {
                "localTime" => object(Node::LocalTime),
                _ => None,
            },
            Node::LocalTime => self.local_time(name, args)?,
            Node::EnteredCode(code) => match name {
                "code" => leaf(*code),
                "rejectable" => leaf(true),
                _ => None,
            },
            Node::BuyerJourney => match name {
                "step" => leaf("CHECKOUT_COMPLETION"),
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
            "cost" => object(Node::CartCost),
            "buyerIdentity" => object(Node::BuyerIdentity),
            "deliveryGroups" => Some(Resolved::List(
                ctx.delivery_groups
                    .iter()
                    .map(Node::DeliveryGroup)
                    .collect(),
            )),
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
        let subtotal: i64 = self.ctx.lines.iter().map(|line| line.subtotal).sum();
        match name {
            "subtotalAmount" | "totalAmount" => object(Node::Money(subtotal)),
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
            "sellingPlanAllocation" => Some(self.selling_plan(line).unwrap_or(Resolved::Null)),
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
            // Discounts are computed from this cost: they are not in it.
            "subtotalAmount" | "totalAmount" => object(Node::Money(line.subtotal)),
            _ => None,
        }
    }

    fn store_variant(&self, line: &ResultLine) -> Option<(&'c Product, &'c Variant)> {
        self.ctx.store.variant(line.variant_id)
    }

    fn variant(&self, line: &'c ResultLine, name: &str) -> Option<Resolved<Node<'c>>> {
        let found = self.store_variant(line);
        match name {
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
            _ => None,
        }
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
            "inAnyCollection" => {
                let ids = args.string_list("ids")?;
                leaf(ids.iter().any(|id| self.in_collection(product, id)))
            }
            "inCollections" => {
                let ids = args.string_list("ids")?;
                Some(Resolved::List(
                    ids.into_iter()
                        .map(|id| {
                            let member = self.in_collection(product, &id);
                            Node::Membership(id, member)
                        })
                        .collect(),
                ))
            }
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
            "cartLines" => Some(Resolved::List(
                ctx.lines
                    .iter()
                    .filter(|line| group.line_ids.contains(&line.id))
                    .map(Node::Line)
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

    /// `LanguageCode` is the upper-case code with `_` for `-`: `PT_BR`.
    fn language(&self) -> String {
        let store = self.ctx.store;
        store
            .languages
            .iter()
            .find(|language| language.primary)
            .or_else(|| store.languages.first())
            .map_or("EN", |language| language.iso_code.as_str())
            .replace('-', "_")
            .to_ascii_uppercase()
    }

    fn market(&self, name: &str) -> Option<Resolved<Node<'c>>> {
        let code = self.country();
        match name {
            "id" => leaf(format!(
                "gid://shopify/Market/{}",
                lsf_core::util::stable_id("market", &code)
            )),
            "handle" => leaf(match self.ctx.store.country(&code) {
                Some(country) => lsf_core::util::handleize(&country.name),
                None => code.to_ascii_lowercase(),
            }),
            "regions" => Some(Resolved::List(vec![Node::MarketRegion(code)])),
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
            Node::Company(company) => Some(Source::Core(Some(&company.metafields))),
            Node::CompanyLocation(location) => Some(Source::Core(Some(&location.metafields))),
            Node::Shop => Some(Source::Core(Some(&store.shop.metafields))),
            _ => None,
        }
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
                let value = match &metafield.value {
                    Json::String(text) => text.clone(),
                    other => other.to_string(),
                };
                Some((metafield.kind.clone(), value))
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
