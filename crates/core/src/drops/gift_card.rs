//! `gift_card`: the card shown by `templates/gift_card.liquid`.

use std::any::Any;

use slt_liquid::{Object, Value};

use super::customer::CustomerDrop;
use super::product::ProductDrop;
use super::{SiteRef, hash, time_value};
use crate::store::GiftCard;

pub struct GiftCardDrop {
    site: SiteRef,
    index: usize,
}

impl GiftCardDrop {
    pub fn value(site: &SiteRef, index: usize) -> Value {
        Value::object(GiftCardDrop {
            site: site.clone(),
            index,
        })
    }

    fn card(&self) -> &GiftCard {
        &self.site.store.gift_cards[self.index]
    }
}

impl Object for GiftCardDrop {
    fn type_name(&self) -> &str {
        "gift_card"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let card = self.card();
        let url = || format!("{}{}", site.request.origin(), card.path(site.store.shop.id));
        Some(match key {
            "balance" => Value::Int(card.balance),
            "code" => Value::from(&card.code),
            "currency" => Value::from(&card.currency),
            "customer" => card
                .customer
                .map_or(Value::Nil, |id| CustomerDrop::value(site, id)),
            "enabled" => Value::Bool(card.enabled),
            "expired" => Value::Bool(card.expires_on.is_some_and(|date| date < site.now)),
            "expires_on" => card
                .expires_on
                .map_or(Value::Nil, |date| time_value(site, date)),
            "initial_value" => Value::Int(card.initial_value),
            "last_four_characters" => {
                let start = card.code.len().saturating_sub(4);
                Value::from(&card.code[start..])
            }
            "message" => Value::from(&card.message),
            // Shopify links to an Apple Wallet pass, which cannot be generated locally.
            "pass_url" => Value::Nil,
            "product" => card
                .product
                .map_or(Value::Nil, |index| ProductDrop::value(site, index)),
            "properties" => Value::hash(
                card.properties
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::from(value)))
                    .collect(),
            ),
            "qr_identifier" => Value::from(format!("shopify-giftcard-v1-{}", card.code)),
            "recipient" => card.recipient.as_ref().map_or(Value::Nil, |recipient| {
                hash([
                    ("name", Value::from(&recipient.name)),
                    ("email", Value::from(&recipient.email)),
                    ("nickname", Value::from(&recipient.nickname)),
                ])
            }),
            "send_on" => card
                .send_on
                .map_or(Value::Nil, |date| time_value(site, date)),
            "template_suffix" => card
                .template_suffix
                .as_ref()
                .map_or(Value::Nil, Value::from),
            "url" => Value::from(url()),
            _ => return None,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
