//! `shop`, `policy`, `address` and `brand`.

use std::any::Any;

use slt_liquid::{Object, Value};

use super::localization::{currency_value, language_value};
use super::media::ImageDrop;
use super::metafield::MetafieldsDrop;
use super::{Memo, SiteRef, hash, strings};
use crate::store::{Address, Policy};

pub struct ShopDrop {
    pub site: SiteRef,
    memo: Memo,
}

impl ShopDrop {
    pub fn value(site: &SiteRef) -> Value {
        Value::object(ShopDrop {
            site: site.clone(),
            memo: Memo::default(),
        })
    }
}

pub fn policy_value(site: &SiteRef, policy: &Policy) -> Value {
    hash([
        (
            "id",
            Value::Int(crate::util::stable_id("policy", &policy.handle) as i64),
        ),
        ("title", Value::from(&policy.title)),
        ("body", Value::from(&policy.body)),
        ("handle", Value::from(&policy.handle)),
        (
            "url",
            Value::from(
                site.request
                    .localized(&format!("/policies/{}", policy.handle)),
            ),
        ),
    ])
}

pub struct AddressDrop {
    pub site: SiteRef,
    pub address: Address,
    /// The customer that owns the address, for its URL.
    pub customer: bool,
}

impl AddressDrop {
    pub fn value(site: &SiteRef, address: &Address, customer: bool) -> Value {
        Value::object(AddressDrop {
            site: site.clone(),
            address: address.clone(),
            customer,
        })
    }
}

impl Object for AddressDrop {
    fn type_name(&self) -> &str {
        "address"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let address = &self.address;
        let name = [address.first_name.as_str(), address.last_name.as_str()]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        Some(match key {
            "id" => Value::Int(address.id as i64),
            "first_name" => Value::from(&address.first_name),
            "last_name" => Value::from(&address.last_name),
            "name" => Value::from(name),
            "company" => Value::from(&address.company),
            "address1" => Value::from(&address.address1),
            "address2" => Value::from(&address.address2),
            "street" => Value::from(
                [address.address1.as_str(), address.address2.as_str()]
                    .iter()
                    .filter(|part| !part.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            "city" => Value::from(&address.city),
            "province" => Value::from(&address.province),
            "province_code" => Value::from(&address.province_code),
            "zip" => Value::from(&address.zip),
            "country_code" => Value::from(&address.country_code),
            // The country prints as its name and exposes `iso_code`.
            "country" => Value::object(CountryName {
                name: address.country.clone(),
                iso_code: address.country_code.clone(),
            }),
            "phone" => Value::from(&address.phone),
            "summary" => Value::from(
                [
                    name.as_str(),
                    address.address1.as_str(),
                    address.city.as_str(),
                    address.province.as_str(),
                    address.country.as_str(),
                ]
                .iter()
                .filter(|part| !part.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", "),
            ),
            "url" => {
                if self.customer {
                    Value::from(
                        self.site
                            .request
                            .localized(&format!("/account/addresses/{}", address.id)),
                    )
                } else {
                    Value::Nil
                }
            }
            _ => return None,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("address:{}", self.address.id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct CountryName {
    name: String,
    iso_code: String,
}

impl Object for CountryName {
    fn type_name(&self) -> &str {
        "country"
    }

    fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "name" => Value::from(&self.name),
            "iso_code" => Value::from(&self.iso_code),
            _ => return None,
        })
    }

    fn to_value(&self) -> Option<Value> {
        Some(Value::from(&self.name))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Object for ShopDrop {
    fn type_name(&self) -> &str {
        "shop"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let store = &site.store;
        let shop = &store.shop;
        let policy = |handle: &str| {
            store
                .policy(handle)
                .map_or(Value::Nil, |policy| policy_value(site, policy))
        };
        Some(match key {
            "id" => Value::Int(shop.id as i64),
            "name" => Value::from(&shop.name),
            "description" => Value::from(&shop.description),
            "email" => Value::from(&shop.email),
            "phone" => Value::from(&shop.phone),
            // Links built from the shop URL must come back to the local server.
            "url" | "secure_url" => Value::from(site.request.origin()),
            "domain" => Value::from(shop.domain.as_ref().unwrap_or(&site.request.host)),
            "permanent_domain" => Value::from(&shop.permanent_domain),
            "currency" => Value::from(&shop.currency),
            "money_format" => Value::from(&shop.money_format),
            "money_with_currency_format" => Value::from(&shop.money_with_currency_format),
            "address" => AddressDrop::value(site, &shop.address, false),
            "metafields" => self.memo.get("metafields", || {
                MetafieldsDrop::value(site, &shop.metafields)
            }),
            "enabled_payment_types" => strings(&shop.enabled_payment_types),
            "enabled_currencies" => {
                let mut codes: Vec<&str> = vec![&shop.currency];
                for country in &store.countries {
                    if !codes.contains(&country.currency.as_str()) {
                        codes.push(&country.currency);
                    }
                }
                Value::array(codes.into_iter().map(currency_value).collect())
            }
            "published_locales" | "enabled_locales" => Value::array(
                store
                    .languages
                    .iter()
                    .map(|language| language_value(site, language))
                    .collect(),
            ),
            "locale" => Value::from(&site.request.locale),
            "customer_accounts_enabled" => Value::Bool(shop.customer_accounts_enabled),
            "customer_accounts_optional" => Value::Bool(shop.customer_accounts_optional),
            "taxes_included" => Value::Bool(shop.taxes_included),
            "password_message" => Value::from(&shop.password_message),
            "policies" => Value::array(
                shop.policies
                    .iter()
                    .map(|policy| policy_value(site, policy))
                    .collect(),
            ),
            "refund_policy" => policy("refund-policy"),
            "shipping_policy" => policy("shipping-policy"),
            "privacy_policy" => policy("privacy-policy"),
            "terms_of_service" => policy("terms-of-service"),
            "subscription_policy" => policy("subscription-policy"),
            "vendors" | "types" => {
                let mut values: Vec<String> = Vec::new();
                for product in &store.products {
                    let value = if key == "vendors" {
                        &product.vendor
                    } else {
                        &product.product_type
                    };
                    if !value.is_empty() && !values.contains(value) {
                        values.push(value.clone());
                    }
                }
                values.sort_by_key(|value| value.to_lowercase());
                strings(&values)
            }
            "products_count" => Value::from(store.products.len()),
            "collections_count" => Value::from(store.collections.len()),
            "accepts_gift_cards" => {
                Value::Bool(store.products.iter().any(|product| product.gift_card))
            }
            "search_types" => Value::array(vec![
                Value::str("product"),
                Value::str("page"),
                Value::str("article"),
            ]),
            "brand" => match &shop.brand {
                Some(brand) => hash([
                    ("slogan", Value::from(&brand.slogan)),
                    ("short_description", Value::from(&brand.short_description)),
                    ("logo", ImageDrop::optional(site, brand.logo.as_ref())),
                    (
                        "square_logo",
                        ImageDrop::optional(site, brand.square_logo.as_ref()),
                    ),
                    (
                        "cover_image",
                        ImageDrop::optional(site, brand.cover_image.as_ref()),
                    ),
                    (
                        "favicon_url",
                        ImageDrop::optional(site, brand.favicon.as_ref()),
                    ),
                    ("colors", Value::from(&brand.colors)),
                ]),
                None => Value::Nil,
            },
            "metaobjects" => {
                Value::object(super::metafield::MetaobjectsDrop { site: site.clone() })
            }
            _ => return None,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
